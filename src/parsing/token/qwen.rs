//! Qwen Code：~/.qwen/projects/**/chats/*.jsonl。
//! 取 systemPayload.uiEvent（事件名 qwen-code.api_response）的
//! total_token_count（缺则 入+出+思考 三和）。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::model::{DayBucket, Platform, Source, SourceScan};
use crate::parsing::token::{
    bj_day_of, clean_model, file_day, parse_timestamp, safe_i64, ScanContext,
};

#[derive(Clone)]
struct Record {
    day: String,
    model: String,
    tokens: i64,
    input: i64,
    output: i64,
    thinking: i64,
    cached: i64,
}

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    let root = mirror.join(".qwen");
    let detected = root.is_dir();
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    if !detected {
        return scan;
    }

    // glob: projects/**/chats/*.jsonl → 父目录名恰为 chats 的 jsonl
    let mut files = Vec::new();
    let projects = root.join("projects");
    collect_chats(&projects, &mut files);
    files.sort();

    let mut unique: HashMap<String, Record> = HashMap::new();
    let mut bad: u64 = 0;
    let mut missing_ts: u64 = 0;
    let mut read_files: u64 = 0;
    let mut read_errors: u64 = 0;

    for path in &files {
        if !ctx.should_read_jsonl(Source::Qwen, path) {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            read_errors += 1;
            continue;
        };
        read_files += 1;
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                bad += 1;
                continue;
            };
            let payload = record
                .get("systemPayload")
                .and_then(|v| v.get("uiEvent"))
                .cloned()
                .unwrap_or(Value::Null);
            let Some(payload_obj) = payload.as_object() else {
                continue;
            };
            let name = payload_obj
                .get("event.name")
                .and_then(Value::as_str)
                .or_else(|| payload_obj.get("name").and_then(Value::as_str));
            if name != Some("qwen-code.api_response") {
                continue;
            }
            let event = match payload.get("event") {
                Some(event @ Value::Object(_)) => event.clone(),
                _ => payload.clone(),
            };
            let total = safe_i64(event.get("total_token_count"));
            let total = if total <= 0 {
                safe_i64(event.get("input_token_count"))
                    + safe_i64(event.get("output_token_count"))
                    + safe_i64(event.get("thoughts_token_count"))
            } else {
                total
            };
            if total <= 0 {
                continue;
            }
            let model = clean_model(event.get("model"));
            let timestamp = event
                .get("event.timestamp")
                .cloned()
                .or_else(|| event.get("timestamp").cloned())
                .or_else(|| record.get("timestamp").cloned())
                .unwrap_or(Value::Null);
            let mut day = bj_day_of(parse_timestamp(&timestamp));
            if day.is_none() {
                missing_ts += 1;
                day = file_day(path);
            }
            let Some(day) = day else {
                continue;
            };
            if !ctx.in_window(Source::Qwen, &day) {
                continue;
            }
            let key = event
                .get("response_id")
                .and_then(Value::as_str)
                .or_else(|| event.get("prompt_id").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| format!("{model}|{total}|{}", timestamp));
            // 与 python 一致：后出现的直接覆盖同键记录
            unique.insert(
                key,
                Record {
                    day,
                    model,
                    tokens: total,
                    input: safe_i64(event.get("input_token_count")),
                    output: safe_i64(event.get("output_token_count")),
                    thinking: safe_i64(event.get("thoughts_token_count")),
                    cached: safe_i64(event.get("cached_content_token_count")),
                },
            );
        }
    }

    let mut buckets: HashMap<(String, String), [i64; 6]> = HashMap::new();
    for record in unique.values() {
        let entry = buckets
            .entry((record.day.clone(), record.model.clone()))
            .or_insert([0; 6]);
        entry[0] += record.tokens;
        entry[1] += record.input;
        entry[2] += record.output;
        entry[3] += record.thinking;
        entry[4] += record.cached;
        entry[5] += 1;
    }

    let mut keys: Vec<_> = buckets.keys().cloned().collect();
    keys.sort();
    let mut coverage_tokens = 0i64;
    for key in &keys {
        let item = &buckets[key];
        coverage_tokens += item[0];
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens: item[0],
            credits: 0.0,
            messages: item[5],
            requests: item[5],
        });
    }

    let responses = unique.len();
    scan.coverage_tokens = coverage_tokens;
    scan.coverage_messages = responses as i64;
    scan.coverage_summary = format!("{responses} 响应 · {coverage_tokens} tokens");
    if files.is_empty() {
        scan.warnings.push("未发现 Qwen Code 会话文件".to_string());
    } else if read_files == 0 && ctx.since_day(Source::Qwen).is_none() {
        scan.warnings.push("未发现 Qwen Code 会话文件".to_string());
    }
    if read_errors > 0 {
        scan.partial = true;
        scan.warnings
            .push(format!("{read_errors} 个 Qwen JSONL 无法读取，本轮结果不完整"));
    }
    if bad > 0 {
        scan.warnings.push(format!("{bad} 行 Qwen JSONL 无法解析，已跳过"));
    }
    if missing_ts > 0 {
        scan.warnings
            .push(format!("{missing_ts} 条 Qwen 记录缺少时间戳，按文件修改日期归档"));
    }
    if unique.is_empty() && read_files > 0 {
        scan.warnings.push("未发现 Qwen Code token 记录".to_string());
    }
    scan
}

fn collect_chats(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_chats(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if path.parent().and_then(|p| p.file_name()).and_then(|n| n.to_str()) == Some("chats") {
                out.push(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    fn line(payload: &str) -> String {
        format!("{{\"systemPayload\":{{\"uiEvent\":{payload}}}}}\n")
    }

    #[test]
    fn parses_api_response_events() {
        let mut jsonl = String::new();
        // 直接 total（真实格式：扁平键 + event 对象并存，event 内带 timestamp）
        jsonl.push_str(&line(
            r#"{"event.name":"qwen-code.api_response","event":{"response_id":"r1","model":"qwen3-coder","total_token_count":500,"input_token_count":400,"output_token_count":80,"thoughts_token_count":20,"cached_content_token_count":10,"timestamp":"2026-10-23T06:00:00Z"},"event.timestamp":"2026-10-23T06:00:00Z"}"#,
        ));
        // 缺 total → 三和（100+50+25=175），无 response_id → 用 prompt_id
        jsonl.push_str(&line(
            r#"{"name":"qwen-code.api_response","event":{"prompt_id":"p2","model":"qwen3-coder","input_token_count":100,"output_token_count":50,"thoughts_token_count":25,"timestamp":"2026-10-24T06:00:00Z"}}"#,
        ));
        // 非目标事件 → 跳过
        jsonl.push_str(&line(r#"{"event.name":"other.event","event":{"response_id":"r3","total_token_count":999}}"#));
        // 坏行
        jsonl.push_str("{broken\n");

        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let file = root.join(".qwen/projects/abc/chats/session.jsonl");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, jsonl).unwrap();
        // chats 之外的 jsonl 不算
        let outside = root.join(".qwen/projects/abc/other.log.jsonl");
        fs::write(&outside, line(r#"{"event.name":"qwen-code.api_response","event":{"response_id":"r9","total_token_count":7777,"model":"qwen3-coder"}}"#)).unwrap();

        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.buckets.len(), 2);
        let day23: i64 = scan
            .buckets
            .iter()
            .filter(|b| b.day == "2026-10-23")
            .map(|b| b.tokens)
            .sum();
        assert_eq!(day23, 500);
        let day24: i64 = scan
            .buckets
            .iter()
            .filter(|b| b.day == "2026-10-24")
            .map(|b| b.tokens)
            .sum();
        assert_eq!(day24, 175);
        assert!(scan.warnings.iter().any(|w| w.contains("无法解析")));
    }
}
