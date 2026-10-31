//! UI 文案国际化：简体中文（默认）/ English。
//!
//! 全部界面字符串经此模块查表；带参数的文案用下方助手函数生成。
//! 语言在启动时从配置读入（`main.rs`），设置保存后即时切换——无需重启：
//! 主界面文案每帧重算，对话框与托盘菜单即用即建。
//! 仅影响显示文案与数值单位（中文 万/亿，英文 K/M/B），不影响采集与统计。

use std::sync::atomic::{AtomicU8, Ordering};

use crate::config::Language;

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn set_language(lang: Language) {
    LANG.store(
        match lang {
            Language::Zh => 0,
            Language::En => 1,
        },
        Ordering::Release,
    );
}

pub fn lang() -> Language {
    match LANG.load(Ordering::Acquire) {
        1 => Language::En,
        _ => Language::Zh,
    }
}

/// 语言单选钮的显示文本：语言名永远用各自语言书写。
pub const LANG_NAME_ZH: &str = "简体中文";
pub const LANG_NAME_EN: &str = "English";

/// 静态文案表。新增文案时同时补齐两个 const，漏字段编译期报错。
pub struct Texts {
    // ---- 主窗口 ----
    pub app_title: &'static str,
    pub tab_totals: &'static str,
    pub tab_local: &'static str,
    pub title_totals: &'static str,
    pub unknown: &'static str,
    pub syncing: &'static str,
    pub scanning: &'static str,
    pub first_sync_hint: &'static str,
    // ---- 仪表盘 ----
    pub top20_title: &'static str,
    pub daily_title: &'static str,
    pub empty_data: &'static str,
    pub model_ranges: [&'static str; 4],
    pub daily_ranges: [&'static str; 2],
    /// 面板标题行里的合计前缀（数值本身由 format_tokens 按语言给单位）。
    pub panel_total: &'static str,
    /// Top20 行内请求数后缀（数值由 format_tokens 按语言给单位）。
    pub requests_suffix: &'static str,
    // ---- 设置 ----
    pub settings_title: &'static str,
    pub label_refresh: &'static str,
    pub label_tpc: &'static str,
    pub label_language: &'static str,
    pub chk_autostart: &'static str,
    pub btn_aliases: &'static str,
    pub btn_clear_cache: &'static str,
    pub btn_save: &'static str,
    pub btn_cancel: &'static str,
    pub section_servers: &'static str,
    pub btn_add_server: &'static str,
    pub footer_cache_hint: &'static str,
    pub cache_cleared: &'static str,
    // ---- 服务器编辑 ----
    pub server_window_title: &'static str,
    pub title_edit_server: &'static str,
    pub title_add_server: &'static str,
    pub label_name: &'static str,
    pub label_host: &'static str,
    pub label_user: &'static str,
    pub label_port: &'static str,
    pub label_auth: &'static str,
    pub radio_password: &'static str,
    pub radio_key: &'static str,
    pub label_key_path: &'static str,
    pub label_passphrase: &'static str,
    pub btn_browse: &'static str,
    pub btn_test: &'static str,
    pub btn_retrust: &'static str,
    pub testing_now: &'static str,
    pub hint_edit: &'static str,
    pub hint_add: &'static str,
    pub filter_key_files: &'static str,
    // ---- 模型别名 ----
    pub alias_window_title: &'static str,
    pub label_from: &'static str,
    pub label_to: &'static str,
    pub btn_add: &'static str,
    pub btn_save_edit: &'static str,
    pub btn_close: &'static str,
    pub empty_aliases: &'static str,
    pub empty_alias_hint: &'static str,
    pub alias_from_empty_status: &'static str,
    // ---- 托盘 ----
    pub menu_open: &'static str,
    pub menu_refresh: &'static str,
    pub menu_settings: &'static str,
    pub menu_exit: &'static str,
    // ---- 测试连接报告的步骤名 ----
    pub step_host_key: &'static str,
    pub step_ssh_login: &'static str,
    pub step_network: &'static str,
    pub step_sftp: &'static str,
}

const ZH: Texts = Texts {
    app_title: "Token 用量观察台",
    tab_totals: "合计",
    tab_local: "本机",
    title_totals: "全部合计",
    unknown: "未知",
    syncing: "Token 同步中",
    scanning: "Token 扫描中",
    first_sync_hint: "正在首次同步 Token 数据（镜像远端各 Agent 日志），可能需要几分钟，完成后显示图表。",
    top20_title: "Token 用量 Top 20",
    daily_title: "每日 Token 用量",
    empty_data: "暂无用量数据（等待首次同步或本机扫描）",
    model_ranges: ["合计", "30天", "7天", "今天"],
    daily_ranges: ["7天", "30天"],
    panel_total: "合计",
    requests_suffix: "次",
    settings_title: "设置",
    label_refresh: "Token 刷新间隔（秒）",
    label_tpc: "每积分折算 Token 数",
    label_language: "语言",
    chk_autostart: "开机自动启动",
    btn_aliases: "模型别名\u{2026}",
    btn_clear_cache: "清空镜像缓存",
    btn_save: "保存",
    btn_cancel: "取消",
    section_servers: "服务器",
    btn_add_server: "+ 添加服务器",
    footer_cache_hint: "镜像缓存含各 Agent 日志副本（隐私敏感），可在上方清空。",
    cache_cleared: "镜像缓存已清空，下次同步将重新全量下载。",
    server_window_title: "服务器",
    title_edit_server: "编辑服务器",
    title_add_server: "添加服务器",
    label_name: "名称（可选）",
    label_host: "主机 / IP",
    label_user: "用户名",
    label_port: "端口",
    label_auth: "认证方式",
    radio_password: "密码",
    radio_key: "私钥文件",
    label_key_path: "私钥路径",
    label_passphrase: "私钥口令",
    btn_browse: "浏览\u{2026}",
    btn_test: "测试连接",
    btn_retrust: "重新信任主机密钥",
    testing_now: "正在测试连接\u{2026}",
    hint_edit: "保存后立即生效；修改主机/端口/认证方式会自动重连。",
    hint_add: "建议先点「测试连接」验证登录与 SFTP 访问。",
    filter_key_files: "私钥文件",
    alias_window_title: "模型别名",
    label_from: "原始模型名",
    label_to: "显示别名",
    btn_add: "添加",
    btn_save_edit: "保存修改",
    btn_close: "关闭",
    empty_aliases: "暂无别名映射",
    empty_alias_hint: "在上方填写原始模型名与显示别名后点「添加」",
    alias_from_empty_status: "原始模型名不能为空。",
    menu_open: "打开面板",
    menu_refresh: "立即刷新",
    menu_settings: "设置\u{2026}",
    menu_exit: "退出",
    step_host_key: "主机密钥",
    step_ssh_login: "SSH 登录",
    step_network: "网络连接",
    step_sftp: "SFTP",
};

const EN: Texts = Texts {
    app_title: "Token Usage Dashboard",
    tab_totals: "All",
    tab_local: "Local",
    title_totals: "All Totals",
    unknown: "Unknown",
    syncing: "Syncing\u{2026}",
    scanning: "Scanning\u{2026}",
    first_sync_hint: "First sync in progress (mirroring agent logs remotely); this can take a few minutes. Charts appear once done.",
    top20_title: "Token Usage Top 20",
    daily_title: "Daily Token Usage",
    empty_data: "No usage data yet (waiting for first sync or local scan)",
    model_ranges: ["All", "30d", "7d", "Today"],
    daily_ranges: ["7d", "30d"],
    panel_total: "Total",
    requests_suffix: " req",
    settings_title: "Settings",
    label_refresh: "Refresh interval (s)",
    label_tpc: "Tokens per credit",
    label_language: "Language",
    chk_autostart: "Start with Windows",
    btn_aliases: "Aliases\u{2026}",
    btn_clear_cache: "Clear cache",
    btn_save: "Save",
    btn_cancel: "Cancel",
    section_servers: "Servers",
    btn_add_server: "+ Add server",
    footer_cache_hint: "Mirror cache holds copies of agent logs (privacy-sensitive); clear it above.",
    cache_cleared: "Mirror cache cleared; the next sync will re-download in full.",
    server_window_title: "Server",
    title_edit_server: "Edit Server",
    title_add_server: "Add Server",
    label_name: "Name",
    label_host: "Host / IP",
    label_user: "Username",
    label_port: "Port",
    label_auth: "Auth method",
    radio_password: "Password",
    radio_key: "Key file",
    label_key_path: "Key path",
    label_passphrase: "Passphrase",
    btn_browse: "Browse\u{2026}",
    btn_test: "Test",
    btn_retrust: "Re-trust host key",
    testing_now: "Testing connection\u{2026}",
    hint_edit: "Takes effect on save; changing host/port/auth reconnects automatically.",
    hint_add: "Click Test first to verify login and SFTP access.",
    filter_key_files: "Key files",
    alias_window_title: "Model Aliases",
    label_from: "Model name",
    label_to: "Alias",
    btn_add: "Add",
    btn_save_edit: "Save",
    btn_close: "Close",
    empty_aliases: "No aliases yet",
    empty_alias_hint: "Fill in the fields above and click Add",
    alias_from_empty_status: "Model name is required.",
    menu_open: "Open panel",
    menu_refresh: "Refresh now",
    menu_settings: "Settings\u{2026}",
    menu_exit: "Exit",
    step_host_key: "Host key",
    step_ssh_login: "SSH login",
    step_network: "Network",
    step_sftp: "SFTP",
};

pub fn texts() -> &'static Texts {
    if lang() == Language::En { &EN } else { &ZH }
}

/// UTF-16 + NUL 结尾：原生 Win32 API 传字符串用。
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// 带参数文案：主窗口 / 对话框
// ---------------------------------------------------------------------------

/// 合计页副标题：`本机 + {servers} 台服务器 · {online}/{total} 在线`。
pub fn subtitle_servers(servers: usize, online: usize, total: usize) -> String {
    match lang() {
        Language::En => format!("Local + {servers} servers · {online}/{total} online"),
        Language::Zh => format!("本机 + {servers} 台服务器 · {online}/{total} 在线"),
    }
}

/// 服务器状态卡片：`连接错误：{err}`。
pub fn conn_error(err: &str) -> String {
    match lang() {
        Language::En => format!("Connection error: {err}"),
        Language::Zh => format!("连接错误：{err}"),
    }
}

/// 服务器状态卡片：`Token 采集失败：{err}`。
pub fn collect_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("Token collection failed: {err}"),
        Language::Zh => format!("Token 采集失败：{err}"),
    }
}

/// 设置页服务器列表溢出：`… 以及 {n} 台未显示`。
pub fn servers_more(n: usize) -> String {
    match lang() {
        Language::En => format!("… and {n} more"),
        Language::Zh => format!("… 以及 {n} 台未显示"),
    }
}

/// 别名列表标题：`已有映射（{n}）`。
pub fn mappings_count(n: usize) -> String {
    match lang() {
        Language::En => format!("Mappings ({n})"),
        Language::Zh => format!("已有映射（{n}）"),
    }
}

/// 别名列表溢出：`… 以及 {n} 条未显示`。
pub fn aliases_more(n: usize) -> String {
    match lang() {
        Language::En => format!("… and {n} more"),
        Language::Zh => format!("… 以及 {n} 条未显示"),
    }
}

pub fn clear_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("Clear failed: {err}"),
        Language::Zh => format!("清空失败：{err}"),
    }
}

pub fn autostart_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("Autostart setup failed: {err}"),
        Language::Zh => format!("自启动设置失败：{err}"),
    }
}

// ---------------------------------------------------------------------------
// 带参数文案：monitoring / ssh（后端产出、界面显示的文本）
// ---------------------------------------------------------------------------

pub fn conn_timeout() -> &'static str {
    match lang() {
        Language::En => "Connection timed out",
        Language::Zh => "连接超时",
    }
}

pub fn sync_timeout() -> &'static str {
    match lang() {
        Language::En => "Sync timed out",
        Language::Zh => "同步超时",
    }
}

pub fn sync_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("Sync failed: {err}"),
        Language::Zh => format!("同步失败：{err}"),
    }
}

pub fn remote_home_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("Cannot determine remote home: {err}"),
        Language::Zh => format!("无法确定远端 home：{err}"),
    }
}

pub fn parse_task_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("Parse task failed: {err}"),
        Language::Zh => format!("解析任务失败：{err}"),
    }
}

/// ConnectOptions::from_server：密码解密失败。
pub fn saved_password_undecryptable() -> String {
    match lang() {
        Language::En => "Saved password cannot be decrypted (Windows user or machine changed); re-edit the server".to_string(),
        Language::Zh => "已保存的密码无法解密（可能更换了 Windows 用户或机器），请重新编辑服务器".to_string(),
    }
}

/// ConnectOptions::from_server：私钥路径为空。
pub fn no_key_path() -> String {
    match lang() {
        Language::En => "Private key path is not configured".to_string(),
        Language::Zh => "未配置私钥文件路径".to_string(),
    }
}

// SshError 各变体的 Display 消息（thiserror 在显示时求值，语言随当前设置）。

pub fn net_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("Network connection failed: {err}"),
        Language::Zh => format!("网络连接失败：{err}"),
    }
}

/// 参数用 `impl Display`：thiserror 的 `.field` 简写绑定的是字段引用。
pub fn timeout_msg(secs: impl std::fmt::Display) -> String {
    match lang() {
        Language::En => format!("Connection timed out ({secs}s): check host, port and firewall"),
        Language::Zh => format!("连接超时（{secs} 秒）：检查主机、端口与防火墙"),
    }
}

pub fn host_key_changed() -> String {
    match lang() {
        Language::En => "Host key changed; connection blocked. Re-trust it in Edit Server after verifying the server was reimaged or its key rotated".to_string(),
        Language::Zh => "主机密钥已更改，连接被阻断；确认服务器重装或换钥后在“编辑服务器”中重新信任".to_string(),
    }
}

pub fn auth_failed() -> String {
    match lang() {
        Language::En => "Authentication failed: wrong username or password/key".to_string(),
        Language::Zh => "认证失败：用户名或密码/私钥不正确".to_string(),
    }
}

pub fn key_unusable(msg: &str) -> String {
    match lang() {
        Language::En => format!("Private key unusable: {msg}"),
        Language::Zh => format!("私钥无法使用：{msg}"),
    }
}

pub fn cmd_failed(msg: &str) -> String {
    match lang() {
        Language::En => format!("Command execution failed: {msg}"),
        Language::Zh => format!("命令执行失败：{msg}"),
    }
}

pub fn sftp_unavailable(msg: &str) -> String {
    match lang() {
        Language::En => format!("SFTP unavailable: {msg}"),
        Language::Zh => format!("SFTP 不可用：{msg}"),
    }
}

pub fn disconnected(msg: &str) -> String {
    match lang() {
        Language::En => format!("Disconnected: {msg}"),
        Language::Zh => format!("连接已断开：{msg}"),
    }
}

// ---------------------------------------------------------------------------
// 带参数文案：测试连接报告（ssh/tester.rs）
// ---------------------------------------------------------------------------

pub fn no_key_selected() -> String {
    match lang() {
        Language::En => "No private key file selected".to_string(),
        Language::Zh => "未选择私钥文件".to_string(),
    }
}

pub fn saved_password_reenter() -> String {
    match lang() {
        Language::En => "Saved password cannot be decrypted; re-enter it".to_string(),
        Language::Zh => "已保存的密码无法解密，请重新输入".to_string(),
    }
}

pub fn all_checks_passed(n: usize) -> String {
    match lang() {
        Language::En => format!("All {n} checks passed"),
        Language::Zh => format!("全部 {n} 项检查通过"),
    }
}

pub fn checks_failed(failed: &[&str]) -> String {
    match lang() {
        Language::En => format!("Failed: {}", failed.join(", ")),
        Language::Zh => format!("未通过：{}", failed.join("、")),
    }
}

pub fn report_headline(ok: bool, summary: &str) -> String {
    match lang() {
        Language::En => {
            if ok {
                format!("Connected: {summary}")
            } else {
                summary.to_string()
            }
        }
        Language::Zh => {
            if ok {
                format!("连接成功，{summary}")
            } else {
                summary.to_string()
            }
        }
    }
}

pub fn auth_ok() -> String {
    match lang() {
        Language::En => "Authenticated".to_string(),
        Language::Zh => "认证成功".to_string(),
    }
}

pub fn fingerprint_match(seen: &str) -> String {
    match lang() {
        Language::En => format!("Fingerprint matches: {seen}"),
        Language::Zh => format!("指纹匹配：{seen}"),
    }
}

pub fn fingerprint_first(seen: &str) -> String {
    match lang() {
        Language::En => format!("First connection; fingerprint saved: {seen}"),
        Language::Zh => format!("首次连接，已记录指纹：{seen}"),
    }
}

pub fn remote_home_ok(home: &str) -> String {
    match lang() {
        Language::En => format!("Remote home accessible: {home}"),
        Language::Zh => format!("可访问远端 home：{home}"),
    }
}

pub fn sftp_subsystem_failed(err: &str) -> String {
    match lang() {
        Language::En => format!("SFTP subsystem unavailable: {err}"),
        Language::Zh => format!("子系统不可用：{err}"),
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
pub fn test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_follow_language() {
        let _guard = test_lock().lock().unwrap();
        set_language(Language::En);
        assert_eq!(texts().tab_totals, "All");
        assert_eq!(texts().panel_total, "Total");
        assert_eq!(texts().requests_suffix, " req");
        assert_eq!(texts().model_ranges, ["All", "30d", "7d", "Today"]);
        assert_eq!(subtitle_servers(3, 2, 3), "Local + 3 servers · 2/3 online");
        assert_eq!(conn_timeout(), "Connection timed out");
        set_language(Language::Zh);
        assert_eq!(texts().tab_totals, "合计");
        assert_eq!(texts().panel_total, "合计");
        assert_eq!(texts().requests_suffix, "次");
        assert_eq!(subtitle_servers(3, 2, 3), "本机 + 3 台服务器 · 2/3 在线");
        assert_eq!(conn_timeout(), "连接超时");
    }

    #[test]
    fn to_wide_is_nul_terminated() {
        assert_eq!(to_wide("ab"), vec!['a' as u16, 'b' as u16, 0]);
    }
}
