//! Pi：~/.pi/agent/sessions 仅统计文件数，token 格式尚未支持（与参考实现一致）。
//! 远端镜像不再下载其会话文件，改为同步时的目录探测结果（`.probe.json`）。

use std::path::Path;

use crate::model::{Platform, SourceScan};
use crate::parsing::token::{probe, ScanContext};

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

/// 只计数源：没有 bucket，不需要增量窗口，每轮都重新数。
pub fn scan_with(mirror: &Path, _ctx: &ScanContext) -> SourceScan {
    let root = mirror.join(".pi");
    // 本机扫描（home 直接可见）走本地计数；远端镜像没有该目录，读探测结果。
    if !root.is_dir() {
        return from_probe(mirror);
    }
    let sessions = root.join("agent").join("sessions");
    let (count, truncated) =
        probe::count_files_limited(&sessions, probe::COUNT_MAX_DEPTH, probe::COUNT_MAX_ENTRIES);
    let mut scan = SourceScan {
        detected: true,
        ..Default::default()
    };
    scan.coverage_messages = count as i64;
    scan.coverage_summary = if truncated {
        format!("{count} 个会话文件（已达上限，实际更多）")
    } else {
        format!("{count} 个会话文件")
    };
    if count == 0 {
        scan.warnings.push("Pi 已安装，但未发现会话记录".to_string());
    } else {
        scan.warnings.push(format!(
            "检测到 {count} 个 Pi 会话文件，但当前采集器尚未支持其 token 格式"
        ));
    }
    scan
}

/// 用远端探测结果构造 SourceScan（未探测到 → detected=false）。
fn from_probe(mirror: &Path) -> SourceScan {
    let mut scan = SourceScan::default();
    let Some(report) = probe::load(mirror) else {
        return scan;
    };
    let Some(entry) = report.get("pi") else {
        return scan;
    };
    scan.detected = entry.detected;
    if !entry.detected {
        return scan;
    }
    scan.coverage_messages = entry.files as i64;
    scan.coverage_summary = probe::count_summary(entry, "个会话文件");
    scan.warnings = probe::warnings(entry, "Pi 远端仅计数");
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProbeEntry;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    #[test]
    fn counts_session_files() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();
        let sessions = root.join(".pi/agent/sessions/a/b");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(sessions.join("s1.json"), "{}").unwrap();
        fs::write(sessions.join("s2.json"), "{}").unwrap();
        fs::create_dir_all(root.join(".pi/agent/sessions/a/c")).unwrap();
        fs::write(root.join(".pi/agent/sessions/a/c/s3.json"), "{}").unwrap();

        let scan = scan(&root);
        assert!(scan.detected);
        assert!(scan.warnings[0].contains("3 个 Pi 会话文件"));
        assert!(scan.buckets.is_empty());
    }

    #[test]
    fn falls_back_to_probe_when_mirror_has_no_dir() {
        let dir = TempDir::new();
        let root = dir.path().to_path_buf();

        // 未探测 → 未检测到
        let empty = scan(&root);
        assert!(!empty.detected);

        probe::save(
            &root,
            &crate::model::ProbeReport {
                version: 1,
                generated_at_unix: 0,
                entries: vec![ProbeEntry {
                    key: "pi".into(),
                    label: "Pi".into(),
                    detected: true,
                    files: 42,
                    dirs: 3,
                    truncated: false,
                    error: None,
                }],
            },
        )
        .unwrap();

        let scan = scan(&root);
        assert!(scan.detected);
        assert_eq!(scan.coverage_messages, 42);
        assert_eq!(scan.coverage_summary, "42 个会话文件");
        assert!(scan.buckets.is_empty());
        assert!(scan.warnings[0].contains("尚未支持"));
    }
}
