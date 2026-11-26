use std::io::Write;
use std::path::Path;

use russh::keys::{parse_public_key_base64, HashAlg};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKeyState {
    Pinned,
    Unknown,
    Changed { pinned: String },
}

pub fn check(
    path: &Path,
    host: &str,
    port: u16,
    presented_blob: &str,
) -> Result<HostKeyState, String> {
    let target = target(host, port);
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(HostKeyState::Unknown)
        }
        Err(error) => {
            return Err(format!(
                "cannot read pinned host keys from {}: {error}",
                path.display()
            ))
        }
    };
    let mut saw_target = false;
    let mut pinned = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(entry_target) = fields.next() else {
            continue;
        };
        if entry_target != target {
            continue;
        }
        saw_target = true;
        let Some(key_type) = fields.next() else {
            continue;
        };
        let Some(blob) = fields.next() else {
            continue;
        };
        if blob == presented_blob {
            return Ok(HostKeyState::Pinned);
        }
        if pinned.is_none() {
            pinned = Some(format!("{key_type} {}", fingerprint(blob)));
        }
    }
    if saw_target {
        Ok(HostKeyState::Changed {
            pinned: pinned.unwrap_or_else(|| "an unreadable pinned key".into()),
        })
    } else {
        Ok(HostKeyState::Unknown)
    }
}

pub fn pin(path: &Path, host: &str, port: u16, key_type: &str, blob: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let needs_newline = std::fs::read(path)
        .map(|bytes| !bytes.is_empty() && !bytes.ends_with(b"\n"))
        .unwrap_or(false);
    if needs_newline {
        writeln!(file).map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    }
    writeln!(file, "{} {} {}", target(host, port), key_type, blob)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

pub fn fingerprint(blob: &str) -> String {
    parse_public_key_base64(blob)
        .map(|key| key.fingerprint(HashAlg::Sha256).to_string())
        .unwrap_or_else(|_| format!("unparseable key {blob}"))
}

pub fn target(host: &str, port: u16) -> String {
    let host = host.trim().trim_start_matches('[').trim_end_matches(']');
    format!("[{host}]:{port}")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    const BLOB_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const BLOB_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIN+Wjn4+4Fcvl2JbKpggT+wCRxpSvtqqpVrQrKN1/A22";

    fn temp_path() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir()
            .join(format!("lightlab-known-hosts-{}-{n}", std::process::id()))
            .join("known_hosts")
    }

    #[test]
    fn first_use_is_unknown_then_pins() {
        let path = temp_path();
        assert_eq!(
            check(&path, "switch.local", 22, BLOB_A).unwrap(),
            HostKeyState::Unknown
        );
        pin(&path, "switch.local", 22, "ssh-ed25519", BLOB_A).unwrap();
        assert_eq!(
            check(&path, "switch.local", 22, BLOB_A).unwrap(),
            HostKeyState::Pinned
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("[switch.local]:22 ssh-ed25519 "));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn changed_key_is_rejected() {
        let path = temp_path();
        pin(&path, "switch.local", 2222, "ssh-ed25519", BLOB_A).unwrap();
        let state = check(&path, "switch.local", 2222, BLOB_B).unwrap();
        match state {
            HostKeyState::Changed { pinned } => {
                assert!(pinned.contains("SHA256:"), "{pinned}");
            }
            other => panic!("expected changed, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn target_brackets_ipv6_and_ports() {
        assert_eq!(target("switch.local", 22), "[switch.local]:22");
        assert_eq!(target("fe80::1", 2222), "[fe80::1]:2222");
        assert_eq!(target("[fe80::1]", 22), "[fe80::1]:22");
    }
}
