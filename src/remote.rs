use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use russh::client::Handle;
use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc::{self as tokio_mpsc, UnboundedSender};

use crate::settings;
use crate::ssh::{self, Handler, HostKeyPrompt, Settings};

const REMOTE_DIR: &str = "/tmp/usw-lightlab";
const REMOTE_SUPERVISOR: &str = "/tmp/usw-lightlab/lightlab-run.sh";
const SUPERVISOR: &str = include_str!("../lightlab-run.sh");

pub struct Remote {
    commands: UnboundedSender<Command>,
    events: Receiver<RemoteEvent>,
}

pub enum RemoteEvent {
    Connecting(String),
    HostKeyPrompt(HostKeyPrompt),
    Connected(String),
    Disconnected(Option<String>),
    Error(String),
    Log(String),
}

enum Command {
    Connect(Settings),
    Disconnect,
}

#[derive(Clone)]
struct Sink {
    tx: Sender<RemoteEvent>,
    ctx: egui::Context,
}

impl Sink {
    fn emit(&self, event: RemoteEvent) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

impl Remote {
    pub fn new(ctx: egui::Context) -> Self {
        let (commands, command_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, events) = mpsc::channel();
        let sink = Sink { tx: event_tx, ctx };
        std::thread::Builder::new()
            .name("lightlab-ssh".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        sink.emit(RemoteEvent::Error(format!(
                            "cannot start the SSH worker: {error}"
                        )));
                        return;
                    }
                };
                runtime.block_on(run(command_rx, sink));
            })
            .ok();
        Self { commands, events }
    }

    pub fn connect(&self, settings: Settings) -> Result<(), String> {
        self.commands
            .send(Command::Connect(settings))
            .map_err(|_| "the SSH worker is not running".to_string())
    }

    pub fn disconnect(&self) -> Result<(), String> {
        self.commands
            .send(Command::Disconnect)
            .map_err(|_| "the SSH worker is not running".to_string())
    }

    pub fn poll(&self) -> Vec<RemoteEvent> {
        self.events.try_iter().collect()
    }
}

async fn run(mut commands: tokio_mpsc::UnboundedReceiver<Command>, sink: Sink) {
    let mut session: Option<Handle<Handler>> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    Command::Connect(settings) => connect(settings, &mut session, &sink).await,
                    Command::Disconnect => {
                        if let Some(handle) = session.take() {
                            let _ = handle.disconnect(Disconnect::ByApplication, "lightlab disconnect", "en").await;
                            sink.emit(RemoteEvent::Log("SSH disconnected".into()));
                        }
                        sink.emit(RemoteEvent::Disconnected(None));
                    }
                }
            }
            _ = tick.tick(), if session.is_some() => {
                if session.as_ref().is_some_and(|handle| handle.is_closed()) {
                    session = None;
                    sink.emit(RemoteEvent::Disconnected(Some("the SSH connection closed".into())));
                }
            }
        }
    }
}

async fn connect(settings: Settings, session: &mut Option<Handle<Handler>>, sink: &Sink) {
    if let Some(handle) = session.take() {
        let _ = handle
            .disconnect(Disconnect::ByApplication, "reconnect", "en")
            .await;
    }
    let endpoint = format!("{}:{}", settings.host.trim(), settings.port);
    sink.emit(RemoteEvent::Connecting(endpoint.clone()));
    let known_hosts = match settings::known_hosts_path() {
        Ok(path) => path,
        Err(error) => {
            sink.emit(RemoteEvent::Error(format!(
                "cannot find the pinned host key file: {error}"
            )));
            return;
        }
    };
    let prompt = {
        let sink = sink.clone();
        Arc::new(move |prompt: HostKeyPrompt| sink.emit(RemoteEvent::HostKeyPrompt(prompt)))
    };
    match ssh::connect(&settings, prompt, known_hosts).await {
        Ok(handle) => {
            *session = Some(handle);
            sink.emit(RemoteEvent::Connected(endpoint));
            sink.emit(RemoteEvent::Log(format!(
                "SSH connected as {}",
                settings.user
            )));
        }
        Err(error) => {
            sink.emit(RemoteEvent::Error(format!(
                "{error}. No remote command ran; led_mode and the pidfile were not changed."
            )));
        }
    }
}

async fn upload_animation(
    handle: &Handle<Handler>,
    script_name: &str,
    script: &[u8],
) -> Result<String, String> {
    let name = safe_animation_name(Path::new(script_name));
    exec_checked(
        handle,
        &format!("mkdir -p {REMOTE_DIR} && chmod 700 {REMOTE_DIR}"),
        None,
    )
    .await?;

    let supervisor = SUPERVISOR.replace("\r\n", "\n");
    write_remote(handle, REMOTE_SUPERVISOR, supervisor.as_bytes()).await?;

    let animation = match std::str::from_utf8(script) {
        Ok(text) => text.replace("\r\n", "\n").into_bytes(),
        Err(_) => script.to_vec(),
    };
    let animation_path = format!("{REMOTE_DIR}/{name}");
    write_remote(handle, &animation_path, &animation).await?;

    let check = format!(
        "sh -n {} && sh -n {}",
        sh_quote(REMOTE_SUPERVISOR),
        sh_quote(&animation_path)
    );
    exec_checked(handle, &check, None)
        .await
        .map_err(|error| format!("uploaded scripts did not pass sh -n: {error}"))?;
    Ok(name)
}

async fn write_remote(handle: &Handle<Handler>, path: &str, contents: &[u8]) -> Result<(), String> {
    let command = format!("cat > {}", sh_quote(path));
    let output = exec(handle, &command, Some(contents)).await?;
    if output.code != Some(0) {
        return Err(format!("cannot write {path}: {}", output.error_text()));
    }
    Ok(())
}

async fn exec_checked(
    handle: &Handle<Handler>,
    command: &str,
    input: Option<&[u8]>,
) -> Result<(), String> {
    let output = exec(handle, command, input).await?;
    if output.code == Some(0) {
        Ok(())
    } else {
        Err(format!(
            "command failed: {command}: {}",
            output.error_text()
        ))
    }
}

async fn exec(
    handle: &Handle<Handler>,
    command: &str,
    input: Option<&[u8]>,
) -> Result<ExecOutput, String> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|error| format!("cannot open an SSH channel: {error}"))?;
    channel
        .exec(true, command)
        .await
        .map_err(|error| format!("cannot run on the switch: {error}"))?;
    if let Some(input) = input {
        channel
            .data_bytes(input.to_vec())
            .await
            .map_err(|error| format!("cannot send data to the switch: {error}"))?;
    }
    channel
        .eof()
        .await
        .map_err(|error| format!("cannot close SSH stdin: {error}"))?;

    let mut output = ExecOutput::default();
    while let Some(message) = channel.wait().await {
        match message {
            ChannelMsg::Data { data } => output.stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, .. } => output.stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => output.code = Some(exit_status),
            _ => {}
        }
    }
    Ok(output)
}

#[derive(Default)]
struct ExecOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: Option<u32>,
}

impl ExecOutput {
    fn error_text(&self) -> String {
        let stderr = String::from_utf8_lossy(&self.stderr);
        let stdout = String::from_utf8_lossy(&self.stdout);
        let text = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        if text.is_empty() {
            format!("exit status {:?}", self.code)
        } else {
            text.to_string()
        }
    }
}

fn safe_animation_name(path: &Path) -> String {
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut name: String = base
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if name.chars().count() > 100 {
        name = name.chars().take(100).collect();
    }
    if name.is_empty() || name == "." || name == ".." || name.starts_with('.') {
        return "animation.sh".into();
    }
    name
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_animation_names() {
        assert_eq!(safe_animation_name(Path::new("kitt.sh")), "kitt.sh");
        assert_eq!(safe_animation_name(Path::new("my kitt.sh")), "my_kitt.sh");
        assert_eq!(safe_animation_name(Path::new("../../etc/passwd")), "passwd");
        assert_eq!(safe_animation_name(Path::new("")), "animation.sh");
        assert_eq!(safe_animation_name(Path::new(".hidden")), "animation.sh");
    }
}
