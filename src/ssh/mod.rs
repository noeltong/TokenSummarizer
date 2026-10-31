//! 内置 SSH 引擎（russh）：密码/键盘交互/OpenSSH·PEM 私钥认证，
//! TOFU 主机密钥固定，exec 与 SFTP。不调用系统 ssh.exe，不依赖 ssh-agent。

pub mod tester;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg};
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use thiserror::Error;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(45);
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub enum AuthKind {
    Password(String),
    PrivateKey {
        path: String,
        passphrase: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: AuthKind,
    /// 期望的主机密钥 SHA-256 指纹；None 表示首次连接（TOFU 记录）。
    pub expected_fingerprint: Option<String>,
}

impl ConnectOptions {
    /// 从持久化配置构造；密码/口令解密失败报错。
    pub fn from_server(server: &crate::config::ServerConfig) -> Result<Self, SshError> {
        let auth = match server.auth_type {
            crate::config::AuthType::Password => match server.password() {
                Some(password) => AuthKind::Password(password),
                None => {
                    return Err(SshError::PrivateKey(
                        crate::i18n::saved_password_undecryptable(),
                    ))
                }
            },
            crate::config::AuthType::PrivateKey => {
                let path = server
                    .private_key_path
                    .clone()
                    .unwrap_or_default();
                if path.trim().is_empty() {
                    return Err(SshError::PrivateKey(crate::i18n::no_key_path()));
                }
                AuthKind::PrivateKey {
                    path,
                    passphrase: server.key_passphrase(),
                }
            }
        };
        Ok(Self {
            host: server.host.clone(),
            port: server.port,
            username: server.username.clone(),
            auth,
            expected_fingerprint: server.host_key_fingerprint_sha256.clone(),
        })
    }
}

// Display 消息在显示时经 i18n 查表求值——错误对象构造时的语言不影响最终文案。
#[derive(Debug, Error)]
pub enum SshError {
    #[error("{}", crate::i18n::net_failed(.0))]
    Network(String),
    #[error("{}", crate::i18n::timeout_msg(.secs))]
    Timeout { secs: u64 },
    #[error("{}", crate::i18n::host_key_changed())]
    HostKeyChanged,
    #[error("{}", crate::i18n::auth_failed())]
    AuthFailed,
    #[error("{}", crate::i18n::key_unusable(.0))]
    PrivateKey(String),
    #[error("{}", crate::i18n::cmd_failed(.0))]
    Command(String),
    #[error("{}", crate::i18n::sftp_unavailable(.0))]
    Sftp(String),
    #[error("{}", crate::i18n::disconnected(.0))]
    Disconnected(String),
}

impl SshError {
    pub fn from_russh(err: russh::Error) -> Self {
        use russh::Error as E;
        match &err {
            E::UnknownKey => SshError::HostKeyChanged,
            E::Disconnect => SshError::Disconnected(err.to_string()),
            _ => {
                let text = err.to_string();
                if text.contains("auth")
                    || text.contains("Authentication")
                    || text.contains("permission")
                {
                    SshError::AuthFailed
                } else {
                    SshError::Network(text)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 主机密钥 TOFU 门
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct HostKeyGate {
    expected: Option<String>,
    seen: Option<String>,
}

struct ClientHandler {
    gate: Arc<Mutex<HostKeyGate>>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = match server_public_key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                key.fingerprint(HashAlg::Sha256).to_string()
            }
            russh::keys::PublicKeyOrCertificate::Certificate(cert) => {
                cert.public_key().fingerprint(HashAlg::Sha256).to_string()
            }
        };
        let Ok(mut gate) = self.gate.lock() else {
            return Ok(false);
        };
        gate.seen = Some(fingerprint.clone());
        match &gate.expected {
            // 宽松比较：忽略 "SHA256:" 前缀差异（有的工具/旧数据存裸 base64）
            Some(expected) => {
                let left = crate::config::normalize_fingerprint(expected);
                let right = crate::config::normalize_fingerprint(&fingerprint);
                Ok(match (left, right) {
                    (Some(left), Some(right)) => left == right,
                    _ => expected == &fingerprint,
                })
            }
            None => Ok(true), // 首次连接：接受并记录
        }
    }
}

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------

pub struct Session {
    handle: Handle<ClientHandler>,
    gate: Arc<Mutex<HostKeyGate>>,
}

impl Session {
    pub async fn connect(options: &ConnectOptions) -> Result<Session, SshError> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(90)),
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 4,
            ..Default::default()
        });
        let gate = Arc::new(Mutex::new(HostKeyGate {
            expected: options.expected_fingerprint.clone(),
            seen: None,
        }));
        let handler = ClientHandler { gate: gate.clone() };

        let connect_fut = client::connect(
            config,
            (options.host.as_str(), options.port),
            handler,
        );
        let mut handle = tokio::time::timeout(CONNECT_TIMEOUT, connect_fut)
            .await
            .map_err(|_| SshError::Timeout {
                secs: CONNECT_TIMEOUT.as_secs(),
            })?
            .map_err(SshError::from_russh)?;

        match &options.auth {
            AuthKind::Password(password) => {
                let result = handle
                    .authenticate_password(options.username.clone(), password.clone())
                    .await
                    .map_err(SshError::from_russh)?;
                if result.success() {
                    // ok
                } else {
                    // 密码认证失败 → 键盘交互回退（对只开 kbdint 的服务器有效）
                    keyboard_interactive_fallback(&mut handle, &options.username, password).await?;
                }
            }
            AuthKind::PrivateKey { path, passphrase } => {
                let key = load_private_key(path, passphrase.as_deref())?;
                let hash = rsa_hash_for(&mut handle, &key).await;
                let result = handle
                    .authenticate_publickey(
                        options.username.clone(),
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await
                    .map_err(SshError::from_russh)?;
                if !result.success() {
                    return Err(SshError::AuthFailed);
                }
            }
        }

        Ok(Session { handle, gate })
    }

    /// 本次连接观察到的主机密钥指纹（调用方在首次连接后持久化）。
    pub fn seen_fingerprint(&self) -> Option<String> {
        self.gate.lock().ok().and_then(|gate| gate.seen.clone())
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn disconnect(&self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "bye", "en")
            .await;
    }

    /// 执行命令并收集 stdout（UTF-8 宽松解码）。
    pub async fn exec(&self, command: &str, timeout: Duration) -> Result<String, SshError> {
        let exec_inner = async {
            let mut channel = self
                .handle
                .channel_open_session()
                .await
                .map_err(SshError::from_russh)?;
            channel
                .exec(true, command)
                .await
                .map_err(SshError::from_russh)?;
            let mut stdout: Vec<u8> = Vec::new();
            let mut stderr_tail: Vec<String> = Vec::new();
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                    ChannelMsg::ExtendedData { data, .. } => {
                        let text = String::from_utf8_lossy(&data).into_owned();
                        stderr_tail.push(text);
                        if stderr_tail.len() > 6 {
                            stderr_tail.remove(0);
                        }
                    }
                    ChannelMsg::ExitStatus { exit_status } => {
                        if exit_status != 0 {
                            let detail = stderr_tail.join("").trim().to_string();
                            return Err(SshError::Command(if detail.is_empty() {
                                format!("退出码 {exit_status}")
                            } else {
                                detail
                            }));
                        }
                    }
                    _ => {}
                }
            }
            Ok(String::from_utf8_lossy(&stdout).into_owned())
        };
        tokio::time::timeout(timeout, exec_inner)
            .await
            .map_err(|_| SshError::Command("命令执行超时".into()))?
    }

    /// 打开 SFTP 子系统会话。
    pub async fn sftp(&self) -> Result<SftpSession, SshError> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(SshError::from_russh)?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(SshError::from_russh)?;
        SftpSession::new(channel.into_stream())
            .await
            .map_err(|err| SshError::Sftp(clean_sftp_error(&err)))
    }
}

fn clean_sftp_error(err: &russh_sftp::client::error::Error) -> String {
    let text = err.to_string();
    let text = text.trim();
    if text.is_empty() {
        "未知错误".to_string()
    } else {
        text.chars().take(200).collect()
    }
}

async fn keyboard_interactive_fallback(
    handle: &mut Handle<ClientHandler>,
    username: &str,
    password: &str,
) -> Result<(), SshError> {
    let mut response = handle
        .authenticate_keyboard_interactive_start(username, None::<String>)
        .await
        .map_err(SshError::from_russh)?;
    // 最多响应 4 轮提示（OTP 等场景），每轮都回填同一密码
    for _ in 0..4 {
        match response {
            client::KeyboardInteractiveAuthResponse::Success => return Ok(()),
            client::KeyboardInteractiveAuthResponse::Failure { .. } => {
                return Err(SshError::AuthFailed);
            }
            client::KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts
                    .iter()
                    .map(|_| password.to_string())
                    .collect::<Vec<_>>();
                response = handle
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(SshError::from_russh)?;
            }
        }
    }
    Err(SshError::AuthFailed)
}

/// 加载 OpenSSH/PEM 私钥；拒绝 PuTTY .ppk。
pub fn load_private_key(path: &str, passphrase: Option<&str>) -> Result<PrivateKey, SshError> {
    let content = std::fs::read_to_string(path)
        .map_err(|err| SshError::PrivateKey(format!("无法读取文件（{err}）")))?;
    if content.trim_start().starts_with("PuTTY-User-Key-File") {
        return Err(SshError::PrivateKey(
            "不支持 PuTTY .ppk 私钥，请先转换为 OpenSSH 格式".to_string(),
        ));
    }
    russh::keys::load_secret_key(path, passphrase).map_err(|err| {
        let text = err.to_string();
        if text.to_lowercase().contains("passphrase") {
            SshError::PrivateKey("私钥口令不正确".to_string())
        } else {
            SshError::PrivateKey(format!("解析失败（{text}）"))
        }
    })
}

async fn rsa_hash_for(
    handle: &mut Handle<ClientHandler>,
    key: &PrivateKey,
) -> Option<russh::keys::HashAlg> {
    if !key.algorithm().is_rsa() {
        return None;
    }
    match handle.best_supported_rsa_hash().await {
        Ok(Some(hash)) => hash,
        _ => Some(HashAlg::Sha256),
    }
}

/// 建立连接 + 认证 + 主机密钥检查的轻量包装（测试连接用）。
pub async fn quick_connect(options: &ConnectOptions) -> Result<Session, SshError> {
    Session::connect(options).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppk_files_are_rejected() {
        let dir = crate::parsing::token::testutil::TempDir::new();
        let path = dir.path().join("key.ppk");
        std::fs::write(&path, "PuTTY-User-Key-File-2: ssh-rsa\n...").unwrap();
        let result = load_private_key(path.to_str().unwrap(), None);
        assert!(matches!(result, Err(SshError::PrivateKey(msg)) if msg.contains(".ppk")));
    }
}
