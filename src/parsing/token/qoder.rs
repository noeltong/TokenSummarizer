//! Qoder 家族（三源，同一套积分口径）：
//! - Qoder       `~/.qoder/projects`     JSONL：assistant 记录 usage.credits
//! - QoderCN     `~/.qoder-cn/projects`  同上（国内版）
//! - QoderCN CLI `~/.qoder-cli`          ai-stats 统计文件，token 格式未支持，仅计数
//! 模型取 message.model，为空回退该文件中最近一条 runtime-config 的 model；
//! 去重键取 message.id / usage.request_id / requestId / uuid / 文件:行号，同键保留 credits 最大者。

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::model::{DayBucket, Platform, Source, SourceScan};
use crate::parsing::token::{bj_day_of, file_day, parse_timestamp, walk_jsonl, ScanContext};

#[derive(Clone)]
struct Candidate {
    day: String,
    model: String,
    credits: f64,
}

/// 按 (基目录列表) 扫描积分 JSONL；`detect_dirs` 任一存在即视为已安装。
fn scan_credits(
    mirror: &Path,
    bases: &[&str],
    detect_dirs: &[&str],
    source: Source,
    ctx: &ScanContext,
) -> SourceScan {
    let detected = detect_dirs.iter().any(|dir| mirror.join(dir).is_dir());
    let mut scan = SourceScan {
        detected,
        ..Default::default()
    };
    if !detected {
        return scan;
    }

    let mut files = Vec::new();
    for base in bases {
        files.extend(walk_jsonl(&mirror.join(base)));
    }
    files.sort();

    let mut unique: HashMap<String, Candidate> = HashMap::new();
    let mut raw: u64 = 0;
    let mut bad: u64 = 0;
    let mut missing_ts: u64 = 0;
    let mut read_files: u64 = 0;
    let mut read_errors: u64 = 0;

    for path in &files {
        if !ctx.should_read_jsonl(source, path) {
            continue;
        }
        let mut current_model: Option<String> = None;
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
            let record_type = record.get("type").and_then(Value::as_str);
            if record_type == Some("runtime-config") {
                if let Some(model) = record.get("model").and_then(Value::as_str) {
                    if !model.is_empty() {
                        current_model = Some(model.to_string());
                    }
                }
                continue;
            }
            if record_type != Some("assistant") {
                continue;
            }
            let Some(message) = record.get("message").and_then(Value::as_object) else {
                continue;
            };
            let usage = message.get("usage").cloned().unwrap_or(Value::Null);
            let credits = match &usage {
                Value::Object(map) => match map.get("credits").cloned().unwrap_or(Value::Null) {
                    Value::Number(number) => number.as_f64().unwrap_or(0.0),
                    Value::String(text) => text.trim().parse::<f64>().unwrap_or(0.0),
                    _ => 0.0,
                },
                _ => 0.0,
            };
            if credits <= 0.0 {
                continue;
            }
            let model_text = match message.get("model").and_then(Value::as_str) {
                Some(model) if !model.trim().is_empty() => model.trim().to_string(),
                _ => current_model.clone().unwrap_or_default(),
            };
            let model = if model_text.is_empty() {
                crate::parsing::token::EMPTY_MODEL.to_string()
            } else {
                model_text
            };
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
            if !ctx.in_window(source, &day) {
                continue;
            }
            raw += 1;
            let fallback = format!("{}:{}", path.display(), raw);
            let key = message
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| usage.get("request_id").and_then(Value::as_str))
                .or_else(|| record.get("requestId").and_then(Value::as_str))
                .or_else(|| record.get("uuid").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or(fallback);
            match unique.get(&key) {
                Some(previous) if previous.credits >= credits => {}
                _ => {
                    unique.insert(key, Candidate { day, model, credits });
                }
            }
        }
    }

    let mut buckets: HashMap<(String, String), (f64, i64)> = HashMap::new();
    for candidate in unique.values() {
        let entry = buckets
            .entry((candidate.day.clone(), candidate.model.clone()))
            .or_insert((0.0, 0));
        entry.0 += candidate.credits;
        entry.1 += 1;
    }

    let mut keys: Vec<_> = buckets.keys().cloned().collect();
    keys.sort();
    let mut total_credits = 0.0;
    for key in &keys {
        let (credits, requests) = buckets[key];
        total_credits += credits;
        scan.buckets.push(DayBucket {
            day: key.0.clone(),
            model: key.1.clone(),
            tokens: 0,
            credits,
            messages: requests,
            requests,
        });
    }

    let requests = unique.len();
    scan.coverage_credits = (total_credits * 1e6).round() / 1e6;
    scan.coverage_messages = requests as i64;
    scan.coverage_summary = format!("{requests} 请求 · {:.3} 积分", scan.coverage_credits);
    if files.is_empty() {
        scan.warnings.push("未发现 Qoder 会话 JSONL".to_string());
    } else if read_files == 0 && ctx.since_day(source).is_none() {
        scan.warnings.push("未发现 Qoder 会话 JSONL".to_string());
    }
    if read_errors > 0 {
        scan.partial = true;
        scan.warnings
            .push(format!("{read_errors} 个 Qoder JSONL 无法读取，本轮结果不完整"));
    }
    if bad > 0 {
        scan.warnings.push(format!("{bad} 行 Qoder JSONL 无法解析，已跳过"));
    }
    if missing_ts > 0 {
        scan.warnings
            .push(format!("{missing_ts} 条 Qoder 记录缺少时间戳，按文件修改日期归档"));
    }
    scan
}

/// 按 ai-stats 统计文件计数（token 格式未支持）。
/// 本机扫描走本地目录计数；远端镜像不再下载这些文件，读同步时写入的探测结果。
fn scan_cli_stats(mirror: &Path) -> SourceScan {
    if !mirror.join(".qoder-cli").is_dir() {
        return cli_stats_from_probe(mirror);
    }
    let stats_root = mirror.join(".qoder-cli").join("ai-stats");
    let count = count_files(&stats_root);
    let mut scan = SourceScan {
        detected: true,
        ..Default::default()
    };
    scan.coverage_messages = count as i64;
    scan.coverage_summary = format!("{count} 个统计文件");
    if count == 0 {
        scan.warnings
            .push("QoderCN CLI 已安装，但未发现统计文件".to_string());
    } else {
        scan.warnings.push(format!(
            "检测到 {count} 个 QoderCN CLI 统计文件，其 token 格式暂未支持，仅计数"
        ));
    }
    scan
}

/// 用远端探测结果构造 QoderCN CLI 的 SourceScan。
fn cli_stats_from_probe(mirror: &Path) -> SourceScan {
    let mut scan = SourceScan::default();
    let Some(report) = super::probe::load(mirror) else {
        return scan;
    };
    let Some(entry) = report.get("qoder-cli") else {
        return scan;
    };
    scan.detected = entry.detected;
    if !entry.detected {
        return scan;
    }
    scan.coverage_messages = entry.files as i64;
    scan.coverage_summary = super::probe::count_summary(entry, "个统计文件");
    scan.warnings = super::probe::warnings(entry, "QoderCN CLI 远端仅计数");
    scan
}

fn count_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += count_files(&path);
        } else {
            count += 1;
        }
    }
    count
}

/// Qoder（国际版）：~/.qoder/projects
pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

pub fn scan_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    scan_credits(
        mirror,
        &[".qoder/projects"],
        &[".qoder"],
        Source::Qoder,
        ctx,
    )
}

/// QoderCN（国内版）：~/.qoder-cn/projects
pub fn scan_cn(mirror: &Path) -> SourceScan {
    scan_cn_with(mirror, &ScanContext::full(Platform::Linux))
}

pub fn scan_cn_with(mirror: &Path, ctx: &ScanContext) -> SourceScan {
    scan_credits(
        mirror,
        &[".qoder-cn/projects"],
        &[".qoder-cn"],
        Source::QoderCn,
        ctx,
    )
}

/// QoderCN CLI：~/.qoder-cli（仅计数）
pub fn scan_cn_cli(mirror: &Path) -> SourceScan {
    scan_cn_cli_with(mirror, &ScanContext::full(Platform::Linux))
}

pub fn scan_cn_cli_with(mirror: &Path, _ctx: &ScanContext) -> SourceScan {
    // 只计数，没有 bucket，因此不需要增量窗口。
    scan_cli_stats(mirror)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    #[test]
    fn splits_qoder_and_qodercn_sources() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let intl = root.join(".qoder/projects/p1/a.jsonl");
        fs::create_dir_all(intl.parent().unwrap()).unwrap();
        fs::write(
            &intl,
            concat!(
                r#"{"type":"runtime-config","model":"qoder-intl"}"#, "\n",
                r#"{"type":"assistant","timestamp":"2026-10-23T09:00:00Z","message":{"usage":{"credits":1.5,"request_id":"r1"}}}"#, "\n",
            ),
        )
        .unwrap();
        let cn = root.join(".qoder-cn/projects/p2/b.jsonl");
        fs::create_dir_all(cn.parent().unwrap()).unwrap();
        fs::write(
            &cn,
            concat!(
                r#"{"type":"assistant","timestamp":"2026-10-23T09:00:00Z","message":{"id":"a1","model":"qmodel_38max","usage":{"credits":4.7168616,"request_id":"r2"}}}"#, "\n",
            ),
        )
        .unwrap();

        let intl_scan = scan(&root);
        assert!(intl_scan.detected);
        assert_eq!(intl_scan.buckets[0].model, "qoder-intl");
        assert_eq!(intl_scan.buckets[0].credits, 1.5);

        let cn_scan = scan_cn(&root);
        assert!(cn_scan.detected);
        assert_eq!(cn_scan.buckets[0].model, "qmodel_38max");
        assert_eq!(cn_scan.buckets[0].credits, 4.7168616);
        // 两源互不串扰
        assert_eq!(intl_scan.buckets.len(), 1);
        assert_eq!(cn_scan.buckets.len(), 1);
    }

    #[test]
    fn cli_stats_counted_with_warning() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let stats = root.join(".qoder-cli/ai-stats/enterprise-v1/hash/projects/p");
        fs::create_dir_all(&stats).unwrap();
        fs::write(stats.join("a.jsonl"), "{}\n").unwrap();
        fs::write(stats.join("b.jsonl"), "{}\n").unwrap();
        let scan = scan_cn_cli(&root);
        assert!(scan.detected);
        assert_eq!(scan.coverage_messages, 2);
        assert!(scan.warnings[0].contains("2 个 QoderCN CLI 统计文件"));
        assert!(scan.buckets.is_empty());
    }

    #[test]
    fn cli_stats_use_probe_when_mirror_not_mirrored() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        assert!(!scan_cn_cli(&root).detected);

        super::super::probe::save(
            &root,
            &crate::model::ProbeReport {
                version: 1,
                generated_at_unix: 0,
                entries: vec![crate::model::ProbeEntry {
                    key: "qoder-cli".into(),
                    label: "QoderCN CLI".into(),
                    detected: true,
                    files: 304,
                    dirs: 12,
                    truncated: false,
                    error: None,
                }],
            },
        )
        .unwrap();

        let scan = scan_cn_cli(&root);
        assert!(scan.detected);
        assert_eq!(scan.coverage_messages, 304);
        assert_eq!(scan.coverage_summary, "304 个统计文件");
        assert!(scan.buckets.is_empty());
    }
}
