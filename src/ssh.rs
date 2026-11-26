use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::agent::client::AgentClient;
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use tokio::net::TcpStream;
use tokio::sync::oneshot;

use crate::hostkeys::{self, HostKeyState};
use crate::settings::AuthMode;

#[derive(Clone, Debug)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: AuthMode,
    pub key_path: String,
    pub password: String,
}

pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub key_type: String,
    pub fingerprint: String,
    reply: Option<oneshot::Sender<bool>>,
}

impl HostKeyPrompt {
    pub fn answer(mut self, accept: bool) {
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(accept);
        }
    }
}

pub type PromptSink = Arc<dyn Fn(HostKeyPrompt) + Send + Sync>;

pub struct Handler {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    prompt: PromptSink,
    failure: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Handler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let public = server_public_key.public_key();
        let openssh = match public.to_openssh() {
            Ok(text) => text,
            Err(error) => {
                self.fail(format!("cannot read the switch host key: {error}"));
                return Ok(false);
            }
        };
        let mut fields = openssh.split_whitespace();
        let key_type = fields.next().unwrap_or("unknown").to_string();
        let Some(blob) = fields.next() else {
            self.fail("the switch presented an unreadable host key".into());
            return Ok(false);
        };
        let fingerprint = hostkeys::fingerprint(blob);
        let state = match hostkeys::check(&self.known_hosts, &self.host, self.port, blob) {
            Ok(state) => state,
            Err(error) => {
                self.fail(error);
                return Ok(false);
            }
        };
        match state {
            HostKeyState::Pinned => Ok(true),
            HostKeyState::Changed { pinned } => {
                self.fail(format!(
                    "host key for {}:{} changed. Pinned {}, switch presented {} ({}). Refusing to connect. If the switch was reimaged, remove its line from {} and reconnect.",
                    self.host,
                    self.port,
                    pinned,
                    fingerprint,
                    key_type,
                    self.known_hosts.display()
                ));
                Ok(false)
            }
            HostKeyState::Unknown => {
                let (reply, answer) = oneshot::channel();
                (self.prompt)(HostKeyPrompt {
                    host: self.host.clone(),
                    port: self.port,
                    key_type: key_type.clone(),
                    fingerprint: fingerprint.clone(),
                    reply: Some(reply),
                });
                let accepted = matches!(answer.await, Ok(true));
                if !accepted {
                    self.fail("the switch host key was not trusted; connection closed".into());
                    return Ok(false);
                }
                if let Err(error) =
                    hostkeys::pin(&self.known_hosts, &self.host, self.port, &key_type, blob)
                {
                    self.fail(format!(
                        "host key was accepted but could not be pinned: {error}"
                    ));
                    return Ok(false);
                }
                Ok(true)
            }
        }
    }
}

impl Handler {
    fn fail(&self, reason: String) {
        *self.failure.lock().unwrap() = Some(reason);
    }
}

pub async fn connect(
    settings: &Settings,
    prompt: PromptSink,
    known_hosts: PathBuf,
) -> Result<Handle<Handler>, String> {
    if settings.host.trim().is_empty() {
        return Err("no switch host was entered".into());
    }
    if settings.user.trim().is_empty() {
        return Err("no SSH user was entered".into());
    }
    if settings.port == 0 {
        return Err("SSH port must be between 1 and 65535".into());
    }
    let endpoint = format!("{}:{}", settings.host.trim(), settings.port);
    let stream = tokio::time::timeout(
        Duration::from_secs(12),
        TcpStream::connect((settings.host.trim(), settings.port)),
    )
    .await
    .map_err(|_| format!("connection to {endpoint} timed out after 12 seconds"))?
    .map_err(|error| format!("cannot reach {endpoint}: {error}"))?;
    let _ = stream.set_nodelay(true);

    let failure = Arc::new(Mutex::new(None));
    let handler = Handler {
        host: settings.host.trim().to_string(),
        port: settings.port,
        known_hosts,
        prompt,
        failure: Arc::clone(&failure),
    };
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        ..Default::default()
    });
    let mut handle: Handle<Handler> = match client::connect_stream(config, stream, handler).await {
        Ok(handle) => handle,
        Err(error) => {
            return Err(failure
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| format!("SSH handshake with {endpoint} failed: {error}")))
        }
    };

    authenticate(&mut handle, settings).await?;
    Ok(handle)
}

async fn authenticate(handle: &mut Handle<Handler>, settings: &Settings) -> Result<(), String> {
    let user = settings.user.trim().to_string();
    match settings.auth {
        AuthMode::Password => {
            if settings.password.is_empty() {
                return Err("password authentication is selected but the password is empty".into());
            }
            let result = handle
                .authenticate_password(user, settings.password.clone())
                .await
                .map_err(|error| format!("password authentication failed: {error}"))?;
            if result.success() {
                Ok(())
            } else {
                Err("password authentication failed; check the password and that root password login is enabled on the switch".into())
            }
        }
        AuthMode::Key => {
            if settings.key_path.trim().is_empty() {
                return Err("key authentication is selected but no key file is set".into());
            }
            let key = load_secret_key(settings.key_path.trim(), None).map_err(|error| {
                format!(
                    "cannot load SSH key {}: {error}. Passphrase-protected keys must be added to the Windows OpenSSH agent and used with agent authentication.",
                    settings.key_path
                )
            })?;
            let hash = if key.algorithm().is_rsa() {
                handle
                    .best_supported_rsa_hash()
                    .await
                    .map_err(|error| {
                        format!("cannot negotiate the RSA signature algorithm: {error}")
                    })?
                    .flatten()
            } else {
                None
            };
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            let result = handle
                .authenticate_publickey(user, key)
                .await
                .map_err(|error| format!("key authentication failed: {error}"))?;
            if result.success() {
                Ok(())
            } else {
                Err(format!(
                    "key authentication failed for {}; the switch did not accept this key",
                    settings.key_path
                ))
            }
        }
        AuthMode::Agent => {
            let mut agent = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
                .await
                .map_err(|error| {
                    format!(
                        "cannot reach the Windows OpenSSH agent: {error}. Start the ssh-agent service and run ssh-add."
                    )
                })?;
            if let Some(binding) = handle.agent_session_binding() {
                let _ = agent.bind_session(binding, false).await;
            }
            let identities = agent.request_identities().await.map_err(|error| {
                format!("cannot list keys in the Windows OpenSSH agent: {error}")
            })?;
            if identities.is_empty() {
                return Err(
                    "the Windows OpenSSH agent has no identities; run ssh-add and reconnect".into(),
                );
            }
            for identity in identities {
                let public = identity.public_key().into_owned();
                let hash = if public.algorithm().is_rsa() {
                    handle
                        .best_supported_rsa_hash()
                        .await
                        .map_err(|error| {
                            format!("cannot negotiate the RSA signature algorithm: {error}")
                        })?
                        .flatten()
                } else {
                    None
                };
                match handle
                    .authenticate_publickey_with(user.clone(), public, hash, &mut agent)
                    .await
                {
                    Ok(result) if result.success() => return Ok(()),
                    Ok(_) => continue,
                    Err(error) => {
                        return Err(format!("the SSH agent could not sign the login: {error}"))
                    }
                }
            }
            Err("the SSH agent keys were not accepted by the switch; run ssh-add -l and check the switch authorized_keys".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_answers_once() {
        let (tx, mut rx) = oneshot::channel();
        let prompt = HostKeyPrompt {
            host: "switch".into(),
            port: 22,
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:test".into(),
            reply: Some(tx),
        };
        prompt.answer(true);
        assert!(rx.try_recv().unwrap());
    }
}
