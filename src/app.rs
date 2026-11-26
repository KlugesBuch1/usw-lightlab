use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle,
};

use crate::action::{ActionGate, ActionKind};
use crate::led::{LedLine, Leds};
use crate::preview::{Preview, PreviewEvent};
use crate::remote::{Remote, RemoteEvent, TestJob};
use crate::settings::{self, AuthMode, Config};
use crate::ssh::{HostKeyPrompt, Settings};
use crate::strip;

pub struct LightLab {
    leds: Leds,
    preview: Preview,
    remote: Remote,
    action: ActionGate,
    config: Config,
    password: String,
    connected: Option<String>,
    test_active: Option<String>,
    host_prompt: Option<HostKeyPrompt>,
    error: Option<String>,
    log: Vec<LogEntry>,
    log_started: Instant,
}

struct LogEntry {
    at: Duration,
    text: String,
    error: bool,
}

impl LightLab {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme(&cc.egui_ctx);
        let mut warnings = Vec::new();
        let mut config = match settings::load() {
            Ok(config) => config,
            Err(error) => {
                warnings.push(error);
                Config::default()
            }
        };
        if config.script_path.trim().is_empty() || !Path::new(&config.script_path).is_file() {
            config.script_path = default_script();
        }
        let mut password = String::new();
        if config.remember_password && !config.host.trim().is_empty() {
            match settings::load_password(&config.user, &config.host, config.port) {
                Ok(Some(saved)) => {
                    password = saved;
                    warnings
                        .push("loaded the saved password from Windows Credential Manager".into());
                }
                Ok(None) => {}
                Err(error) => warnings.push(error),
            }
        }
        let mut app = Self {
            leds: Leds::new(),
            preview: Preview::new(),
            remote: Remote::new(cc.egui_ctx.clone()),
            action: ActionGate::default(),
            config,
            password,
            connected: None,
            test_active: None,
            host_prompt: None,
            error: None,
            log: Vec::new(),
            log_started: Instant::now(),
        };
        for warning in warnings {
            app.push_log(warning);
        }
        if let Some(error) = app.preview.wsl_error().map(str::to_string) {
            app.push_log(error);
        } else {
            app.push_log("WSL ready; preview runs animations with LIGHTLAB_EMIT=1");
        }
        app
    }

    fn push_log(&mut self, text: impl Into<String>) {
        let text = text.into();
        for line in text.lines() {
            self.log.push(LogEntry {
                at: self.log_started.elapsed(),
                text: line.to_string(),
                error: false,
            });
        }
        if self.log.len() > 2000 {
            let cutoff = self.log.len() - 2000;
            self.log.drain(..cutoff);
        }
    }

    fn push_error(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.error = Some(text.clone());
        for line in text.lines() {
            self.log.push(LogEntry {
                at: self.log_started.elapsed(),
                text: line.to_string(),
                error: true,
            });
        }
    }

    fn handle_preview_events(&mut self) {
        let stopping = self.preview.is_stopping();
        let events = self.preview.poll();
        for event in events {
            match event {
                PreviewEvent::Started(pid) => {
                    self.push_log(format!("preview running in WSL as PID {pid}"));
                }
                PreviewEvent::Frame(lines) => {
                    for line in lines {
                        if let LedLine::Color {
                            port,
                            channel,
                            value,
                        } = line
                        {
                            self.leds.set(port, channel, value);
                        }
                    }
                }
                PreviewEvent::Log(line) => self.push_log(line),
                PreviewEvent::Exited(code) => {
                    self.leds.clear();
                    match code {
                        Some(code) if stopping => {
                            self.push_log(format!("preview stopped (exit {code})"));
                        }
                        Some(code) => self.push_log(format!("preview exited with status {code}")),
                        None => self.push_log("preview exited"),
                    }
                }
            }
        }
    }

    fn handle_remote_events(&mut self) {
        for event in self.remote.poll() {
            match event {
                RemoteEvent::Connecting(endpoint) => {
                    self.push_log(format!("connecting to {endpoint}"));
                }
                RemoteEvent::HostKeyPrompt(prompt) => {
                    self.host_prompt = Some(prompt);
                }
                RemoteEvent::Connected(endpoint) => {
                    self.connected = Some(endpoint.clone());
                    self.host_prompt = None;
                    if self.action.current() == Some(ActionKind::Connect) {
                        self.action.finish();
                    }
                    self.save_password();
                    if let Err(error) = settings::save(&self.config) {
                        self.push_log(format!("settings were not saved: {error}"));
                    }
                    self.push_log(format!("SSH connected to {endpoint}"));
                }
                RemoteEvent::TestStarted { script } => {
                    self.test_active = Some(script.clone());
                    self.action.finish();
                    self.push_log(format!("test attached to the switch with {script}"));
                }
                RemoteEvent::TestOutput(text) => self.push_log(text),
                RemoteEvent::TestExited {
                    script,
                    code,
                    stopped,
                    error,
                } => {
                    self.test_active = None;
                    self.action.finish();
                    if let Some(error) = error {
                        self.push_error(error);
                    } else if stopped {
                        match code {
                            Some(code) => self.push_log(format!(
                                "test stopped; switch cleanup finished (exit {code})"
                            )),
                            None => self.push_log("test stopped"),
                        }
                    } else {
                        match code {
                            Some(code) => {
                                self.push_log(format!("{script} exited with status {code}"))
                            }
                            None => self.push_log(format!("{script} exited")),
                        }
                    }
                }
                RemoteEvent::Disconnected(reason) => {
                    self.connected = None;
                    self.test_active = None;
                    self.host_prompt = None;
                    self.action.finish();
                    match reason {
                        Some(reason) => self.push_log(format!("SSH disconnected: {reason}")),
                        None => self.push_log("SSH disconnected"),
                    }
                }
                RemoteEvent::Error(message) => {
                    self.test_active = None;
                    self.host_prompt = None;
                    self.action.finish();
                    self.push_error(message);
                }
                RemoteEvent::Log(line) => self.push_log(line),
            }
        }
    }

    fn save_password(&mut self) {
        if self.config.auth != AuthMode::Password {
            return;
        }
        if self.config.remember_password {
            match settings::save_password(
                &self.config.user,
                &self.config.host,
                self.config.port,
                &self.password,
            ) {
                Ok(()) => self.push_log("password saved in Windows Credential Manager"),
                Err(error) => self.push_log(format!("password was not saved: {error}")),
            }
        } else if let Err(error) =
            settings::forget_password(&self.config.user, &self.config.host, self.config.port)
        {
            self.push_log(format!("saved password was not removed: {error}"));
        }
    }

    fn ssh_settings(&self) -> Result<Settings, String> {
        let host = self.config.host.trim();
        let user = self.config.user.trim();
        if host.is_empty() {
            return Err("the switch host is empty".into());
        }
        if user.is_empty() {
            return Err("the SSH user is empty".into());
        }
        if self.config.port == 0 {
            return Err("the SSH port must be between 1 and 65535".into());
        }
        match self.config.auth {
            AuthMode::Key if self.config.key_path.trim().is_empty() => {
                return Err("key authentication needs a key file".into())
            }
            AuthMode::Password if self.password.is_empty() => {
                return Err("password authentication needs a password".into())
            }
            _ => {}
        }
        Ok(Settings {
            host: host.to_string(),
            port: self.config.port,
            user: user.to_string(),
            auth: self.config.auth,
            key_path: self.config.key_path.trim().to_string(),
            password: self.password.clone(),
        })
    }

    fn connect_or_disconnect(&mut self) {
        if self.action.is_busy() {
            return;
        }
        if self.connected.is_some() {
            if !self.action.try_begin(ActionKind::Disconnect) {
                return;
            }
            if let Err(error) = self.remote.disconnect() {
                self.action.finish();
                self.push_error(format!("cannot disconnect: {error}"));
            }
            return;
        }
        let settings = match self.ssh_settings() {
            Ok(settings) => settings,
            Err(error) => {
                self.push_error(format!(
                    "Cannot connect: {error}. No remote command ran; led_mode and the pidfile were not changed."
                ));
                return;
            }
        };
        if !self.action.try_begin(ActionKind::Connect) {
            return;
        }
        self.error = None;
        if let Err(error) = settings::save(&self.config) {
            self.push_log(format!("settings were not saved: {error}"));
        }
        if let Err(error) = self.remote.connect(settings) {
            self.action.finish();
            self.push_error(format!(
                "Cannot connect: {error}. No remote command ran; led_mode and the pidfile were not changed."
            ));
        }
    }

    fn test_action(&mut self) {
        if self.test_active.is_some() {
            self.stop_test();
        } else {
            self.start_test();
        }
    }

    fn start_test(&mut self) {
        if self.action.is_busy() || self.test_active.is_some() {
            return;
        }
        let path = self.config.script_path.trim().to_string();
        if path.is_empty() {
            self.push_error(
                "Cannot test: no animation script selected. No remote command ran; led_mode and the pidfile were not changed.",
            );
            return;
        }
        let script = match std::fs::read(&path) {
            Ok(script) => script,
            Err(error) => {
                self.push_error(format!(
                    "Cannot test: cannot read {path}: {error}. No remote command ran; led_mode and the pidfile were not changed."
                ));
                return;
            }
        };
        let settings = match self.ssh_settings() {
            Ok(settings) => settings,
            Err(error) => {
                self.push_error(format!(
                    "Cannot test: {error}. No remote command ran; led_mode and the pidfile were not changed."
                ));
                return;
            }
        };
        if !self.action.try_begin(ActionKind::Test) {
            return;
        }
        self.error = None;
        if let Err(error) = settings::save(&self.config) {
            self.push_log(format!("settings were not saved: {error}"));
        }
        self.push_log(format!("uploading and attaching {path}"));
        if let Err(error) = self.remote.start_test(TestJob {
            settings,
            script_name: path,
            script,
        }) {
            self.action.finish();
            self.push_error(format!(
                "Cannot test: {error}. No remote command ran; led_mode and the pidfile were not changed."
            ));
        }
    }

    fn stop_test(&mut self) {
        if !self.action.try_begin(ActionKind::Stop) {
            return;
        }
        if let Err(error) = self.remote.stop_test() {
            self.action.finish();
            self.push_error(format!("cannot stop the test: {error}"));
        }
    }

    fn preview_controls(&mut self, ui: &mut egui::Ui) {
        let running = self.preview.is_running();
        let stopping = self.preview.is_stopping();
        let wsl_ready = self.preview.wsl_error().is_none();
        let button = if running {
            if stopping {
                "Stopping..."
            } else {
                "Stop"
            }
        } else {
            "Preview"
        };
        if ui
            .add_enabled(
                wsl_ready && !stopping && !self.action.is_busy(),
                egui::Button::new(button).min_size(egui::vec2(88.0, 32.0)),
            )
            .clicked()
            && self.action.try_begin(ActionKind::Preview)
        {
            if running {
                self.error = None;
                self.preview.stop();
                self.push_log("stopping preview");
            } else {
                self.start_preview();
            }
            self.action.finish();
        }
    }

    fn start_preview(&mut self) {
        self.error = None;
        if self.config.script_path.trim().is_empty() {
            self.push_error("Cannot preview: no animation script selected.");
            return;
        }
        let path = self.config.script_path.trim().to_string();
        match self.preview.start(Path::new(&path)) {
            Ok(()) => {
                self.leds.clear();
                let name = Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or(path);
                self.push_log(format!("starting preview of {name}"));
            }
            Err(error) => self.push_error(error),
        }
    }

    fn host_key_prompt(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.host_prompt.as_ref() else {
            return;
        };
        let mut decision = None;
        let known_hosts = settings::known_hosts_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "%APPDATA%\\lightlab\\known_hosts".into());
        egui::Window::new("Verify switch host key")
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .movable(false)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label(
                    RichText::new(format!("First connection to {}:{}", prompt.host, prompt.port))
                        .size(15.0)
                        .strong(),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "The switch presented this host key. Compare it with the switch before trusting it.",
                    )
                    .size(12.0)
                    .color(Color32::from_rgb(158, 166, 178)),
                );
                ui.add_space(10.0);
                egui::Frame::NONE
                    .fill(Color32::from_rgb(9, 11, 14))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(42, 48, 57)))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(&prompt.fingerprint)
                                .monospace()
                                .size(13.0)
                                .color(Color32::from_rgb(238, 241, 246)),
                        );
                        ui.label(
                            RichText::new(&prompt.key_type)
                                .monospace()
                                .size(10.5)
                                .color(Color32::from_rgb(132, 140, 152)),
                        );
                    });
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("Pinned in {known_hosts}"))
                        .size(10.5)
                        .color(Color32::from_rgb(112, 120, 132)),
                );
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Trust and connect").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        if let Some(accept) = decision {
            let prompt = self.host_prompt.take().unwrap();
            let label = format!("{}:{}", prompt.host, prompt.port);
            let fingerprint = prompt.fingerprint.clone();
            prompt.answer(accept);
            if accept {
                self.push_log(format!("trusted host key {fingerprint} for {label}"));
            } else {
                self.push_log(format!("rejected host key for {label}"));
            }
        }
    }
}

impl eframe::App for LightLab {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_preview_events();
        self.handle_remote_events();
        if self.preview.is_running() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        egui::TopBottomPanel::top("header")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(13, 15, 19))
                    .inner_margin(Margin::symmetric(22, 18)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("lightlab")
                            .font(FontId::new(25.0, FontFamily::Proportional))
                            .strong()
                            .color(Color32::from_rgb(240, 242, 245)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new("USW-Pro-Max-48 LED bench")
                            .size(12.0)
                            .color(Color32::from_rgb(120, 128, 140)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, color) = if self.connected.is_some() {
                            ("switch connected", Color32::from_rgb(104, 214, 153))
                        } else if self.preview.is_running() {
                            ("preview live", Color32::from_rgb(255, 92, 106))
                        } else {
                            ("offline", Color32::from_rgb(112, 120, 132))
                        };
                        ui.label(RichText::new(label).size(11.0).color(color));
                        let (dot, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, color);
                    });
                });
            });

        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .default_height(150.0)
            .min_height(105.0)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(9, 11, 14))
                    .inner_margin(Margin::symmetric(22, 12))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(28, 32, 39))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("LOG")
                            .font(FontId::new(10.0, FontFamily::Monospace))
                            .color(Color32::from_rgb(142, 150, 162)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("clear").clicked() {
                            self.log.clear();
                        }
                    });
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.log.is_empty() {
                            ui.label(
                                RichText::new("no activity yet")
                                    .monospace()
                                    .size(11.0)
                                    .color(Color32::from_rgb(88, 96, 108)),
                            );
                        }
                        for entry in &self.log {
                            let seconds = entry.at.as_secs();
                            let color = if entry.error {
                                Color32::from_rgb(255, 126, 136)
                            } else {
                                Color32::from_rgb(164, 172, 184)
                            };
                            ui.label(
                                RichText::new(format!(
                                    "{:02}:{:02}  {}",
                                    seconds / 60,
                                    seconds % 60,
                                    entry.text
                                ))
                                .monospace()
                                .size(11.0)
                                .color(color),
                            );
                        }
                    });
            });

        egui::TopBottomPanel::bottom("actions")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(17, 20, 25))
                    .inner_margin(Margin::symmetric(22, 10))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(31, 36, 44))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let active = self.test_active.is_some();
                    let starting = self.action.current() == Some(ActionKind::Test);
                    let stopping = self.action.current() == Some(ActionKind::Stop);
                    let (status, color) = if starting {
                        (
                            "attaching test...".to_string(),
                            Color32::from_rgb(230, 176, 100),
                        )
                    } else if stopping {
                        (
                            "stopping test...".to_string(),
                            Color32::from_rgb(230, 176, 100),
                        )
                    } else if let Some(script) = &self.test_active {
                        (
                            format!("test attached · {script}"),
                            Color32::from_rgb(104, 214, 153),
                        )
                    } else if self.connected.is_some() {
                        (
                            "connected · no run attached".to_string(),
                            Color32::from_rgb(160, 168, 180),
                        )
                    } else {
                        (
                            "idle · not connected".to_string(),
                            Color32::from_rgb(112, 120, 132),
                        )
                    };
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, color);
                    ui.label(RichText::new(status).size(11.0).color(color));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let busy = self.action.is_busy();
                        let label = if starting {
                            "Attaching..."
                        } else if stopping {
                            "Stopping..."
                        } else if active {
                            "Stop test"
                        } else {
                            "Test animation"
                        };
                        let button = ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(RichText::new(label).color(Color32::WHITE))
                                    .min_size(egui::vec2(150.0, 32.0))
                                    .fill(Color32::from_rgb(196, 43, 56)),
                            )
                            .on_hover_text(if active {
                                "Stop closes test stdin; the supervisor restores LEDs."
                            } else {
                                "Uploads and attaches a test run, stopping it on Stop or disconnect."
                            });
                        if button.clicked() {
                            self.test_action();
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(13, 15, 19))
                    .inner_margin(Margin::symmetric(22, 6)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if let Some(error) = &self.error {
                            egui::Frame::NONE
                                .fill(Color32::from_rgb(43, 21, 25))
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(125, 45, 55)))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::symmetric(12, 9))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(error)
                                            .size(12.0)
                                            .color(Color32::from_rgb(255, 170, 178)),
                                    );
                                });
                            ui.add_space(10.0);
                        }

                        egui::Frame::NONE
                            .fill(Color32::from_rgb(19, 22, 28))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(31, 36, 44)))
                            .corner_radius(CornerRadius::same(14))
                            .inner_margin(Margin::same(16))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(section_label("ANIMATION"));
                                    let field_width = (ui.available_width() - 184.0).max(180.0);
                                    ui.add_sized(
                                        [field_width, 27.0],
                                        egui::TextEdit::singleline(&mut self.config.script_path)
                                            .hint_text("path to a .sh animation"),
                                    );
                                    if ui.button("Browse").clicked() {
                                        let mut dialog = rfd::FileDialog::new()
                                            .add_filter("Shell scripts", &["sh"]);
                                        if let Some(parent) = Path::new(&self.config.script_path)
                                            .parent()
                                            .filter(|parent| parent.is_dir())
                                        {
                                            dialog = dialog.set_directory(parent);
                                        }
                                        if let Some(path) = dialog.pick_file() {
                                            self.config.script_path =
                                                path.to_string_lossy().to_string();
                                        }
                                    }
                                    self.preview_controls(ui);
                                });
                                ui.add_space(3.0);
                                let status = if self.preview.is_running() {
                                    let name = self
                                        .preview
                                        .script()
                                        .and_then(|path| path.file_name())
                                        .map(|name| name.to_string_lossy().to_string())
                                        .unwrap_or_else(|| "animation".into());
                                    let seconds = self.preview.elapsed().as_secs();
                                    match self.preview.pid() {
                                        Some(pid) => format!(
                                            "previewing {name} · WSL PID {pid} · {seconds}s"
                                        ),
                                        None => "preview starting".to_string(),
                                    }
                                } else {
                                    "not previewing".to_string()
                                };
                                ui.label(
                                    RichText::new(status)
                                        .size(11.0)
                                        .color(Color32::from_rgb(140, 148, 160)),
                                );
                                if let Some(error) = self.preview.wsl_error() {
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(error)
                                            .size(11.0)
                                            .color(Color32::from_rgb(230, 160, 120)),
                                    );
                                }
                            });

                        ui.add_space(12.0);

                        egui::Frame::NONE
                            .fill(Color32::from_rgb(19, 22, 28))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(31, 36, 44)))
                            .corner_radius(CornerRadius::same(14))
                            .inner_margin(Margin::same(16))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("PREVIEW")
                                            .font(FontId::new(10.0, FontFamily::Monospace))
                                            .color(Color32::from_rgb(150, 158, 170)),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.label(
                                                RichText::new("ports 1-48 / 24 pairs")
                                                    .size(10.0)
                                                    .color(Color32::from_rgb(105, 113, 125)),
                                            );
                                        },
                                    );
                                });
                                ui.add_space(10.0);
                                strip::show(ui, &self.leds);
                            });

                        ui.add_space(12.0);

                        let busy = self.action.is_busy();
                        let locked = busy || self.test_active.is_some() || self.connected.is_some();
                        egui::Frame::NONE
                            .fill(Color32::from_rgb(19, 22, 28))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(31, 36, 44)))
                            .corner_radius(CornerRadius::same(14))
                            .inner_margin(Margin::same(16))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(section_label("SWITCH"));
                                    let connecting =
                                        self.action.current() == Some(ActionKind::Connect);
                                    let disconnecting =
                                        self.action.current() == Some(ActionKind::Disconnect);
                                    let connect_label = if connecting {
                                        "Connecting..."
                                    } else if disconnecting {
                                        "Disconnecting..."
                                    } else if self.connected.is_some() {
                                        "Disconnect"
                                    } else {
                                        "Connect"
                                    };
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add_enabled(
                                                    !busy && self.test_active.is_none(),
                                                    egui::Button::new(connect_label)
                                                        .min_size(egui::vec2(110.0, 30.0)),
                                                )
                                                .clicked()
                                            {
                                                self.connect_or_disconnect();
                                            }
                                            ui.add_space(6.0);
                                            let (text, color) =
                                                if let Some(endpoint) = &self.connected {
                                                    (
                                                        format!("connected · {endpoint}"),
                                                        Color32::from_rgb(104, 214, 153),
                                                    )
                                                } else {
                                                    (
                                                        "not connected".to_string(),
                                                        Color32::from_rgb(112, 120, 132),
                                                    )
                                                };
                                            ui.label(RichText::new(text).size(11.0).color(color));
                                            let (dot, _) = ui.allocate_exact_size(
                                                egui::vec2(8.0, 8.0),
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().circle_filled(dot.center(), 3.5, color);
                                        },
                                    );
                                });
                                ui.add_space(10.0);
                                ui.add_enabled_ui(!locked, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(section_label("HOST"));
                                        ui.add_sized(
                                            [238.0, 27.0],
                                            egui::TextEdit::singleline(&mut self.config.host)
                                                .hint_text("switch address"),
                                        );
                                        ui.label(section_label("PORT"));
                                        ui.add(
                                            egui::DragValue::new(&mut self.config.port)
                                                .range(1..=65535)
                                                .speed(1.0),
                                        );
                                        ui.label(section_label("USER"));
                                        ui.add_sized(
                                            [110.0, 27.0],
                                            egui::TextEdit::singleline(&mut self.config.user)
                                                .hint_text("root"),
                                        );
                                    });
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label(section_label("AUTH"));
                                        egui::ComboBox::from_id_salt("auth_mode")
                                            .selected_text(self.config.auth.label())
                                            .width(118.0)
                                            .show_ui(ui, |ui| {
                                                for mode in [
                                                    AuthMode::Key,
                                                    AuthMode::Agent,
                                                    AuthMode::Password,
                                                ] {
                                                    ui.selectable_value(
                                                        &mut self.config.auth,
                                                        mode,
                                                        mode.label(),
                                                    );
                                                }
                                            });
                                        match self.config.auth {
                                            AuthMode::Key => {
                                                ui.add_sized(
                                                    [436.0, 27.0],
                                                    egui::TextEdit::singleline(
                                                        &mut self.config.key_path,
                                                    )
                                                    .hint_text("private key file"),
                                                );
                                                if ui.button("Browse").clicked() {
                                                    let mut dialog = rfd::FileDialog::new();
                                                    if let Some(parent) =
                                                        Path::new(&self.config.key_path)
                                                            .parent()
                                                            .filter(|parent| parent.is_dir())
                                                    {
                                                        dialog = dialog.set_directory(parent);
                                                    }
                                                    if let Some(path) = dialog.pick_file() {
                                                        self.config.key_path =
                                                            path.to_string_lossy().to_string();
                                                    }
                                                }
                                            }
                                            AuthMode::Agent => {
                                                ui.label(
                                                    RichText::new("uses the Windows OpenSSH agent")
                                                        .size(11.5)
                                                        .color(Color32::from_rgb(132, 140, 152)),
                                                );
                                            }
                                            AuthMode::Password => {
                                                ui.add_sized(
                                                    [304.0, 27.0],
                                                    egui::TextEdit::singleline(&mut self.password)
                                                        .password(true)
                                                        .hint_text("password"),
                                                );
                                                ui.checkbox(
                                                    &mut self.config.remember_password,
                                                    "remember",
                                                );
                                            }
                                        }
                                    });
                                });
                            });
                    });
            });

        self.host_key_prompt(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.preview.shutdown();
        let _ = settings::save(&self.config);
    }
}

fn section_label(text: &str) -> RichText {
    RichText::new(text)
        .font(FontId::new(10.0, FontFamily::Monospace))
        .color(Color32::from_rgb(150, 158, 170))
}

fn default_script() -> String {
    std::env::current_dir()
        .ok()
        .map(|dir| dir.join("kitt.sh"))
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(13, 15, 19);
    style.visuals.window_fill = Color32::from_rgb(19, 22, 28);
    style.visuals.extreme_bg_color = Color32::from_rgb(9, 11, 14);
    style.visuals.faint_bg_color = Color32::from_rgb(24, 28, 34);
    style.visuals.override_text_color = Some(Color32::from_rgb(219, 224, 231));
    style.visuals.selection.bg_fill = Color32::from_rgb(196, 43, 56);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, Color32::from_rgb(255, 225, 228));
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(19, 22, 28);
    style.visuals.widgets.noninteractive.bg_stroke =
        Stroke::new(1.0_f32, Color32::from_rgb(37, 42, 50));
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(30, 34, 41);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(24, 27, 33);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(45, 51, 60));
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(39, 44, 52);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(32, 36, 43);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(73, 80, 92));
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(196, 43, 56);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(196, 43, 56);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(255, 107, 118));
    style.spacing.item_spacing = egui::vec2(8.0, 9.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(11.5, FontFamily::Monospace),
        ),
    ]
    .into();
    ctx.set_style(style);
}
