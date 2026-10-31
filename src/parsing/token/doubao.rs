//! 豆包工作（Doubao Work）：Windows app 形态，token 记录格式未公开。
//!
//! 实测 `%LOCALAPPDATA%\DoubaoWork` 只有 Chromium 壳（`User Data`）与安装器日志，
//! 没有可解析的用量文件；`%LOCALAPPDATA%\Doubao` 是消费端浏览器配置，与 coding
//! agent 无关。因此本源**只做安装检测与浅层文件计数**，不产出任何 token ——
//! 宁可显示"仅计数"，也不能编造用量。
//!
//! 若后续版本出现了可解析的用量文件，在这里补 `scan` 的解析分支即可，
//! 上层（平台计划 / 历史库 / UI）都不需要改。

use std::path::{Path, PathBuf};

use crate::model::SourceScan;
use crate::parsing::token::{probe, ScanContext};

/// `%LOCALAPPDATA%`；取不到时返回 None（非 Windows 或环境异常）。
fn local_appdata() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

pub fn scan(mirror: &Path) -> SourceScan {
    scan_with(mirror, &ScanContext::full(crate::model::Platform::Windows))
}

/// 入口：`root` 是 home，但豆包工作的数据在 `%LOCALAPPDATA%`，因此不看 `root`。
pub fn scan_with(_root: &Path, _ctx: &ScanContext) -> SourceScan {
    match local_appdata() {
        Some(appdata) => scan_at(&appdata),
        None => SourceScan::default(),
    }
}

/// 在给定 `%LOCALAPPDATA%` 下检测豆包工作。抽出来是为了可单测。
pub fn scan_at(local_appdata: &Path) -> SourceScan {
    let dir = local_appdata.join("DoubaoWork");
    if !dir.is_dir() {
        return SourceScan::default();
    }
    // 只数浅层文件：`User Data` 里是 Chromium 缓存，深挖既慢又没有意义。
    let (files, truncated) = probe::count_files_limited(&dir, 2, 500);
    let mut scan = SourceScan {
        detected: true,
        ..Default::default()
    };
    scan.coverage_messages = files as i64;
    scan.coverage_summary = if truncated {
        format!("{files} 个文件（已达上限，实际更多）")
    } else {
        format!("{files} 个文件")
    };
    scan.warnings.push(
        "豆包工作 已安装，但未发现可解析的 token 记录（当前仅检测安装状态）".to_string(),
    );
    scan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::token::testutil::TempDir;
    use std::fs;

    #[test]
    fn not_detected_without_app_dir() {
        let dir = TempDir::new();
        let scan = scan_at(dir.path());
        assert!(!scan.detected);
        assert!(scan.buckets.is_empty());
    }

    #[test]
    fn detects_app_and_counts_shallow_files() {
        let dir = TempDir::new();
        let root = dir.path();
        fs::create_dir_all(root.join("DoubaoWork/User Data/Default")).unwrap();
        fs::write(root.join("DoubaoWork/channel_name"), "stable").unwrap();
        fs::write(root.join("DoubaoWork/User Data/Default/Bookmarks"), "{}").unwrap();
        // 第四层文件不应被计数（depth 上限 2）
        fs::create_dir_all(root.join("DoubaoWork/User Data/Default/Cache/x")).unwrap();
        fs::write(root.join("DoubaoWork/User Data/Default/Cache/x/blob"), "x").unwrap();

        let scan = scan_at(root);
        assert!(scan.detected);
        assert!(scan.buckets.is_empty(), "只计数源不得产生 bucket");
        assert!(scan.coverage_summary.ends_with("个文件"));
        assert!(scan.warnings[0].contains("仅检测安装状态"));
    }
}
