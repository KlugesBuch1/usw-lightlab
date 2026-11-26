use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const SERVICE: &str = "lightlab";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    #[default]
    Key,
    Agent,
    Password,
}

impl AuthMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Key => "Key file",
            Self::Agent => "SSH agent",
            Self::Password => "Password",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMode,
    pub key_path: String,
    pub script_path: String,
    pub remember_password: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            user: "root".into(),
            auth: AuthMode::Key,
            key_path: default_key_path(),
            script_path: String::new(),
            remember_password: true,
        }
    }
}

pub fn app_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "APPDATA is not set; cannot locate the lightlab config directory.".to_string()
        })?;
    Ok(base.join("lightlab"))
}

pub fn known_hosts_path() -> Result<PathBuf, String> {
    Ok(app_dir()?.join("known_hosts"))
}

pub fn load() -> Result<Config, String> {
    let path = app_dir()?.join("config.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("cannot read {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

pub fn save(config: &Config) -> Result<(), String> {
    let dir = app_dir()?;
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    let path = dir.join("config.json");
    let text = serde_json::to_string_pretty(config)
        .map_err(|error| format!("cannot encode settings: {error}"))?;
    std::fs::write(&path, text).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

pub fn load_password(user: &str, host: &str, port: u16) -> Result<Option<String>, String> {
    let account = account(user, host, port);
    let entry = keyring::Entry::new(SERVICE, &account)
        .map_err(|error| format!("cannot open Windows Credential Manager: {error}"))?;
    match entry.get_password() {
        Ok(password) => Ok(Some(password)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!(
            "cannot read the saved password for {account} from Windows Credential Manager: {error}"
        )),
    }
}

pub fn save_password(user: &str, host: &str, port: u16, password: &str) -> Result<(), String> {
    let account = account(user, host, port);
    let entry = keyring::Entry::new(SERVICE, &account)
        .map_err(|error| format!("cannot open Windows Credential Manager: {error}"))?;
    entry.set_password(password).map_err(|error| {
        format!("cannot save the password for {account} to Windows Credential Manager: {error}")
    })
}

pub fn forget_password(user: &str, host: &str, port: u16) -> Result<(), String> {
    let account = account(user, host, port);
    let entry = keyring::Entry::new(SERVICE, &account)
        .map_err(|error| format!("cannot open Windows Credential Manager: {error}"))?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "cannot remove the saved password for {account} from Windows Credential Manager: {error}"
        )),
    }
}

fn account(user: &str, host: &str, port: u16) -> String {
    format!("{user}@{host}:{port}")
}

fn default_key_path() -> String {
    let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
        return String::new();
    };
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let candidate = home.join(".ssh").join(name);
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }
    String::new()
}
