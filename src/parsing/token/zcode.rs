//! ZCode 用量：以 SQLite 的 `model_usage` 表为权威源，JSONL rollout 只补齐
//! 「数据库镜像尚未包含」的最新记录。
//!
//! - 权威源 `<home>/.zcode/cli/db/db.sqlite` → `model_usage`：请求级一行，
//!   `computed_total_tokens = input + output + reasoning`（`cache_read` 已含在
//!   `input_tokens` 内，**不能**再叠加 cache 列），按 `completed_at` 归北京日
//!   （0/NULL 回退 `started_at`）。
//! - 补充源 `~/.zcode/cli/rollout/model-io-session_*.jsonl`：每行一次模型请求，
//!   只在记录时间晚于数据库水位线（`max(completed_at) + 60s`）时才采纳。
//!   rollout 的 requestId 与数据库 ID 空间不相交（实测交集 0），因此不做 ID 匹配；
//!   已入库请求的 rollout 时间戳恒早于水位线（实测早 7~98ms），不会重复计。
//! - 数据库缺失/无表/无行/读失败时整轮回退到 rollout 全量，不静默归零。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;
use serde_json::Value;

use crate::model::{DayBucket, Platform, SourceScan};
use crate::parsing::token::{
    bj_day_of, clean_model, file_day, parse_timestamp, safe_i64, sqlite, walk_jsonl, ScanContext,
};

/// rollout 相对数据库水位线的安全容差（ms）。实测 rollout 的时间戳（客户端）
/// 比数据库的 `completed_at`（服务端）早 7~98ms，留 60s 吸收时钟漂移；
/// 被挡在容差内的新记录下一轮必然由数据库覆盖，不影响最终一致。
const WATERMARK_SLACK_MS: i64 = 60_000;

/// 单条候选（两条来源共用）。`ts_ms = 0` 表示时间戳无法解析。
#[derive(Clone)]
struct Candidate {
    day: String,
    model: String,
    tokens: i64,
    ts_ms: i64,
}

/// 数据库基线：候选 + 水位线（最大完成时刻）。
struct DbBaseline {
    candidates: Vec<Candidate>,
    watermark_ms: i64,
    /// 重复 `logical_request_id` 的条数（当前恒为 0，出现即说明库结构变了）。
    duplicates: u64,
}

/// `model_usage` 的一行（只取需要的 5 列）。
struct RawUsage {
    model: String,
    tokens: i64,
    completed_at: i64,
    started_at: i64,
    request_id: String,
}

/// rollout 解析统计（诊断/告警用）。
#[derive(Default)]
struct RolloutStats {
    files: usize,
    raw_rows: u64,
    bad: u64,
    missing_ts: u64,
}

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

/// ZCode 每次返回的都是「数据库全量 + 水位线之后的 rollout」的完整快照，
/// 因此是 `StorageMode::Snapshot`：不接受日期窗口，整源替换。
pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    let zcode_dir = mirror.join(".zcode");
    let detected = zcode_dir.is_dir();
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    if !detected {
        return scan;
    }

    let cli = zcode_dir.join("cli");
    let db_path = cli.join("db").join("db.sqlite");
    let rollout_dir = cli.join("rollout");

    // 1) 数据库基线
    let mut baseline: Option<DbBaseline> = None;
    if db_path.is_file() {
        match db_candidates(&db_path, ctx.platform) {
            Ok(Some(found)) => baseline = Some(found),
            Ok(None) => scan
                .warnings
                .push("ZCode 数据库无可用用量记录（缺 model_usage 表或表为空），回退 rollout 记录".into()),
            Err(err) => {
                scan.error = Some(truncate_error(&err));
                scan.warnings
                    .push("ZCode 数据库读取失败，已回退 rollout 记录".into());
            }
        }
    } else {
        scan.warnings
            .push("未找到 ~/.zcode/cli/db/db.sqlite，回退 rollout 记录".into());
    }
    // 数据库不可用时只能看到最近的 rollout 窗口 —— 那是残缺快照，
    // 不能拿它覆盖历史，交给历史库跳过本源的替换。
    let db_usable = baseline.is_some();
    if !db_usable && db_path.is_file() {
        scan.partial = true;
    }

    // 2) rollout（DB 可用时只保留水位线之后的记录）
    let mut stats = RolloutStats::default();
    let mut rollout_rows = rollout_candidates(&rollout_dir, &mut stats);
    if let Some(base) = &baseline {
        if base.duplicates > 0 {
            scan.warnings.push(format!(
                "{} 条 ZCode 用量存在重复 logical_request_id，按 tokens 最大值保留",
                base.duplicates
            ));
        }
    }
    let has_baseline = baseline.is_some();
    let mut appended = 0usize;
    let candidates = match baseline {
        Some(base) => {
            let cutoff = base.watermark_ms.saturating_add(WATERMARK_SLACK_MS);
            let scanned = rollout_rows.len();
            rollout_rows.retain(|item| item.ts_ms > cutoff);
            appended = rollout_rows.len();
            if appended > 0 {
                scan.warnings.push(format!(
                    "ZCode 数据库未包含最新的 {appended} 条记录，已由 rollout 补充（另有 {} 条已入库，跳过）",
                    scanned.saturating_sub(appended)
                ));
            }
            let mut merged = base.candidates;
            merged.extend(rollout_rows);
            merged
        }
        None => rollout_rows,
    };

    let rollout_relevant = !has_baseline || appended > 0;
    aggregate(candidates, &mut scan);
    scan.messages_total = stats.raw_rows as i64;

    if rollout_relevant {
        if stats.bad > 0 {
            scan.warnings
                .push(format!("{} 行 ZCode JSONL 无法解析，已跳过", stats.bad));
        }
        if stats.missing_ts > 0 {
            scan.warnings.push(format!(
                "{} 条 ZCode 记录缺少时间戳，按文件修改日期归档",
                stats.missing_ts
            ));
        }
    }
    if scan.buckets.is_empty() && scan.error.is_none() {
        scan.warnings.push("未发现 ZCode 会话记录".to_string());
    }
    scan
}

// ---------------------------------------------------------------------------
// 数据库来源
// ---------------------------------------------------------------------------

fn db_candidates(db_path: &Path, platform: Platform) -> Result<Option<DbBaseline>, String> {
    let rows = match sqlite::open_for_scan(db_path, platform).and_then(|conn| read_usage_rows(&conn))
    {
        Ok(rows) => rows,
        Err(first) => {
            // 只读连接无法执行 WAL 恢复；镜像三件套可能瞬时不自洽，复制后重试一次。
            match read_usage_via_copy(db_path) {
                Ok(rows) => rows,
                Err(second) => return Err(format!("{first} / 复制重试：{second}")),
            }
        }
    };
    if rows.is_empty() {
        return Ok(None);
    }

    let mut unique: HashMap<String, Candidate> = HashMap::new();
    let mut watermark_ms = 0i64;
    let mut duplicates = 0u64;

    for row in rows {
        let ts_ms = if row.completed_at > 0 {
            row.completed_at
        } else {
            row.started_at
        };
        watermark_ms = watermark_ms.max(ts_ms);
        let model_value = Value::String(row.model);
        let Some(day) = bj_day_of(parse_timestamp(&Value::from(ts_ms))) else {
            continue;
        };
        let candidate = Candidate {
            day,
            model: clean_model(Some(&model_value)),
            tokens: row.tokens,
            ts_ms,
        };
        // 当前库里 logical_request_id 100% 唯一、attempt_index 全 0，去重等同 no-op；
        // 保留它是为了固定「同一次逻辑请求的重试行取最大值」这条语义。
        match unique.get(&row.request_id) {
            Some(previous) if previous.tokens >= candidate.tokens => duplicates += 1,
            _ => {
                unique.insert(row.request_id, candidate);
            }
        }
    }

    if unique.is_empty() {
        return Ok(None);
    }
    Ok(Some(DbBaseline {
        candidates: unique.into_values().collect(),
        watermark_ms,
        duplicates,
    }))
}

fn read_usage_rows(conn: &Connection) -> Result<Vec<RawUsage>, String> {
    let has_table = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'model_usage'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|err| format!("查询 sqlite_master 失败: {err}"))?
        > 0;
    if !has_table {
        return Ok(Vec::new());
    }

    // 只取 computed_total_tokens：实测 = input + output + reasoning，且
    // cache_read_input_tokens 已包含在 input_tokens 内，cache 列根本不进结果集，
    // 从结构上杜绝「再叠一次缓存」的错误（会让大部分行虚高）。
    // 不按 status 过滤：error/cancelled 的 tokens 恒为 0，已被 > 0 排除，
    // 显式过滤反而会在 ZCode 新增终态时误删有效行。
    let mut stmt = conn
        .prepare(
            "SELECT model_id, computed_total_tokens, completed_at, started_at, logical_request_id \
             FROM model_usage WHERE computed_total_tokens > 0",
        )
        .map_err(|err| format!("准备 model_usage 查询失败: {err}"))?;
    let mut rows = stmt
        .query_map([], |row| {
            Ok(RawUsage {
                model: text_of(row.get(0)?),
                tokens: int_of(row.get(1)?),
                completed_at: int_of(row.get(2)?),
                started_at: int_of(row.get(3)?),
                request_id: text_of(row.get(4)?),
            })
        })
        .map_err(|err| format!("执行 model_usage 查询失败: {err}"))?;

    let mut out = Vec::new();
    for row in rows.by_ref() {
        match row {
            Ok(item) => out.push(item),
            Err(err) => return Err(format!("读取 model_usage 行失败: {err}")),
        }
    }
    Ok(out)
}

/// WAL 兜底：把 `db` + `-wal` + `-shm` 复制到临时目录后以读写方式打开，
/// 让 SQLite 自己完成 WAL 恢复（只读连接做不到）。
fn read_usage_via_copy(db_path: &Path) -> Result<Vec<RawUsage>, String> {
    let dir = std::env::temp_dir().join(format!(
        "token_summarizer_zcode_{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).map_err(|err| format!("创建临时目录失败: {err}"))?;
    let _guard = TempDirGuard(dir.clone());

    let file_name = db_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "db.sqlite".to_string());
    let copy = dir.join(&file_name);
    std::fs::copy(db_path, &copy).map_err(|err| format!("复制数据库失败: {err}"))?;
    for suffix in ["-wal", "-shm"] {
        let source = db_path.with_file_name(format!("{file_name}{suffix}"));
        if source.is_file() {
            let target = dir.join(format!("{file_name}{suffix}"));
            std::fs::copy(&source, &target).map_err(|err| format!("复制 {suffix} 失败: {err}"))?;
        }
    }

    let conn = Connection::open(&copy).map_err(|err| format!("打开副本失败: {err}"))?;
    read_usage_rows(&conn)
}

struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text_of(value: SqlValue) -> String {
    match value {
        SqlValue::Text(text) => text,
        SqlValue::Integer(number) => number.to_string(),
        SqlValue::Real(number) => number.to_string(),
        SqlValue::Null | SqlValue::Blob(_) => String::new(),
    }
}

fn int_of(value: SqlValue) -> i64 {
    match value {
        SqlValue::Integer(number) => number,
        SqlValue::Real(number) => number as i64,
        SqlValue::Text(text) => text.trim().parse::<f64>().map(|f| f as i64).unwrap_or(0),
        SqlValue::Null | SqlValue::Blob(_) => 0,
    }
}

fn truncate_error(text: &str) -> String {
    let trimmed = text.trim();
    let text = if trimmed.is_empty() { text } else { trimmed };
    text.chars().take(400).collect()
}

// ---------------------------------------------------------------------------
// rollout 来源
// ---------------------------------------------------------------------------

fn rollout_candidates(rollout: &Path, stats: &mut RolloutStats) -> Vec<Candidate> {
    let files = walk_jsonl(rollout);
    stats.files = files.len();

    let mut unique: HashMap<String, Candidate> = HashMap::new();
    let mut raw: u64 = 0;

    for path in &files {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                stats.bad += 1;
                continue;
            };
            let total = usage_total(&record);
            if total <= 0 {
                continue; // 无 usage 的行（失败/中断请求）
            }
            raw += 1;
            let model_value = record
                .pointer("/model/modelId")
                .filter(|value| {
                    value.as_str().map(str::trim).is_some_and(|text| !text.is_empty())
                })
                .or_else(|| record.pointer("/response/modelId"))
                .or_else(|| record.pointer("/request/body/model"));
            let model = clean_model(model_value);
            let completed = record.get("completedAt").unwrap_or(&Value::Null);
            let parsed = parse_timestamp(completed);
            let ts_ms = parsed.map(|dt| dt.timestamp_millis()).unwrap_or(0);
            let mut day = bj_day_of(parsed);
            if day.is_none() {
                stats.missing_ts += 1;
                day = file_day(path);
            }
            let Some(day) = day else {
                continue;
            };
            let fallback = format!("{}:{}", path.display(), raw);
            let key = record
                .get("requestId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or(fallback);
            match unique.get(&key) {
                Some(previous) if previous.tokens >= total => {}
                _ => {
                    unique.insert(
                        key,
                        Candidate {
                            day,
                            model,
                            tokens: total,
                            ts_ms,
                        },
                    );
                }
            }
        }
    }
    stats.raw_rows = raw;
    unique.into_values().collect()
}

// ---------------------------------------------------------------------------
// 聚合
// ---------------------------------------------------------------------------

fn aggregate(candidates: Vec<Candidate>, scan: &mut SourceScan) {
    let mut buckets: HashMap<(String, String), (i64, i64)> = HashMap::new();
    for candidate in &candidates {
        let entry = buckets
            .entry((candidate.day.clone(), candidate.model.clone()))
            .or_insert((0, 0));
        entry.0 += candidate.tokens;
        entry.1 += 1;
    }

    let mut keys: Vec<_> = buckets.keys().cloned().collect();
    keys.sort();
    let mut coverage_tokens = 0i64;
    for key in &keys {
        let (tokens, requests) = buckets[key];
        coverage_tokens += tokens;
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens,
            credits: 0.0,
            messages: requests,
            requests,
        });
    }

    let requests = candidates.len();
    scan.coverage_tokens = coverage_tokens;
    scan.coverage_messages = requests as i64;
    scan.coverage_summary = format!("{requests} 次请求 · {coverage_tokens} tokens");
}

/// response.usage（camelCase）：totalTokens 优先，缺则 input+output；
/// 再回退 response.providerMetadata 里的 snake_case 口径求和。
fn usage_total(record: &Value) -> i64 {
    if let Some(usage) = record.pointer("/response/usage").and_then(Value::as_object) {
        let total = safe_i64(usage.get("totalTokens"));
        if total > 0 {
            return total;
        }
        let sum = safe_i64(usage.get("inputTokens")) + safe_i64(usage.get("outputTokens"));
        if sum > 0 {
            return sum;
        }
    }
    let provider = record
        .pointer("/response/providerMetadata/anthropic/usage")
        .cloned()
        .unwrap_or(Value::Null);
    if let Some(usage) = provider.as_object() {
        let sum = safe_i64(usage.get("input_tokens"))
            + safe_i64(usage.get("output_tokens"))
            + safe_i64(usage.get("cache_read_input_tokens"))
            + safe_i64(usage.get("cache_creation_input_tokens"));
        if sum > 0 {
            return sum;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use rusqlite::params;
    use std::fs;

    /// 建一个只含 `model_usage` 的测试库（省略外键与 CHECK，查询不依赖它们）。
    fn fixture_db(root: &Path) -> Connection {
        let path = root.join(".zcode/cli/db/db.sqlite");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                 model_id text not null,
                 status text not null default 'completed',
                 started_at integer not null,
                 completed_at integer,
                 input_tokens integer not null default 0,
                 output_tokens integer not null default 0,
                 reasoning_tokens integer not null default 0,
                 cache_creation_input_tokens integer not null default 0,
                 cache_read_input_tokens integer not null default 0,
                 provider_total_tokens integer,
                 computed_total_tokens integer not null default 0,
                 logical_request_id text not null
             );",
        )
        .unwrap();
        conn
    }

    fn insert_usage(
        conn: &Connection,
        model: &str,
        completed_at: Option<i64>,
        started_at: i64,
        computed: i64,
        request_id: &str,
    ) {
        conn.execute(
            "INSERT INTO model_usage
                 (model_id, started_at, completed_at, computed_total_tokens, logical_request_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![model, started_at, completed_at, computed, request_id],
        )
        .unwrap();
    }

    fn write_rollout(root: &Path, lines: &[String]) {
        let file = root.join(".zcode/cli/rollout/model-io-session_test.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mut body = lines.join("\n");
        body.push('\n');
        fs::write(&file, body).unwrap();
    }

    fn rollout_line(completed_at: &str, model: &str, tokens: i64, request_id: &str) -> String {
        format!(
            r#"{{"completedAt":"{completed_at}","requestId":"{request_id}","model":{{"modelId":"{model}"}},"response":{{"usage":{{"totalTokens":{tokens}}}}}}}"#
        )
    }

    fn ms(text: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(text)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn db_takes_priority_and_skips_covered_rollout() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let at = ms("2026-10-24T10:00:00Z");
        {
            let conn = fixture_db(&root);
            insert_usage(&conn, "GLM-5.3", Some(at), at, 125, "msg_a");
        }
        // 同一请求在 rollout 里也有一份（值故意写大）：水位线内必须被跳过，否则会双计。
        write_rollout(
            &root,
            &[rollout_line("2026-10-24T10:00:00Z", "GLM-5.3", 999, "r1")],
        );

        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.coverage_tokens, 125, "DB 已覆盖的 rollout 不得相加");
        assert_eq!(scan.coverage_messages, 1);
        assert_eq!(scan.buckets.len(), 1);
        assert!(scan.error.is_none());
    }

    #[test]
    fn rollout_after_watermark_is_appended() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let at = ms("2026-10-24T10:00:00Z");
        {
            let conn = fixture_db(&root);
            insert_usage(&conn, "GLM-5.3", Some(at), at, 125, "msg_a");
        }
        // 5 分钟后完成 → 越过水位线 + 容差，应补入。
        write_rollout(
            &root,
            &[rollout_line(
                "2026-10-24T10:05:00Z",
                "GLM-5.3-Flash",
                999,
                "r2",
            )],
        );

        let scan = scan(&root);
        assert_eq!(scan.coverage_tokens, 125 + 999);
        assert_eq!(scan.coverage_messages, 2);
        assert_eq!(scan.buckets.len(), 2);
        assert!(
            scan.warnings.iter().any(|w| w.contains("由 rollout 补充")),
            "补齐应留下可观测的提示：{:?}",
            scan.warnings
        );
    }

    #[test]
    fn cache_columns_are_not_added_twice() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let at = ms("2026-10-24T10:00:00Z");
        {
            let conn = fixture_db(&root);
            // computed = input(1000) + output(50) + reasoning(25) = 1075；
            // cache_read(800) 已含在 input 内，cache_creation(100) 亦不得再加。
            conn.execute(
                "INSERT INTO model_usage
                     (model_id, started_at, completed_at, input_tokens, output_tokens,
                      reasoning_tokens, cache_read_input_tokens, cache_creation_input_tokens,
                      computed_total_tokens, logical_request_id)
                 VALUES (?1, ?2, ?3, 1000, 50, 25, 800, 100, 1075, ?4)",
                params!["GLM-5.3", at, at, "msg_a"],
            )
            .unwrap();
        }

        let scan = scan(&root);
        assert_eq!(scan.coverage_tokens, 1075, "叠加 cache 列会虚高到 1975");
    }

    #[test]
    fn db_day_uses_completed_at_then_started_at() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        {
            let conn = fixture_db(&root);
            // A：UTC 10-23 15:30 开始、16:30 完成 → 北京 10-24 00:30，应归 10-24。
            insert_usage(
                &conn,
                "GLM-5.3",
                Some(ms("2026-10-23T16:30:00Z")),
                ms("2026-10-23T15:30:00Z"),
                100,
                "msg_a",
            );
            // B：缺 completed_at → 回退 started_at → 北京 10-23 23:30，应归 10-23。
            insert_usage(
                &conn,
                "GLM-5.3-Flash",
                None,
                ms("2026-10-23T15:30:00Z"),
                200,
                "msg_b",
            );
        }

        let scan = scan(&root);
        let mut days: Vec<&str> = scan.buckets.iter().map(|b| b.day.as_str()).collect();
        days.sort();
        assert_eq!(days, vec!["2026-10-23", "2026-10-24"]);
    }

    #[test]
    fn missing_db_and_bad_db_fall_back_to_rollout() {
        // 无数据库 → 全量 rollout
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        write_rollout(
            &root,
            &[rollout_line("2026-10-24T10:00:00Z", "GLM-5.3", 500, "r1")],
        );
        let first = scan(&root);
        assert_eq!(first.coverage_tokens, 500);
        assert!(first.error.is_none());
        assert!(
            first.warnings.iter().any(|w| w.contains("回退")),
            "{:?}",
            first.warnings
        );

        // 数据库文件损坏 → 记 error，但数字仍来自 rollout
        let dir2 = TempDir::new();
        let root2 = dir2.path().to_path_buf();
        let db_path = root2.join(".zcode/cli/db/db.sqlite");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::write(&db_path, b"not a database").unwrap();
        write_rollout(
            &root2,
            &[rollout_line("2026-10-24T10:00:00Z", "GLM-5.3", 500, "r1")],
        );
        let scan2 = scan(&root2);
        assert!(scan2.error.is_some(), "损坏的库应记 error");
        assert_eq!(scan2.coverage_tokens, 500, "仍应回退到 rollout");    }

    #[test]
    fn parses_zcode_rollout_with_dedup_and_fallbacks() {
        let jsonl = concat!(
            // 常规：response.usage.totalTokens
            r#"{"completedAt":"2026-10-24T15:36:40.518Z","requestId":"r1","attempt":1,"model":{"modelId":"GLM-5.3"},"response":{"usage":{"inputTokens":327838,"outputTokens":173,"totalTokens":328011,"cacheReadTokens":327616,"cacheWriteTokens":0}}}"#, "\n",
            // 同 requestId 重试（attempt 2，总量更小）→ 保留最大
            r#"{"completedAt":"2026-10-24T15:36:41.000Z","requestId":"r1","attempt":2,"model":{"modelId":"GLM-5.3"},"response":{"usage":{"inputTokens":100,"outputTokens":20,"totalTokens":120,"cacheReadTokens":0,"cacheWriteTokens":0}}}"#, "\n",
            // 缺 modelId → 回退 request.body.model；缺 totalTokens → input+output
            r#"{"completedAt":"2026-10-24T16:00:00Z","requestId":"r2","model":{"modelId":""},"request":{"body":{"model":"glm-5.3"}},"response":{"usage":{"inputTokens":500,"outputTokens":50}}}"#, "\n",
            // 无 response.usage → 回退 providerMetadata snake_case（222+173+327616=328011，
            // completedAt 为 UTC 10-23 18:00 = 北京 10-24 02:00，归入北京 10-24）
            r#"{"completedAt":"2026-10-23T18:00:00Z","requestId":"r3","model":{"modelId":"GLM-5.3"},"response":{"providerMetadata":{"anthropic":{"usage":{"input_tokens":222,"output_tokens":173,"cache_read_input_tokens":327616}}}}}"#, "\n",
            // 无任何 usage（失败请求）→ 跳过
            r#"{"completedAt":"2026-10-24T17:00:00Z","requestId":"r4","model":{"modelId":"GLM-5.3"},"error":"timeout"}"#, "\n",
            // 无时间戳 → 按文件 mtime 归档；requestId 缺失 → 文件:行号键
            r#"{"model":{"modelId":"GLM-5.3"},"response":{"usage":{"totalTokens":42}}}"#, "\n",
            // 坏行
            "{broken\n"
        );
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let file = root.join(".zcode/cli/rollout/model-io-session_abc.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, jsonl).unwrap();

        let scan = scan(&root);
        assert!(scan.detected);
        // 去重（r1 保留 328011）、回退（r2=550, r3=222+173+327616=328011）、
        // 无时间戳行（42，归入文件 mtime 日，不依赖运行日期断言）
        assert_eq!(scan.coverage_tokens, 328011 + 550 + 328011 + 42);
        let fallback_model = scan
            .buckets
            .iter()
            .find(|b| b.model == "glm-5.3")
            .expect("模型回退到 request.body.model");
        assert_eq!(fallback_model.tokens, 550);
        // completedAt 为 UTC 2026-10-24T15:36 → 北京同日；attempt 2 的小记录被去重
        let day24_glm: i64 = scan
            .buckets
            .iter()
            .filter(|b| b.day == "2026-10-24" && b.model == "GLM-5.3")
            .map(|b| b.tokens)
            .sum();
        assert_eq!(day24_glm, 328011 + 328011);
        // 4 条有效记录（r1 去重后算 1 条）
        assert_eq!(scan.coverage_messages, 4);
        assert!(scan.coverage_summary.contains("4 次请求"));
        assert!(scan.warnings.iter().any(|w| w.contains("无法解析")));
        assert!(scan.warnings.iter().any(|w| w.contains("缺少时间戳")));
    }

    #[test]
    fn beijing_day_for_completed_at() {
        // UTC 2026-10-23T18:00 = 北京 2026-10-24 02:00
        let value = serde_json::json!("2026-10-23T18:00:00Z");
        assert_eq!(
            bj_day_of(parse_timestamp(&value)).as_deref(),
            Some("2026-10-24")
        );
    }

    #[test]
    fn not_detected_returns_empty() {
        let dir = TempDir::new();
        let scan = scan(&dir.path());
        assert!(!scan.detected);
        assert!(scan.buckets.is_empty());
    }
}
