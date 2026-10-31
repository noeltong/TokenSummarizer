use std::path::PathBuf;

/// %APPDATA%\TokenSummarizer — 配置与日志所在目录。
pub fn app_dir() -> PathBuf {
    roaming_dir().join("TokenSummarizer")
}

/// %LOCALAPPDATA%\TokenSummarizer\cache — 各服务器日志镜像缓存。
pub fn cache_dir() -> PathBuf {
    local_dir().join("TokenSummarizer").join("cache")
}

/// %LOCALAPPDATA%\TokenSummarizer — 持久化数据目录（历史库、临时副本）。
///
/// 与 `cache_dir()` 同级而不是其子目录：设置里的"清空缓存"只删 `cache`，
/// 历史库必须保留，否则一次误点就丢掉全部历史。
pub fn data_dir() -> PathBuf {
    local_dir().join("TokenSummarizer")
}

/// 本地历史库：平台 × agent × 模型 × 日期的 token 用量。
pub fn history_db_path() -> PathBuf {
    data_dir().join("history.sqlite3")
}

/// 临时目录（WSL / UNC 上的 SQLite 复制出来再打开）。
pub fn temp_dir() -> PathBuf {
    data_dir().join("tmp")
}

pub fn config_path() -> PathBuf {
    app_dir().join("config.json")
}

pub fn log_dir() -> PathBuf {
    app_dir().join("logs")
}

fn roaming_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(home_fallback)
}

fn local_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(home_fallback)
}

fn home_fallback() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
