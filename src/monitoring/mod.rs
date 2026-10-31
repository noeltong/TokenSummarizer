//! 监控编排：本机（Windows + WSL）与每台服务器各一个采集循环。
//!
//! 两个循环共用同一条流水线：
//! `历史库定窗口 → 扫描（窗口内） → 写库 → 从库重建全历史快照 → UI`。
//! 这样 UI 拿到的始终是完整历史，而每轮真正解析的只有最后一天到现在的数据。

pub mod state;
pub mod token_sync;
pub mod wsl;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

use crate::model::{Phase, Platform, Source, TokenSnapshot};
use crate::parsing::token::{self, ScanContext};
use crate::ssh::{ConnectOptions, Session, TRANSFER_TIMEOUT};
use crate::store::{ScanTarget, TokenStore};
use state::AppState;

/// 来源不明/不可用时的兜底刷新间隔（与配置默认值一致）。
const FALLBACK_INTERVAL_SECS: u64 = 3600;

/// UI → 后台命令。
pub enum UiCommand {
    /// 配置已变更（增删服务器 / 改间隔）：重建全部监控循环。
    RestartMonitors,
    /// 立即刷新全部服务器。
    RefreshAll,
    /// 测试连接（编辑对话框用），结果经 oneshot 返回。
    TestConnection {
        server: crate::config::ServerConfig,
        fingerprint_override: Option<String>,
        reply: tokio::sync::oneshot::Sender<crate::ssh::tester::TestReport>,
    },
}

/// 后台运行体：占用当前 tokio 运行时，直到 UI 关闭命令通道。
pub async fn run(state: Arc<AppState>, mut commands: mpsc::UnboundedReceiver<UiCommand>) {
    let refresh = Arc::new(Notify::new());
    let mut tasks = tokio::task::JoinSet::new();
    spawn_all(&state, &refresh, &mut tasks);
    crate::log_info!("监控已启动");
    while let Some(command) = commands.recv().await {
        match command {
            UiCommand::RestartMonitors => {
                tasks.abort_all();
                while tasks.join_next().await.is_some() {}
                spawn_all(&state, &refresh, &mut tasks);
                crate::log_info!("监控已按新配置重启");
            }
            UiCommand::RefreshAll => {
                refresh.notify_waiters();
            }
            UiCommand::TestConnection {
                server,
                fingerprint_override,
                reply,
            } => {
                tokio::spawn(async move {
                    let report =
                        crate::ssh::tester::test_connection(&server, fingerprint_override.as_deref())
                            .await;
                    let _ = reply.send(report);
                });
            }
        }
    }
    tasks.abort_all();
    crate::log_info!("监控已停止");
}

fn spawn_all(state: &Arc<AppState>, refresh: &Arc<Notify>, tasks: &mut tokio::task::JoinSet<()>) {
    // 本机 agent 扫描（Windows app + WSL，与 SSH 服务器同级的数据源）
    tasks.spawn(local_loop(state.clone(), refresh.clone()));
    let servers = state
        .cfg
        .read()
        .map(|cfg| cfg.servers.clone())
        .unwrap_or_default();
    for server in servers {
        tasks.spawn(token_loop(state.clone(), server.id, refresh.clone()));
    }
}

fn refresh_interval(state: &AppState) -> u64 {
    state
        .cfg
        .read()
        .map(|cfg| cfg.token_refresh_seconds as u64)
        .unwrap_or(FALLBACK_INTERVAL_SECS)
}

/// 本机 loop：Windows app 与各个 WSL 发行版分别作为独立 target 扫描，
/// 结果在内存里合并成一份快照 —— UI 只有一个「本机」选项卡。
async fn local_loop(state: Arc<AppState>, refresh: Arc<Notify>) {
    let Some(home) = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from) else {
        crate::log_error!("未找到 USERPROFILE，本机 agent 扫描不可用");
        return;
    };

    // 上次异常退出可能留下 WSL 的 SQLite 临时副本，启动时清一次。
    token::sqlite::cleanup_stale(Duration::from_secs(24 * 3600));

    let mut wsl = wsl::WslCache::new();
    let mut first_round = true;

    loop {
        // 1) 先把库里的历史发出去：重启后不必等 WSL 扫完就能看到 30 天/全部图表。
        publish_local(&state);

        // 2) Windows 原生 app
        let windows_target = ScanTarget::local_windows(&home);
        if !scan_target(&state, &windows_target).await {
            crate::log_error!("本机 Windows 扫描失败");
        }
        publish_local(&state);

        // 3) WSL：按 Linux 规则扫，逐个发布，慢的发行版不挡住 Windows 结果
        wsl.refresh(first_round);
        let homes = wsl.homes().to_vec();
        for home in homes {
            if wsl.in_backoff(&home.distro) {
                continue;
            }
            let target = ScanTarget::wsl(&home.distro, &home.user, &home.root, &home.unix_home);
            if scan_target(&state, &target).await {
                wsl.note_success(&home.distro);
            } else {
                wsl.note_failure(&home.distro);
                crate::log_error!("{} 扫描失败", home.label());
            }
            publish_local(&state);
        }
        first_round = false;

        let interval = refresh_interval(&state);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
            _ = refresh.notified() => {}
        }
    }
}

/// 扫描一个本机 target（Windows 或某个 WSL 发行版）并写入历史库。
/// 返回 false 表示扫描/写库失败（调用方决定是否退避）。
async fn scan_target(state: &AppState, target: &ScanTarget) -> bool {
    let store = state.store.clone();
    let target = target.clone();
    match tokio::task::spawn_blocking(move || scan_local_target(&store, &target)).await {
        Ok(Ok(_)) => true,
        Ok(Err(err)) => {
            crate::log_error!("扫描失败: {err}");
            false
        }
        Err(err) => {
            crate::log_error!("解析任务失败: {err}");
            false
        }
    }
}

/// 同步版：扫描一个本地 target（本机 home / WSL home）→ 写库 → 读回全历史快照。
///
/// `--check` 直接用这个函数，避免再走一遍 tokio。
pub fn scan_local_target(
    store: &TokenStore,
    target: &ScanTarget,
) -> Result<TokenSnapshot, String> {
    let epoch = target.local_epoch();
    let started_day = TokenSnapshot::beijing_today(Utc::now());
    let plan = store
        .prepare(target, &epoch, token::sources_for(target.platform))
        .map_err(|err| format!("{}: 读取历史库失败：{err}", target.label))?;
    let full = plan.full_scan;
    let scanned = {
        let ctx = match plan.window() {
            Some(window) => ScanContext::with_window(target.platform, window),
            None => ScanContext::full(target.platform),
        };
        token::scan_all(&target.root, &ctx)
    };
    if full {
        crate::log_info!("{}: 首次/全量扫描完成，开始建库", target.label);
    }
    store
        .apply(target, &epoch, &plan, &scanned, &started_day, &[])
        .map_err(|err| format!("{}: 写入历史库失败：{err}", target.label))?;
    Ok(store.load(target)?.unwrap_or(scanned))
}

/// 从历史库把所有本机 target（Windows + WSL）合并后发布给 UI。
fn publish_local(state: &AppState) {
    let Ok(targets) = state.store.list_targets() else {
        return;
    };
    let mut snapshots = Vec::new();
    let mut latest: Option<chrono::DateTime<Utc>> = None;
    for target in targets
        .iter()
        .filter(|target| matches!(target.platform, Platform::Windows | Platform::Wsl))
    {
        if let Ok(Some(snapshot)) = state.store.load(target) {
            // WSL 的告警/摘要在合并后会混在一起，加前缀区分来源。
            let snapshot = if target.platform == Platform::Wsl {
                prefix_snapshot(snapshot, &target.label)
            } else {
                snapshot
            };
            snapshots.push(snapshot);
        }
        if let Some(at) = state.store.last_success_at(&target.id) {
            latest = Some(latest.map_or(at, |prev| prev.max(at)));
        }
    }
    if snapshots.is_empty() {
        return;
    }
    let merged = TokenSnapshot::merge_all(snapshots.iter());
    state.set_local_snapshot(merged, latest.unwrap_or_else(Utc::now));
    state.post_update();
}

fn prefix_snapshot(mut snapshot: TokenSnapshot, label: &str) -> TokenSnapshot {
    for (_, scan) in &mut snapshot.scans {
        for warning in &mut scan.warnings {
            *warning = format!("[{label}] {warning}");
        }
    }
    snapshot
}

/// Token 同步循环：SFTP 增量镜像 → 窗口解析 → 写库 → 从库重建全历史快照。
/// phase/error 由本循环独占维护。
async fn token_loop(state: Arc<AppState>, server_id: Uuid, refresh: Arc<Notify>) {
    let mut backoff_secs: u64 = 1;
    let mut mirror = token_sync::MirrorSync::new();
    let mut session: Option<Session> = None;
    let mut first_round = true;

    // 启动时先把库里的历史发出去，UI 不必等第一轮同步完成。
    if let Some(server_cfg) = state.server_config(server_id) {
        let mirror_root = crate::util::paths::cache_dir().join(server_cfg.id.to_string());
        let target = ScanTarget::server(
            server_cfg.id,
            &server_cfg.display_label(),
            &server_cfg.host,
            server_cfg.port,
            &server_cfg.username,
            &mirror_root,
        );
        if let Ok(Some(snapshot)) = state.store.load(&target) {
            let at = state
                .store
                .last_success_at(&target.id)
                .unwrap_or_else(Utc::now);
            state.update_server(server_id, |data| {
                data.token = Some(snapshot);
                data.token_at = Some(at);
            });
            state.post_update();
        }
    }

    loop {
        let Some(server_cfg) = state.server_config(server_id) else {
            break;
        };
        let interval_secs = refresh_interval(&state);

        state.update_server(server_id, |data| {
            data.phase = Phase::Connecting;
            data.token_syncing = true;
        });
        state.post_update();

        let result: Result<(), String> = async {
            if session.as_ref().map(|s| s.is_closed()).unwrap_or(true) {
                session = None;
                let options = ConnectOptions::from_server(&server_cfg).map_err(|e| e.to_string())?;
                let new_session =
                    tokio::time::timeout(crate::ssh::CONNECT_TIMEOUT, Session::connect(&options))
                        .await
                        .map_err(|_| crate::i18n::conn_timeout().to_string())?
                        .map_err(|e| e.to_string())?;
                // 首次连接：TOFU 持久化指纹
                if server_cfg.host_key_fingerprint_sha256.is_none() {
                    if let Some(seen) = new_session.seen_fingerprint() {
                        state.set_host_fingerprint(server_id, seen.clone());
                        crate::log_info!("{}: 记录主机密钥指纹 {seen}", server_cfg.display_label());
                    }
                }
                session = Some(new_session);
            }
            let session_ref = session.as_ref().expect("session just connected");
            let (outcome, snapshot) =
                sync_with_session(session_ref, &server_cfg, &mut mirror, &state.store).await?;

            if first_round {
                crate::log_info!(
                    "{}: 首轮同步 {} 个文件，下载 {} 个 / {} 字节",
                    server_cfg.display_label(),
                    outcome.stats.files_seen,
                    outcome.stats.files_downloaded,
                    outcome.stats.bytes_downloaded
                );
                first_round = false;
            }

            state.update_server(server_id, |data| {
                data.phase = Phase::Online;
                data.error = None;
                data.token = Some(snapshot);
                data.token_at = Some(Utc::now());
                data.token_syncing = false;
                data.token_error = None;
            });
            Ok(())
        }
        .await;

        if let Err(err) = result {
            if let Some(session) = session.take() {
                session.disconnect().await;
            }
            state.update_server(server_id, |data| {
                data.phase = Phase::Error;
                data.error = Some(err.clone());
                data.token_syncing = false;
                data.token_error = Some(err);
            });
            state.post_update();
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(60);
            continue;
        }
        backoff_secs = 1;
        state.post_update();

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {}
            _ = refresh.notified() => {}
        }
    }
}

/// 在既有会话上执行一轮：镜像同步 → 增量扫描 → 写库 → 从库重建快照。
pub async fn sync_with_session(
    session: &Session,
    server_cfg: &crate::config::ServerConfig,
    mirror: &mut token_sync::MirrorSync,
    store: &TokenStore,
) -> Result<(token_sync::SyncOutcome, TokenSnapshot), String> {
    let sftp = session.sftp().await.map_err(|e| e.to_string())?;
    let home = sftp
        .canonicalize(".")
        .await
        .map_err(|e| crate::i18n::remote_home_failed(&e.to_string()))?;
    let mirror_root = crate::util::paths::cache_dir().join(server_cfg.id.to_string());
    std::fs::create_dir_all(&mirror_root).map_err(|e| e.to_string())?;

    let outcome = tokio::time::timeout(TRANSFER_TIMEOUT, mirror.sync(&sftp, &home, &mirror_root))
        .await
        .map_err(|_| crate::i18n::sync_timeout().to_string())?
        .map_err(|e| crate::i18n::sync_failed(&e.to_string()))?;

    let target = ScanTarget::server(
        server_cfg.id,
        &server_cfg.display_label(),
        &server_cfg.host,
        server_cfg.port,
        &server_cfg.username,
        &mirror_root,
    );
    let snapshot = scan_mirror(store, &target, &outcome.epoch, &outcome.failed_sources).await?;
    Ok((outcome, snapshot))
}

/// 扫描镜像目录：定窗口 → 解析 → 写库 → 读回全历史快照。
async fn scan_mirror(
    store: &TokenStore,
    target: &ScanTarget,
    epoch: &str,
    failed: &[Source],
) -> Result<TokenSnapshot, String> {
    let started_day = TokenSnapshot::beijing_today(Utc::now());
    let plan = store.prepare(target, epoch, token::sources_for(Platform::Linux))?;
    let window: Option<HashMap<Source, String>> = plan.window().cloned();
    let root = target.root.clone();
    let platform = Platform::Linux;
    let scanned = tokio::task::spawn_blocking(move || {
        let ctx = match &window {
            Some(window) => ScanContext::with_window(platform, window),
            None => ScanContext::full(platform),
        };
        token::scan_all(&root, &ctx)
    })
    .await
    .map_err(|err| crate::i18n::parse_task_failed(&err.to_string()))?;

    store.apply(target, epoch, &plan, &scanned, &started_day, failed)?;
    // 从库里读回来：这样 UI 拿到的是全历史，而不是本轮的窗口。
    Ok(store.load(target)?.unwrap_or(scanned))
}

/// 一次性同步（--check 诊断用）：连接 → 同步 → 解析 → 写库 → 断开。
pub async fn sync_once(
    server_cfg: &crate::config::ServerConfig,
    store: &TokenStore,
) -> Result<(token_sync::SyncOutcome, TokenSnapshot), String> {
    let options = ConnectOptions::from_server(server_cfg).map_err(|e| e.to_string())?;
    let session = tokio::time::timeout(crate::ssh::CONNECT_TIMEOUT, Session::connect(&options))
        .await
        .map_err(|_| crate::i18n::conn_timeout().to_string())?
        .map_err(|e| e.to_string())?;
    let mut mirror = token_sync::MirrorSync::new();
    let result = sync_with_session(&session, server_cfg, &mut mirror, store).await;
    session.disconnect().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceScan;

    fn snapshot_with_warning(warning: &str) -> TokenSnapshot {
        TokenSnapshot {
            generated_at: None,
            scans: vec![(
                Source::Claude,
                SourceScan {
                    detected: true,
                    warnings: vec![warning.to_string()],
                    ..Default::default()
                },
            )],
        }
    }

    #[test]
    fn wsl_warnings_get_a_prefix() {
        let prefixed = prefix_snapshot(snapshot_with_warning("数据库读取失败"), "WSL Ubuntu");
        assert_eq!(
            prefixed.scans[0].1.warnings,
            vec!["[WSL Ubuntu] 数据库读取失败".to_string()]
        );
    }
}
