//! Hermes：~/.hermes/state.db 的 sessions 表。
//! 必需列 model/started_at/input_tokens/output_tokens，其余可选；
//! total = input + output + cache_read + cache_write；按 started_at 归档。

use std::collections::HashMap;
use std::path::Path;

use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;
use serde_json::Value;

use crate::model::{DayBucket, Platform, SourceScan};
use crate::parsing::token::{bj_day_of, clean_model, parse_timestamp, safe_i64, sqlite, ScanContext};

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

/// `sessions` 表是**会话累计值**、归日 `started_at`，因此整源重算（Snapshot）。
pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    let root = mirror.join(".hermes");
    let detected = root.is_dir();
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    let db_path = root.join("state.db");
    if !db_path.is_file() {
        if detected {
            scan.warnings.push("未找到 ~/.hermes/state.db".to_string());
        }
        return scan;
    }
    scan_impl(&db_path, ctx.platform, &mut scan);
    scan
}

fn scan_impl(db_path: &Path, platform: Platform, scan: &mut SourceScan) {
    let connection = match sqlite::open_for_scan(db_path, platform) {
        Ok(connection) => connection,
        Err(err) => {
            scan.error = Some(err.to_string());
            scan.warnings.push("Hermes 数据库读取失败".to_string());
            return;
        }
    };
    let tables = match table_names(&connection) {
        Ok(tables) => tables,
        Err(err) => {
            scan.error = Some(err);
            scan.warnings.push("Hermes 数据库读取失败".to_string());
            return;
        }
    };
    if !tables.iter().any(|t| t == "sessions") {
        scan.warnings.push("Hermes 数据库缺少 sessions 表".to_string());
        return;
    }
    if tables.iter().any(|t| t == "messages") {
        if let Ok(count) =
            connection.query_row("select count(*) from messages", [], |row| row.get::<_, i64>(0))
        {
            scan.messages_total = count;
        }
    }
    let columns = match session_columns(&connection) {
        Ok(columns) => columns,
        Err(err) => {
            scan.error = Some(err);
            scan.warnings.push("Hermes 数据库读取失败".to_string());
            return;
        }
    };
    for required in ["model", "started_at", "input_tokens", "output_tokens"] {
        if !columns.iter().any(|column| column == required) {
            scan.warnings.push("Hermes sessions 表字段不完整".to_string());
            return;
        }
    }
    let selected: Vec<&str> = [
        "id",
        "model",
        "started_at",
        "ended_at",
        "message_count",
        "api_call_count",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
    ]
    .into_iter()
    .filter(|name| columns.iter().any(|column| column == name))
    .collect();
    let sql = format!("select {} from sessions", selected.join(","));

    let mut buckets: HashMap<(String, String), (i64, i64)> = HashMap::new();
    let mut used: i64 = 0;
    let mut skipped_dates: u64 = 0;
    let mut total_sessions: i64 = 0;

    let read_result: Result<(), String> = (|| {
        let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
        let column_count = statement.column_count();
        let rows = statement
            .query_map([], |row| {
                let mut values = Vec::with_capacity(column_count);
                for index in 0..column_count {
                    values.push(row.get::<_, SqlValue>(index)?);
                }
                Ok(values)
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        total_sessions = rows.len() as i64;
        for values in rows {
            let row: HashMap<&str, SqlValue> = selected
                .iter()
                .copied()
                .zip(values.into_iter())
                .collect();
            let input_tokens = safe_i64(row.get("input_tokens").map(sql_to_json).as_ref());
            let output_tokens = safe_i64(row.get("output_tokens").map(sql_to_json).as_ref());
            let cache_read = safe_i64(row.get("cache_read_tokens").map(sql_to_json).as_ref());
            let cache_write = safe_i64(row.get("cache_write_tokens").map(sql_to_json).as_ref());
            let total = input_tokens + output_tokens + cache_read + cache_write;
            if total <= 0 {
                continue;
            }
            // started_at 可能是数字或数字字符串
            let started_raw = row.get("started_at").map(sql_to_json).unwrap_or(Value::Null);
            let started = match &started_raw {
                Value::String(text) => text
                    .trim()
                    .parse::<f64>()
                    .map(|number| Value::Number(
                        serde_json::Number::from_f64(number).unwrap_or_else(|| 0i64.into()),
                    ))
                    .unwrap_or_else(|_| started_raw.clone()),
                _ => started_raw.clone(),
            };
            let Some(day) = bj_day_of(parse_timestamp(&started)) else {
                skipped_dates += 1;
                continue;
            };
            let model = clean_model(row.get("model").map(sql_to_json).as_ref());
            let entry = buckets.entry((day, model)).or_insert((0, 0));
            entry.0 += total;
            entry.1 += 1;
            used += 1;
        }
        Ok(())
    })();

    if let Err(err) = read_result {
        scan.error = Some(err);
        scan.warnings.push("Hermes 数据库读取失败".to_string());
        return;
    }

    let mut keys: Vec<_> = buckets.keys().cloned().collect();
    keys.sort();
    let mut coverage_tokens = 0i64;
    for key in &keys {
        let (tokens, sessions) = buckets[key];
        coverage_tokens += tokens;
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens,
            credits: 0.0,
            messages: sessions,
            // 库里只有会话级累计值，请求数回退为会话数
            requests: sessions,
        });
    }

    scan.coverage_tokens = coverage_tokens;
    scan.coverage_messages = used;
    scan.coverage_summary = format!("{total_sessions} 会话 · {coverage_tokens} tokens");
    if skipped_dates > 0 {
        scan.warnings.push(format!(
            "{skipped_dates} 个 Hermes 会话缺少有效时间戳，未计入统计"
        ));
    }
    if used == 0 {
        scan.warnings.push("未发现 Hermes 用量记录".to_string());
    }
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

fn session_columns(connection: &Connection) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("pragma table_info(sessions)")
        .map_err(|e| e.to_string())?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(names)
}

fn sql_to_json(value: &SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => Value::Number((*i).into()),
        SqlValue::Real(f) => serde_json::Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
        SqlValue::Text(text) => Value::String(text.clone()),
        SqlValue::Blob(blob) => Value::String(String::from_utf8_lossy(blob).into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    #[test]
    fn reads_sessions_with_optional_columns() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let hermes = root.join(".hermes");
        fs::create_dir_all(&hermes).unwrap();
        let db = hermes.join("state.db");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE sessions (id INTEGER PRIMARY KEY, model TEXT, started_at REAL, \
                 input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER, cache_write_tokens INTEGER)",
                [],
            )
            .unwrap();
        // 缺 cache 列也应能工作 → 单独建一个缺列库验证
        connection
            .execute(
                "INSERT INTO sessions (model, started_at, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens) VALUES \
                 ('glm-4.7', 1760000100, 100, 50, 10, 5), \
                 ('glm-4.7', '1760000200', 20, 10, 0, 0), \
                 ('kimi-k2', 1760000300, 0, 0, 0, 0)",
                [],
            )
            .unwrap();
        drop(connection);

        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.buckets.len(), 1);
        let glm = &scan.buckets[0];
        assert_eq!(glm.model, "glm-4.7");
        assert_eq!(glm.tokens, 195); // (100+50+10+5) + (20+10) ，字符串时间戳也计入
        assert_eq!(glm.messages, 2);
        assert!(scan.coverage_summary.contains("3 会话"));
    }

    #[test]
    fn works_without_cache_columns() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let hermes = root.join(".hermes");
        fs::create_dir_all(&hermes).unwrap();
        let db = hermes.join("state.db");
        let connection = Connection::open(&db).unwrap();
        connection
            .execute(
                "CREATE TABLE sessions (model TEXT, started_at REAL, input_tokens INTEGER, output_tokens INTEGER)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sessions VALUES ('m1', 1760000100, 10, 5)",
                [],
            )
            .unwrap();
        drop(connection);
        let scan = scan(&root);
        assert_eq!(scan.buckets.len(), 1);
        assert_eq!(scan.buckets[0].tokens, 15);
    }
}
