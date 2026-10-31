//! WorkBuddy：`~/.workbuddy/projects/**/*.jsonl`（含 `subagents/`）。
//!
//! 实测格式（2026-10 本机样本）：
//! - 每行一个 JSON 对象，`type` 取 message / function_call / reasoning /
//!   credit-usage / session-meta 等；**usage 只出现在 `function_call` 与
//!   `message`+`role=assistant` 上**，位置是 `providerData.usage`。
//! - `usage.totalTokens == inputTokens + outputTokens`；`inputTokensDetails[].cached_tokens`
//!   是 input 的子集、`outputTokensDetails[].reasoning_tokens` 是 output 的子集，
//!   **不能再叠加**，否则会严重虚高。
//! - 记录唯一键是 `id`（`resp_...`，实测 843 条全唯一）；`conversationRequestId`
//!   只有 53 个唯一值（一次请求对应多次模型调用），**不能用来去重**。
//! - `credit-usage` 记录带 `credit` 但无 model、粒度是 stage，不参与统计。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::model::{DayBucket, Platform, Source, SourceScan};
use crate::parsing::token::{bj_day_of, clean_model, file_day, parse_timestamp, safe_i64, walk_jsonl, ScanContext};

#[derive(Clone)]
struct Candidate {
    day: String,
    model: String,
    tokens: i64,
}

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Windows))
}

pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    let root = mirror.join(".workbuddy");
    let detected = root.is_dir();
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    if !detected {
        return scan;
    }

    let files = walk_jsonl(&root.join("projects"));
    let mut unique: HashMap<String, Candidate> = HashMap::new();
    let mut raw: u64 = 0;
    let mut bad: u64 = 0;
    let mut missing_ts: u64 = 0;
    let mut read_files: u64 = 0;
    let mut read_errors: u64 = 0;

    for path in &files {
        if !ctx.should_read_jsonl(Source::WorkBuddy, path) {
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
            let Some(provider) = record.get("providerData").and_then(Value::as_object) else {
                continue;
            };
            let Some(usage) = provider.get("usage").and_then(Value::as_object) else {
                continue;
            };
            // usage 既挂在 function_call 上，也挂在 assistant message 上；
            // 只筛 role=assistant 会漏掉绝大多数模型调用。
            let record_type = record.get("type").and_then(Value::as_str).unwrap_or_default();
            let is_usage_record = record_type == "function_call"
                || (record_type == "message"
                    && record.get("role").and_then(Value::as_str) == Some("assistant"));
            if !is_usage_record {
                continue;
            }

            let mut tokens = safe_i64(usage.get("totalTokens"));
            if tokens <= 0 {
                tokens = safe_i64(usage.get("inputTokens")) + safe_i64(usage.get("outputTokens"));
            }
            if tokens <= 0 {
                continue;
            }

            let model = provider
                .get("model")
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .or_else(|| {
                    provider
                        .get("requestModelName")
                        .and_then(Value::as_str)
                        .filter(|text| !text.trim().is_empty())
                })
                .map(|text| clean_model(Some(&Value::String(text.to_string()))))
                .unwrap_or_else(|| crate::parsing::token::EMPTY_MODEL.to_string());

            let mut day = bj_day_of(parse_timestamp(
                record.get("timestamp").unwrap_or(&Value::Null),
            ));
            if day.is_none() {
                missing_ts += 1;
                day = file_day(path);
            }
            let Some(day) = day else {
                continue;
            };
            if !ctx.in_window(Source::WorkBuddy, &day) {
                continue;
            }
            raw += 1;
            let key = record
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{}:{}", path.display(), raw));
            match unique.get(&key) {
                Some(previous) if previous.tokens >= tokens => {}
                _ => {
                    unique.insert(key, Candidate { day, model, tokens });
                }
            }
        }
    }

    let mut buckets: HashMap<(String, String), (i64, i64)> = HashMap::new();
    for candidate in unique.values() {
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
        let (tokens, responses) = buckets[key];
        coverage_tokens += tokens;
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens,
            credits: 0.0,
            messages: responses,
            requests: responses,
        });
    }

    let responses = unique.len();
    scan.coverage_tokens = coverage_tokens;
    scan.coverage_messages = responses as i64;
    scan.coverage_summary = format!("{responses} 响应 · {coverage_tokens} tokens");
    if files.is_empty() {
        scan.warnings.push("未发现 WorkBuddy 会话 JSONL".to_string());
    } else if read_files == 0 && ctx.since_day(Source::WorkBuddy).is_none() {
        scan.warnings.push("未发现 WorkBuddy 会话 JSONL".to_string());
    }
    if read_errors > 0 {
        scan.partial = true;
        scan.warnings
            .push(format!("{read_errors} 个 WorkBuddy JSONL 无法读取，本轮结果不完整"));
    }
    if bad > 0 {
        scan.warnings
            .push(format!("{bad} 行 WorkBuddy JSONL 无法解析，已跳过"));
    }
    if missing_ts > 0 {
        scan.warnings.push(format!(
            "{missing_ts} 条 WorkBuddy 记录缺少时间戳，按文件修改日期归档"
        ));
    }
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    fn write(dir: &TempDir, rel: &str, lines: &[&str]) -> std::path::PathBuf {
        let root = dir.path().to_path_buf();
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, lines.join("\n")).unwrap();
        root
    }

    #[test]
    fn counts_function_call_and_assistant_message() {
        let dir = TempDir::new();
        let root = write(
            &dir,
            ".workbuddy/projects/p1/s1.jsonl",
            &[
                // function_call 带 usage（绝大多数模型调用是这个形态）
                r#"{"id":"resp_1","timestamp":1785760421660,"type":"function_call","providerData":{"model":"deepseek-pro","usage":{"inputTokens":100,"outputTokens":20,"totalTokens":120,"outputTokensDetails":[{"reasoning_tokens":10}]}}}"#,
                // assistant message 带 usage
                r#"{"id":"resp_2","timestamp":1785760421661,"type":"message","role":"assistant","providerData":{"model":"gpt-5.6-luna","usage":{"inputTokens":50,"outputTokens":5,"totalTokens":55}}}"#,
                // user message 没有 usage → 跳过
                r#"{"id":"u1","timestamp":1785760421662,"type":"message","role":"user","providerData":{"model":"x"}}"#,
                // credit-usage 有 credit 但无 usage → 跳过
                r#"{"id":"c1","timestamp":1785760421663,"type":"credit-usage","providerData":{"credit":0.32}}"#,
            ],
        );

        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.buckets.len(), 2);
        let pro = scan.buckets.iter().find(|b| b.model == "deepseek-pro").unwrap();
        // totalTokens 直接可用；details 里的 reasoning/cached 是子集，不能叠加
        assert_eq!(pro.tokens, 120);
        assert_eq!(pro.messages, 1);
        let luna = scan.buckets.iter().find(|b| b.model == "gpt-5.6-luna").unwrap();
        assert_eq!(luna.tokens, 55);
        assert!(scan.coverage_summary.contains("2 响应"));
    }

    #[test]
    fn falls_back_to_input_plus_output_without_total() {
        let dir = TempDir::new();
        let root = write(
            &dir,
            ".workbuddy/projects/p1/s1.jsonl",
            &[
                r#"{"id":"r1","timestamp":1785760421660,"type":"function_call","providerData":{"model":"m","usage":{"inputTokens":7,"outputTokens":3}}}"#,
            ],
        );
        let scan = scan(&root);
        assert_eq!(scan.buckets[0].tokens, 10);
    }

    #[test]
    fn dedupes_by_record_id_keeping_max() {
        let dir = TempDir::new();
        let root = write(
            &dir,
            ".workbuddy/projects/p1/s1.jsonl",
            &[
                r#"{"id":"resp_x","timestamp":1785760421660,"type":"function_call","providerData":{"model":"m","usage":{"totalTokens":100}}}"#,
                r#"{"id":"resp_x","timestamp":1785760421660,"type":"function_call","providerData":{"model":"m","usage":{"totalTokens":160}}}"#,
            ],
        );
        let scan = scan(&root);
        assert_eq!(scan.buckets.len(), 1);
        assert_eq!(scan.buckets[0].tokens, 160);
        assert_eq!(scan.buckets[0].messages, 1);
    }

    #[test]
    fn conversation_request_id_is_not_the_dedup_key() {
        // 同一个 conversationRequestId 对应多次模型调用，必须分别计
        let dir = TempDir::new();
        let root = write(
            &dir,
            ".workbuddy/projects/p1/s1.jsonl",
            &[
                r#"{"id":"resp_a","timestamp":1785760421660,"type":"function_call","providerData":{"conversationRequestId":"cr1","model":"m","usage":{"totalTokens":10}}}"#,
                r#"{"id":"resp_b","timestamp":1785760421661,"type":"function_call","providerData":{"conversationRequestId":"cr1","model":"m","usage":{"totalTokens":20}}}"#,
            ],
        );
        let scan = scan(&root);
        assert_eq!(scan.buckets[0].tokens, 30);
        assert_eq!(scan.buckets[0].messages, 2);
    }

    #[test]
    fn reads_subagent_jsonl_recursively() {
        let dir = TempDir::new();
        let root = write(
            &dir,
            ".workbuddy/projects/p1/subagents/agent-1.jsonl",
            &[
                r#"{"id":"resp_s","timestamp":1785760421660,"type":"function_call","providerData":{"model":"m","usage":{"totalTokens":9}}}"#,
            ],
        );
        assert_eq!(scan(&root).buckets[0].tokens, 9);
    }

    #[test]
    fn window_filters_older_records() {
        let dir = TempDir::new();
        // 北京 2026-10-20 00:00 = UTC 2026-10-19 16:00；北京 2026-10-25 00:00 同理
        let old_ms = chrono::DateTime::parse_from_rfc3339("2026-10-19T16:00:00Z")
            .unwrap()
            .timestamp_millis();
        let new_ms = chrono::DateTime::parse_from_rfc3339("2026-10-24T16:00:00Z")
            .unwrap()
            .timestamp_millis();
        let old_line = format!(
            r#"{{"id":"old","timestamp":{old_ms},"type":"function_call","providerData":{{"model":"m","usage":{{"totalTokens":5}}}}}}"#
        );
        let new_line = format!(
            r#"{{"id":"new","timestamp":{new_ms},"type":"function_call","providerData":{{"model":"m","usage":{{"totalTokens":7}}}}}}"#
        );
        let root = write(
            &dir,
            ".workbuddy/projects/p1/s1.jsonl",
            &[old_line.as_str(), new_line.as_str()],
        );
        let mut since = HashMap::new();
        since.insert(Source::WorkBuddy, "2026-10-24".to_string());
        let ctx = ScanContext::with_window(Platform::Windows, &since);
        let scan = scan_with(&root, &ctx);
        assert_eq!(scan.buckets.len(), 1);
        assert_eq!(scan.buckets[0].tokens, 7, "窗口外的旧记录必须被过滤");
    }

    #[test]
    fn not_detected_without_dir() {
        let dir = TempDir::new();
        let scan = scan(&dir.path());
        assert!(!scan.detected);
        assert!(scan.buckets.is_empty());
    }
}
