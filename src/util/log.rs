//! 最小化文件日志：只记录错误与关键事件，不记录凭据，不记录采样数据。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use chrono::Local;

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

struct Logger {
    path: PathBuf,
}

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

/// 初始化日志。失败时静默（日志本身不可用不应影响程序）。
pub fn init() {
    let dir = crate::util::paths::log_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!("日志目录创建失败: {err}");
        return;
    }
    let path = dir.join("app.log");
    if fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = fs::rename(&path, dir.join("app.old.log"));
    }
    let _ = LOGGER.set(Mutex::new(Logger { path }));
}

/// Removes credential-like values before text reaches a persistent log or diagnostic surface.
/// Hostnames, usernames, paths, and fingerprints are intentionally retained for troubleshooting.
pub fn redact_sensitive_text(input: &str) -> String {
    let mut out = input.to_string();
    if let Some(start) = out.find("-----BEGIN ") {
        let end = out[start..].find("-----END").and_then(|end_rel| {
            out[start + end_rel..].find("-----").map(|i| start + end_rel + i + 5)
        }).unwrap_or(out.len());
        out.replace_range(start..end, "[REDACTED PRIVATE KEY]");
    }
    for marker in ["password", "passwd", "passphrase", "api_key", "apikey", "access_token", "refresh_token", "client_secret", "authorization", "bearer", "token"] {
        let lower = out.to_ascii_lowercase();
        let Some(pos) = lower.find(marker) else { continue };
        let value_start = out[pos..].find(['=', ':']).map(|i| pos + i + 1);
        if let Some(start) = value_start {
            let tail = &out[start..];
            let end = tail.find([',', ';', ' ', '\n', '\r', '}', ']']).unwrap_or(tail.len());
            out.replace_range(start..start + end, "[REDACTED]");
        }
    }
    out.split_whitespace()
        .map(|word| if word.starts_with("eyJ") && word.matches('.').count() >= 2 { "[REDACTED JWT]" } else { word })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn log(level: &str, msg: &str) {
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let Ok(logger) = logger.lock() else {
        return;
    };
    let line = format!(
        "{} [{}] {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        level,
        redact_sensitive_text(&msg.replace('\n', " "))
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&logger.path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::redact_sensitive_text;

    #[test]
    fn redacts_credentials_but_keeps_diagnostics() {
        let text = "host=server.example user=ops password=secret-pass token=eyJabc.def.ghi";
        let safe = redact_sensitive_text(text);
        assert!(safe.contains("server.example"));
        assert!(safe.contains("ops"));
        assert!(!safe.contains("secret-pass"));
        assert!(!safe.contains("eyJabc.def.ghi"));
    }

    #[test]
    fn redacts_private_key_marker() {
        let safe = redact_sensitive_text("-----BEGIN PRIVATE KEY----- hidden");
        assert!(!safe.contains("BEGIN PRIVATE KEY"));
        assert!(!safe.contains("hidden"));
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::util::log::log("INFO", &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::util::log::log("ERROR", &format!($($arg)*)) };
}
