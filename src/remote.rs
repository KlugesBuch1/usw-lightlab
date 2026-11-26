use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use russh::client::Handle;
use russh::Disconnect;
use tokio::sync::mpsc::{self as tokio_mpsc, UnboundedSender};

use crate::settings;
use crate::ssh::{self, Handler, HostKeyPrompt, Settings};

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
