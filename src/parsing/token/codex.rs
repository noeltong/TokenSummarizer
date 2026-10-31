//! Codex CLI：~/.codex/state_5.sqlite。
//! SELECT model, tokens_used, created_at FROM threads
//! WHERE tokens_used IS NOT NULL AND tokens_used > 0。
//! 镜像已包含 wal/shm 副本，此处直接打开本地缓存副本。

use std::collections::HashMap;
use std::path::Path;

use rusqlite::types::Value as SqlValue;

use crate::model::{DayBucket, Platform, SourceScan};
use crate::parsing::token::{bj_day_of, clean_model, parse_timestamp, safe_i64, sqlite, ScanContext};

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

/// Codex 的 `threads.tokens_used` 是**线程累计值**、归日 `created_at`：
/// 一个跨天长线程的后续增长会记在创建日。按日期窗口过滤会永久丢掉这部分，
/// 因此本源是 `StorageMode::Snapshot`，每轮全量重算（行数很少）。
pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    let codex_dir = mirror.join(".codex");
    let detected = codex_dir.is_dir();
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    let db_path = codex_dir.join("state_5.sqlite");
    if !db_path.is_file() {
        if detected {
            scan.warnings.push("未找到 ~/.codex/state_5.sqlite".to_string());
        }
        return scan;
    }

    let result: Result<(i64, Vec<(SqlValue, SqlValue, SqlValue)>), String> = (|| {
        let connection = sqlite::open_for_scan(&db_path, ctx.platform)?;
        let total: i64 = connection
            .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT model, tokens_used, created_at FROM threads \
                 WHERE tokens_used IS NOT NULL AND tokens_used > 0",
            )
            .map_err(|e| e.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, SqlValue>(0)?,
                    row.get::<_, SqlValue>(1)?,
                    row.get::<_, SqlValue>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok((total, rows))
    })();

    let (total_threads, rows) = match result {
        Ok(value) => value,
        Err(err) => {
            scan.error = Some(truncate_error(&err));
            scan.warnings.push("Codex 数据库读取失败".to_string());
            return scan;
        }
    };

    let mut buckets: HashMap<(String, String), (i64, i64)> = HashMap::new();
    let mut valid_threads: i64 = 0;
    let mut skipped_dates: u64 = 0;

    for (model_value, tokens_value, created_at_value) in rows {
        let token_count = safe_i64(Some(&sql_to_json(&tokens_value)));
        if token_count <= 0 {
            continue;
        }
        let when = parse_timestamp(&sql_to_json(&created_at_value));
        let day = bj_day_of(when);
        let Some(day) = day else {
            skipped_dates += 1;
            continue;
        };
        let model_json = sql_to_json(&model_value);
        let model = clean_model(Some(&model_json));
        let entry = buckets.entry((day, model)).or_insert((0, 0));
        entry.0 += token_count;
        entry.1 += 1;
        valid_threads += 1;
    }

    let mut keys: Vec<_> = buckets.keys().cloned().collect();
    keys.sort();
    let mut coverage_tokens = 0i64;
    for key in &keys {
        let (tokens, threads) = buckets[key];
        coverage_tokens += tokens;
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens,
            credits: 0.0,
            messages: threads,
            // 库里只有线程级累计值，请求数回退为线程数
            requests: threads,
        });
    }

    scan.coverage_tokens = coverage_tokens;
    scan.coverage_messages = valid_threads;
    scan.coverage_summary = format!("{total_threads} 线程 · {coverage_tokens} tokens");
    if skipped_dates > 0 {
        scan.warnings.push(format!(
            "{skipped_dates} 个 Codex 线程缺少有效时间戳，未计入统计"
        ));
    }
    scan
}

fn sql_to_json(value: &SqlValue) -> serde_json::Value {
    match value {
        SqlValue::Null => serde_json::Value::Null,
        SqlValue::Integer(i) => serde_json::Value::Number((*i).into()),
        SqlValue::Real(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        SqlValue::Text(text) => serde_json::Value::String(text.clone()),
        SqlValue::Blob(blob) => serde_json::Value::String(String::from_utf8_lossy(blob).into_owned()),
    }
}

fn truncate_error(text: &str) -> String {
    let trimmed = text.trim();
    let text = if trimmed.is_empty() { text } else { trimmed };
    text.chars().take(400).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn reads_threads_table() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let codex = root.join(".codex");
        fs::create_dir_all(&codex).unwrap();
        let db = codex.join("state_5.sqlite");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE threads (id INTEGER PRIMARY KEY, model TEXT, tokens_used INTEGER, created_at TEXT)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads (model, tokens_used, created_at) VALUES \
                 ('gpt-5.1', 1000, '2026-10-23T05:00:00Z'), \
                 ('gpt-5.1', 2000, '2026-10-23T09:00:00Z'), \
                 ('o4-mini', 500, '2026-10-22T05:00:00Z'), \
                 ('x', NULL, '2026-10-23T05:00:00Z'), \
                 ('y', 0, '2026-10-23T05:00:00Z'), \
                 ('z', 300, NULL)",
                [],
            )
            .unwrap();
        drop(connection);

        let scan = scan(&root);
        assert!(scan.detected);
        // NULL/0 tokens 与缺时间戳的行不计入
        assert_eq!(scan.buckets.len(), 2);
        let gpt = scan.buckets.iter().find(|b| b.model == "gpt-5.1").unwrap();
        assert_eq!(gpt.tokens, 3000);
        assert_eq!(gpt.messages, 2);
        assert!(scan.coverage_summary.contains("6 线程"));
        assert!(scan.warnings.iter().any(|w| w.contains("缺少有效时间戳")));
    }

    #[test]
    fn missing_db_reports_warning() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join(".codex")).unwrap();
        let scan = scan(&root);
        assert!(scan.detected);
        assert!(scan.warnings.iter().any(|w| w.contains("state_5.sqlite")));
        assert!(scan.error.is_none());
    }
}
