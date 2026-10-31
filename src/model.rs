//! 跨线程共享的运行时数据模型（后台采集写、UI 读）。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 服务器状态
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Stopped,
    Connecting,
    Online,
    Error,
}

/// 单台服务器的全部运行时状态（纯 token 工具：无硬件字段）。
#[derive(Debug, Clone)]
pub struct ServerData {
    pub id: Uuid,
    pub phase: Phase,
    pub error: Option<String>,
    pub token: Option<TokenSnapshot>,
    pub token_at: Option<DateTime<Utc>>,
    /// 首次 token 全量同步尚未完成。
    pub token_syncing: bool,
    pub token_error: Option<String>,
}

impl ServerData {
    pub fn new(id: Uuid) -> Self {
        Self {
            id,
            phase: Phase::Stopped,
            error: None,
            token: None,
            token_at: None,
            token_syncing: false,
            token_error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Token 用量快照（口径与 token_summary/remote_collect.py 一致）
// ---------------------------------------------------------------------------

/// 平台：同一套 agent 在不同平台上的数据位置与规则不同。
///
/// `Wsl` 用 **Linux 规则** 扫描，但单独分桶存放（`\\wsl$\<distro>\home\<user>`），
/// 避免与 Windows 原生 app 的数据混在一起后无法回溯。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Platform {
    Linux,
    Windows,
    Wsl,
}

impl Platform {
    pub const fn as_str(self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Windows => "windows",
            Platform::Wsl => "wsl",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "linux" => Some(Platform::Linux),
            "windows" => Some(Platform::Windows),
            "wsl" => Some(Platform::Wsl),
            _ => None,
        }
    }

    /// WSL 与 Linux 共用扫描规则（文件位置等价于 Linux home）。
    pub fn is_unix(self) -> bool {
        matches!(self, Platform::Linux | Platform::Wsl)
    }
}

/// 源在本地历史库里的写入方式。
///
/// 关键区别在于「数据是记录级还是累计值」：记录级的源可以只重算
/// `[水位线, 今天]` 窗口；累计值的源（一个线程/会话一行、数值持续增长）
/// 必须整源重算，否则跨天实体的后续增长会永久丢失。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    /// 按天窗口替换：只重算 `day >= since` 的行。
    Window,
    /// 每轮全量重算并替换该源全部历史（累积值源）。
    Snapshot,
    /// 不产生 bucket，只更新 detected / 计数状态。
    ProbeOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    Codex,
    Claude,
    Opencode,
    Qwen,
    Hermes,
    Qoder,
    QoderCn,
    QoderCnCli,
    Zcode,
    Pi,
    WorkBuddy,
    Doubao,
    CommandCode,
}

impl Source {
    pub const ALL: [Source; 13] = [
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
        Source::WorkBuddy,
        Source::Doubao,
        Source::CommandCode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Source::Codex => "Codex",
            Source::Claude => "Claude",
            Source::Opencode => "OpenCode",
            Source::Qwen => "Qwen",
            Source::Hermes => "Hermes",
            Source::Qoder => "Qoder",
            Source::QoderCn => "QoderCN",
            Source::QoderCnCli => "QoderCN CLI",
            Source::Zcode => "ZCode",
            Source::Pi => "Pi",
            Source::WorkBuddy => "WorkBuddy",
            Source::Doubao => "豆包工作",
            Source::CommandCode => "Command Code",
        }
    }

    /// 历史库里的稳定键。**不要用枚举序号或 `label()` 入库**：
    /// 调整 `ALL` 顺序或改显示名都不能污染已落盘的历史。
    pub fn key(self) -> &'static str {
        match self {
            Source::Codex => "codex",
            Source::Claude => "claude",
            Source::Opencode => "opencode",
            Source::Qwen => "qwen",
            Source::Hermes => "hermes",
            Source::Qoder => "qoder",
            Source::QoderCn => "qoder_cn",
            Source::QoderCnCli => "qoder_cn_cli",
            Source::Zcode => "zcode",
            Source::Pi => "pi",
            Source::WorkBuddy => "workbuddy",
            Source::Doubao => "doubao",
            Source::CommandCode => "command_code",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Source::ALL.iter().copied().find(|s| s.key() == key)
    }

    /// 见 `StorageMode` 的文档：累计值源必须整源重算。
    pub fn storage_mode(self) -> StorageMode {
        match self {
            Source::Codex | Source::Hermes | Source::Zcode => StorageMode::Snapshot,
            Source::QoderCnCli | Source::Pi | Source::Doubao | Source::CommandCode => {
                StorageMode::ProbeOnly
            }
            Source::Claude
            | Source::Opencode
            | Source::Qwen
            | Source::Qoder
            | Source::QoderCn
            | Source::WorkBuddy => StorageMode::Window,
        }
    }

    /// 覆盖摘要里的计数单位（"123 响应 · 45,678 tokens"）。
    pub fn record_unit(self) -> &'static str {
        match self {
            Source::Codex => "线程",
            Source::Claude | Source::Qwen | Source::WorkBuddy => "响应",
            Source::Opencode => "条用量",
            Source::Hermes => "会话",
            Source::Qoder | Source::QoderCn | Source::Zcode => "请求",
            Source::QoderCnCli | Source::Pi | Source::Doubao | Source::CommandCode => "个文件",
        }
    }

    /// 由覆盖合计生成摘要。汇总口径下必须用它重新生成，而不是把各目标
    /// 的摘要拼起来（"19 线程 · A tokens；55 线程 · B tokens" 无法阅读）。
    pub fn coverage_summary(self, tokens: i64, credits: f64, messages: i64) -> String {
        if self.counts_credits() {
            format!("{messages} 请求 · {credits:.3} 积分")
        } else {
            format!("{messages} {} · {tokens} tokens", self.record_unit())
        }
    }

    /// 以「积分」而非 token 计量的源（Qoder 国际版与国内版同口径）。
    pub fn counts_credits(self) -> bool {
        matches!(self, Source::Qoder | Source::QoderCn)
    }

    /// 该源一个 bucket 的等效 token（积分按 tokens_per_credit 折算）。
    pub fn effective_tokens(self, bucket: &DayBucket, tokens_per_credit: f64) -> i64 {
        if self.counts_credits() {
            bucket.tokens + (bucket.credits * tokens_per_credit) as i64
        } else {
            bucket.tokens
        }
    }

    /// 枚举序号（新增变体时这里不编译 → 强制同步 ALL）。仅供一致性测试使用。
    #[allow(dead_code)]
    fn index_in_all(self) -> usize {
        match self {
            Source::Codex => 0,
            Source::Claude => 1,
            Source::Opencode => 2,
            Source::Qwen => 3,
            Source::Hermes => 4,
            Source::Qoder => 5,
            Source::QoderCn => 6,
            Source::QoderCnCli => 7,
            Source::Zcode => 8,
            Source::Pi => 9,
            Source::WorkBuddy => 10,
            Source::Doubao => 11,
            Source::CommandCode => 12,
        }
    }
}

/// 数据源数量守卫：与 README、`scan_all` 的平台扫描计划一致。
const _: () = assert!(Source::ALL.len() == 13);

/// 统一的 (北京日, 模型) 聚合行。
#[derive(Debug, Clone)]
pub struct DayBucket {
    /// 北京时间自然日 "YYYY-MM-DD"。
    pub day: String,
    pub model: String,
    pub tokens: i64,
    /// 仅 Qoder 使用。
    pub credits: f64,
    /// 响应/请求/线程/会话数（按源含义不同）。
    pub messages: i64,
    /// 请求数：该源能拿到的最细请求级记录数，与历史库 `day_usage.requests`
    /// 同口径。当前各源取值与 `messages` 一致（Codex 回退为线程数、Hermes 为
    /// 会话数——它们的原始数据里没有请求级记录）；独立成列是为了有统一命名
    /// 的「请求数」指标，后续可按源分化。
    pub requests: i64,
}

#[derive(Debug, Clone, Default)]
pub struct SourceScan {
    pub detected: bool,
    pub buckets: Vec<DayBucket>,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub coverage_tokens: i64,
    pub coverage_credits: f64,
    pub coverage_messages: i64,
    /// 源内记录总数（opencode 消息数 / hermes 消息表数），仅诊断用。
    pub messages_total: i64,
    /// 覆盖摘要，如 "123 响应 · 45,678 tokens"。
    pub coverage_summary: String,
    /// 本轮只拿到部分数据（数据库不可用、某个文件读取失败等）。
    ///
    /// 只用于决定「本轮的 buckets 是否可以覆盖历史库」；UI 不读取。
    /// 为真时历史库跳过该源的历史替换，保留上一轮的完整结果。
    pub partial: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TokenSnapshot {
    pub generated_at: Option<DateTime<Utc>>,
    pub scans: Vec<(Source, SourceScan)>,
}

// ---------------------------------------------------------------------------
// 远端目录探测（仅计数、不下载内容的源）
// ---------------------------------------------------------------------------

/// 远端目录探测结果，落在镜像根 `.probe.json`。
/// 用于 QoderCN CLI / Pi 这类「只能计数、无法解析用量」的源：
/// 不再整目录镜像，只记录远端是否存在与文件数。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeReport {
    pub version: u32,
    /// Unix 秒；不使用 chrono 类型（未启用 serde feature）。
    pub generated_at_unix: i64,
    pub entries: Vec<ProbeEntry>,
}

impl ProbeReport {
    pub const VERSION: u32 = 1;

    pub fn get(&self, key: &str) -> Option<&ProbeEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProbeEntry {
    /// 稳定标识："qoder-cli" / "pi"。
    pub key: String,
    /// 显示名："QoderCN CLI" / "Pi"。
    pub label: String,
    pub detected: bool,
    pub files: u64,
    pub dirs: u64,
    /// 达到深度/条目上限，计数为下限。
    pub truncated: bool,
    pub error: Option<String>,
}

/// 超过该天数视为"全部历史"——避免对 chrono::Duration::days 传入
/// 巨大值导致内部乘 86400 溢出 panic（曾使程序在渲染"总共"时 abort）。
const ALL_TIME_DAYS: i64 = 365_000;

impl TokenSnapshot {
    pub fn beijing_today(now: DateTime<Utc>) -> String {
        Self::beijing_days_ago(now, 0)
    }

    /// 北京时间自然日 "YYYY-MM-DD"；days 为距今天数（0=今天）。
    pub fn beijing_days_ago(now: DateTime<Utc>, days: i64) -> String {
        let bj = chrono::FixedOffset::east_opt(8 * 3600).expect("固定 UTC+8");
        (now.with_timezone(&bj).date_naive() - chrono::Duration::days(days))
            .format("%Y-%m-%d")
            .to_string()
    }

    fn min_day(days: i64, now: DateTime<Utc>) -> String {
        if days >= ALL_TIME_DAYS {
            String::new() // 空串与任何日期串比较都成立 → 全部历史
        } else {
            Self::beijing_days_ago(now, days)
        }
    }

    /// 把多台服务器/多个平台的快照按 (源, 日, 模型) 合并求和——"合计"选项卡用。
    pub fn merge_all<'a>(snapshots: impl Iterator<Item = &'a TokenSnapshot>) -> TokenSnapshot {
        let mut scans: HashMap<Source, SourceScan> = HashMap::new();
        for snapshot in snapshots {
            for (source, scan) in &snapshot.scans {
                let entry = scans.entry(*source).or_default();
                entry.detected |= scan.detected;
                entry.partial |= scan.partial;
                entry.coverage_tokens += scan.coverage_tokens;
                entry.coverage_credits += scan.coverage_credits;
                entry.coverage_messages += scan.coverage_messages;
                entry.messages_total += scan.messages_total;
                if let Some(error) = &scan.error {
                    if !entry.error.as_deref().is_some_and(|existing| existing == error) {
                        if let Some(existing) = &mut entry.error {
                            existing.push_str("；");
                            existing.push_str(error);
                        } else {
                            entry.error = Some(error.clone());
                        }
                    }
                }
                for warning in &scan.warnings {
                    if !entry.warnings.iter().any(|existing| existing == warning) {
                        entry.warnings.push(warning.clone());
                    }
                }
                // 只计数源没有可加和的数值，摘要原样保留第一个非空描述。
                if source.storage_mode() == StorageMode::ProbeOnly
                    && entry.coverage_summary.is_empty()
                    && !scan.coverage_summary.is_empty()
                {
                    entry.coverage_summary = scan.coverage_summary.clone();
                }
                for bucket in &scan.buckets {
                    let merged = entry
                        .buckets
                        .iter_mut()
                        .find(|existing| existing.day == bucket.day && existing.model == bucket.model);
                    match merged {
                        Some(existing) => {
                            existing.tokens += bucket.tokens;
                            existing.credits += bucket.credits;
                            existing.messages += bucket.messages;
                            existing.requests += bucket.requests;
                        }
                        None => entry.buckets.push(bucket.clone()),
                    }
                }
            }
        }
        let mut scans: Vec<(Source, SourceScan)> = scans.into_iter().collect();
        scans.sort_by_key(|(source, _)| {
            Source::ALL.iter().position(|s| s == source).unwrap_or(usize::MAX)
        });
        // 汇总口径的摘要必须由合计值重新生成，不能把各目标/各平台的摘要拼起来。
        // 只计数源没有可加和的 token，保留首次出现的描述性文案。
        for (source, scan) in scans.iter_mut() {
            if source.storage_mode() != StorageMode::ProbeOnly {
                scan.coverage_summary = source.coverage_summary(
                    scan.coverage_tokens,
                    scan.coverage_credits,
                    scan.coverage_messages,
                );
            }
        }
        TokenSnapshot {
            generated_at: Some(Utc::now()),
            scans,
        }
    }

    /// 范围内按模型聚合的 (模型显示名, tokens, 请求数)，按 tokens 降序。
    /// days = 0 仅今天；极大值 = 全部历史。
    /// Qoder 积分按 tokens_per_credit 折算；模型名经别名表（精确匹配）归并。
    pub fn model_totals(
        &self,
        days: i64,
        now: DateTime<Utc>,
        aliases: &[(String, String)],
        tokens_per_credit: f64,
    ) -> Vec<(String, i64, i64)> {
        let min_day = Self::min_day(days, now);
        let mut totals: HashMap<String, (i64, i64)> = HashMap::new();
        for (source, scan) in &self.scans {
            for bucket in &scan.buckets {
                if bucket.day < min_day {
                    continue;
                }
                let amount = source.effective_tokens(bucket, tokens_per_credit);
                if amount == 0 {
                    continue;
                }
                let name = resolve_alias(&bucket.model, aliases);
                let entry = totals.entry(name).or_insert((0, 0));
                entry.0 += amount;
                entry.1 += bucket.requests;
            }
        }
        let mut rows: Vec<(String, i64, i64)> = totals
            .into_iter()
            .map(|(name, (tokens, requests))| (name, tokens, requests))
            .collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        rows
    }

    /// 最近 `n_days` 天（含今天，北京时间）的每日 token 合计，
    /// 按旧→新排列，缺失日期补 0。
    pub fn daily_totals(
        &self,
        n_days: i64,
        now: DateTime<Utc>,
        tokens_per_credit: f64,
    ) -> Vec<(String, i64)> {
        let n_days = n_days.clamp(1, ALL_TIME_DAYS);
        let min_day = Self::beijing_days_ago(now, n_days - 1);
        let mut per_day: HashMap<String, i64> = HashMap::new();
        for (source, scan) in &self.scans {
            for bucket in &scan.buckets {
                if bucket.day < min_day {
                    continue;
                }
                let amount = source.effective_tokens(bucket, tokens_per_credit);
                *per_day.entry(bucket.day.clone()).or_insert(0) += amount;
            }
        }
        (0..n_days)
            .rev()
            .map(|ago| {
                let day = Self::beijing_days_ago(now, ago);
                let total = per_day.get(&day).copied().unwrap_or(0);
                (day, total)
            })
            .collect()
    }

    /// 每个已检测源的全历史等效 token（积分口径统一），按 `Source::ALL` 顺序。
    /// 供 `--check` 输出使用，保证与控制台/图表口径一致。
    pub fn source_totals(&self, tokens_per_credit: f64) -> Vec<(Source, i64)> {
        let mut totals: HashMap<Source, i64> = HashMap::new();
        for (source, scan) in &self.scans {
            if !scan.detected {
                continue;
            }
            let sum: i64 = scan
                .buckets
                .iter()
                .map(|bucket| source.effective_tokens(bucket, tokens_per_credit))
                .sum();
            *totals.entry(*source).or_insert(0) += sum;
        }
        Source::ALL
            .iter()
            .filter_map(|source| totals.get(source).map(|total| (*source, *total)))
            .collect()
    }
}

/// 模型名别名解析：精确匹配 from → to；未命中保留原名。
fn resolve_alias(model: &str, aliases: &[(String, String)]) -> String {
    aliases
        .iter()
        .find(|(from, _)| from == model)
        .map(|(_, to)| to.clone())
        .unwrap_or_else(|| model.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(day: &str, model: &str, tokens: i64, credits: f64) -> DayBucket {
        DayBucket {
            day: day.into(),
            model: model.into(),
            tokens,
            credits,
            messages: 1,
            requests: 1,
        }
    }

    fn scan(buckets: Vec<DayBucket>) -> SourceScan {
        SourceScan {
            detected: true,
            buckets,
            ..Default::default()
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-25T16:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn model_totals_merges_aliases_and_converts_credits() {
        let snapshot = TokenSnapshot {
            generated_at: None,
            scans: vec![
                (
                    Source::Claude,
                    scan(vec![
                        bucket("2026-10-25", "claude-sonnet-4-5", 1000, 0.0),
                        bucket("2026-10-24", "claude-sonnet-4-5", 500, 0.0),
                        bucket("2026-10-25", "claude-opus-4", 200, 0.0),
                    ]),
                ),
                (
                    Source::Qoder,
                    scan(vec![bucket("2026-10-25", "qoder-max", 0, 2.0)]),
                ),
            ],
        };
        // 7 天范围：sonnet 别名归并（1000+500=1500，请求 1+1=2）+ Qoder 2 积分 × 20000 = 40000
        let aliases = vec![(
            "claude-sonnet-4-5".to_string(),
            "Sonnet 4.5".to_string(),
        )];
        let rows = snapshot.model_totals(6, now(), &aliases, 20000.0);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("qoder-max".to_string(), 40000, 1));
        assert_eq!(rows[1], ("Sonnet 4.5".to_string(), 1500, 2));
        assert_eq!(rows[2], ("claude-opus-4".to_string(), 200, 1));
        // 今天（北京 10-26 00:00）：所有 bucket 都是北京日 10-25 → 不在今天范围
        let today = snapshot.model_totals(0, now(), &aliases, 20000.0);
        assert!(today.is_empty());
    }

    #[test]
    fn model_totals_keeps_unlabeled_row_and_sums_it() {
        // UI 契约：未命别名在未设别名时以 EMPTY_MODEL 原名成行；
        // Top20 列表的过滤在 UI 层做，面板合计必须包含它的用量。
        let snapshot = TokenSnapshot {
            generated_at: None,
            scans: vec![
                (
                    Source::Claude,
                    scan(vec![bucket(
                        "2026-10-25",
                        crate::parsing::token::EMPTY_MODEL,
                        500,
                        0.0,
                    )]),
                ),
                (Source::Qwen, scan(vec![bucket("2026-10-25", "m", 300, 0.0)])),
            ],
        };
        let rows = snapshot.model_totals(6, now(), &[], 20000.0);
        assert_eq!(
            rows,
            vec![
                (crate::parsing::token::EMPTY_MODEL.to_string(), 500, 1),
                ("m".to_string(), 300, 1),
            ]
        );
        let total: i64 = rows.iter().map(|(_, total, _)| *total).sum();
        assert_eq!(total, 800);
    }

    #[test]
    fn all_time_range_does_not_panic() {
        let snapshot = TokenSnapshot {
            generated_at: None,
            scans: vec![(Source::Claude, scan(vec![bucket("2020-01-01", "m", 42, 0.0)]))],
        };
        // 回归：曾因 chrono::Duration::days 溢出 panic
        let rows = snapshot.model_totals(i64::MAX / 4, now(), &[], 20000.0);
        assert_eq!(rows, vec![("m".to_string(), 42, 1)]);
    }

    #[test]
    fn daily_totals_fill_missing_days() {
        let snapshot = TokenSnapshot {
            generated_at: None,
            scans: vec![(Source::Claude, scan(vec![bucket("2026-10-25", "m", 300, 0.0)]))],
        };
        // now = 2026-10-25T16:00Z → 北京 2026-10-26；近 7 天 = 10-20..10-26
        let days = snapshot.daily_totals(7, now(), 20000.0);
        assert_eq!(days.len(), 7);
        assert_eq!(days[6], ("2026-10-26".to_string(), 0));
        assert_eq!(days[5], ("2026-10-25".to_string(), 300));
        assert_eq!(days[0], ("2026-10-20".to_string(), 0));
        // 旧→新排列
        assert!(days.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn merge_all_sums_buckets_across_servers() {
        let a = TokenSnapshot {
            generated_at: None,
            scans: vec![(Source::Claude, scan(vec![bucket("2026-10-25", "m", 100, 0.0)]))],
        };
        let b = TokenSnapshot {
            generated_at: None,
            scans: vec![
                (Source::Claude, scan(vec![bucket("2026-10-25", "m", 50, 0.0)])),
                (Source::Zcode, scan(vec![bucket("2026-10-24", "GLM-5.3", 70, 0.0)])),
            ],
        };
        let merged = TokenSnapshot::merge_all([&a, &b].into_iter());
        let claude = merged
            .scans
            .iter()
            .find(|(source, _)| *source == Source::Claude)
            .unwrap();
        assert_eq!(claude.1.buckets[0].tokens, 150);
        let zcode = merged
            .scans
            .iter()
            .find(|(source, _)| *source == Source::Zcode)
            .unwrap();
        assert_eq!(zcode.1.buckets[0].tokens, 70);
        // 合并后的聚合与单机一致（两台各 1 次请求 → 合计 2）
        let rows = merged.model_totals(6, now(), &[], 20000.0);
        assert_eq!(rows[0], ("m".to_string(), 150, 2));
    }

    #[test]
    fn beijing_day_bucketing() {
        // UTC 2026-10-23 20:00 = 北京 2026-10-24 04:00
        assert_eq!(TokenSnapshot::beijing_days_ago(now(), 0), "2026-10-26"); // UTC 16:00 = 北京次日 00:00
    }

    #[test]
    fn all_variants_listed_in_order() {
        assert_eq!(Source::ALL.len(), 13);
        for (position, source) in Source::ALL.iter().enumerate() {
            assert_eq!(source.index_in_all(), position, "{source:?} 的 ALL 顺序不一致");
        }
        let mut labels: Vec<&str> = Source::ALL.iter().map(|source| source.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 13, "label 必须唯一");
    }

    #[test]
    fn source_keys_are_stable_and_unique() {
        let mut keys: Vec<&str> = Source::ALL.iter().map(|source| source.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 13, "key 必须唯一");
        for source in Source::ALL {
            assert_eq!(Source::from_key(source.key()), Some(source), "{source:?} key 不可往返");
        }
        assert_eq!(Source::from_key("nope"), None);
    }

    #[test]
    fn storage_modes_match_data_shape() {
        // 累积值源：一行一个线程/会话，数值持续增长 → 必须整源重算
        assert_eq!(Source::Codex.storage_mode(), StorageMode::Snapshot);
        assert_eq!(Source::Hermes.storage_mode(), StorageMode::Snapshot);
        assert_eq!(Source::Zcode.storage_mode(), StorageMode::Snapshot);
        // 记录级源：可按天窗口替换
        assert_eq!(Source::Claude.storage_mode(), StorageMode::Window);
        assert_eq!(Source::WorkBuddy.storage_mode(), StorageMode::Window);
        assert_eq!(Source::Opencode.storage_mode(), StorageMode::Window);
        // 只计数
        assert_eq!(Source::Pi.storage_mode(), StorageMode::ProbeOnly);
        assert_eq!(Source::Doubao.storage_mode(), StorageMode::ProbeOnly);
        assert_eq!(Source::CommandCode.storage_mode(), StorageMode::ProbeOnly);
        assert_eq!(Source::QoderCnCli.storage_mode(), StorageMode::ProbeOnly);
        // 新增源不参与积分折算
        assert!(!Source::WorkBuddy.counts_credits());
        assert!(!Source::Doubao.counts_credits());
        assert!(!Source::CommandCode.counts_credits());
    }

    #[test]
    fn platforms_roundtrip() {
        for platform in [Platform::Linux, Platform::Windows, Platform::Wsl] {
            assert_eq!(Platform::from_key(platform.as_str()), Some(platform));
        }
        assert_eq!(Platform::from_key("darwin"), None);
        assert!(Platform::Wsl.is_unix());
        assert!(Platform::Linux.is_unix());
        assert!(!Platform::Windows.is_unix());
    }

    #[test]
    fn merge_all_keeps_warnings_and_summaries() {
        let mut first = scan(vec![bucket("2026-10-25", "m", 10, 0.0)]);
        first.warnings.push("甲".into());
        first.coverage_summary = "1 响应".into();
        first.coverage_tokens = 10;
        first.coverage_messages = 1;
        let mut second = scan(vec![bucket("2026-10-25", "m", 20, 0.0)]);
        second.warnings.push("乙".into());
        second.warnings.push("甲".into()); // 重复告警只保留一次
        second.coverage_summary = "2 响应".into();
        second.coverage_tokens = 20;
        second.coverage_messages = 2;
        second.partial = true;
        let a = TokenSnapshot {
            generated_at: None,
            scans: vec![(Source::Claude, first)],
        };
        let b = TokenSnapshot {
            generated_at: None,
            scans: vec![(Source::Claude, second)],
        };
        let merged = TokenSnapshot::merge_all([&a, &b].into_iter());
        let (_, claude) = merged
            .scans
            .iter()
            .find(|(source, _)| *source == Source::Claude)
            .unwrap();
        assert_eq!(claude.buckets[0].tokens, 30);
        assert_eq!(claude.warnings, vec!["甲".to_string(), "乙".to_string()]);
        // 摘要是由合计值重新生成的，不是把两个目标的描述拼起来
        assert_eq!(claude.coverage_summary, "3 响应 · 30 tokens");
        assert!(claude.partial);
    }

    #[test]
    fn merge_all_keeps_probe_summary_verbatim() {
        let mut probe = SourceScan {
            detected: true,
            ..Default::default()
        };
        probe.coverage_summary = "304 个统计文件".into();
        probe.coverage_messages = 304;
        let a = TokenSnapshot {
            generated_at: None,
            scans: vec![(Source::QoderCnCli, probe)],
        };
        let merged = TokenSnapshot::merge_all([&a].into_iter());
        assert_eq!(merged.scans[0].1.coverage_summary, "304 个统计文件");
    }

    #[test]
    fn qoder_cn_credits_are_converted() {
        // 本机实测口径：.qoder-cn 的 credits 必须与 Qoder 一样折算，否则 UI 恒为 0
        let snapshot = TokenSnapshot {
            generated_at: None,
            scans: vec![(
                Source::QoderCn,
                scan(vec![bucket("2026-10-25", "qmodel_38max", 0, 4.7168616)]),
            )],
        };
        let rows = snapshot.model_totals(6, now(), &[], 20000.0);
        assert_eq!(rows, vec![("qmodel_38max".to_string(), 94_337, 1)]);
        let days = snapshot.daily_totals(7, now(), 20000.0);
        assert_eq!(days[5], ("2026-10-25".to_string(), 94_337));
    }

    #[test]
    fn count_only_sources_never_convert_credits() {
        assert!(!Source::QoderCnCli.counts_credits());
        assert!(!Source::Pi.counts_credits());
        assert!(Source::Qoder.counts_credits());
        assert!(Source::QoderCn.counts_credits());
        let bucket = bucket("2026-10-25", "m", 0, 3.0);
        assert_eq!(Source::QoderCnCli.effective_tokens(&bucket, 20000.0), 0);
    }

    #[test]
    fn source_totals_agree_with_model_and_daily() {
        let snapshot = TokenSnapshot {
            generated_at: None,
            scans: vec![
                (
                    Source::Claude,
                    scan(vec![bucket("2026-10-25", "m", 300, 0.0)]),
                ),
                (Source::QoderCn, scan(vec![bucket("2026-10-25", "q", 0, 1.5)])),
                (Source::QoderCnCli, {
                    let mut only_count = SourceScan {
                        detected: true,
                        ..Default::default()
                    };
                    only_count.coverage_messages = 12;
                    only_count
                }),
            ],
        };
        let totals = snapshot.source_totals(20000.0);
        assert_eq!(
            totals,
            vec![(Source::Claude, 300), (Source::QoderCn, 30_000), (Source::QoderCnCli, 0)]
        );
        let sum: i64 = totals.iter().map(|(_, total)| total).sum();
        let model_sum: i64 = snapshot
            .model_totals(6, now(), &[], 20000.0)
            .iter()
            .map(|(_, total, _)| total)
            .sum();
        let daily_sum: i64 = snapshot
            .daily_totals(7, now(), 20000.0)
            .iter()
            .map(|(_, total)| total)
            .sum();
        assert_eq!(sum, model_sum);
        assert_eq!(sum, daily_sum);
        // 未检测到的源不出现在 source_totals 里
        assert!(!totals.iter().any(|(source, _)| *source == Source::Pi));
    }
}
