use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Stroke, TextStyle,
};

use crate::led::{LedLine, Leds};
use crate::preview::{Preview, PreviewEvent};
use crate::strip;

pub struct LightLab {
    leds: Leds,
    preview: Preview,
    script_path: String,
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
        let mut app = Self {
            leds: Leds::new(),
            preview: Preview::new(),
            script_path: default_script(),
            error: None,
            log: Vec::new(),
            log_started: Instant::now(),
        };
        if let Some(error) = app.preview.wsl_error().map(str::to_string) {
            app.push_log(error);
        } else {
            app.push_log("WSL ready; preview runs kitt.sh with LIGHTLAB_EMIT=1");
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
                wsl_ready && !stopping,
                egui::Button::new(button).min_size(egui::vec2(88.0, 32.0)),
            )
            .clicked()
        {
            if running {
                self.error = None;
                self.preview.stop();
                self.push_log("stopping preview");
            } else {
                self.start_preview();
            }
        }
    }

    fn start_preview(&mut self) {
        self.error = None;
        if self.script_path.trim().is_empty() {
            self.push_error("Cannot preview: no animation script selected.");
            return;
        }
        let path = self.script_path.trim().to_string();
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
}

impl eframe::App for LightLab {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_preview_events();
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
                        let (label, color) = if self.preview.is_running() {
                            ("preview live", Color32::from_rgb(255, 92, 106))
                        } else {
                            ("preview idle", Color32::from_rgb(112, 120, 132))
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
            .default_height(210.0)
            .min_height(130.0)
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
                                    ui.label(
                                        RichText::new("ANIMATION")
                                            .font(FontId::new(10.0, FontFamily::Monospace))
                                            .color(Color32::from_rgb(150, 158, 170)),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.button("Browse").clicked() {
                                                let mut dialog = rfd::FileDialog::new()
                                                    .add_filter("Shell scripts", &["sh"]);
                                                if let Some(parent) = Path::new(&self.script_path)
                                                    .parent()
                                                    .filter(|parent| parent.is_dir())
                                                {
                                                    dialog = dialog.set_directory(parent);
                                                }
                                                if let Some(path) = dialog.pick_file() {
                                                    self.script_path =
                                                        path.to_string_lossy().to_string();
                                                }
                                            }
                                            let field =
                                                egui::TextEdit::singleline(&mut self.script_path)
                                                    .hint_text("path to a .sh animation")
                                                    .desired_width(f32::INFINITY);
                                            ui.add(field);
                                        },
                                    );
                                });
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
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
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            self.preview_controls(ui);
                                        },
                                    );
                                });
                                if let Some(error) = self.preview.wsl_error() {
                                    ui.add_space(4.0);
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
                    });
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.preview.shutdown();
    }
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
