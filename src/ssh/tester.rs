//! 「测试连接」：依次验证 SSH 登录、主机密钥与 SFTP 可用，输出可读的本地化报告。

use super::{AuthKind, ConnectOptions, Session, SshError};
use crate::config::ServerConfig;

#[derive(Debug, Clone)]
pub struct TestReport {
    pub ok: bool,
    /// 逐项检查结果 (项目, 通过与否, 说明)。
    pub steps: Vec<(&'static str, bool, String)>,
    /// 首次连接观察到的主机密钥指纹（调用方可持久化）。
    pub seen_fingerprint: Option<String>,
}

impl TestReport {
    fn summary(&self) -> String {
        let failed: Vec<&str> = self
            .steps
            .iter()
            .filter(|(_, ok, _)| !ok)
            .map(|(name, _, _)| *name)
            .collect();
        if failed.is_empty() {
            crate::i18n::all_checks_passed(self.steps.len())
        } else {
            crate::i18n::checks_failed(&failed)
        }
    }

    pub fn headline(&self) -> String {
        crate::i18n::report_headline(self.ok, &self.summary())
    }
}

pub async fn test_connection(server: &ServerConfig, fingerprint_override: Option<&str>) -> TestReport {
    let mut report = TestReport {
        ok: false,
        steps: Vec::new(),
        seen_fingerprint: None,
    };

    let auth = match server.auth_type {
        crate::config::AuthType::Password => match server.password() {
            Some(password) => AuthKind::Password(password),
            None => {
                report
                    .steps
                    .push((
                        crate::i18n::texts().step_ssh_login,
                        false,
                        crate::i18n::saved_password_reenter(),
                    ));
                return report;
            }
        },
        crate::config::AuthType::PrivateKey => {
            let Some(path) = server.private_key_path.as_deref() else {
                report
                    .steps
                    .push((
                        crate::i18n::texts().step_ssh_login,
                        false,
                        crate::i18n::no_key_selected(),
                    ));
                return report;
            };
            AuthKind::PrivateKey {
                path: path.to_string(),
                passphrase: server.key_passphrase(),
            }
        }
    };

    let options = ConnectOptions {
        host: server.host.clone(),
        port: server.port,
        username: server.username.clone(),
        auth,
        expected_fingerprint: fingerprint_override
            .map(str::to_string)
            .or_else(|| server.host_key_fingerprint_sha256.clone()),
    };

    let session = match Session::connect(&options).await {
        Ok(session) => session,
        Err(err) => {
            let texts = crate::i18n::texts();
            let (name, detail) = match &err {
                SshError::HostKeyChanged => (texts.step_host_key, err.to_string()),
                SshError::AuthFailed => (texts.step_ssh_login, err.to_string()),
                SshError::PrivateKey(msg) => (texts.step_ssh_login, msg.clone()),
                _ => (texts.step_network, err.to_string()),
            };
            report.steps.push((name, false, detail));
            return report;
        }
    };

    report.steps.push((crate::i18n::texts().step_ssh_login, true, crate::i18n::auth_ok()));
    if let Some(seen) = session.seen_fingerprint() {
        report.seen_fingerprint = Some(seen.clone());
        let known = options.expected_fingerprint.is_some();
        report.steps.push((
            crate::i18n::texts().step_host_key,
            true,
            if known {
                crate::i18n::fingerprint_match(&seen)
            } else {
                crate::i18n::fingerprint_first(&seen)
            },
        ));
    }

    // SFTP：token 镜像同步依赖它
    match session.sftp().await {
        Ok(sftp) => match sftp.canonicalize(".").await {
            Ok(home) => report.steps.push((
                crate::i18n::texts().step_sftp,
                true,
                crate::i18n::remote_home_ok(&home),
            )),
            Err(err) => report.steps.push((
                crate::i18n::texts().step_sftp,
                false,
                crate::i18n::remote_home_failed(&err.to_string()),
            )),
        },
        Err(err) => report.steps.push((
            crate::i18n::texts().step_sftp,
            false,
            crate::i18n::sftp_subsystem_failed(&err.to_string()),
        )),
    }

    session.disconnect().await;
    report.ok = report.steps.iter().all(|(_, ok, _)| *ok);
    report
}
