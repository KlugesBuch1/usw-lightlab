use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use std::os::windows::process::CommandExt;

use crate::led::{parse_line, LedLine};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PID_MARKER: &str = "__LIGHTLAB_PID__ ";
const MAX_LOG_LINE: usize = 32 * 1024;

#[derive(Debug)]
pub enum PreviewEvent {
    Started(u32),
    Frame(Vec<LedLine>),
    Log(String),
    Exited(Option<i32>),
}

pub struct Preview {
    events: Receiver<PreviewEvent>,
    running: bool,
    stopping: bool,
    wsl_error: Option<String>,
    pid: Arc<Mutex<Option<u32>>>,
    stop_requested: Arc<AtomicBool>,
    script: Option<PathBuf>,
    started: Option<Instant>,
}

impl Preview {
    pub fn new() -> Self {
        let wsl_error = detect_wsl().err();
        let (_tx, events) = mpsc::channel();
        Self {
            events,
            running: false,
            stopping: false,
            wsl_error,
            pid: Arc::new(Mutex::new(None)),
            stop_requested: Arc::new(AtomicBool::new(false)),
            script: None,
            started: None,
        }
    }

    pub fn wsl_error(&self) -> Option<&str> {
        self.wsl_error.as_deref()
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping
    }

    pub fn script(&self) -> Option<&Path> {
        self.script.as_deref()
    }

    pub fn elapsed(&self) -> Duration {
        self.started.map(|time| time.elapsed()).unwrap_or_default()
    }

    pub fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap()
    }

    pub fn start(&mut self, path: &Path) -> Result<(), String> {
        if self.running {
            return Err("a preview is already running".into());
        }
        if let Some(error) = &self.wsl_error {
            return Err(error.clone());
        }
        let canonical = std::fs::canonicalize(path)
            .map_err(|error| format!("cannot open script {}: {error}", path.display()))?;
        if !canonical.is_file() {
            return Err(format!("script is not a file: {}", canonical.display()));
        }
        let wsl_path = to_wsl_path(&canonical)?;
        let script = "LIGHTLAB_EMIT=1 setsid sh ".to_string()
            + &shell_quote(&wsl_path)
            + " & p=$!; printf '__LIGHTLAB_PID__ %s\\n' \"$p\" >&2; wait \"$p\"";
        let mut child = Command::new("wsl.exe")
            .args(["-e", "sh", "-c", script.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("cannot start WSL: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "WSL stdout was not captured".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "WSL stderr was not captured".to_string())?;

        let (tx, rx) = mpsc::channel();
        let pid = Arc::clone(&self.pid);
        *pid.lock().unwrap() = None;
        self.stop_requested = Arc::new(AtomicBool::new(false));
        let stop_requested = Arc::clone(&self.stop_requested);

        spawn_stdout_reader(stdout, tx.clone());
        spawn_stderr_reader(stderr, tx.clone(), Arc::clone(&pid), stop_requested);
        spawn_waiter(child, tx);

        self.events = rx;
        self.running = true;
        self.stopping = false;
        self.script = Some(canonical);
        self.started = Some(Instant::now());
        Ok(())
    }

    pub fn stop(&mut self) {
        if !self.running || self.stopping {
            return;
        }
        self.stopping = true;
        self.stop_requested.store(true, Ordering::SeqCst);
        let recorded = *self.pid.lock().unwrap();
        if let Some(pid) = recorded {
            spawn_kill(pid);
        }
    }

    pub fn poll(&mut self) -> Vec<PreviewEvent> {
        let mut events = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => {
                    if let PreviewEvent::Exited(_) = event {
                        self.running = false;
                        self.stopping = false;
                        self.started = None;
                    }
                    events.push(event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        events
    }

    pub fn shutdown(&mut self) {
        if !self.running {
            return;
        }
        self.stopping = true;
        self.stop_requested.store(true, Ordering::SeqCst);
        let pid = self.wait_for_pid(Duration::from_millis(1500));
        if let Some(pid) = pid {
            let started = Instant::now();
            if let Some(mut helper) = spawn_kill(pid) {
                while started.elapsed() < Duration::from_secs(5) {
                    match helper.try_wait() {
                        Ok(Some(_)) | Err(_) => break,
                        Ok(None) => thread::sleep(Duration::from_millis(25)),
                    }
                }
            }
        }
        self.running = false;
        self.stopping = false;
    }

    fn wait_for_pid(&self, timeout: Duration) -> Option<u32> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Some(pid) = *self.pid.lock().unwrap() {
                return Some(pid);
            }
            thread::sleep(Duration::from_millis(20));
        }
        *self.pid.lock().unwrap()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn spawn_stdout_reader<R: Read + Send + 'static>(stdout: R, tx: Sender<PreviewEvent>) {
    thread::spawn(move || {
        let mut reader = stdout;
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(error) => {
                    let _ = tx.send(PreviewEvent::Log(format!("preview stdout: {error}")));
                    break;
                }
            }
            let mut frames = Vec::new();
            while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
                let line = String::from_utf8_lossy(&buffer[..newline]).to_string();
                buffer.drain(..=newline);
                match parse_line(&line) {
                    Some(parsed @ LedLine::Color { .. }) => frames.push(parsed),
                    Some(LedLine::Mode(_)) => {}
                    None => {
                        let line = line.trim();
                        if !line.is_empty() {
                            let _ = tx.send(PreviewEvent::Log(format!("preview: {line}")));
                        }
                    }
                }
            }
            if buffer.len() > MAX_LOG_LINE {
                buffer.clear();
                let _ = tx.send(PreviewEvent::Log(
                    "preview: dropped an oversized output line".into(),
                ));
            }
            if !frames.is_empty() {
                let _ = tx.send(PreviewEvent::Frame(frames));
            }
        }
        if !buffer.is_empty() {
            let line = String::from_utf8_lossy(&buffer).trim().to_string();
            if !line.is_empty() {
                let _ = tx.send(PreviewEvent::Log(format!("preview: {line}")));
            }
        }
    });
}

fn spawn_stderr_reader<R: Read + Send + 'static>(
    stderr: R,
    tx: Sender<PreviewEvent>,
    pid: Arc<Mutex<Option<u32>>>,
    stop_requested: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Some(rest) = line.strip_prefix(PID_MARKER) {
                if let Ok(value) = rest.trim().parse::<u32>() {
                    *pid.lock().unwrap() = Some(value);
                    let _ = tx.send(PreviewEvent::Started(value));
                    if stop_requested.load(Ordering::SeqCst) {
                        spawn_kill(value);
                    }
                }
                continue;
            }
            let line = line.trim_end();
            if !line.is_empty() {
                let _ = tx.send(PreviewEvent::Log(format!("preview: {line}")));
            }
        }
    });
}

fn spawn_waiter(mut child: Child, tx: Sender<PreviewEvent>) {
    thread::spawn(move || {
        let code = match child.wait() {
            Ok(status) => status.code(),
            Err(error) => {
                let _ = tx.send(PreviewEvent::Log(format!("preview process: {error}")));
                None
            }
        };
        let _ = tx.send(PreviewEvent::Exited(code));
    });
}

fn spawn_kill(pid: u32) -> Option<Child> {
    let script = format!(
        "p={pid}; kill -TERM -$p 2>/dev/null || kill -TERM $p 2>/dev/null; i=0; while kill -0 $p 2>/dev/null && [ $i -lt 40 ]; do sleep 0.05; i=$((i+1)); done; kill -KILL -$p 2>/dev/null || kill -KILL $p 2>/dev/null; true"
    );
    Command::new("wsl.exe")
        .args(["-e", "sh", "-c", script.as_str()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()
}

fn detect_wsl() -> Result<(), String> {
    let mut child = Command::new("wsl.exe")
        .args(["-e", "sh", "-c", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| {
            format!("WSL is unavailable ({error}). Run 'wsl --install', then reopen lightlab.")
        })?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => {
                return Err(
                    "WSL has no ready Linux distribution. Run 'wsl --install -d Ubuntu', then reopen lightlab."
                        .into(),
                )
            }
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                thread::sleep(Duration::from_millis(40));
            }
            Ok(None) => {
                let _ = child.kill();
                return Err(
                    "WSL did not respond within 5 seconds. Run 'wsl --status', then reopen lightlab."
                        .into(),
                );
            }
            Err(error) => {
                return Err(format!(
                    "WSL could not be checked ({error}). Run 'wsl --status', then reopen lightlab."
                ))
            }
        }
    }
}

fn to_wsl_path(path: &Path) -> Result<String, String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    drive_path_to_wsl(&canonical.to_string_lossy())
}

fn drive_path_to_wsl(path: &str) -> Result<String, String> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    if path.starts_with(r"\\") || path.starts_with("//") || path.starts_with("UNC\\") {
        return Err("WSL preview needs a script on a local drive, not a network share.".into());
    }
    let bytes = path.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || (bytes[2] != b'\\' && bytes[2] != b'/')
    {
        return Err(format!(
            "cannot map {path} to a WSL path; choose a script on a local drive."
        ));
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let rest = path[3..].replace('\\', "/");
    if rest.is_empty() {
        Ok(format!("/mnt/{drive}"))
    } else {
        Ok(format!("/mnt/{drive}/{rest}"))
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_drive_paths_into_wsl() {
        assert_eq!(
            drive_path_to_wsl(r"C:\Users\me\kitt.sh").unwrap(),
            "/mnt/c/Users/me/kitt.sh"
        );
        assert_eq!(
            drive_path_to_wsl(r"\\?\C:\Users\me\kitt.sh").unwrap(),
            "/mnt/c/Users/me/kitt.sh"
        );
        assert_eq!(
            drive_path_to_wsl("D:/animations/kitt.sh").unwrap(),
            "/mnt/d/animations/kitt.sh"
        );
        let quoted = shell_quote("/mnt/c/Users/me/it's here.sh");
        assert_eq!(quoted, "'/mnt/c/Users/me/it'\\''s here.sh'");
    }

    #[test]
    fn rejects_network_paths() {
        for path in [
            r"\\server\share\kitt.sh",
            r"\\?\UNC\server\share\kitt.sh",
            "//server/share/kitt.sh",
        ] {
            let error = drive_path_to_wsl(path).unwrap_err();
            assert!(error.contains("local drive"));
        }
    }

    #[test]
    fn resolves_a_real_file() {
        let path = std::env::current_dir()
            .unwrap()
            .join("animations")
            .join("kitt.sh");
        let mapped = to_wsl_path(&path).unwrap();
        assert!(mapped.starts_with("/mnt/"), "{mapped}");
        assert!(mapped.ends_with("/kitt.sh"), "{mapped}");
    }
}
