//! 本地历史库：平台 × agent × 模型 × 日期的 token 用量。
//!
//! 为什么需要它：每次刷新都全量重扫所有 agent 的数据（远端镜像 + 本机 + WSL）
//! 代价越来越高。库里已经有历史，所以每轮只需要重算
//! 「每个源最后一次成功覆盖到的北京日 → 今天」这一段，写回后再从库里重建
//! 完整快照交给 UI —— UI 因此什么都不用改。
//!
//! 三条不变量：
//! 1. **日期由远及近**：`day_usage` 按 `(target_id, day, source, model)` 建索引，
//!    读取一律 `ORDER BY day ASC`。
//! 2. **水位线是"每源最后一次成功覆盖到的日期"**，不是 `MAX(day)`：
//!    某源今天没用量、或上次扫描失败时，`MAX(day)` 会给出错误的起点。
//! 3. **不可信的扫描结果绝不覆盖历史**：`!detected` / `partial` / `error` /
//!    同步失败的源只更新状态字段，历史行原样保留。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::model::{DayBucket, Platform, Source, SourceScan, StorageMode, TokenSnapshot};

/// 一次扫描针对的对象：可能是本机 Windows、某个 WSL 发行版、或一台 SSH 服务器。
#[derive(Debug, Clone)]
pub struct ScanTarget {
    /// 库主键：`local` / `wsl:<distro>:<user>` / 服务器 UUID。
    pub id: String,
    pub platform: Platform,
    /// 诊断用显示名。
    pub label: String,
    /// 扫描根：`%USERPROFILE%` / `\\wsl$\<distro>\home\<user>` / 镜像目录。
    pub root: PathBuf,
    /// 数据源身份（host:port:user 或 home 路径）。变化即视为另一个数据源，
    /// 强制全量重建，避免把两台机器的历史混在一起。
    pub identity: String,
}

impl ScanTarget {
    /// 本机 Windows。
    pub fn local_windows(home: &Path) -> Self {
        Self {
            id: "local".to_string(),
            platform: Platform::Windows,
            label: "本机 Windows".to_string(),
            root: home.to_path_buf(),
            identity: format!("home:{}", home.display()),
        }
    }

    /// 某个 WSL 发行版里的 Linux home。
    pub fn wsl(distro: &str, user: &str, root: &Path, unix_home: &str) -> Self {
        Self {
            id: format!("wsl:{distro}:{user}"),
            platform: Platform::Wsl,
            label: format!("WSL {distro}"),
            root: root.to_path_buf(),
            identity: format!("wsl:{distro}:{unix_home}"),
        }
    }

    /// 一台 SSH 服务器（远端永远按 Linux 规则，扫描根是本地镜像目录）。
    pub fn server(
        id: uuid::Uuid,
        label: &str,
        host: &str,
        port: u16,
        username: &str,
        mirror_root: &Path,
    ) -> Self {
        Self {
            id: id.to_string(),
            platform: Platform::Linux,
            label: label.to_string(),
            root: mirror_root.to_path_buf(),
            identity: format!("server:{host}:{port}:{username}"),
        }
    }

    /// 本地目标没有"镜像被清空"的问题，用身份串当 epoch 即可。
    pub fn local_epoch(&self) -> String {
        self.identity.clone()
    }
}

/// 本轮扫描计划：哪些源只扫窗口、是否整库重建。
#[derive(Debug, Clone, Default)]
pub struct ScanPlan {
    /// 按天窗口重算的源 → 起点（北京日，含当天）。
    pub since_days: HashMap<Source, String>,
    /// true = 全量重建（首次建库 / 身份或镜像 epoch 变化 / 上次全量未完成）。
    pub full_scan: bool,
}

impl ScanPlan {
    pub fn since_day(&self, source: Source) -> Option<&str> {
        self.since_days.get(&source).map(String::as_str)
    }

    /// 传给解析层的窗口表；全量时返回 None。
    pub fn window(&self) -> Option<&HashMap<Source, String>> {
        if self.full_scan {
            None
        } else {
            Some(&self.since_days)
        }
    }

    /// 该源本轮是否需要「整源替换」（累积值源总是如此）。
    pub fn replaces_whole_source(&self, source: Source) -> bool {
        self.full_scan
            || source.storage_mode() == StorageMode::Snapshot
            || self.since_day(source).is_none()
    }
}

/// 解析口径版本：**改了某源的读取逻辑（表/字段/去重键/换算）就把它 +1**。
/// 历史库在下一轮发现 `rev:<source>` 不匹配时，会让该源整源重扫一次
/// （不设水位线 → `ScanPlan::replaces_whole_source` 为真），其它源仍走窗口。
///
/// 为什么需要它：`last_synced_day` 只描述「上次覆盖到哪天」，无法表达
/// 「上次读的那张表已经不对了」。OpenCode 换库结构时不加这个会永远只补当天。
fn parser_revision(source: Source) -> u32 {
    match source {
        // 2 = 支持新版 session_message 表（OpenCode 库结构升级）。
        // 3 = 新增 requests 请求数列（历史库 schema V2）：全源回填请求数。
        Source::Opencode => 3,
        Source::Codex
        | Source::Claude
        | Source::Qwen
        | Source::Hermes
        | Source::Qoder
        | Source::QoderCn
        | Source::QoderCnCli
        | Source::Zcode
        | Source::Pi
        | Source::WorkBuddy
        | Source::Doubao
        | Source::CommandCode => 2,
    }
}

/// V1（历史版本，仅为迁移测试保留）：day_usage 无 requests 列。
#[cfg(test)]
const SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS target (
    target_id    TEXT PRIMARY KEY NOT NULL,
    platform     TEXT NOT NULL,
    label        TEXT NOT NULL,
    root         TEXT NOT NULL,
    identity     TEXT NOT NULL,
    first_seen   INTEGER NOT NULL,
    last_attempt INTEGER,
    last_success INTEGER,
    last_error   TEXT
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS meta (
    target_id TEXT NOT NULL,
    key       TEXT NOT NULL,
    value     TEXT NOT NULL,
    PRIMARY KEY (target_id, key)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS source_state (
    target_id         TEXT NOT NULL,
    source            TEXT NOT NULL,
    detected          INTEGER NOT NULL DEFAULT 0,
    partial           INTEGER NOT NULL DEFAULT 0,
    coverage_tokens   INTEGER NOT NULL DEFAULT 0,
    coverage_credits  REAL NOT NULL DEFAULT 0,
    coverage_messages INTEGER NOT NULL DEFAULT 0,
    messages_total    INTEGER NOT NULL DEFAULT 0,
    coverage_summary  TEXT NOT NULL DEFAULT '',
    warnings_json     TEXT NOT NULL DEFAULT '[]',
    error             TEXT,
    last_synced_day   TEXT,
    updated_at        INTEGER NOT NULL,
    PRIMARY KEY (target_id, source)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS day_usage (
    target_id TEXT NOT NULL,
    platform  TEXT NOT NULL,
    source    TEXT NOT NULL,
    model     TEXT NOT NULL,
    day       TEXT NOT NULL,
    tokens    INTEGER NOT NULL DEFAULT 0,
    credits   REAL NOT NULL DEFAULT 0,
    messages  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (target_id, source, day, model)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_usage_target_day
    ON day_usage(target_id, day, source, model);
CREATE INDEX IF NOT EXISTS idx_usage_target_source_day
    ON day_usage(target_id, source, day);
CREATE INDEX IF NOT EXISTS idx_usage_platform_day
    ON day_usage(platform, day);
"#;

const SCHEMA_V2: &str = r#"
CREATE TABLE IF NOT EXISTS target (
    target_id    TEXT PRIMARY KEY NOT NULL,
    platform     TEXT NOT NULL,
    label        TEXT NOT NULL,
    root         TEXT NOT NULL,
    identity     TEXT NOT NULL,
    first_seen   INTEGER NOT NULL,
    last_attempt INTEGER,
    last_success INTEGER,
    last_error   TEXT
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS meta (
    target_id TEXT NOT NULL,
    key       TEXT NOT NULL,
    value     TEXT NOT NULL,
    PRIMARY KEY (target_id, key)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS source_state (
    target_id         TEXT NOT NULL,
    source            TEXT NOT NULL,
    detected          INTEGER NOT NULL DEFAULT 0,
    partial           INTEGER NOT NULL DEFAULT 0,
    coverage_tokens   INTEGER NOT NULL DEFAULT 0,
    coverage_credits  REAL NOT NULL DEFAULT 0,
    coverage_messages INTEGER NOT NULL DEFAULT 0,
    messages_total    INTEGER NOT NULL DEFAULT 0,
    coverage_summary  TEXT NOT NULL DEFAULT '',
    warnings_json     TEXT NOT NULL DEFAULT '[]',
    error             TEXT,
    last_synced_day   TEXT,
    updated_at        INTEGER NOT NULL,
    PRIMARY KEY (target_id, source)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS day_usage (
    target_id TEXT NOT NULL,
    platform  TEXT NOT NULL,
    source    TEXT NOT NULL,
    model     TEXT NOT NULL,
    day       TEXT NOT NULL,
    tokens    INTEGER NOT NULL DEFAULT 0,
    credits   REAL NOT NULL DEFAULT 0,
    messages  INTEGER NOT NULL DEFAULT 0,
    requests  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (target_id, source, day, model)
) WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_usage_target_day
    ON day_usage(target_id, day, source, model);
CREATE INDEX IF NOT EXISTS idx_usage_target_source_day
    ON day_usage(target_id, source, day);
CREATE INDEX IF NOT EXISTS idx_usage_platform_day
    ON day_usage(platform, day);
"#;

pub struct TokenStore {
    conn: Mutex<Connection>,
    path: Option<PathBuf>,
}

impl TokenStore {
    /// 打开（或新建）历史库。文件不存在时按 schema 建库。
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败：{e}"))?;
        }
        let conn = Connection::open(path).map_err(|e| format!("打开历史库失败：{e}"))?;
        Self::init(conn, Some(path.to_path_buf()))
    }

    /// 默认库位置；损坏时改名 `.corrupt` 后重建一次。
    pub fn open_default() -> Result<Self, String> {
        let path = crate::util::paths::history_db_path();
        match Self::open(&path) {
            Ok(store) => Ok(store),
            Err(err) => {
                crate::log_error!("历史库不可用（{err}），尝试重建");
                let backup = path.with_extension("sqlite3.corrupt");
                let _ = std::fs::remove_file(&backup);
                if path.exists() {
                    std::fs::rename(&path, &backup)
                        .map_err(|e| format!("历史库损坏且无法改名：{e}"))?;
                }
                for suffix in ["-wal", "-shm"] {
                    let side = PathBuf::from(format!("{}{suffix}", path.display()));
                    let _ = std::fs::remove_file(side);
                }
                Self::open(&path).map_err(|e| format!("重建历史库失败：{e}"))
            }
        }
    }

    /// 内存库：磁盘不可用时的降级（程序可用，但不持久化）。
    pub fn open_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
        Self::init(conn, None)
    }

    fn init(conn: Connection, path: Option<PathBuf>) -> Result<Self, String> {
        let store = Self {
            conn: Mutex::new(conn),
            path,
        };
        {
            let conn = store.lock();
            // WAL：允许 --check 与 GUI 同时打开；内存库不支持，忽略失败。
            let _ = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get::<_, String>(0));
            conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;")
                .map_err(|e| e.to_string())?;
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .map_err(|e| e.to_string())?;
            match version {
                0 => {
                    conn.execute_batch(SCHEMA_V2).map_err(|e| e.to_string())?;
                    conn.pragma_update(None, "user_version", 2)
                        .map_err(|e| e.to_string())?;
                }
                // V1 → V2：day_usage 补 requests 列；旧行保持 0，
                // 历史数值由 parser_revision 升级触发的整源重扫回填。
                1 => {
                    conn.execute_batch(
                        "ALTER TABLE day_usage ADD COLUMN requests INTEGER NOT NULL DEFAULT 0;",
                    )
                    .map_err(|e| e.to_string())?;
                    conn.pragma_update(None, "user_version", 2)
                        .map_err(|e| e.to_string())?;
                }
                2 => {}
                other => {
                    return Err(format!(
                        "历史库版本 {other} 高于程序支持的 2，请升级程序后再试"
                    ));
                }
            }
        }
        Ok(store)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        // 锁中毒不应让整个采集停摆：连接本身仍然是可用的。
        self.conn.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// 记录一次尝试（UI 的"新鲜度"与失败诊断用）。
    pub fn begin_round(&self, target: &ScanTarget) -> Result<(), String> {
        let conn = self.lock();
        Self::upsert_target(&conn, target).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE target SET last_attempt = ?2 WHERE target_id = ?1",
            params![target.id, now_unix()],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn record_failure(&self, target: &ScanTarget, error: &str) -> Result<(), String> {
        let conn = self.lock();
        Self::upsert_target(&conn, target).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE target SET last_attempt = ?2, last_error = ?3 WHERE target_id = ?1",
            params![target.id, now_unix(), error],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn upsert_target(conn: &Connection, target: &ScanTarget) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO target (target_id, platform, label, root, identity, first_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(target_id) DO UPDATE SET
                 platform = excluded.platform,
                 label    = excluded.label,
                 root     = excluded.root,
                 identity = excluded.identity",
            params![
                target.id,
                target.platform.as_str(),
                target.label,
                target.root.display().to_string(),
                target.identity,
                now_unix()
            ],
        )?;
        Ok(())
    }

    fn meta_get(conn: &Connection, id: &str, key: &str) -> Option<String> {
        conn.query_row(
            "SELECT value FROM meta WHERE target_id = ?1 AND key = ?2",
            params![id, key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    fn meta_set(conn: &Connection, id: &str, key: &str, value: &str) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT INTO meta (target_id, key, value) VALUES (?1, ?2, ?3)
             ON CONFLICT(target_id, key) DO UPDATE SET value = excluded.value",
            params![id, key, value],
        )?;
        Ok(())
    }

    /// 该源的解析口径是否已是当前版本（决定这一轮要不要整源重扫）。
    fn revision_is_current(conn: &Connection, id: &str, source: Source) -> bool {
        let expected = parser_revision(source);
        match Self::meta_get(conn, id, &Self::revision_key(source)) {
            Some(value) => value == expected.to_string(),
            // 升级前建的库没有 rev 记录：只让「口径已经变过」的源重扫一次，
            // 其余源视为初版口径，避免给所有源做无谓的全量。
            None => expected <= 1,
        }
    }

    fn revision_key(source: Source) -> String {
        format!("rev:{}", source.key())
    }

    /// 决定本轮扫描窗口。
    ///
    /// - 库里没有这个 target / 身份或镜像 epoch 变了 / 上次全量没跑完 → 全量。
    /// - 否则每个 Window 源从自己的水位线开始；没有水位线的源（新增或上次失败）全量。
    pub fn prepare(
        &self,
        target: &ScanTarget,
        epoch: &str,
        plan: &[Source],
    ) -> Result<ScanPlan, String> {
        let conn = self.lock();
        Self::upsert_target(&conn, target).map_err(|e| e.to_string())?;

        let identity_ok = Self::meta_get(&conn, &target.id, "identity").as_deref()
            == Some(target.identity.as_str());
        let epoch_ok = Self::meta_get(&conn, &target.id, "epoch").as_deref() == Some(epoch);
        let full_done =
            Self::meta_get(&conn, &target.id, "full_scan_done").as_deref() == Some("1");
        if !identity_ok || !epoch_ok || !full_done {
            return Ok(ScanPlan {
                since_days: HashMap::new(),
                full_scan: true,
            });
        }

        let mut since_days = HashMap::new();
        for source in plan {
            if source.storage_mode() != StorageMode::Window {
                continue;
            }
            // 解析口径升级过：该源本轮整源重扫，不给水位线。
            if !Self::revision_is_current(&conn, &target.id, *source) {
                continue;
            }
            let day: Option<String> = conn
                .query_row(
                    "SELECT last_synced_day FROM source_state
                     WHERE target_id = ?1 AND source = ?2",
                    params![target.id, source.key()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .ok()
                .flatten()
                .flatten();
            if let Some(day) = day.filter(|day| !day.is_empty()) {
                since_days.insert(*source, day);
            }
        }
        Ok(ScanPlan {
            since_days,
            full_scan: false,
        })
    }

    /// 写入一轮扫描结果。
    ///
    /// `started_day` 是本轮**开始**时的北京日：窗口重算会把该日整天重新覆盖一遍，
    /// 因此它是安全的水位线（跨午夜时宁可下轮多扫一天，也不跳过最后一分钟）。
    pub fn apply(
        &self,
        target: &ScanTarget,
        epoch: &str,
        plan: &ScanPlan,
        snapshot: &TokenSnapshot,
        started_day: &str,
        failed: &[Source],
    ) -> Result<(), String> {
        let mut conn = self.lock();
        Self::upsert_target(&conn, target).map_err(|e| e.to_string())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;

        let mut written = 0usize;
        let mut written_requests: Vec<String> = Vec::new();
        for source in Self::planned_sources(plan, snapshot) {
            let Some(scan) = snapshot
                .scans
                .iter()
                .find(|(candidate, _)| *candidate == source)
                .map(|(_, scan)| scan)
            else {
                // 计划里有、结果里没有：视为本轮没拿到该源，只更新状态。
                Self::upsert_state(&tx, target, source, None, started_day)
                    .map_err(|e| e.to_string())?;
                continue;
            };

            let trusted = scan.detected
                && !scan.partial
                && scan.error.is_none()
                && !failed.contains(&source);
            if !trusted {
                Self::upsert_state(&tx, target, source, Some(scan), started_day)
                    .map_err(|e| e.to_string())?;
                continue;
            }

            let mut revision_done = true;
            match source.storage_mode() {
                StorageMode::ProbeOnly => { /* 只计数源没有 bucket */ }
                StorageMode::Snapshot => {
                    // 累积值源：整源替换。结果为空时不删旧行——那更可能是
                    // "数据库暂时读不到"，删了就再也回不来。
                    if !scan.buckets.is_empty() {
                        tx.execute(
                            "DELETE FROM day_usage WHERE target_id = ?1 AND source = ?2",
                            params![target.id, source.key()],
                        )
                        .map_err(|e| e.to_string())?;
                        Self::insert_buckets(&tx, target, source, &scan.buckets)
                            .map_err(|e| e.to_string())?;
                        written += 1;
                        written_requests.push(Self::requests_detail(source, &scan.buckets));
                    }
                }
                StorageMode::Window => {
                    // 整源替换（全量或该源没有水位线）时不带 day 条件。
                    if plan.replaces_whole_source(source) && scan.buckets.is_empty() {
                        // 与 Snapshot 分支同理：整源重扫却一条都没读到，更可能是
                        // 库暂时读空。保留旧历史，并且**不记 revision**，下轮再试。
                        revision_done = false;
                    } else {
                        if plan.replaces_whole_source(source) {
                            tx.execute(
                                "DELETE FROM day_usage WHERE target_id = ?1 AND source = ?2",
                                params![target.id, source.key()],
                            )
                            .map_err(|e| e.to_string())?;
                        } else {
                            let since = plan.since_day(source).unwrap_or_default();
                            tx.execute(
                                "DELETE FROM day_usage
                                 WHERE target_id = ?1 AND source = ?2 AND day >= ?3",
                                params![target.id, source.key(), since],
                            )
                            .map_err(|e| e.to_string())?;
                        }
                        Self::insert_buckets(&tx, target, source, &scan.buckets)
                            .map_err(|e| e.to_string())?;
                        written += 1;
                        written_requests.push(Self::requests_detail(source, &scan.buckets));
                    }
                }
            }
            Self::upsert_state(&tx, target, source, Some(scan), started_day)
                .map_err(|e| e.to_string())?;
            if revision_done {
                let revision = parser_revision(source).to_string();
                Self::meta_set(&tx, &target.id, &Self::revision_key(source), &revision)
                    .map_err(|e| e.to_string())?;
            }
        }

        Self::meta_set(&tx, &target.id, "identity", &target.identity)
            .map_err(|e| e.to_string())?;
        Self::meta_set(&tx, &target.id, "epoch", epoch).map_err(|e| e.to_string())?;
        Self::meta_set(&tx, &target.id, "full_scan_done", "1").map_err(|e| e.to_string())?;
        Self::meta_set(&tx, &target.id, "last_synced_day", started_day)
            .map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE target SET last_attempt = ?2, last_success = ?2, last_error = NULL
             WHERE target_id = ?1",
            params![target.id, now_unix()],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        // 本轮新增数据的来源与请求数：按源列出（请求数 = 各 bucket 的
        // 请求级记录数合计；ProbeOnly 源没有 bucket，不出现）。
        let detail = if written_requests.is_empty() {
            String::new()
        } else {
            format!("，本轮请求 {}", written_requests.join("、"))
        };
        crate::log_info!(
            "{}: 历史库写入 {written} 个源（窗口起点 {started_day}）{detail}",
            target.label
        );
        Ok(())
    }

    /// 该源本轮写入的请求数摘要，如 `claude=12`。
    fn requests_detail(source: Source, buckets: &[DayBucket]) -> String {
        let total: i64 = buckets.iter().map(|bucket| bucket.requests).sum();
        format!("{}={}", source.key(), total)
    }

    fn planned_sources(plan: &ScanPlan, snapshot: &TokenSnapshot) -> Vec<Source> {
        // 以快照里的源为准（= 该平台扫描计划），保持 Source::ALL 顺序。
        let mut sources: Vec<Source> = snapshot.scans.iter().map(|(source, _)| *source).collect();
        if plan.full_scan {
            sources.sort();
            sources.dedup();
        }
        sources.sort_by_key(|source| {
            Source::ALL
                .iter()
                .position(|candidate| candidate == source)
                .unwrap_or(usize::MAX)
        });
        sources
    }

    fn insert_buckets(
        conn: &Connection,
        target: &ScanTarget,
        source: Source,
        buckets: &[DayBucket],
    ) -> rusqlite::Result<()> {
        let mut statement = conn.prepare_cached(
            "INSERT INTO day_usage
                 (target_id, platform, source, model, day, tokens, credits, messages, requests)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(target_id, source, day, model) DO UPDATE SET
                 tokens   = day_usage.tokens + excluded.tokens,
                 credits  = day_usage.credits + excluded.credits,
                 messages = day_usage.messages + excluded.messages,
                 requests = day_usage.requests + excluded.requests,
                 platform = excluded.platform",
        )?;
        for bucket in buckets {
            statement.execute(params![
                target.id,
                target.platform.as_str(),
                source.key(),
                bucket.model,
                bucket.day,
                bucket.tokens,
                bucket.credits,
                bucket.messages,
                bucket.requests
            ])?;
        }
        Ok(())
    }

    /// 更新源状态。
    ///
    /// `scan = None` 或不可信时只写状态字段，`coverage_*` 与 `last_synced_day`
    /// 保持上一轮的值——这是「失败不覆盖历史」的落点。
    fn upsert_state(
        conn: &Connection,
        target: &ScanTarget,
        source: Source,
        scan: Option<&SourceScan>,
        started_day: &str,
    ) -> rusqlite::Result<()> {
        let trusted = scan
            .map(|scan| scan.detected && !scan.partial && scan.error.is_none())
            .unwrap_or(false);
        let detected = scan.map(|scan| scan.detected).unwrap_or(false);
        let partial = scan.map(|scan| scan.partial).unwrap_or(true);
        let warnings = scan
            .map(|scan| {
                serde_json::to_string(&scan.warnings).unwrap_or_else(|_| "[]".to_string())
            })
            .unwrap_or_else(|| "[]".to_string());
        let error = scan.and_then(|scan| scan.error.clone());

        if trusted {
            let scan = scan.expect("trusted 分支必然有 scan");
            conn.execute(
                "INSERT INTO source_state
                    (target_id, source, detected, partial, coverage_tokens, coverage_credits,
                     coverage_messages, messages_total, coverage_summary, warnings_json, error,
                     last_synced_day, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11)
                 ON CONFLICT(target_id, source) DO UPDATE SET
                     detected = excluded.detected,
                     partial = 0,
                     coverage_tokens = excluded.coverage_tokens,
                     coverage_credits = excluded.coverage_credits,
                     coverage_messages = excluded.coverage_messages,
                     messages_total = excluded.messages_total,
                     coverage_summary = excluded.coverage_summary,
                     warnings_json = excluded.warnings_json,
                     error = NULL,
                     last_synced_day = excluded.last_synced_day,
                     updated_at = excluded.updated_at",
                params![
                    target.id,
                    source.key(),
                    i64::from(detected),
                    scan.coverage_tokens,
                    scan.coverage_credits,
                    scan.coverage_messages,
                    scan.messages_total,
                    scan.coverage_summary,
                    warnings,
                    started_day,
                    now_unix()
                ],
            )?;
        } else {
            conn.execute(
                "INSERT INTO source_state
                    (target_id, source, detected, partial, coverage_tokens, coverage_credits,
                     coverage_messages, messages_total, coverage_summary, warnings_json, error,
                     last_synced_day, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 0, '', ?5, ?6, NULL, ?7)
                 ON CONFLICT(target_id, source) DO UPDATE SET
                     detected = excluded.detected,
                     partial = excluded.partial,
                     warnings_json = excluded.warnings_json,
                     error = excluded.error,
                     updated_at = excluded.updated_at",
                params![
                    target.id,
                    source.key(),
                    i64::from(detected),
                    i64::from(partial),
                    warnings,
                    error,
                    now_unix()
                ],
            )?;
        }
        Ok(())
    }

    /// 从库里重建某目标的完整快照（全历史，日期由远及近）。
    pub fn load(&self, target: &ScanTarget) -> Result<Option<TokenSnapshot>, String> {
        let conn = self.lock();
        let mut buckets: HashMap<Source, Vec<DayBucket>> = HashMap::new();
        {
            let mut statement = conn
                .prepare(
                    "SELECT source, day, model, tokens, credits, messages, requests
                     FROM day_usage WHERE target_id = ?1
                     ORDER BY day ASC, source ASC, model ASC",
                )
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map(params![target.id], |row| {
                    let key: String = row.get(0)?;
                    Ok((
                        key,
                        DayBucket {
                            day: row.get(1)?,
                            model: row.get(2)?,
                            tokens: row.get(3)?,
                            credits: row.get(4)?,
                            messages: row.get(5)?,
                            requests: row.get(6)?,
                        },
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (key, bucket) = row.map_err(|e| e.to_string())?;
                if let Some(source) = Source::from_key(&key) {
                    buckets.entry(source).or_default().push(bucket);
                }
            }
        }

        let mut states: HashMap<Source, StoredState> = HashMap::new();
        {
            let mut statement = conn
                .prepare(
                    "SELECT source, detected, partial, coverage_summary, warnings_json, error,
                            messages_total
                     FROM source_state WHERE target_id = ?1",
                )
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map(params![target.id], |row| {
                    let key: String = row.get(0)?;
                    Ok((
                        key,
                        StoredState {
                            detected: row.get::<_, i64>(1)? != 0,
                            partial: row.get::<_, i64>(2)? != 0,
                            coverage_summary: row.get(3)?,
                            warnings_json: row.get(4)?,
                            error: row.get(5)?,
                            messages_total: row.get(6)?,
                        },
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (key, state) = row.map_err(|e| e.to_string())?;
                if let Some(source) = Source::from_key(&key) {
                    states.insert(source, state);
                }
            }
        }

        if buckets.is_empty() && states.is_empty() {
            return Ok(None);
        }

        let mut scans = Vec::new();
        for source in Source::ALL {
            let source_buckets = buckets.remove(&source).unwrap_or_default();
            let state = states.remove(&source);
            if source_buckets.is_empty() && state.is_none() {
                continue;
            }
            let state = state.unwrap_or_default();
            // 历史有数据但本轮没检测到：仍然要显示，否则用户会以为历史丢了。
            let mut detected = state.detected || !source_buckets.is_empty();
            let mut warnings: Vec<String> =
                serde_json::from_str(&state.warnings_json).unwrap_or_default();
            if !state.detected && !source_buckets.is_empty() {
                detected = true;
                warnings.push(format!(
                    "当前未检测到 {}，显示历史数据",
                    source.label()
                ));
            }
            let totals = Self::totals(&source_buckets);
            let summary = match source.storage_mode() {
                StorageMode::ProbeOnly => state.coverage_summary.clone(),
                _ => Self::summary(source, &totals),
            };
            scans.push((
                source,
                SourceScan {
                    detected,
                    buckets: source_buckets,
                    warnings,
                    error: state.error,
                    coverage_tokens: totals.0,
                    coverage_credits: totals.1,
                    coverage_messages: totals.2,
                    messages_total: state.messages_total,
                    coverage_summary: summary,
                    partial: state.partial,
                },
            ));
        }

        Ok(Some(TokenSnapshot {
            generated_at: Some(Utc::now()),
            scans,
        }))
    }

    fn totals(buckets: &[DayBucket]) -> (i64, f64, i64) {
        let mut tokens = 0i64;
        let mut credits = 0f64;
        let mut messages = 0i64;
        for bucket in buckets {
            tokens += bucket.tokens;
            credits += bucket.credits;
            messages += bucket.messages;
        }
        (tokens, credits, messages)
    }

    /// 覆盖摘要用全历史汇总重新生成，避免"摘要只说本轮窗口、合计却是全历史"。
    fn summary(source: Source, totals: &(i64, f64, i64)) -> String {
        let (tokens, credits, messages) = *totals;
        source.coverage_summary(tokens, credits, messages)
    }

    pub fn list_targets(&self) -> Result<Vec<ScanTarget>, String> {
        let conn = self.lock();
        let mut statement = conn
            .prepare("SELECT target_id, platform, label, root, identity FROM target ORDER BY target_id")
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let platform: String = row.get(1)?;
                let label: String = row.get(2)?;
                let root: String = row.get(3)?;
                let identity: String = row.get(4)?;
                Ok((id, platform, label, root, identity))
            })
            .map_err(|e| e.to_string())?;
        let mut targets = Vec::new();
        for row in rows {
            let (id, platform, label, root, identity) = row.map_err(|e| e.to_string())?;
            let Some(platform) = Platform::from_key(&platform) else {
                continue;
            };
            targets.push(ScanTarget {
                id,
                platform,
                label,
                root: PathBuf::from(root),
                identity,
            });
        }
        Ok(targets)
    }

    pub fn last_success_at(&self, target_id: &str) -> Option<DateTime<Utc>> {
        let conn = self.lock();
        let seconds: Option<i64> = conn
            .query_row(
                "SELECT last_success FROM target WHERE target_id = ?1",
                params![target_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        seconds.and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
    }

    /// 某目标库里的日期范围（诊断用）。
    pub fn day_range(&self, target_id: &str) -> Option<(String, String)> {
        let conn = self.lock();
        conn.query_row(
            "SELECT MIN(day), MAX(day) FROM day_usage WHERE target_id = ?1",
            params![target_id],
            |row| {
                let min: Option<String> = row.get(0)?;
                let max: Option<String> = row.get(1)?;
                Ok((min, max))
            },
        )
        .optional()
        .ok()
        .flatten()
        .and_then(|(min, max)| Some((min?, max?)))
    }
}

#[derive(Debug, Default, Clone)]
struct StoredState {
    detected: bool,
    partial: bool,
    coverage_summary: String,
    warnings_json: String,
    error: Option<String>,
    messages_total: i64,
}

fn now_unix() -> i64 {
    Utc::now().timestamp()
}

/// 便捷构造：把 `TokenStore` 放进 `Arc` 供多个采集循环共享。
pub fn shared(store: TokenStore) -> Arc<TokenStore> {
    Arc::new(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceScan;

    fn target(platform: Platform, id: &str) -> ScanTarget {
        ScanTarget {
            id: id.to_string(),
            platform,
            label: id.to_string(),
            root: PathBuf::from("C:/tmp"),
            identity: format!("identity:{id}"),
        }
    }

    fn memory() -> TokenStore {
        TokenStore::open_memory().expect("内存库")
    }

    fn bucket(day: &str, model: &str, tokens: i64) -> DayBucket {
        DayBucket {
            day: day.to_string(),
            model: model.to_string(),
            tokens,
            credits: 0.0,
            messages: 1,
            requests: 1,
        }
    }

    fn scan(detected: bool, buckets: Vec<DayBucket>) -> SourceScan {
        SourceScan {
            detected,
            buckets,
            ..Default::default()
        }
    }

    fn snapshot(entries: Vec<(Source, SourceScan)>) -> TokenSnapshot {
        TokenSnapshot {
            generated_at: None,
            scans: entries,
        }
    }

    #[test]
    fn schema_initialises_and_reports_version() {
        let store = memory();
        let conn = store.lock();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 2);
    }

    #[test]
    fn v1_database_migrates_and_keeps_history() {
        let dir = crate::util::testutil::TempDir::new();
        let path = dir.path().join("history.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.pragma_update(None, "user_version", 1).unwrap();
            conn.execute(
                "INSERT INTO day_usage
                     (target_id, platform, source, model, day, tokens, credits, messages)
                 VALUES ('local', 'windows', 'claude', 'm', '2026-10-24', 100, 0, 3)",
                [],
            )
            .unwrap();
        }
        let store = TokenStore::open(&path).unwrap();
        {
            let conn = store.lock();
            let version: i64 = conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap();
            assert_eq!(version, 2);
        }
        let t = target(Platform::Windows, "local");
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets[0].tokens, 100);
        assert_eq!(
            loaded.scans[0].1.buckets[0].requests, 0,
            "V1 旧行迁移后请求数为 0，等待整源重扫回填"
        );

        // 新一轮写入带 requests：整源替换后读回新值
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        let mut fresh = bucket("2026-10-24", "m", 50);
        fresh.requests = 7;
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(true, vec![fresh]))]),
                "2026-10-25",
                &[],
            )
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets[0].tokens, 50);
        assert_eq!(loaded.scans[0].1.buckets[0].requests, 7);
    }

    #[test]
    fn requests_roundtrip_without_double_counting() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Zcode]).unwrap();
        let mut first = bucket("2026-10-24", "glm", 100);
        first.requests = 4;
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Zcode, scan(true, vec![first]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets[0].requests, 4);

        // 同窗口重跑（Snapshot 源整源替换）：请求数必须还是 4，不是 8
        let plan = store.prepare(&t, "e", &[Source::Zcode]).unwrap();
        let mut again = bucket("2026-10-24", "glm", 100);
        again.requests = 4;
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Zcode, scan(true, vec![again]))]),
                "2026-10-25",
                &[],
            )
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets[0].requests, 4);
    }

    #[test]
    fn first_round_is_full_then_uses_watermark() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store
            .prepare(&t, "epoch-1", &[Source::Claude, Source::Codex])
            .expect("prepare");
        assert!(plan.full_scan, "首次必须先全量建库");
        assert!(plan.since_days.is_empty());

        let first = snapshot(vec![
            (Source::Claude, scan(true, vec![bucket("2026-10-24", "m", 100)])),
            (Source::Codex, scan(true, vec![bucket("2026-10-24", "m", 500)])),
        ]);
        store
            .apply(&t, "epoch-1", &plan, &first, "2026-10-24", &[])
            .expect("apply");

        let plan = store
            .prepare(&t, "epoch-1", &[Source::Claude, Source::Codex])
            .expect("prepare");
        assert!(!plan.full_scan);
        // 只有记录级源带窗口；累积值源（Codex）走整源替换
        assert_eq!(plan.since_day(Source::Claude), Some("2026-10-24"));
        assert_eq!(plan.since_day(Source::Codex), None);
    }

    #[test]
    fn window_apply_replaces_instead_of_double_counting() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(true, vec![bucket("2026-10-24", "m", 100)]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();

        // 同一窗口重跑一次：结果必须还是 100，不是 200
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(true, vec![bucket("2026-10-24", "m", 100)]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets[0].tokens, 100);
    }

    #[test]
    fn window_keeps_older_days_and_adds_new_ones() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(
                    true,
                    vec![bucket("2026-10-23", "m", 10), bucket("2026-10-24", "m", 100)],
                ))]),
                "2026-10-24",
                &[],
            )
            .unwrap();

        // 第二天：只重算 10-24 之后
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        assert_eq!(plan.since_day(Source::Claude), Some("2026-10-24"));
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(
                    true,
                    vec![bucket("2026-10-24", "m", 120), bucket("2026-10-25", "m", 7)],
                ))]),
                "2026-10-25",
                &[],
            )
            .unwrap();

        let loaded = store.load(&t).unwrap().unwrap();
        let days: Vec<(String, i64)> = loaded.scans[0]
            .1
            .buckets
            .iter()
            .map(|bucket| (bucket.day.clone(), bucket.tokens))
            .collect();
        assert_eq!(
            days,
            vec![
                ("2026-10-23".to_string(), 10),
                ("2026-10-24".to_string(), 120),
                ("2026-10-25".to_string(), 7),
            ],
            "旧日期保留、窗口内被替换、新日期追加，且按 day 升序"
        );
    }

    #[test]
    fn snapshot_source_replaces_whole_history() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Codex]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Codex, scan(true, vec![bucket("2026-10-20", "m", 100)]))]),
                "2026-10-20",
                &[],
            )
            .unwrap();
        // 线程继续增长，仍归到创建日：必须变成 300，而不是 400
        let plan = store.prepare(&t, "e", &[Source::Codex]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Codex, scan(true, vec![bucket("2026-10-20", "m", 300)]))]),
                "2026-10-21",
                &[],
            )
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets.len(), 1);
        assert_eq!(loaded.scans[0].1.buckets[0].tokens, 300);
    }

    #[test]
    fn untrusted_scan_never_overwrites_history() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(true, vec![bucket("2026-10-24", "m", 100)]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();

        // 下一轮：部分数据 / 未检测到 / 明确失败，三种都不能删历史
        for (label, entry) in [
            ("partial", {
                let mut s = scan(true, vec![bucket("2026-10-24", "m", 5)]);
                s.partial = true;
                s
            }),
            ("undetected", scan(false, Vec::new())),
            ("error", {
                let mut s = scan(true, Vec::new());
                s.error = Some("数据库损坏".to_string());
                s
            }),
        ] {
            let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
            store
                .apply(&t, "e", &plan, &snapshot(vec![(Source::Claude, entry)]), "2026-10-25", &[])
                .unwrap();
            let loaded = store.load(&t).unwrap().unwrap();
            assert_eq!(
                loaded.scans[0].1.buckets[0].tokens, 100,
                "{label} 不得覆盖历史"
            );
            // 水位线也不能前进
            let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
            assert_eq!(
                plan.since_day(Source::Claude),
                Some("2026-10-24"),
                "{label} 不得推进水位线"
            );
        }
    }

    #[test]
    fn failed_sources_are_skipped() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(true, vec![bucket("2026-10-24", "m", 100)]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(true, vec![bucket("2026-10-24", "m", 1)]))]),
                "2026-10-25",
                &[Source::Claude],
            )
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets[0].tokens, 100);
    }

    #[test]
    fn snapshot_source_with_empty_result_keeps_history() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Zcode]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Zcode, scan(true, vec![bucket("2026-10-24", "glm", 9)]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();
        let plan = store.prepare(&t, "e", &[Source::Zcode]).unwrap();
        store
            .apply(&t, "e", &plan, &snapshot(vec![(Source::Zcode, scan(true, Vec::new()))]), "2026-10-25", &[])
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        assert_eq!(loaded.scans[0].1.buckets[0].tokens, 9);
    }

    #[test]
    fn epoch_change_forces_full_rescan() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "epoch-1", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "epoch-1",
                &plan,
                &snapshot(vec![(Source::Claude, scan(true, vec![bucket("2026-10-24", "m", 1)]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();
        assert!(!store.prepare(&t, "epoch-1", &[Source::Claude]).unwrap().full_scan);
        // 镜像被清空 → 新 epoch → 必须全量，否则会用不完整数据覆盖历史
        let plan = store.prepare(&t, "epoch-2", &[Source::Claude]).unwrap();
        assert!(plan.full_scan);
    }

    #[test]
    fn identity_change_forces_full_rescan() {
        let store = memory();
        let mut t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(&t, "e", &plan, &snapshot(vec![(Source::Claude, scan(true, Vec::new()))]), "2026-10-24", &[])
            .unwrap();
        t.identity = "server:other:22:root".to_string();
        assert!(store.prepare(&t, "e", &[Source::Claude]).unwrap().full_scan);
    }

    #[test]
    fn load_returns_full_history_and_summary() {
        let store = memory();
        let t = target(Platform::Windows, "local");
        let plan = store.prepare(&t, "e", &[Source::Claude, Source::Qoder]).unwrap();
        let mut qoder = scan(true, vec![DayBucket {
            day: "2026-10-25".to_string(),
            model: "qm".to_string(),
            tokens: 0,
            credits: 1.5,
            messages: 2,
            requests: 2,
        }]);
        qoder.coverage_summary = "2 请求 · 1.500 积分".to_string();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![
                    (
                        Source::Claude,
                        scan(
                            true,
                            vec![bucket("2026-10-24", "m", 10), bucket("2026-10-25", "m", 20)],
                        ),
                    ),
                    (Source::Qoder, qoder),
                ]),
                "2026-10-25",
                &[],
            )
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        let claude = &loaded.scans[0].1;
        assert_eq!(claude.coverage_tokens, 30, "coverage 必须是全历史合计");
        assert_eq!(claude.coverage_summary, "2 响应 · 30 tokens");
        assert_eq!(claude.buckets[0].day, "2026-10-24", "读取顺序为 day ASC");
        let qoder = &loaded.scans[1].1;
        assert_eq!(qoder.coverage_summary, "2 请求 · 1.500 积分");
    }

    #[test]
    fn load_marks_historical_source_as_detected() {
        let store = memory();
        let t = target(Platform::Windows, "local");
        let plan = store.prepare(&t, "e", &[Source::WorkBuddy]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::WorkBuddy, scan(true, vec![bucket("2026-10-24", "m", 5)]))]),
                "2026-10-24",
                &[],
            )
            .unwrap();
        // 之后 WorkBuddy 被卸载：历史仍要显示，并给出提示
        let plan = store.prepare(&t, "e", &[Source::WorkBuddy]).unwrap();
        store
            .apply(&t, "e", &plan, &snapshot(vec![(Source::WorkBuddy, scan(false, Vec::new()))]), "2026-10-25", &[])
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        let scan = &loaded.scans[0].1;
        assert!(scan.detected);
        assert!(scan.warnings.iter().any(|warning| warning.contains("历史数据")));
        assert_eq!(scan.buckets[0].tokens, 5);
    }

    #[test]
    fn probe_only_source_stores_counts_without_buckets() {
        let store = memory();
        let t = target(Platform::Windows, "local");
        let plan = store.prepare(&t, "e", &[Source::Doubao]).unwrap();
        let mut probe = scan(true, Vec::new());
        probe.coverage_summary = "42 个文件".to_string();
        probe.coverage_messages = 42;
        store
            .apply(&t, "e", &plan, &snapshot(vec![(Source::Doubao, probe)]), "2026-10-25", &[])
            .unwrap();
        let loaded = store.load(&t).unwrap().unwrap();
        let scan = &loaded.scans[0].1;
        assert!(scan.buckets.is_empty());
        assert_eq!(scan.coverage_summary, "42 个文件");
    }

    #[test]
    fn list_targets_and_last_success() {
        let store = memory();
        let a = target(Platform::Windows, "local");
        let b = target(Platform::Wsl, "wsl:Ubuntu:u");
        for t in [&a, &b] {
            let plan = store.prepare(t, "e", &[Source::Claude]).unwrap();
            store
                .apply(t, "e", &plan, &snapshot(vec![(Source::Claude, scan(true, Vec::new()))]), "2026-10-25", &[])
                .unwrap();
        }
        let targets = store.list_targets().unwrap();
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|t| t.platform == Platform::Wsl));
        assert!(store.last_success_at("local").is_some());
        assert!(store.last_success_at("missing").is_none());
    }

    #[test]
    fn day_range_reports_far_to_near_bounds() {
        let store = memory();
        let t = target(Platform::Linux, "srv");
        let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
        store
            .apply(
                &t,
                "e",
                &plan,
                &snapshot(vec![(Source::Claude, scan(
                    true,
                    vec![bucket("2026-10-01", "m", 1), bucket("2026-10-25", "m", 2)],
                ))]),
                "2026-10-25",
                &[],
            )
            .unwrap();
        assert_eq!(
            store.day_range("srv"),
            Some(("2026-10-01".to_string(), "2026-10-25".to_string()))
        );
    }

    #[test]
    fn concurrent_applies_from_multiple_threads() {
        let store = Arc::new(memory());
        let mut handles = Vec::new();
        for index in 0..4 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                let id = format!("srv-{index}");
                let t = target(Platform::Linux, &id);
                for round in 0..10 {
                    let plan = store.prepare(&t, "e", &[Source::Claude]).unwrap();
                    store
                        .apply(
                            &t,
                            "e",
                            &plan,
                            &snapshot(vec![(Source::Claude, scan(
                                true,
                                vec![bucket("2026-10-24", "m", round)],
                            ))]),
                            "2026-10-24",
                            &[],
                        )
                        .expect("并发写入不得失败");
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(store.list_targets().unwrap().len(), 4);
    }
}
