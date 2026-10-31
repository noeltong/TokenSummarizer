#![allow(unsafe_op_in_unsafe_fn)]
// 托盘常驻程序：不弹控制台窗口（日志写入 %APPDATA%\TokenSummarizer\logs）
#![windows_subsystem = "windows"]
//! TokenSummarizer 入口：DPI 感知、COM STA、GDI+、单实例、后台监控线程、
//! 托盘 + 气泡主窗口消息循环。

use std::sync::Arc;

use token_summarizer::app::{single_instance, tray::Tray};
use token_summarizer::config;
use token_summarizer::monitoring::{self, state::AppState, UiCommand};
use token_summarizer::ui;
use token_summarizer::util::{log, paths};

fn main() {
    // 命令行诊断模式：token_summarizer --check
    if std::env::args().any(|arg| arg == "--check") {
        let code = run_check();
        std::process::exit(code);
    }

    // 1) PerMonitorV2 DPI 感知（必须在任何窗口创建之前）
    unsafe {
        let context = windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2.0 as *mut _,
        );
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(context);
    }

    // 2) COM STA（Toast / 文件对话框需要）
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    // 2.5) comctl32 v6（app.manifest 声明了 Common-Controls 6，需显式初始化，
    //      否则复选框/单选等原生控件创建后不绘制）
    unsafe {
        use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES,
            INITCOMMONCONTROLSEX};
        let init = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&init);
    }

    // 3) 日志与配置
    log::init();
    let cfg = config::load(&paths::config_path());
    token_summarizer::i18n::set_language(cfg.language);
    let first_run = cfg.servers.is_empty();
    // 本地历史库：磁盘不可用时降级为内存库（UI 仍可用，只是不持久化）。
    let store = match token_summarizer::store::TokenStore::open_default() {
        Ok(store) => Arc::new(store),
        Err(err) => {
            token_summarizer::log_error!("历史库不可用，改用内存库: {err}");
            match token_summarizer::store::TokenStore::open_memory() {
                Ok(store) => Arc::new(store),
                Err(err) => {
                    token_summarizer::log_error!("内存历史库创建失败: {err}");
                    return;
                }
            }
        }
    };
    let state = Arc::new(AppState::new(cfg, store));

    // 4) GDI+
    let _gdiplus = ui::gdip::GdiPlusGuard::init();

    // 5) 单实例：重复启动唤起已有实例后退出
    match single_instance::acquire(ui::window::POPUP_CLASS) {
        single_instance::SingleInstanceResult::AlreadyRunning => {
            return;
        }
        single_instance::SingleInstanceResult::Primary(guard) => {
            // guard 存活期间互斥体保持占用
            std::mem::forget(guard);
        }
    }

    // 6) 后台监控（tokio 运行时占用独立线程）
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel::<UiCommand>();
    {
        let state = state.clone();
        std::thread::Builder::new()
            .name("monitor-runtime".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(4)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                runtime.block_on(monitoring::run(state, cmd_rx));
            })
            .expect("启动监控线程");
    }

    // 7) 窗口类与主窗口
    if !ui::window::register_class()
        || !ui::settings::register_class()
        || !ui::server_edit::register_class()
        || !ui::alias_edit::register_class()
    {
        token_summarizer::log_error!("窗口类注册失败");
        return;
    }
    let hwnd = match ui::window::create(state.clone(), cmd_tx.clone()) {
        Ok(hwnd) => hwnd,
        Err(err) => {
            token_summarizer::log_error!("主窗口创建失败: {err}");
            return;
        }
    };
    state.set_hwnd(hwnd);

    // 8) 托盘图标（交给主窗口的 PopupApp 持有，气泡回退需要）
    let tray = Tray::create(hwnd);
    if tray.is_none() {
        token_summarizer::log_error!("托盘图标创建失败（资源管理器未就绪？）");
    }
    unsafe {
        let ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::GWLP_USERDATA,
        ) as *mut token_summarizer::ui::window::PopupApp;
        if !ptr.is_null() {
            (*ptr).tray = tray;
        }
    }

    // 9) 首次运行：自动显示面板（其中会弹出"添加服务器"）
    if first_run {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SendMessageW(
                hwnd,
                token_summarizer::app::WM_APP_SHOW,
                None,
                None,
            );
        }
    }

    token_summarizer::log_info!("程序启动完成");
    ui::window::run_message_loop();
    token_summarizer::log_info!("程序退出");
}

/// `--check` 诊断模式：逐服务器连接测试 + 单轮同步 + 每源汇总，打印到 stdout。
/// 凭据不输出。退出码：0 全部成功，1 有失败。
fn run_check() -> i32 {
    log::init();
    println!("TokenSummarizer 诊断（--check）");
    let mut cfg = config::load(&paths::config_path());
    cfg.clamp_refresh_values();
    let tokens_per_credit = cfg.tokens_per_credit;
    println!(
        "配置：{} 台服务器，刷新间隔 {}s，积分折算 {} tokens/credit",
        cfg.servers.len(),
        cfg.token_refresh_seconds,
        tokens_per_credit
    );
    if cfg.servers.is_empty() {
        println!("提示：没有服务器配置，只做本机扫描。");
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("诊断 runtime");
    let mut failures = 0usize;

    // 历史库：--check 也走同一条流水线（窗口增量 + 写库），保证与 GUI 口径一致。
    let store = match token_summarizer::store::TokenStore::open_default() {
        Ok(store) => store,
        Err(err) => {
            println!("[!] 历史库不可用（{err}），本次检查使用内存库（不落盘）");
            match token_summarizer::store::TokenStore::open_memory() {
                Ok(store) => store,
                Err(err) => {
                    println!("[FAIL] 无法创建内存历史库：{err}");
                    return 1;
                }
            }
        }
    };
    if let Some(path) = store.path() {
        println!("历史库：{}", path.display());
    }

    // 本机：Windows app 与各 WSL 发行版分别扫描、分别打印，最后给一份合并结果
    println!("== 本机（Windows） ==");
    let mut local_snapshots: Vec<token_summarizer::model::TokenSnapshot> = Vec::new();
    match std::env::var_os("USERPROFILE").map(std::path::PathBuf::from) {
        Some(home) => {
            match std::env::var("COMPUTERNAME") {
                Ok(name) => println!("  主机：{name}"),
                Err(_) => println!("  主机：本机"),
            }
            let target = token_summarizer::store::ScanTarget::local_windows(&home);
            match monitoring::scan_local_target(&store, &target) {
                Ok(snapshot) => {
                    print_snapshot_summary(&snapshot, tokens_per_credit);
                    local_snapshots.push(snapshot);
                }
                Err(err) => {
                    println!("  [FAIL] {}", token_summarizer::util::log::redact_sensitive_text(&err.to_string()));
                    failures += 1;
                }
            }
        }
        None => {
            println!("  [FAIL] 未找到 USERPROFILE");
            failures += 1;
        }
    }

    // WSL：按 Linux 规则扫描；不可用只提示，不算失败
    let mut wsl_cache = monitoring::wsl::WslCache::new();
    wsl_cache.refresh(true);
    if let Some(err) = wsl_cache.last_error() {
        println!("== 本机（WSL） ==");
        println!("  [!] WSL 枚举失败：{err}");
    }
    for home in wsl_cache.homes().to_vec() {
        println!("== 本机（{}） ==", home.label());
        let target = token_summarizer::store::ScanTarget::wsl(
            &home.distro,
            &home.user,
            &home.root,
            &home.unix_home,
        );
        match monitoring::scan_local_target(&store, &target) {
            Ok(snapshot) => {
                print_snapshot_summary(&snapshot, tokens_per_credit);
                local_snapshots.push(snapshot);
            }
            Err(err) => println!("  [!] {err}（本机 Windows 结果不受影响）"),
        }
    }

    if local_snapshots.len() > 1 {
        println!("== 本机（合并：Windows + WSL） ==");
        let merged =
            token_summarizer::model::TokenSnapshot::merge_all(local_snapshots.iter());
        print_snapshot_summary(&merged, tokens_per_credit);
    }

    for server in &cfg.servers {
        println!("== {} ({}) ==", server.display_label(), server.display_user_host());
        let report = runtime.block_on(token_summarizer::ssh::tester::test_connection(server, None));
        for (name, ok, detail) in &report.steps {
            println!(
                "  [{}] {}: {}",
                if *ok { "OK " } else { "FAIL" },
                name,
                token_summarizer::util::log::redact_sensitive_text(detail)
            );
        }
        println!("  {}", report.headline());
        if report.ok {
            match runtime.block_on(monitoring::sync_once(server, &store)) {
                Ok((outcome, snapshot)) => {
                    println!(
                        "  [OK ] 同步：发现 {} 个文件 / 下载 {} 个 / {} 字节（探测命中 {} 个仅计数源）",
                        outcome.stats.files_seen,
                        outcome.stats.files_downloaded,
                        outcome.stats.bytes_downloaded,
                        outcome.stats.probe_detected
                    );
                    if !outcome.failed_sources.is_empty() {
                        let names: Vec<&str> =
                            outcome.failed_sources.iter().map(|s| s.label()).collect();
                        println!("  [!] 本轮同步失败（保留历史）：{}", names.join("、"));
                    }
                    let mirror = paths::cache_dir().join(server.id.to_string());
                    print_probe_summary(&mirror);
                    print_snapshot_summary(&snapshot, tokens_per_credit);
                }
                Err(err) => {
                    println!("  [FAIL] 同步：{err}");
                    failures += 1;
                }
            }
        } else {
            failures += 1;
        }
    }
    if failures == 0 {
        println!("检查完成：全部通过");
        0
    } else {
        println!("检查完成：{failures} 项失败");
        1
    }
}

/// 打印镜像根下 `.probe.json` 的探测结论（仅计数源）。
fn print_probe_summary(mirror: &std::path::Path) {
    use token_summarizer::parsing::token::probe;
    let Some(report) = probe::load(mirror) else {
        return;
    };
    for entry in &report.entries {
        if let Some(error) = &entry.error {
            println!("  探测 [FAIL] {}：{error}", entry.label);
        } else if entry.detected {
            println!(
                "  探测 {}：{} 个文件 / {} 个目录{}",
                entry.label,
                entry.files,
                entry.dirs,
                if entry.truncated { "（已达上限，实际更多）" } else { "" }
            );
        } else {
            println!("  探测 {}：未安装", entry.label);
        }
    }
}

/// 打印快照中每个源的汇总行（口径与 UI 图表一致：积分统一折算）。
fn print_snapshot_summary(
    snapshot: &token_summarizer::model::TokenSnapshot,
    tokens_per_credit: f64,
) {
    use token_summarizer::ui::drawing::format_tokens;
    let totals = snapshot.source_totals(tokens_per_credit);
    let mut undetected = Vec::new();
    for (source, scan) in &snapshot.scans {
        if !scan.detected {
            undetected.push(source.label());
            continue;
        }
        let total = totals
            .iter()
            .find(|(candidate, _)| candidate == source)
            .map(|(_, total)| *total)
            .unwrap_or(0);
        println!(
            "  {:<12} {}（折合 {} tokens）",
            source.label(),
            scan.coverage_summary,
            format_tokens(total)
        );
        if let Some(error) = &scan.error {
            println!("    [FAIL] {error}");
        }
        for warning in scan.warnings.iter().take(3) {
            println!("    [!] {warning}");
        }
    }
    if !undetected.is_empty() {
        println!("  未检测到：{}", undetected.join("、"));
    }
}
