//! Command Code：`~/.commandcode`（兼容 `~/.command-code`）。
//!
//! 本机与参考实现都没有它的 token 数据样本，格式未知，因此当前**只做安装检测与
//! 文件计数**，不产出 token。远端 Linux 走同步时的目录探测（`.probe.json`），
//! 本机则直接计数。
//!
//! 一旦拿到真实数据，在 `scan_with` 里加解析分支即可；平台计划、历史库、UI
//! 都不需要改。

use std::path::Path;

use crate::model::{Platform, SourceScan};
use crate::parsing::token::{probe, ScanContext};

/// 可能的安装目录（相对 home），按优先级尝试。
const LEAVES: [&str; 2] = [".commandcode", ".command-code"];
const PROBE_KEY: &str = "command-code";
const LABEL: &str = "Command Code";

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(Platform::Linux))
}

pub fn scan_with(root: &Path, _ctx: &ScanContext) -> SourceScan {
    for leaf in LEAVES {
        let dir = root.join(leaf);
        if dir.is_dir() {
            return probe::scan_counts(root, leaf, PROBE_KEY, LABEL, "个会话文件", "仅计数");
        }
    }
    // 远端镜像没有该目录（探测源不下载内容）→ 回退探测结果
    probe::scan_from_probe(root, PROBE_KEY, LABEL, "个会话文件", "Command Code 远端仅计数")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProbeEntry;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    #[test]
    fn counts_local_sessions() {
        let dir = TempDir::new();
        let root = dir.path();
        fs::create_dir_all(root.join(".commandcode/projects/p")).unwrap();
        fs::write(root.join(".commandcode/projects/p/a.jsonl"), "{}\n").unwrap();
        fs::write(root.join(".commandcode/projects/p/b.jsonl"), "{}\n").unwrap();

        let scan = scan(root);
        assert!(scan.detected);
        assert_eq!(scan.coverage_messages, 2);
        assert!(scan.buckets.is_empty());
        assert!(scan.warnings[0].contains("尚未支持"));
    }

    #[test]
    fn accepts_dashed_legacy_dir() {
        let dir = TempDir::new();
        let root = dir.path();
        fs::create_dir_all(root.join(".command-code")).unwrap();
        fs::write(root.join(".command-code/session.jsonl"), "{}\n").unwrap();
        assert!(scan(root).detected);
    }

    #[test]
    fn falls_back_to_probe() {
        let dir = TempDir::new();
        let root = dir.path();
        assert!(!scan(root).detected);

        probe::save(
            root,
            &crate::model::ProbeReport {
                version: 1,
                generated_at_unix: 0,
                entries: vec![ProbeEntry {
                    key: PROBE_KEY.into(),
                    label: LABEL.into(),
                    detected: true,
                    files: 7,
                    dirs: 1,
                    truncated: false,
                    error: None,
                }],
            },
        )
        .unwrap();

        let scan = scan(root);
        assert!(scan.detected);
        assert_eq!(scan.coverage_messages, 7);
        assert_eq!(scan.coverage_summary, "7 个会话文件");
    }
}
