//! 镜像根 `.probe.json` 的读写：远端目录探测结果（QoderCN CLI / Pi / Command Code 仅计数）。
//!
//! 这些源的数据无法解析出 token/credits，因此不再整目录镜像（`.qoder-cli`
//! 的 ai-stats 里含完整源码快照，既费带宽又是隐私负担），改为同步时用 SFTP
//! `read_dir` 只统计文件数，写入镜像根的 `.probe.json`。

use std::path::Path;

use crate::model::{ProbeEntry, ProbeReport, SourceScan};

/// 探测结果文件名（镜像根下）。
pub const PROBE_FILE: &str = ".probe.json";

/// 本地目录计数的深度与条目上限：只计数，成本必须可控。
pub const COUNT_MAX_DEPTH: usize = 6;
pub const COUNT_MAX_ENTRIES: u64 = 5000;

/// 读取镜像根下的探测结果；文件缺失/损坏返回 None（视为未探测）。
pub fn load(mirror: &Path) -> Option<ProbeReport> {
    let text = std::fs::read_to_string(mirror.join(PROBE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 原子写入探测结果（临时文件 + rename）。
pub fn save(mirror: &Path, report: &ProbeReport) -> Result<(), String> {
    std::fs::create_dir_all(mirror).map_err(|e| e.to_string())?;
    let tmp = mirror.join(format!("{PROBE_FILE}.tmp"));
    let text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, mirror.join(PROBE_FILE)).map_err(|e| e.to_string())
}

/// 有界递归计数：跳过符号链接（避免链接环），限制深度与条目数。
/// 返回 (文件数, 是否达到上限)。
pub fn count_files_limited(dir: &Path, max_depth: usize, max_entries: u64) -> (u64, bool) {
    let mut files = 0u64;
    let mut entries = 0u64;
    let mut truncated = false;
    count_walk(dir, 0, max_depth, max_entries, &mut files, &mut entries, &mut truncated);
    (files, truncated)
}

fn count_walk(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    max_entries: u64,
    files: &mut u64,
    entries: &mut u64,
    truncated: &mut bool,
) {
    if *truncated || depth > max_depth {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        *entries += 1;
        if *entries > max_entries {
            *truncated = true;
            return;
        }
        // 符号链接一律跳过：`~/.pi` 之类目录里可能有指向整个磁盘的链接。
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            count_walk(
                &entry.path(),
                depth + 1,
                max_depth,
                max_entries,
                files,
                entries,
                truncated,
            );
        } else {
            *files += 1;
        }
    }
}

/// 「只计数、不解析」源的统一构造：本地目录可读时本地计数，否则回退探测结果。
pub fn scan_counts(
    mirror: &Path,
    source_leaf: &str,
    probe_key: &str,
    label: &str,
    unit: &str,
    unsupported_hint: &str,
) -> SourceScan {
    let root = mirror.join(source_leaf);
    if !root.is_dir() {
        return scan_from_probe(mirror, probe_key, label, unit, unsupported_hint);
    }
    let (files, truncated) = count_files_limited(&root, COUNT_MAX_DEPTH, COUNT_MAX_ENTRIES);
    let mut scan = SourceScan {
        detected: true,
        ..Default::default()
    };
    scan.coverage_messages = files as i64;
    scan.coverage_summary = if truncated {
        format!("{files} {unit}（已达上限，实际更多）")
    } else {
        format!("{files} {unit}")
    };
    if files == 0 {
        scan.warnings
            .push(format!("{label} 已安装，但未发现统计文件"));
    } else {
        scan.warnings.push(format!(
            "检测到 {files} 个 {label} 统计文件，但当前采集器尚未支持其 token 格式（{unsupported_hint}）"
        ));
    }
    scan
}

/// 用远端探测结果构造 SourceScan（未探测到 → detected=false）。
pub fn scan_from_probe(
    mirror: &Path,
    probe_key: &str,
    _label: &str,
    unit: &str,
    unsupported_hint: &str,
) -> SourceScan {
    let mut scan = SourceScan::default();
    let Some(report) = load(mirror) else {
        return scan;
    };
    let Some(entry) = report.get(probe_key) else {
        return scan;
    };
    scan.detected = entry.detected;
    if !entry.detected {
        return scan;
    }
    scan.coverage_messages = entry.files as i64;
    scan.coverage_summary = count_summary(entry, unit);
    scan.warnings = warnings(entry, unsupported_hint);
    scan
}

/// 覆盖摘要，如 "304 个统计文件"。
pub fn count_summary(entry: &ProbeEntry, unit: &str) -> String {
    let mut text = format!("{} {unit}", entry.files);
    if entry.truncated {
        text.push_str("（已达上限，实际更多）");
    }
    text
}

/// 探测条目的告警文案（未安装 / 已安装但不支持解析 / 探测失败）。
pub fn warnings(entry: &ProbeEntry, unsupported_hint: &str) -> Vec<String> {
    let mut items = Vec::new();
    if let Some(error) = &entry.error {
        items.push(format!("{} 目录探测失败：{error}", entry.label));
        return items;
    }
    if !entry.detected {
        return items;
    }
    if entry.files == 0 {
        items.push(format!("{} 已安装，但未发现统计文件", entry.label));
    } else {
        items.push(format!(
            "检测到 {} 个 {} 统计文件，但当前采集器尚未支持其 token 格式（{unsupported_hint}）",
            entry.files, entry.label
        ));
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    fn entry(key: &str, files: u64, truncated: bool) -> ProbeEntry {
        ProbeEntry {
            key: key.into(),
            label: if key == "pi" { "Pi".into() } else { "QoderCN CLI".into() },
            detected: true,
            files,
            dirs: 1,
            truncated,
            error: None,
        }
    }

    #[test]
    fn save_then_load_roundtrip() {
        let temp = TempDir::new();
        let report = ProbeReport {
            version: ProbeReport::VERSION,
            generated_at_unix: 1_700_000_000,
            entries: vec![entry("qoder-cli", 304, false), entry("pi", 12, true)],
        };
        save(temp.path(), &report).expect("写入探测结果");
        let back = load(temp.path()).expect("读回探测结果");
        assert_eq!(back.entries.len(), 2);
        assert_eq!(back.get("qoder-cli").unwrap().files, 304);
        assert!(back.get("pi").unwrap().truncated);
        assert!(back.get("missing").is_none());
    }

    #[test]
    fn load_missing_or_broken_returns_none() {
        let temp = TempDir::new();
        assert!(load(temp.path()).is_none());
        std::fs::write(temp.path().join(PROBE_FILE), "{ broken").unwrap();
        assert!(load(temp.path()).is_none());
    }

    #[test]
    fn summaries_mention_counts_and_limits() {
        assert_eq!(count_summary(&entry("qoder-cli", 304, false), "个统计文件"), "304 个统计文件");
        assert!(count_summary(&entry("pi", 9, true), "个会话文件").contains("已达上限"));

        let unsupported = warnings(&entry("qoder-cli", 304, false), "仅计数");
        assert_eq!(unsupported.len(), 1);
        assert!(unsupported[0].contains("304"));

        let mut quiet = entry("pi", 0, false);
        quiet.detected = false;
        assert!(warnings(&quiet, "仅计数").is_empty());
    }
}
