//! OpenCode：~/.local/share/opencode/opencode.db
//!
//! 库有两种存储结构，都要支持：
//!
//! - **旧版**：`message` 表。data JSON 里 `role=="assistant"`，
//!   `tokens{input,output,reasoning,total,cache{read,write}}`，模型在顶层 `modelID`。
//! - **新版**（引入 `session_v2` 之后）：`session_message` 表。`type=="assistant"`，
//!   `tokens` **没有 total**，模型在 `model.{id,providerID}`，`time_created` 列是毫秒。
//!
//! 升级后的库会把旧 `message` 保留成迁移副本（实测 id 与新表 100% 重叠），所以必须
//! **按 id 去重、新表优先**：只读新表会让旧版库（未升级的机器）读不到，两表相加会双计。
//!
//! 口径：`total` 有值用 total；否则 `input+output+reasoning+cache.read+cache.write`。
//! 旧库实测 total 恒等于该分项和（2706/2999 行抽查一致），新旧因此同口径。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;
use serde_json::Value;

use crate::model::{DayBucket, Platform, Source, SourceScan};
use crate::parsing::token::{
    bj_day_of, clean_model, day_minus, parse_timestamp, safe_i64, sqlite, ScanContext,
};

/// 新版（session_v2 迁移后）的消息表。
const NEW_TABLE: &str = "session_message";
/// 旧版消息表；升级后的库里只剩一份迁移副本。
const OLD_TABLE: &str = "message";

/// 窗口粗筛：`time_created` 可能是秒、毫秒、文本或 NULL，因此只在同一行内
/// 按自身量级比较，做完粗筛后再由记录级 `in_window` 精确归日。
/// `?1` = 秒级 cutoff，`?2` = `?1 * 1000`。
const WINDOW_FILTER: &str = "
    ?1 is null
    or typeof(time_created) <> 'integer'
    or time_created = 0
    or time_created >= (case when time_created >= 100000000000 then ?2 else ?1 end)";

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableKind {
    /// 新表：assistant 由 SQL 的 `type='assistant'` 过滤。
    SessionMessage,
    /// 旧表：role 过滤只能在记录级做（data 是 JSON 文本）。
    Message,
}

struct MessageRow {
    id: String,
    time_created: SqlValue,
    record: Value,
}

impl MessageRow {
    /// 记录时间：优先 `data.time.created`，缺失时退回 `time_created` 列。
    fn day(&self) -> Option<String> {
        let when = self
            .record
            .get("time")
            .and_then(|time| time.get("created"))
            .cloned()
            .unwrap_or_else(|| sql_to_json(&self.time_created));
        bj_day_of(parse_timestamp(&when))
    }
}

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    let base = mirror.join(".local").join("share").join("opencode");
    let detected = base.is_dir();
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    let db_path = base.join("opencode.db");
    if !db_path.is_file() {
        if detected {
            scan.warnings
                .push("未找到 ~/.local/share/opencode/opencode.db".to_string());
        }
        return scan;
    }
    scan_impl(&db_path, ctx, &mut scan);
    scan
}

/// 窗口下界（UTC 秒）：`since - 1 天` 的北京 00:00。
/// `time_created` 列可能是秒/毫秒/NULL/文本，因此 SQL 里只做保守的粗筛，
/// 真正的归日与过滤仍在记录级完成。
fn window_epoch_seconds(ctx: &ScanContext) -> Option<i64> {
    let since = ctx.since_day(Source::Opencode)?;
    let cutoff = day_minus(since, 1)?;
    let date = chrono::NaiveDate::parse_from_str(&cutoff, "%Y-%m-%d").ok()?;
    let midnight = date.and_hms_opt(0, 0, 0)?;
    Some(midnight.and_utc().timestamp() - 8 * 3600)
}

fn scan_impl(db_path: &Path, ctx: &ScanContext, scan: &mut SourceScan) {
    let connection = match sqlite::open_for_scan(db_path, ctx.platform) {
        Ok(connection) => connection,
        Err(err) => {
            scan.error = Some(err.to_string());
            scan.warnings.push("OpenCode 数据库读取失败".to_string());
            return;
        }
    };
    let tables = match table_names(&connection) {
        Ok(tables) => tables,
        Err(err) => {
            scan.error = Some(err);
            scan.warnings.push("OpenCode 数据库读取失败".to_string());
            return;
        }
    };
    let has_new = tables.iter().any(|table| table == NEW_TABLE);
    let has_old = tables.iter().any(|table| table == OLD_TABLE);
    if !has_new && !has_old {
        scan.warnings
            .push("OpenCode 数据库缺少 message / session_message 表".to_string());
        return;
    }
    let sessions = session_count(&connection, &tables);
    let cutoff = window_epoch_seconds(ctx);

    let new_rows = match read_rows(&connection, has_new, NEW_TABLE, TableKind::SessionMessage, cutoff)
    {
        Ok(rows) => rows,
        Err(err) => {
            scan.error = Some(err);
            scan.warnings.push("OpenCode 数据库读取失败".to_string());
            return;
        }
    };
    let old_rows = match read_rows(&connection, has_old, OLD_TABLE, TableKind::Message, cutoff) {
        Ok(rows) => rows,
        Err(err) => {
            scan.error = Some(err);
            scan.warnings.push("OpenCode 数据库读取失败".to_string());
            return;
        }
    };

    // 新表是迁移后的权威副本：同 id 的旧行一律丢弃，否则一次会话会被算两遍。
    let migrated: HashSet<&str> = new_rows.iter().map(|row| row.id.as_str()).collect();

    let mut buckets: HashMap<(String, String), (i64, i64)> = HashMap::new();
    let mut used: i64 = 0;
    let mut skipped_dates: u64 = 0;

    for row in new_rows
        .iter()
        .chain(old_rows.iter().filter(|row| !migrated.contains(row.id.as_str())))
    {
        let Some((tokens, model)) = usage_of(&row.record) else {
            continue;
        };
        let Some(day) = row.day() else {
            skipped_dates += 1;
            continue;
        };
        if !ctx.in_window(Source::Opencode, &day) {
            continue;
        }
        let entry = buckets.entry((day, model)).or_insert((0, 0));
        entry.0 += tokens;
        entry.1 += 1;
        used += 1;
    }

    let mut keys: Vec<_> = buckets.keys().cloned().collect();
    keys.sort();
    let mut coverage_tokens = 0i64;
    for key in &keys {
        let (tokens, messages_in_bucket) = buckets[key];
        coverage_tokens += tokens;
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens,
            credits: 0.0,
            messages: messages_in_bucket,
            requests: messages_in_bucket,
        });
    }

    scan.coverage_tokens = coverage_tokens;
    scan.coverage_messages = used;
    scan.coverage_summary = format!("{used} 条用量 · {coverage_tokens} tokens · {sessions} 会话");
    if skipped_dates > 0 {
        scan.warnings.push(format!(
            "{skipped_dates} 条 OpenCode 记录缺少有效时间戳，未计入统计"
        ));
    }
    if used == 0 && ctx.since_day(Source::Opencode).is_none() {
        scan.warnings.push("未发现 OpenCode 用量记录".to_string());
    }
}

/// 读一张消息表。表不存在时返回空集（`present` 为 false），不算错误。
fn read_rows(
    connection: &Connection,
    present: bool,
    table: &str,
    kind: TableKind,
    cutoff: Option<i64>,
) -> Result<Vec<MessageRow>, String> {
    if !present {
        return Ok(Vec::new());
    }
    let type_filter = if kind == TableKind::SessionMessage {
        "type = 'assistant' and"
    } else {
        ""
    };
    let sql = format!(
        "select id, time_created, data from {table} where {type_filter} ({WINDOW_FILTER})"
    );
    let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([cutoff, cutoff.map(|value| value.saturating_mul(1000))], |row| {
            let id: String = row.get(0)?;
            let time_created: SqlValue = row.get(1)?;
            let data: SqlValue = row.get(2)?;
            Ok((id, time_created, data))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    let rows = rows
        .into_iter()
        .filter_map(|(id, time_created, data)| {
            let record = match data {
                SqlValue::Text(text) => serde_json::from_str::<Value>(&text).ok()?,
                SqlValue::Blob(blob) => serde_json::from_slice::<Value>(&blob).ok()?,
                _ => return None,
            };
            // 旧表的 role 只能在记录级判断；新表已由 SQL 的 type 过滤。
            if kind == TableKind::Message
                && record.get("role").and_then(Value::as_str) != Some("assistant")
            {
                return None;
            }
            Some(MessageRow {
                id,
                time_created,
                record,
            })
        })
        .collect();
    Ok(rows)
}

/// 从一条 assistant 记录取（tokens, model）。没有用量就返回 None。
///
/// `total` 缺失时按分项求和（含 cache），与旧库 total 的实测口径一致。
/// 模型名：新版 `model.id` 优先，旧版 `modelID` 回退；不拼 providerID。
fn usage_of(record: &Value) -> Option<(i64, String)> {
    let usage = record.get("tokens")?.as_object()?;
    let cache = usage.get("cache");
    let cache_read = if cache.map(Value::is_object).unwrap_or(false) {
        safe_i64(cache.and_then(|value| value.get("read")))
    } else {
        0
    };
    let cache_write = if cache.map(Value::is_object).unwrap_or(false) {
        safe_i64(cache.and_then(|value| value.get("write")))
    } else {
        0
    };
    let input = safe_i64(usage.get("input"));
    let output = safe_i64(usage.get("output"));
    let reasoning = safe_i64(usage.get("reasoning"));
    let mut total = safe_i64(usage.get("total"));
    if total <= 0 {
        total = input + output + reasoning + cache_read + cache_write;
    }
    if total <= 0 {
        return None;
    }
    let model = record
        .get("model")
        .and_then(|model| model.get("id"))
        .or_else(|| record.get("modelID"));
    Some((total, clean_model(model)))
}

/// 会话数：新版看 `session_v2`（迁移后 `session` 只剩旧副本），旧版回退 `session`。
fn session_count(connection: &Connection, tables: &[String]) -> i64 {
    let table = if tables.iter().any(|name| name == "session_v2") {
        "session_v2"
    } else if tables.iter().any(|name| name == "session") {
        "session"
    } else {
        return 0;
    };
    connection
        .query_row(&format!("select count(*) from {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0)
}

fn table_names(connection: &Connection) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("select name from sqlite_master where type='table'")
        .map_err(|e| e.to_string())?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(names)
}

fn sql_to_json(value: &SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => Value::Number((*i).into()),
        SqlValue::Real(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        SqlValue::Text(text) => Value::String(text.clone()),
        SqlValue::Blob(blob) => Value::String(String::from_utf8_lossy(blob).into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use rusqlite::params;
    use std::fs;

    fn open_db(root: &Path) -> Connection {
        let base = root.join(".local/share/opencode");
        fs::create_dir_all(&base).unwrap();
        Connection::open(base.join("opencode.db")).unwrap()
    }

    #[test]
    fn reads_message_table() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let connection = open_db(&root);
        connection
            .execute("CREATE TABLE session (id TEXT PRIMARY KEY)", [])
            .unwrap();
        connection.execute("INSERT INTO session (id) VALUES ('s1')", []).unwrap();
        connection
            .execute(
                "CREATE TABLE message (id TEXT PRIMARY KEY, time_created REAL, data TEXT)",
                [],
            )
            .unwrap();
        let rows = [
            // assistant 带 total
            ("m1", 1760000100.0, r#"{"role":"assistant","modelID":"claude-sonnet-4","tokens":{"input":100,"output":50,"reasoning":10,"total":175,"cache":{"read":5,"write":10}},"time":{"created":"2026-10-23T05:00:00Z"}}"#),
            // assistant 缺 total → 分项求和（含 cache）；缺 time.created → 用 time_created 列
            ("m2", 1760000200.0, r#"{"role":"assistant","modelID":"gpt-5","tokens":{"input":30,"output":20,"reasoning":5,"cache":{"read":5,"write":10}}}"#),
            // user 行跳过
            ("m3", 1760000300.0, r#"{"role":"user","tokens":{"total":999}}"#),
        ];
        for (id, time, data) in rows {
            connection
                .execute(
                    "INSERT INTO message (id, time_created, data) VALUES (?1, ?2, ?3)",
                    params![id, time, data],
                )
                .unwrap();
        }
        drop(connection);

        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.buckets.len(), 2);
        let sonnet = scan.buckets.iter().find(|b| b.model == "claude-sonnet-4").unwrap();
        assert_eq!(sonnet.tokens, 175);
        let gpt = scan.buckets.iter().find(|b| b.model == "gpt-5").unwrap();
        assert_eq!(gpt.tokens, 70); // 30+20+5+5+10
        assert!(scan.coverage_summary.contains("2 条用量"));
        assert!(scan.coverage_summary.contains("1 会话"));
    }

    /// 新版结构：session_v2 + session_message，模型在 model.id，时间列是毫秒，tokens 无 total。
    fn seed_new_schema(connection: &Connection) {
        connection
            .execute("CREATE TABLE session_v2 (id TEXT PRIMARY KEY)", [])
            .unwrap();
        connection
            .execute("CREATE TABLE session (id TEXT PRIMARY KEY)", [])
            .unwrap();
        connection
            .execute(
                "CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                    type TEXT NOT NULL, seq INTEGER NOT NULL, time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL, data TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "CREATE TABLE message (id TEXT PRIMARY KEY, time_created INTEGER, data TEXT)",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO session_v2 (id) VALUES ('s1')", []).unwrap();
    }

    #[test]
    fn reads_session_message_table() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let connection = open_db(&root);
        seed_new_schema(&connection);
        // 北京 2026-10-26 18:00:37 → 毫秒列；input 68 + output 633 + cache.read 466432
        let rows = [
            ("msg_1", "assistant", 1793008837308i64, r#"{"time":{"created":1793008837308},"model":{"id":"deepseek-pro","providerID":"HISLab","variant":"high"},"tokens":{"input":68,"output":633,"reasoning":0,"cache":{"read":466432,"write":0}},"finish":"stop","cost":0}"#),
            ("msg_2", "user", 1793008830000i64, r#"{"time":{"created":1793008830000},"text":"hi"}"#),
        ];
        for (id, kind, time, data) in rows {
            connection
                .execute(
                    "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                     VALUES (?1, 's1', ?2, 0, ?3, ?3, ?4)",
                    params![id, kind, time, data],
                )
                .unwrap();
        }
        drop(connection);

        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.buckets.len(), 1);
        let bucket = &scan.buckets[0];
        assert_eq!(bucket.model, "deepseek-pro", "新版模型名在 model.id");
        assert_eq!(bucket.day, "2026-10-26", "毫秒时间戳要归到北京日");
        assert_eq!(bucket.tokens, 467_133);
        assert_eq!(scan.coverage_messages, 1, "user 行不进统计");
        assert!(scan.coverage_summary.contains("1 会话"), "会话数取 session_v2");
    }

    #[test]
    fn migrated_message_copy_is_not_double_counted() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let connection = open_db(&root);
        seed_new_schema(&connection);
        let new_data = r#"{"time":{"created":1793008837308},"model":{"id":"deepseek-pro"},"tokens":{"input":10,"output":20}}"#;
        connection
            .execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                 VALUES ('msg_same', 's1', 'assistant', 0, 1793008837308, 1793008837308, ?1)",
                params![new_data],
            )
            .unwrap();
        // 迁移前的旧副本：同 id，旧结构，total 故意写大，若被计入就会露馅
        let old_data = r#"{"role":"assistant","modelID":"deepseek-v4-pro","tokens":{"total":999999},"time":{"created":1793008837308}}"#;
        connection
            .execute(
                "INSERT INTO message (id, time_created, data) VALUES ('msg_same', 1793008837308, ?1)",
                params![old_data],
            )
            .unwrap();
        // 只在旧表存在的另一条（未迁移的新会话），必须仍然算进来
        let extra = r#"{"role":"assistant","modelID":"deepseek-chat","tokens":{"total":500},"time":{"created":1793008837308}}"#;
        connection
            .execute(
                "INSERT INTO message (id, time_created, data) VALUES ('msg_only_old', 1793008837308, ?1)",
                params![extra],
            )
            .unwrap();
        drop(connection);

        let scan = scan(&root);
        let total: i64 = scan.buckets.iter().map(|bucket| bucket.tokens).sum();
        assert_eq!(total, 530, "同 id 的迁移副本不能重复计入，新表优先");
        assert_eq!(scan.coverage_messages, 2);
    }

    #[test]
    fn window_skips_days_before_watermark() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let connection = open_db(&root);
        seed_new_schema(&connection);
        let rows = [
            ("msg_today", 1793008837308i64),   // 2026-10-26 北京
            ("msg_old", 1786152638364i64),     // 2026-08-06 北京
        ];
        for (id, time) in rows {
            let data = format!(
                r#"{{"time":{{"created":{time}}},"model":{{"id":"deepseek-pro"}},"tokens":{{"input":100,"output":100}}}}"#
            );
            connection
                .execute(
                    "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                     VALUES (?1, 's1', 'assistant', 0, ?2, ?2, ?3)",
                    params![id, time, data],
                )
                .unwrap();
        }
        drop(connection);

        let mut since = HashMap::new();
        since.insert(Source::Opencode, "2026-10-26".to_string());
        let context = ScanContext::with_window(Platform::Linux, &since);
        let scan = scan_with(&root, &context);
        assert_eq!(scan.buckets.len(), 1);
        assert_eq!(scan.buckets[0].day, "2026-10-26");
        assert_eq!(scan.coverage_messages, 1);
    }
}
