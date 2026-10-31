//! Token 用量解析：口径与 token_summary/remote_collect.py 逐条对应。
//!
//! 输入是本地镜像目录（目录结构等价远端 home），由 monitoring::token_sync
//! 通过 SFTP 增量维护；本机 Windows 与 WSL 则直接指向真实的 home。本模块只做
//! 纯读取与聚合。
//!
//! 平台差异只体现在「扫哪些源」（`sources_for`）与「路径怎么拼」上：
//! Linux 与 WSL 共用一套规则，Windows 额外包含 app 形态的 agent。

pub mod claude;
pub mod codex;
pub mod commandcode;
pub mod doubao;
pub mod hermes;
pub mod opencode;
pub mod pi;
pub mod probe;
pub mod qoder;
pub mod qwen;
pub mod sqlite;
pub mod workbuddy;
pub mod zcode;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use serde_json::Value;

use crate::model::{Platform, Source, SourceScan, TokenSnapshot};

pub const EMPTY_MODEL: &str = "未标注模型";

/// 文件级跳过时额外放宽的天数：吸收时区、时钟偏移与跨午夜追加。
const FILE_MTIME_SLACK_DAYS: i64 = 1;

/// 平台扫描计划。
///
/// Windows 除 CLI 形态的 agent 外，还包含以 app 形态落盘的 agent（WorkBuddy /
/// 豆包工作）。WorkBuddy 跨平台：Linux 服务器同样写 `~/.workbuddy/projects`，
/// 因此两个计划都包含它；只有豆包工作是 Windows-only。WSL 复用 Linux 计划
/// （`\\wsl$\<distro>\home\<user>` 就是 Linux home）。
pub const WINDOWS_SCAN_PLAN: &[Source] = &[
    Source::Codex,
    Source::Claude,
    Source::Qoder,
    Source::QoderCn,
    Source::QoderCnCli,
    Source::Zcode,
    Source::WorkBuddy,
    Source::Doubao,
    Source::CommandCode,
];

pub const LINUX_SCAN_PLAN: &[Source] = &[
    Source::Codex,
    Source::Claude,
    Source::Opencode,
    Source::Qwen,
    Source::Hermes,
    Source::Qoder,
    Source::QoderCn,
    Source::QoderCnCli,
    Source::Zcode,
    Source::Pi,
    // Linux 服务器同样写 `~/.workbuddy/projects`（顺序按 `Source::ALL` 下标）
    Source::WorkBuddy,
    Source::CommandCode,
];

pub fn sources_for(platform: Platform) -> &'static [Source] {
    match platform {
        Platform::Windows => WINDOWS_SCAN_PLAN,
        Platform::Linux | Platform::Wsl => LINUX_SCAN_PLAN,
    }
}

/// 扫描上下文：平台 + 可选的增量窗口。
#[derive(Debug, Clone)]
pub struct ScanContext<'a> {
    pub platform: Platform,
    /// None = 全量；Some = 只重算窗口内的记录（按源给出起点，含当天）。
    pub since_days: Option<&'a HashMap<Source, String>>,
}

impl<'a> ScanContext<'a> {
    pub fn full(platform: Platform) -> ScanContext<'static> {
        ScanContext {
            platform,
            since_days: None,
        }
    }

    pub fn with_window(platform: Platform, since_days: &'a HashMap<Source, String>) -> Self {
        ScanContext {
            platform,
            since_days: Some(since_days),
        }
    }

    pub fn since_day(&self, source: Source) -> Option<&'a str> {
        self.since_days
            .and_then(|map| map.get(&source))
            .map(String::as_str)
    }

    /// 记录级过滤：`day` 早于该源水位线就丢弃。
    pub fn in_window(&self, source: Source, day: &str) -> bool {
        match self.since_day(source) {
            None => true,
            Some(since) => day >= since,
        }
    }

    /// 文件级跳过：文件最后修改日早于水位线（含 1 天余量）就整个不读。
    ///
    /// 只是省 I/O 的优化，**不能替代** `in_window` 的记录级过滤：一个文件可以
    /// 同时包含窗口内与窗口外的记录。读不到 mtime 时保守返回 true。
    pub fn should_read_jsonl(&self, source: Source, path: &Path) -> bool {
        let Some(since) = self.since_day(source) else {
            return true;
        };
        let Some(cutoff) = day_minus(since, FILE_MTIME_SLACK_DAYS) else {
            return true;
        };
        match file_day(path) {
            Some(day) => day >= cutoff,
            None => true,
        }
    }
}

pub fn bj_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("固定 UTC+8")
}

/// 北京日字符串减若干天；日期格式非法时返回 None（调用方应保守处理）。
pub fn day_minus(day: &str, days: i64) -> Option<String> {
    let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(
        (date - chrono::Duration::days(days))
            .format("%Y-%m-%d")
            .to_string(),
    )
}

/// 北京时间自然日 "YYYY-MM-DD"；None 输入返回 None。
pub fn bj_day_of(dt: Option<DateTime<Utc>>) -> Option<String> {
    dt.map(|dt| {
        dt.with_timezone(&bj_offset())
            .date_naive()
            .format("%Y-%m-%d")
            .to_string()
    })
}

/// 解析时间戳：数字 > 1e12 视为毫秒；字符串依次尝试 ISO8601 与几种
/// strptime 变体；无时区按 UTC。
pub fn parse_timestamp(value: &Value) -> Option<DateTime<Utc>> {
    match value {
        Value::Number(number) => {
            let mut num = number.as_f64()?;
            if num > 1e12 {
                num /= 1000.0;
            }
            Utc.timestamp_opt(num as i64, 0).single()
        }
        Value::String(text) => parse_timestamp_str(text),
        _ => None,
    }
}

pub fn parse_timestamp_str(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }
    // 尾部 Z 但带毫秒/微秒变体
    let normalized;
    let text = if text.ends_with('Z') {
        normalized = format!("{}+00:00", &text[..text.len() - 1]);
        &normalized
    } else {
        text
    };
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }
    // 无时区：按 UTC
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(text, fmt) {
            return Some(naive.and_utc());
        }
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
    }
    None
}

pub fn safe_i64(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f as i64))
            .unwrap_or(0),
        Some(Value::String(text)) => text.trim().parse::<f64>().map(|f| f as i64).unwrap_or(0),
        _ => 0,
    }
}

pub fn safe_f64(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::String(text)) => text.trim().parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn clean_model(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => text.trim().to_string(),
        _ => EMPTY_MODEL.to_string(),
    }
}

/// 镜像文件的"远端修改日"（同步层会把本地 mtime 设为远端 mtime）。
pub fn file_day(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    Some(bj_day_of(Some(modified.into()))?)
}

/// 递归收集 *.jsonl（等价 python glob "**/*.jsonl"），结果排序。
pub fn walk_jsonl(base: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk_dir_jsonl(base, &mut found);
    found.sort();
    found
}

fn walk_dir_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_dir_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// 扫描给定根目录（等价目标 home / 镜像根）下该平台的全部源。
pub fn scan_all(root: &Path, ctx: &ScanContext) -> TokenSnapshot {
    let mut snapshot = TokenSnapshot {
        generated_at: Some(Utc::now()),
        scans: Vec::new(),
    };
    for source in sources_for(ctx.platform) {
        snapshot.scans.push((*source, scan_one(*source, root, ctx)));
    }
    snapshot
}

/// 单源分发。刻意不用通配分支：新增 `Source` 时这里必须编译失败。
fn scan_one(source: Source, root: &Path, ctx: &ScanContext) -> SourceScan {
    match source {
        Source::Codex => codex::scan_with(root, ctx),
        Source::Claude => claude::scan_with(root, ctx),
        Source::Opencode => opencode::scan_with(root, ctx),
        Source::Qwen => qwen::scan_with(root, ctx),
        Source::Hermes => hermes::scan_with(root, ctx),
        Source::Qoder => qoder::scan_with(root, ctx),
        Source::QoderCn => qoder::scan_cn_with(root, ctx),
        Source::QoderCnCli => qoder::scan_cn_cli_with(root, ctx),
        Source::Zcode => zcode::scan_with(root, ctx),
        Source::Pi => pi::scan_with(root, ctx),
        Source::WorkBuddy => workbuddy::scan_with(root, ctx),
        Source::Doubao => doubao::scan_with(root, ctx),
        Source::CommandCode => commandcode::scan_with(root, ctx),
    }
}

/// 全量扫描（包装，供单测与不关心增量的调用方使用）。
pub fn scan_all_full(root: &Path, platform: Platform) -> TokenSnapshot {
    scan_all(root, &ScanContext::full(platform))
}

/// 单元测试用临时目录（Drop 时递归删除）。实现已上移到 `util::testutil`。
#[cfg(test)]
pub mod testutil {
    pub use crate::util::testutil::TempDir;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_variants() {
        // ISO8601 带时区
        let dt = parse_timestamp_str("2026-10-23T12:34:56+08:00").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-10-23T04:34:56+00:00");
        // Z 尾
        assert!(parse_timestamp_str("2026-10-23T04:34:56Z").is_some());
        // 毫秒 + Z
        assert!(parse_timestamp_str("2026-10-23T04:34:56.123Z").is_some());
        // 无时区 → UTC
        let dt = parse_timestamp_str("2026-10-23 12:34:56").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-10-23T12:34:56+00:00");
        // 秒级数字与毫秒级数字
        let value = serde_json::json!(1760000000);
        assert!(parse_timestamp(&value).is_some());
        let value = serde_json::json!(1760000000000_i64);
        assert!(parse_timestamp(&value).is_some());
        // 无法解析
        assert!(parse_timestamp_str("not-a-time").is_none());
    }

    #[test]
    fn beijing_day_bucketing() {
        // UTC 2026-10-23 20:00 = 北京 2026-10-24 04:00
        let dt = parse_timestamp_str("2026-10-23T20:00:00Z").unwrap();
        assert_eq!(bj_day_of(Some(dt)).as_deref(), Some("2026-10-24"));
        // UTC 2026-10-23 10:00 = 北京 2026-10-23 18:00
        let dt = parse_timestamp_str("2026-10-23T10:00:00Z").unwrap();
        assert_eq!(bj_day_of(Some(dt)).as_deref(), Some("2026-10-23"));
    }

    #[test]
    fn platform_plans_cover_every_source_once() {
        // 每个源至少属于一个平台计划；计划内不允许重复。
        for plan in [WINDOWS_SCAN_PLAN, LINUX_SCAN_PLAN] {
            let mut seen = plan.to_vec();
            seen.sort();
            seen.dedup();
            assert_eq!(seen.len(), plan.len(), "扫描计划内不得重复");
        }
        for source in Source::ALL {
            let listed = WINDOWS_SCAN_PLAN.contains(&source) || LINUX_SCAN_PLAN.contains(&source);
            assert!(listed, "{source:?} 没有出现在任何平台扫描计划里");
        }
        // 只有豆包工作是 Windows-only；WorkBuddy 跨平台，两个计划都必须有
        assert!(WINDOWS_SCAN_PLAN.contains(&Source::WorkBuddy));
        assert!(LINUX_SCAN_PLAN.contains(&Source::WorkBuddy));
        assert!(WINDOWS_SCAN_PLAN.contains(&Source::Doubao));
        assert!(!LINUX_SCAN_PLAN.contains(&Source::Doubao));
        assert!(LINUX_SCAN_PLAN.contains(&Source::Pi));
        assert!(LINUX_SCAN_PLAN.contains(&Source::Qwen));
        // WSL 与 Linux 同规则
        assert_eq!(sources_for(Platform::Wsl), LINUX_SCAN_PLAN);
        assert_eq!(sources_for(Platform::Windows), WINDOWS_SCAN_PLAN);
    }

    #[test]
    fn scan_plans_stay_in_source_order() {
        // 计划顺序必须与 Source::ALL 一致，保证 scans 的稳定顺序。
        for plan in [WINDOWS_SCAN_PLAN, LINUX_SCAN_PLAN] {
            let mut positions: Vec<usize> = plan
                .iter()
                .map(|source| Source::ALL.iter().position(|s| s == source).unwrap())
                .collect();
            let sorted = {
                let mut clone = positions.clone();
                clone.sort_unstable();
                clone
            };
            assert_eq!(positions, sorted, "计划顺序应与 Source::ALL 一致");
            positions.clear();
        }
    }

    #[test]
    fn window_filters_records_and_files() {
        let mut since = HashMap::new();
        since.insert(Source::Claude, "2026-10-24".to_string());
        let ctx = ScanContext::with_window(Platform::Linux, &since);
        assert_eq!(ctx.since_day(Source::Claude), Some("2026-10-24"));
        assert_eq!(ctx.since_day(Source::Codex), None, "未给窗口的源按全量");
        assert!(ctx.in_window(Source::Claude, "2026-10-24"));
        assert!(ctx.in_window(Source::Claude, "2026-10-25"));
        assert!(!ctx.in_window(Source::Claude, "2026-10-23"));
        assert!(ctx.in_window(Source::Codex, "1999-01-01"));

        let full = ScanContext::full(Platform::Linux);
        assert!(full.in_window(Source::Claude, "1999-01-01"));
    }

    #[test]
    fn day_minus_handles_boundaries() {
        assert_eq!(day_minus("2026-10-01", 1).as_deref(), Some("2026-09-30"));
        assert_eq!(day_minus("2026-03-01", 1).as_deref(), Some("2026-02-28"));
        assert_eq!(day_minus("2026-01-01", 0).as_deref(), Some("2026-01-01"));
        assert_eq!(day_minus("not-a-day", 1), None);
    }

    #[test]
    fn file_skip_keeps_one_day_of_slack() {
        let dir = testutil::TempDir::new();
        let path = dir.path().join("a.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        // 文件刚写入 → mtime 是今天，任何窗口都应该读它
        let mut since = HashMap::new();
        since.insert(
            Source::Claude,
            crate::model::TokenSnapshot::beijing_days_ago(Utc::now(), 0),
        );
        let ctx = ScanContext::with_window(Platform::Linux, &since);
        assert!(ctx.should_read_jsonl(Source::Claude, &path));
        // 没有窗口 → 一律读
        assert!(ScanContext::full(Platform::Linux).should_read_jsonl(Source::Claude, &path));
    }
}
