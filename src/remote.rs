use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use russh::client::Handle;
use russh::{ChannelMsg, Disconnect};
use tokio::sync::mpsc::{self as tokio_mpsc, UnboundedSender};
use tokio::sync::oneshot;

use crate::settings;
use crate::ssh::{self, Handler, HostKeyPrompt, Settings};

const REMOTE_DIR: &str = "/tmp/usw-lightlab";
const REMOTE_SUPERVISOR: &str = "/tmp/usw-lightlab/lightlab-run.sh";
const SUPERVISOR: &str = include_str!("../lightlab-run.sh");

const STATUS_SCRIPT: &str = r#"pidfile=/tmp/usw-lightlab.pid
mode=$(cat /proc/led/led_mode 2>/dev/null)
present=0
[ -e "$pidfile" ] && present=1
pid=$(cat "$pidfile" 2>/dev/null)
case $pid in
    ''|*[!0-9]*) pid= ;;
esac
alive=0
marker=0
script=
if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
    alive=1
    cmd=$(tr '\0' ' ' < "/proc/$pid/cmdline" 2>/dev/null)
    case $cmd in
        *lightlab-run.sh*) marker=1 ;;
    esac
    if [ "$marker" = 1 ]; then
        script=$(printf '%s\n' "$cmd" | awk '{for (i = 1; i <= NF; i++) if ($i ~ /lightlab-run\.sh$/) {print $(i + 1); exit}}')
        script=${script##*/}
    fi
fi
printf 'MODE=%s\n' "${mode:-unknown}"
printf 'PRESENT=%s\n' "$present"
printf 'PID=%s\n' "$pid"
printf 'ALIVE=%s\n' "$alive"
printf 'MARKER=%s\n' "$marker"
printf 'SCRIPT=%s\n' "$(printf '%s' "$script" | base64 | tr -d '\n')"
"#;

pub struct Remote {
    commands: UnboundedSender<Command>,
    events: Receiver<RemoteEvent>,
}

pub enum RemoteEvent {
    Connecting(String),
    HostKeyPrompt(HostKeyPrompt),
    Connected(String),
    Disconnected(Option<String>),
    TestStarted {
        script: String,
    },
    TestOutput(String),
    TestExited {
        script: String,
        code: Option<u32>,
        stopped: bool,
        error: Option<String>,
    },
    Error(String),
    Log(String),
}

pub struct TestJob {
    pub settings: Settings,
    pub script_name: String,
    pub script: Vec<u8>,
}

enum Command {
    Connect(Settings),
    Disconnect,
    StartTest(TestJob),
    StopTest,
}

#[derive(Clone, Debug, Default)]
pub struct RunStatus {
    pub led_mode: Option<String>,
    pub pidfile: Option<u32>,
    pub pidfile_present: bool,
    pub alive: bool,
    pub marker: bool,
    pub script: String,
}

impl RunStatus {
    pub fn is_running(&self) -> bool {
        self.pidfile.is_some() && self.alive && self.marker
    }

    pub fn summary(&self) -> String {
        let mode = self.led_mode.as_deref().unwrap_or("unknown");
        if self.is_running() {
            format!(
                "led_mode={mode}, pidfile PID {} running {}",
                self.pidfile.unwrap_or_default(),
                self.script
            )
        } else if self.pidfile_present {
            format!(
                "led_mode={mode}, pidfile PID {} is not a lightlab supervisor",
                self.pidfile
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "malformed".into())
            )
        } else {
            format!("led_mode={mode}, pidfile absent")
        }
    }
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

    pub fn start_test(&self, job: TestJob) -> Result<(), String> {
        self.commands
            .send(Command::StartTest(job))
            .map_err(|_| "the SSH worker is not running".to_string())
    }

    pub fn stop_test(&self) -> Result<(), String> {
        self.commands
            .send(Command::StopTest)
            .map_err(|_| "the SSH worker is not running".to_string())
    }

    pub fn poll(&self) -> Vec<RemoteEvent> {
        self.events.try_iter().collect()
    }
}

async fn run(mut commands: tokio_mpsc::UnboundedReceiver<Command>, sink: Sink) {
    let mut session: Option<Handle<Handler>> = None;
    let mut active_stop: Option<oneshot::Sender<()>> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                match command {
                    Command::Connect(settings) => connect(settings, &mut session, &sink).await,
                    Command::Disconnect => {
                        active_stop = None;
                        if let Some(handle) = session.take() {
                            let _ = handle.disconnect(Disconnect::ByApplication, "lightlab disconnect", "en").await;
                            sink.emit(RemoteEvent::Log("SSH disconnected".into()));
                        }
                        sink.emit(RemoteEvent::Disconnected(None));
                    }
                    Command::StartTest(job) => {
                        start_test(job, &mut session, &mut active_stop, &sink).await;
                    }
                    Command::StopTest => {
                        if let Some(stop) = active_stop.take() {
                            let _ = stop.send(());
                            sink.emit(RemoteEvent::Log("closing test stdin; waiting for the switch to restore LEDs".into()));
                        } else {
                            sink.emit(RemoteEvent::Error(
                                "no test is attached to stop; no remote command ran".into(),
                            ));
                        }
                    }
                }
            }
            _ = tick.tick(), if session.is_some() => {
                if session.as_ref().is_some_and(|handle| handle.is_closed()) {
                    session = None;
                    active_stop = None;
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
        }
        Err(error) => {
            sink.emit(RemoteEvent::Error(format!(
                "{error}. No remote command ran; led_mode and the pidfile were not changed."
            )));
        }
    }
}

async fn ensure_connected(
    settings: &Settings,
    session: &mut Option<Handle<Handler>>,
    sink: &Sink,
) -> bool {
    if session.as_ref().is_some_and(|handle| !handle.is_closed()) {
        return true;
    }
    connect(settings.clone(), session, sink).await;
    session.as_ref().is_some_and(|handle| !handle.is_closed())
}

async fn start_test(
    job: TestJob,
    session: &mut Option<Handle<Handler>>,
    active_stop: &mut Option<oneshot::Sender<()>>,
    sink: &Sink,
) {
    *active_stop = None;
    if !ensure_connected(&job.settings, session, sink).await {
        return;
    }
    let handle = session.as_ref().unwrap();
    let status = match read_status(handle).await {
        Ok(status) => status,
        Err(error) => {
            sink.emit(RemoteEvent::Error(format!(
                "cannot read the switch state before testing: {error}. No animation was started; led_mode and the pidfile are unknown."
            )));
            return;
        }
    };
    if status.is_running() {
        sink.emit(RemoteEvent::Error(format!(
            "a run is already live: PID {} with script {}. Stop it with Kill before testing. Nothing changed; {}.",
            status.pidfile.unwrap_or_default(),
            status.script,
            status.summary()
        )));
        return;
    }
    if status.pidfile_present {
        if status.alive {
            sink.emit(RemoteEvent::Log(format!(
                "pidfile PID {} is alive but is not lightlab-run.sh; the supervisor will unlink it without signalling",
                status.pidfile.unwrap_or_default()
            )));
        } else {
            sink.emit(RemoteEvent::Log(
                "the stale pidfile will be removed by the supervisor".into(),
            ));
        }
    }

    let name = match upload_animation(handle, &job.script_name, &job.script).await {
        Ok(name) => name,
        Err(error) => {
            sink.emit(RemoteEvent::Error(format!(
                "cannot upload the animation: {error}. No animation was started; {}.",
                status.summary()
            )));
            return;
        }
    };
    let animation_path = format!("{REMOTE_DIR}/{name}");
    let command = format!(
        "sh {} {} attached",
        sh_quote(REMOTE_SUPERVISOR),
        sh_quote(&animation_path)
    );
    let channel = match handle.channel_open_session().await {
        Ok(channel) => channel,
        Err(error) => {
            let state = state_or_unknown(handle).await;
            sink.emit(RemoteEvent::Error(format!(
                "cannot open the test channel: {error}. {state}."
            )));
            return;
        }
    };
    if let Err(error) = channel.exec(true, command).await {
        let state = state_or_unknown(handle).await;
        sink.emit(RemoteEvent::Error(format!(
            "cannot attach the test supervisor: {error}. {state}."
        )));
        return;
    }

    let (stop, stop_requested) = oneshot::channel();
    *active_stop = Some(stop);
    sink.emit(RemoteEvent::TestStarted {
        script: name.clone(),
    });
    spawn_test(channel, sink.clone(), stop_requested, name);
}

async fn state_or_unknown(handle: &Handle<Handler>) -> String {
    match read_status(handle).await {
        Ok(status) => status.summary(),
        Err(error) => format!("switch state unknown ({error})"),
    }
}

fn spawn_test(
    mut channel: russh::Channel<russh::client::Msg>,
    sink: Sink,
    mut stop_requested: oneshot::Receiver<()>,
    script: String,
) {
    tokio::spawn(async move {
        let mut stop = false;
        let mut deadline: Option<tokio::time::Instant> = None;
        let mut code = None;
        let mut error = None;
        loop {
            tokio::select! {
                message = channel.wait() => {
                    match message {
                        Some(ChannelMsg::Data { data }) => {
                            sink.emit(RemoteEvent::TestOutput(String::from_utf8_lossy(&data).to_string()));
                        }
                        Some(ChannelMsg::ExtendedData { data, .. }) => {
                            sink.emit(RemoteEvent::TestOutput(String::from_utf8_lossy(&data).to_string()));
                        }
                        Some(ChannelMsg::ExitStatus { exit_status }) => code = Some(exit_status),
                        Some(_) => {}
                        None => break,
                    }
                }
                result = &mut stop_requested, if !stop => {
                    if result.is_ok() {
                        stop = true;
                        deadline = Some(tokio::time::Instant::now() + Duration::from_secs(8));
                        if let Err(close_error) = channel.eof().await {
                            error = Some(format!("cannot close test stdin: {close_error}"));
                            break;
                        }
                    } else {
                        break;
                    }
                }
                _ = async { tokio::time::sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                    let _ = channel.close().await;
                    error = Some("the test supervisor did not exit within 8 seconds of Stop; run Kill to verify the switch".into());
                    break;
                }
            }
        }
        if code.is_none() && error.is_none() && !stop {
            error = Some(
                "the test channel closed without an exit status; the switch restores LEDs when its stdin closes"
                    .into(),
            );
        }
        sink.emit(RemoteEvent::TestExited {
            script,
            code,
            stopped: stop,
            error,
        });
    });
}

async fn read_status(handle: &Handle<Handler>) -> Result<RunStatus, String> {
    let output = exec(handle, STATUS_SCRIPT, None).await?;
    if output.code != Some(0) {
        return Err(output.error_text());
    }
    parse_status(&String::from_utf8_lossy(&output.stdout))
}

fn parse_status(text: &str) -> Result<RunStatus, String> {
    use base64::Engine;

    let mut mode = None;
    let mut present = None;
    let mut pid = None;
    let mut alive = None;
    let mut marker = None;
    let mut script = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "MODE" => mode = Some(value.to_string()),
            "PRESENT" => present = Some(value == "1"),
            "PID" => pid = value.parse::<u32>().ok(),
            "ALIVE" => alive = Some(value == "1"),
            "MARKER" => marker = Some(value == "1"),
            "SCRIPT" => {
                script = if value.is_empty() {
                    Some(String::new())
                } else {
                    Some(
                        String::from_utf8(
                            base64::engine::general_purpose::STANDARD
                                .decode(value)
                                .map_err(|error| {
                                    format!("cannot decode the switch status: {error}")
                                })?,
                        )
                        .map_err(|error| format!("cannot decode the switch status: {error}"))?,
                    )
                };
            }
            _ => {}
        }
    }
    Ok(RunStatus {
        led_mode: mode,
        pidfile_present: present.ok_or("switch status did not include the pidfile state")?,
        pidfile: pid,
        alive: alive.ok_or("switch status did not include the PID state")?,
        marker: marker.ok_or("switch status did not include the supervisor marker")?,
        script: script.unwrap_or_default(),
    })
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

    #[test]
    fn parses_running_status() {
        let status =
            parse_status("MODE=0\nPRESENT=1\nPID=123\nALIVE=1\nMARKER=1\nSCRIPT=a2l0dC5zaA==\n")
                .unwrap();
        assert!(status.is_running());
        assert_eq!(status.pidfile, Some(123));
        assert_eq!(status.script, "kitt.sh");
        assert!(status.summary().contains("running kitt.sh"));
    }

    #[test]
    fn parses_stale_and_unrelated_pidfiles() {
        let stale =
            parse_status("MODE=1\nPRESENT=1\nPID=123\nALIVE=0\nMARKER=0\nSCRIPT=\n").unwrap();
        assert!(!stale.is_running());
        assert!(stale.summary().contains("pidfile PID 123"));
        let unrelated =
            parse_status("MODE=unknown\nPRESENT=1\nPID=456\nALIVE=1\nMARKER=0\nSCRIPT=\n").unwrap();
        assert!(!unrelated.is_running());
        assert!(unrelated.summary().contains("not a lightlab supervisor"));
    }
}
