pub mod secret;

use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_TOKEN_REFRESH_SECONDS: u32 = 3600;
/// 旧版默认值：已保存配置里若仍是该值，视为未自定义，迁移到新默认。
pub const LEGACY_TOKEN_REFRESH_SECONDS: u32 = 120;
pub const DEFAULT_TOKENS_PER_CREDIT: f64 = 20000.0;

pub const MIN_TOKEN_REFRESH_SECONDS: u32 = 30;
pub const MAX_TOKEN_REFRESH_SECONDS: u32 = 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthType {
    #[default]
    Password,
    PrivateKey,
}

/// 界面语言：简体中文（默认）或 English。仅影响 UI 显示文案与数值单位
/// （中文 万/亿，英文 K/M/B），不影响采集与统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Zh,
    En,
}

fn default_port() -> u16 {
    22
}

/// 模型别名映射：把原始模型名（精确匹配）显示/聚合为更友好的名称。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AliasEntry {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth_type: AuthType,
    /// DPAPI(CurrentUser) 加密后的密码，base64 存储。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_key_path: Option<String>,
    /// DPAPI(CurrentUser) 加密后的私钥口令。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_key_passphrase: Option<String>,
    /// TOFU 固定的主机密钥 SHA-256 指纹（如 "SHA256:xxxx"）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_key_fingerprint_sha256: Option<String>,
}

impl ServerConfig {
    pub fn display_host(&self) -> String {
        if self.port == 22 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub fn display_user_host(&self) -> String {
        format!("{}@{}", self.username, self.display_host())
    }

    pub fn display_label(&self) -> String {
        match &self.name {
            Some(name) if !name.trim().is_empty() => name.trim().to_string(),
            _ => self.display_host(),
        }
    }

    /// 解密密码；解密失败（如换机器/换用户）返回 None。
    pub fn password(&self) -> Option<String> {
        self.protected_password
            .as_deref()
            .and_then(secret::unprotect)
    }

    pub fn key_passphrase(&self) -> Option<String> {
        self.protected_key_passphrase
            .as_deref()
            .and_then(secret::unprotect)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: i32,
    pub token_refresh_seconds: u32,
    pub tokens_per_credit: f64,
    pub start_with_windows: bool,
    //界面语言：zh（简体中文，默认）/ en（English）。
    pub language: Language,
    //模型别名：聚合图表中把 from 名合并显示为 to 名。
    pub model_aliases: Vec<AliasEntry>,
    pub servers: Vec<ServerConfig>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            token_refresh_seconds: DEFAULT_TOKEN_REFRESH_SECONDS,
            tokens_per_credit: DEFAULT_TOKENS_PER_CREDIT,
            start_with_windows: false,
            language: Language::Zh,
            model_aliases: Vec::new(),
            servers: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn clamp_refresh_values(&mut self) {
        self.token_refresh_seconds = self
            .token_refresh_seconds
            .clamp(MIN_TOKEN_REFRESH_SECONDS, MAX_TOKEN_REFRESH_SECONDS);
        if !self.tokens_per_credit.is_finite() || self.tokens_per_credit <= 0.0 {
            self.tokens_per_credit = DEFAULT_TOKENS_PER_CREDIT;
        }
    }

    /// 供聚合使用的 (from, to) 元组视图。
    pub fn alias_pairs(&self) -> Vec<(String, String)> {
        self.model_aliases
            .iter()
            .map(|entry| (entry.from.clone(), entry.to.clone()))
            .collect()
    }
}

/// 加载配置。文件不存在 → 默认配置（首次运行）；
/// 损坏 → 备份原文件后使用默认配置。
pub fn load(path: &Path) -> AppConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str::<AppConfig>(&text) {
            Ok(mut cfg) => {
                if cfg.token_refresh_seconds == LEGACY_TOKEN_REFRESH_SECONDS {
                    cfg.token_refresh_seconds = DEFAULT_TOKEN_REFRESH_SECONDS;
                    crate::log_info!(
                        "token 刷新间隔从旧默认 {}s 迁移为 {}s",
                        LEGACY_TOKEN_REFRESH_SECONDS,
                        DEFAULT_TOKEN_REFRESH_SECONDS
                    );
                }
                cfg.clamp_refresh_values();
                cfg
            }
            Err(err) => {
                crate::log_error!("配置解析失败，已备份并使用默认配置: {err}");
                let _ = std::fs::rename(path, path.with_extension("json.bak"));
                AppConfig::default()
            }
        },
        Err(_) => AppConfig::default(),
    }
}

/// 原子写：临时文件 + rename。
pub fn save(path: &Path, cfg: &AppConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text.as_bytes())?;
    std::fs::rename(&tmp, path)
}

/// 统一主机密钥指纹格式：有的工具/旧数据存的是裸 base64，
/// 我们（与 OpenSSH 一致）存 `SHA256:<base64>`。无法识别的格式返回 None
/// （调用方按「未记录指纹」处理，退回 TOFU）。
pub fn normalize_fingerprint(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }
    let value = match text.split_once(':') {
        Some((alg, rest)) if alg.eq_ignore_ascii_case("SHA256") => rest.trim(),
        // 形如 "ab:cd:ef..." 的十六进制（MD5/SHA1）无法换算成 SHA256 base64
        Some(_) => return None,
        None => text,
    };
    let value = value.trim_end_matches('=');
    // SHA256 的 base64 摘要固定 43 字符（无填充）
    if value.len() != 43 {
        return None;
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
    {
        return None;
    }
    Some(format!("SHA256:{value}"))
}

/// 把编辑对话框里的明文密码加密挂到配置上；空明文且已有密文时保留原密文。
pub fn set_protected_password(server: &mut ServerConfig, plain_password: Option<&str>) {
    if let Some(plain) = plain_password {
        if plain.is_empty() {
            server.protected_password = None;
        } else {
            server.protected_password = secret::protect(plain);
        }
    }
}

pub fn set_protected_passphrase(server: &mut ServerConfig, plain: Option<&str>) {
    if let Some(plain) = plain {
        if plain.is_empty() {
            server.protected_key_passphrase = None;
        } else {
            server.protected_key_passphrase = secret::protect(plain);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_roundtrip_defaults() {
        let cfg = AppConfig::default();
        let text = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back.token_refresh_seconds, DEFAULT_TOKEN_REFRESH_SECONDS);
        assert!(back.servers.is_empty());
        assert!(back.model_aliases.is_empty());
    }

    #[test]
    fn missing_fields_use_defaults() {
        let text = r#"{"schemaVersion":1,"servers":[]}"#;
        let cfg: AppConfig = serde_json::from_str(text).unwrap();
        assert_eq!(cfg.token_refresh_seconds, DEFAULT_TOKEN_REFRESH_SECONDS);
        assert_eq!(cfg.tokens_per_credit, DEFAULT_TOKENS_PER_CREDIT);
        assert_eq!(cfg.language, Language::Zh);
    }

    #[test]
    fn language_roundtrip() {
        let text = r#"{"schemaVersion":1,"language":"en","servers":[]}"#;
        let cfg: AppConfig = serde_json::from_str(text).unwrap();
        assert_eq!(cfg.language, Language::En);
        let serialized = serde_json::to_string(&cfg).unwrap();
        assert!(serialized.contains(r#""language":"en""#));
        let back: AppConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(back.language, Language::En);
    }

    #[test]
    fn aliases_roundtrip() {
        let text = r#"{"model_aliases":[{"from":"a","to":"A"}]}"#;
        let cfg: AppConfig = serde_json::from_str(text).unwrap();
        assert_eq!(cfg.model_aliases.len(), 1);
        assert_eq!(cfg.alias_pairs(), vec![("a".to_string(), "A".to_string())]);
    }

    #[test]
    fn server_config_labels() {
        let mut server = ServerConfig {
            id: Uuid::new_v4(),
            name: None,
            host: "10.0.0.1".into(),
            port: 22,
            username: "opsuser".into(),
            auth_type: AuthType::Password,
            protected_password: None,
            private_key_path: None,
            protected_key_passphrase: None,
            host_key_fingerprint_sha256: None,
        };
        assert_eq!(server.display_label(), "10.0.0.1");
        server.port = 2222;
        assert_eq!(server.display_host(), "10.0.0.1:2222");
        server.name = Some("  node01 ".into());
        assert_eq!(server.display_label(), "node01");
    }

    #[test]
    fn fingerprint_format_is_normalized() {
        let bare = "lnUVSrQrqoQhhTNU/HFOTGsVDTisHdpu/3pAGzSVVOI";
        assert_eq!(
            normalize_fingerprint(bare).as_deref(),
            Some("SHA256:lnUVSrQrqoQhhTNU/HFOTGsVDTisHdpu/3pAGzSVVOI")
        );
        assert_eq!(
            normalize_fingerprint(&format!("sha256:{bare}=")).as_deref(),
            Some("SHA256:lnUVSrQrqoQhhTNU/HFOTGsVDTisHdpu/3pAGzSVVOI")
        );
        // 十六进制 MD5 / 空值 / 长度不对 → None（退回 TOFU）
        assert!(normalize_fingerprint("ab:cd:ef:01:23:45").is_none());
        assert!(normalize_fingerprint("").is_none());
        assert!(normalize_fingerprint("SHA256:ff").is_none());
    }
}
