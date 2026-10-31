//! Claude Code：~/.claude/projects 与 ~/.claude/sessions 下的 JSONL。
//! 取 assistant 记录 message.usage 的四类 token；模型取 message.model；
//! 跳过 <synthetic>；去重键取 message.id / requestId / uuid / 文件:行号，同键保留 total 最大者。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::model::{DayBucket, Platform, Source, SourceScan};
use crate::parsing::token::{
    bj_day_of, clean_model, file_day, parse_timestamp, safe_i64, walk_jsonl, ScanContext,
};

const USAGE_FIELDS: [&str; 4] = [
    "input_tokens",
    "output_tokens",
    "cache_creation_input_tokens",
    "cache_read_input_tokens",
];

#[derive(Clone)]
struct Candidate {
    day: String,
    model: String,
    input: i64,
    output: i64,
    cache_create: i64,
    cache_read: i64,
    total: i64,
}

/// 全量扫描（单测与全量重建用）。
pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    let root = mirror.join(".claude");
    let detected = root.is_dir();
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    if !detected {
        return scan;
    }

    let bases = [root.join("projects"), root.join("sessions")];
    let mut files = Vec::new();
    for base in &bases {
        files.extend(walk_jsonl(base));
    }
    files.sort();

    let mut unique: HashMap<String, Candidate> = HashMap::new();
    let mut raw: u64 = 0;
    let mut bad: u64 = 0;
    let mut missing_ts: u64 = 0;
    let mut read_files: u64 = 0;
    let mut read_errors: u64 = 0;

    for path in &files {
        // 文件级跳过只是省 I/O；窗口内外的记录可能混在同一文件里。
        if !ctx.should_read_jsonl(Source::Claude, path) {
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
            let Some(record_obj) = record.as_object() else {
                continue;
            };
            if record_obj.get("type").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(message) = record.get("message").and_then(Value::as_object) else {
                continue;
            };
            let Some(usage) = message.get("usage").and_then(Value::as_object) else {
                continue;
            };
            let counts: Vec<i64> = USAGE_FIELDS
                .iter()
                .map(|field| safe_i64(usage.get(*field)))
                .collect();
            let total: i64 = counts.iter().sum();
            if total <= 0 {
                continue;
            }
            let model = clean_model(message.get("model"));
            if model == "<synthetic>" {
                continue;
            }
            let mut day = bj_day_of(parse_timestamp(
                record.get("timestamp").unwrap_or(&serde_json::Value::Null),
            ));
            if day.is_none() {
                missing_ts += 1;
                day = file_day(path);
            }
            let Some(day) = day else {
                continue;
            };
            if !ctx.in_window(Source::Claude, &day) {
                continue;
            }
            raw += 1;
            raw_index_key(&mut unique, &record, path, raw, Candidate {
                day,
                model,
                input: counts[0],
                output: counts[1],
                cache_create: counts[2],
                cache_read: counts[3],
                total,
            });
        }
    }

    let mut buckets: HashMap<(String, String), [i64; 5]> = HashMap::new();
    for candidate in unique.values() {
        let entry = buckets
            .entry((candidate.day.clone(), candidate.model.clone()))
            .or_insert([0; 5]);
        entry[0] += candidate.input;
        entry[1] += candidate.output;
        entry[2] += candidate.cache_create;
        entry[3] += candidate.cache_read;
        entry[4] += 1;
    }

    let mut keys: Vec<_> = buckets.keys().cloned().collect();
    keys.sort();
    let mut coverage_tokens = 0i64;
    for key in &keys {
        let item = &buckets[key];
        let tokens = item[0] + item[1] + item[2] + item[3];
        coverage_tokens += tokens;
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens,
            credits: 0.0,
            messages: item[4],
            requests: item[4],
        });
    }

    let responses = unique.len();
    scan.coverage_tokens = coverage_tokens;
    scan.coverage_messages = responses as i64;
    scan.coverage_summary = format!("{responses} 响应 · {coverage_tokens} tokens");
    if read_files == 0 && !files.is_empty() {
        // 文件全被增量窗口跳过：这是正常情况，不要说成"未发现会话"。
        if ctx.since_day(Source::Claude).is_none() {
            scan.warnings.push("未发现 Claude Code 会话 JSONL".to_string());
        }
    } else if files.is_empty() {
        scan.warnings.push("未发现 Claude Code 会话 JSONL".to_string());
    }
    if read_errors > 0 {
        scan.partial = true;
        scan.warnings
            .push(format!("{read_errors} 个 Claude JSONL 无法读取，本轮结果不完整"));
    }
    if bad > 0 {
        scan.warnings.push(format!("{bad} 行 Claude JSONL 无法解析，已跳过"));
    }
    if missing_ts > 0 {
        scan.warnings
            .push(format!("{missing_ts} 条 Claude 记录缺少时间戳，按文件修改日期归档"));
    }
    scan
}

fn raw_index_key(
    unique: &mut HashMap<String, Candidate>,
    record: &Value,
    path: &Path,
    raw: u64,
    candidate: Candidate,
) {
    let message = &record["message"];
    let fallback = format!("{}:{}", path.display(), raw);
    let key = ["id"]
        .iter()
        .find_map(|field| message.get(*field).and_then(Value::as_str))
        .or_else(|| record.get("requestId").and_then(Value::as_str))
        .or_else(|| record.get("uuid").and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or(fallback);
    match unique.get(&key) {
        Some(previous) if previous.total >= candidate.total => {}
        _ => {
            unique.insert(key, candidate);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    fn fixture(dir: &TempDir, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = dir.path().to_path_buf();
        for (rel, content) in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        root
    }

    #[test]
    fn parses_claude_sessions_with_dedup() {
        let jsonl = concat!(
            r#"{"type":"user","message":{"model":"x"}}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-10-23T10:00:00Z","uuid":"u1","message":{"id":"m1","model":"claude-sonnet-4","usage":{"input_tokens":100,"output_tokens":50,"cache_creation_input_tokens":10,"cache_read_input_tokens":5}}}"#, "\n",
            // 同 id 重复 → 保留 total 更大的（300）
            r#"{"type":"assistant","timestamp":"2026-10-23T10:00:01Z","message":{"id":"m1","model":"claude-sonnet-4","usage":{"input_tokens":200,"output_tokens":80,"cache_creation_input_tokens":15,"cache_read_input_tokens":5}}}"#, "\n",
            // synthetic 跳过
            r#"{"type":"assistant","timestamp":"2026-10-23T10:00:02Z","message":{"id":"m2","model":"<synthetic>","usage":{"input_tokens":999}}}"#, "\n",
            // 无时间戳 → 按文件 mtime 归档
            r#"{"type":"assistant","message":{"id":"m3","model":"claude-opus-4","usage":{"output_tokens":7}}}"#, "\n",
            // 坏行
            "{broken json\n"
        );
        let dir = TempDir::new();
        let root = fixture(&dir, &[(".claude/projects/p1/session.jsonl", jsonl)]);
        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.buckets.len(), 2);
        let sonnet = scan
            .buckets
            .iter()
            .find(|b| b.model == "claude-sonnet-4")
            .unwrap();
        assert_eq!(sonnet.tokens, 300); // 200+80+15+5
        assert_eq!(sonnet.messages, 1);
        let opus = scan
            .buckets
            .iter()
            .find(|b| b.model == "claude-opus-4")
            .unwrap();
        assert_eq!(opus.tokens, 7);
        assert!(scan.coverage_summary.contains("2 响应"));
        assert!(scan.warnings.iter().any(|w| w.contains("无法解析")));
        assert!(scan.warnings.iter().any(|w| w.contains("缺少时间戳")));
    }

    #[test]
    fn not_detected_returns_empty() {
        let dir = TempDir::new();
        let scan = scan(&dir.path());
        assert!(!scan.detected);
        assert!(scan.buckets.is_empty());
    }
}
