//! SFTP 增量镜像：把远端 home 下各 agent 的 JSONL / SQLite 副本到
//! %LOCALAPPDATA% 镜像目录，供本地解析。JSONL 按 offset 只拉新增尾部；
//! SQLite 三件套（db/-wal/-shm）变更时整体重下、临时文件 + 原子替换；
//! 远端删除的文件同步从镜像清理；本地文件 mtime 设为远端 mtime，
//! 使"缺时间戳按文件修改日归档"的口径与参考实现一致。
//!
//! 镜像根有一个 `.mirror_epoch` 标记：它是"这一份缓存"的身份。用户点
//! 「清空缓存」后目录被删、标记随之消失，下一轮会生成新 epoch ——
//! 历史库据此强制全量重扫，避免拿半套镜像按旧水位线覆盖历史。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::model::Source;
use crate::parsing::token::probe;

/// 镜像缓存身份标记（镜像根下，不在任何 JSONL base 内，不会被 prune 命中）。
pub const EPOCH_FILE: &str = ".mirror_epoch";

/// JSONL 源规格（相对 home），全部递归收集 *.jsonl。
struct JsonlSpec {
    source: Source,
    base: &'static str,
    /// Qwen 只取 `chats/` 目录下的直接子文件。
    qwen_chats_only: bool,
}

const LINUX_JSONL_BASES: &[JsonlSpec] = &[
    JsonlSpec { source: Source::Claude, base: ".claude/projects", qwen_chats_only: false },
    JsonlSpec { source: Source::Claude, base: ".claude/sessions", qwen_chats_only: false },
    JsonlSpec { source: Source::Qoder, base: ".qoder/projects", qwen_chats_only: false },
    JsonlSpec { source: Source::QoderCn, base: ".qoder-cn/projects", qwen_chats_only: false },
    JsonlSpec { source: Source::Qwen, base: ".qwen/projects", qwen_chats_only: true },
    // ZCode 模型请求记录：仅补齐 DB 水位线之后的最新记录
    JsonlSpec { source: Source::Zcode, base: ".zcode/cli/rollout", qwen_chats_only: false },
    // WorkBuddy：只镜像 projects/。`.workbuddy` 下还有二进制/日志/扩展等目录，
    // 无解析价值却要逐目录 walk；解析器也只读 `.workbuddy/projects`。
    JsonlSpec { source: Source::WorkBuddy, base: ".workbuddy/projects", qwen_chats_only: false },
];

/// SQLite 源（相对 home）。
/// 注意 `.zcode/cli/db/db.sqlite` 整库约 30MB（其中真正要用的 `model_usage` 不到 1MB），
/// WAL checkpoint 会改主库 mtime，活跃期最坏每轮重下约 34MB —— 这是拿到 ZCode 全量
/// 模型分布的代价：rollout 目录只保留最近约 2 小时，早期模型（如 GLM-5.3）只能从库里取。
struct SqliteSpec {
    source: Source,
    rel: &'static str,
}

const LINUX_SQLITE_DBS: &[SqliteSpec] = &[
    SqliteSpec { source: Source::Codex, rel: ".codex/state_5.sqlite" },
    SqliteSpec { source: Source::Opencode, rel: ".local/share/opencode/opencode.db" },
    SqliteSpec { source: Source::Hermes, rel: ".hermes/state.db" },
    SqliteSpec { source: Source::Zcode, rel: ".zcode/cli/db/db.sqlite" },
];

/// 只计数、不解析用量的源：不再镜像内容（`.qoder-cli/ai-stats` 里是含完整源码
/// 的归属统计），改为目录探测后写入镜像根 `.probe.json`。
struct ProbeSpec {
    source: Source,
    key: &'static str,
    label: &'static str,
    /// 判定「已安装」的目录（相对 home）
    detect_rel: &'static str,
    /// 统计文件数的目录（相对 home），与本机解析口径保持一致
    count_rel: &'static str,
}

const LINUX_PROBE_SPECS: &[ProbeSpec] = &[
    ProbeSpec {
        source: Source::QoderCnCli,
        key: "qoder-cli",
        label: "QoderCN CLI",
        detect_rel: ".qoder-cli",
        count_rel: ".qoder-cli/ai-stats",
    },
    ProbeSpec {
        source: Source::Pi,
        key: "pi",
        label: "Pi",
        detect_rel: ".pi",
        count_rel: ".pi/agent/sessions",
    },
    ProbeSpec {
        source: Source::CommandCode,
        key: "command-code",
        label: "Command Code",
        detect_rel: ".commandcode",
        count_rel: ".commandcode",
    },
];

const MAX_FILES_PER_BASE: usize = 5000;
const MAX_WALK_DEPTH: usize = 8;
/// 目录探测的深度与条目上限（只计数，成本要可控）。
const PROBE_MAX_DEPTH: usize = 6;
const PROBE_MAX_ENTRIES: u64 = 5000;

#[derive(Debug, Default, Clone)]
pub struct SyncStats {
    pub files_seen: usize,
    pub files_downloaded: usize,
    pub bytes_downloaded: u64,
    /// 探测到已安装的「仅计数」源数量。
    pub probe_detected: usize,
}

#[derive(Debug, Clone)]
struct RemoteFile {
    /// 相对 home 的路径（正斜杠）。
    rel: String,
    size: u64,
    mtime: u32,
}

#[derive(Debug, Clone, Default)]
struct FileState {
    size: u64,
    mtime: u32,
    /// 已镜像字节数（JSONL 对齐到最后一个完整行）。
    have: u64,
}

/// 每服务器镜像同步器；offset 状态仅在内存中，重启后首轮全量。
pub struct MirrorSync {
    states: HashMap<String, FileState>,
}

/// 一轮同步的结果。
#[derive(Debug, Clone)]
pub struct SyncOutcome {
    pub stats: SyncStats,
    /// 本轮镜像的缓存身份；历史库据此决定是否强制全量重扫。
    pub epoch: String,
    /// 有源没同步成功（读失败/探测失败）：对应源本轮不可信。
    pub failed_sources: Vec<Source>,
}

impl MirrorSync {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    pub async fn sync(
        &mut self,
        sftp: &SftpSession,
        home: &str,
        mirror_root: &Path,
    ) -> Result<SyncOutcome, String> {
        sftp.set_timeout(90);
        let mut stats = SyncStats::default();
        let mut failed_sources: Vec<Source> = Vec::new();

        // 缓存身份：标记缺失 = 首次运行或用户清空了缓存。
        // 此时内存里的 offset 全部失效（文件已经不在磁盘上），必须清空，
        // 否则会按旧 have 往一个被删掉的文件里追加，产出半截 JSONL。
        let (epoch, marker_present) = read_or_create_epoch(mirror_root);
        if !marker_present {
            self.states.clear();
            crate::log_info!("镜像缓存标记缺失，按全新缓存处理（epoch {epoch}）");
        }

        // JSONL：逐基目录枚举 + 增量下载 + 清理远端已删除文件
        for spec in LINUX_JSONL_BASES {
            let base = spec.base;
            let remote_base = format!("{home}/{base}");
            let mut files = Vec::new();
            walk_remote(sftp, &remote_base, base, spec.qwen_chats_only, &mut files).await;
            stats.files_seen += files.len();

            let seen: std::collections::HashSet<String> =
                files.iter().map(|file| file.rel.clone()).collect();

            for file in &files {
                if let Err(err) =
                    sync_jsonl_file(sftp, home, mirror_root, &mut self.states, file, &mut stats)
                        .await
                {
                    crate::log_error!("同步 {} 失败: {err}", file.rel);
                    failed_sources.push(spec.source);
                }
            }
            prune_local(mirror_root, base, &seen);
        }

        // SQLite：三件套一起比对
        for spec in LINUX_SQLITE_DBS {
            if !sync_sqlite_bundle(sftp, home, mirror_root, &mut self.states, spec.rel, &mut stats)
                .await
            {
                failed_sources.push(spec.source);
            }
        }

        // 仅计数源：目录探测（不下载内容），写镜像根 .probe.json
        let (report, probe_failures) = probe_remote(sftp, home).await;
        stats.probe_detected = report.entries.iter().filter(|entry| entry.detected).count();
        match probe::save(mirror_root, &report) {
            Ok(()) => prune_legacy_mirrors(mirror_root, &mut self.states),
            // 探测结果写不进去就保留旧镜像/旧探测，避免源彻底消失
            Err(err) => {
                crate::log_error!("探测结果写入失败: {err}");
                failed_sources.extend(probe_failures.iter().copied());
            }
        }
        failed_sources.sort();
        failed_sources.dedup();

        Ok(SyncOutcome {
            stats,
            epoch,
            failed_sources,
        })
    }
}

/// 读取或创建镜像缓存的 epoch 标记。
/// 返回值第二项表示「标记原本就存在」—— 不存在说明这是全新（或被清空的）缓存。
fn read_or_create_epoch(mirror_root: &Path) -> (String, bool) {
    let path = mirror_root.join(EPOCH_FILE);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(epoch) = value.get("epoch").and_then(|value| value.as_str()) {
                if !epoch.is_empty() {
                    return (epoch.to_string(), true);
                }
            }
        }
    }
    let epoch = uuid::Uuid::new_v4().to_string();
    let payload = serde_json::json!({
        "version": 1,
        "epoch": epoch,
        "created_at_unix": chrono::Utc::now().timestamp(),
    });
    let _ = std::fs::create_dir_all(mirror_root);
    let tmp = mirror_root.join(format!("{EPOCH_FILE}.tmp"));
    if std::fs::write(&tmp, payload.to_string()).is_ok()
        && std::fs::rename(&tmp, &path).is_err()
    {
        let _ = std::fs::remove_file(&tmp);
    }
    (epoch, false)
}

/// 探测所有「仅计数」源（只 read_dir 计数，不下载文件）。
/// 返回探测报告 + 探测失败（真正的 IO 错误）的源。
async fn probe_remote(
    sftp: &SftpSession,
    home: &str,
) -> (crate::model::ProbeReport, Vec<Source>) {
    let mut entries = Vec::new();
    let mut failed = Vec::new();
    for spec in LINUX_PROBE_SPECS {
        let detect_path = format!("{home}/{}", spec.detect_rel);
        let mut entry = crate::model::ProbeEntry {
            key: spec.key.to_string(),
            label: spec.label.to_string(),
            ..Default::default()
        };
        match sftp.read_dir(detect_path).await {
            Ok(_) => entry.detected = true,
            Err(err) => {
                // 目录不存在属正常（未安装），只有真正的 IO 错误才记录
                if !is_not_found(&err) {
                    entry.error = Some(err.to_string());
                    failed.push(spec.source);
                }
                entries.push(entry);
                continue;
            }
        }
        let count_path = format!("{home}/{}", spec.count_rel);
        let (files, dirs, truncated) =
            count_remote_tree(sftp, &count_path, PROBE_MAX_DEPTH, PROBE_MAX_ENTRIES).await;
        entry.files = files;
        entry.dirs = dirs;
        entry.truncated = truncated;
        entries.push(entry);
    }
    (
        crate::model::ProbeReport {
            version: crate::model::ProbeReport::VERSION,
            generated_at_unix: chrono::Utc::now().timestamp(),
            entries,
        },
        failed,
    )
}

/// 远端目录是否不存在（各 SSH 服务端错误文案不一致，只做粗判）。
fn is_not_found(err: &russh_sftp::client::error::Error) -> bool {
    let text = err.to_string().to_lowercase();
    text.contains("no such file") || text.contains("not found") || text.contains("does not exist")
}

/// 递归统计远端目录下的文件/子目录数量（深度与条目上限内）。
async fn count_remote_tree(
    sftp: &SftpSession,
    remote_root: &str,
    max_depth: usize,
    max_entries: u64,
) -> (u64, u64, bool) {
    let mut files = 0u64;
    let mut dirs = 0u64;
    let mut truncated = false;
    let mut stack: Vec<(String, usize)> = vec![(remote_root.to_string(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > max_depth {
            truncated = true;
            continue;
        }
        let entries = match sftp.read_dir(dir.clone()).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            if files + dirs >= max_entries {
                truncated = true;
                return (files, dirs, truncated);
            }
            if entry.file_type().is_dir() {
                dirs += 1;
                stack.push((format!("{dir}/{}", entry.file_name()), depth + 1));
            } else {
                files += 1;
            }
        }
    }
    (files, dirs, truncated)
}

/// 清理历史遗留的整目录镜像（`.qoder-cli`）及其 offset 状态。
fn prune_legacy_mirrors(
    mirror_root: &Path,
    states: &mut HashMap<String, FileState>,
) {
    let legacy = mirror_root.join(".qoder-cli");
    if legacy.is_dir() {
        let _ = std::fs::remove_dir_all(&legacy);
    }
    states.retain(|rel, _| !rel.starts_with(".qoder-cli/"));
}

async fn walk_remote(
    sftp: &SftpSession,
    remote_root: &str,
    rel_root: &str,
    qwen_base: bool,
    out: &mut Vec<RemoteFile>,
) {
    // 迭代式遍历（async fn 不能直接递归）
    let mut stack: Vec<(String, String, usize)> = vec![(remote_root.to_string(), rel_root.to_string(), 0)];
    while let Some((remote_dir, rel_base, depth)) = stack.pop() {
        if depth > MAX_WALK_DEPTH || out.len() >= MAX_FILES_PER_BASE {
            continue;
        }
        let entries = match sftp.read_dir(remote_dir.clone()).await {
            Ok(entries) => entries,
            Err(_) => continue, // 子目录不存在或不可读
        };
        // 当前目录名（用于 .qwen/projects/**/chats/*.jsonl 过滤）
        let dir_name = remote_dir.rsplit('/').next().unwrap_or_default();
        for entry in entries {
            let name = entry.file_name();
            let metadata = entry.metadata();
            if entry.file_type().is_dir() {
                let child = format!("{remote_dir}/{name}");
                let child_rel = format!("{rel_base}/{name}");
                stack.push((child, child_rel, depth + 1));
            } else if name.ends_with(".jsonl") {
                if qwen_base && dir_name != "chats" {
                    continue; // Qwen 只取 chats/ 目录直接子文件
                }
                let rel = format!("{rel_base}/{name}");
                out.push(RemoteFile {
                    rel,
                    size: metadata.size.unwrap_or(0),
                    mtime: metadata.mtime.unwrap_or(0),
                });
                if out.len() >= MAX_FILES_PER_BASE {
                    return;
                }
            }
        }
    }
    // 结果按 rel 排序，保证与本地解析顺序稳定
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
}

async fn sync_jsonl_file(
    sftp: &SftpSession,
    home: &str,
    mirror_root: &Path,
    states: &mut HashMap<String, FileState>,
    file: &RemoteFile,
    stats: &mut SyncStats,
) -> Result<(), String> {
    let state = states.get(&file.rel);
    if let Some(state) = state {
        if state.size == file.size && state.mtime == file.mtime && state.have == file.size {
            return Ok(()); // 未变化
        }
    }

    let local = mirror_root.join(file.rel.replace('\\', "/"));
    let local_size = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
    // 本地文件不见了（被手工删掉 / 换了缓存目录）→ 不能按 offset 追加，必须整份重下。
    let local_present = local.is_file() && local_size > 0;
    let have = match state {
        Some(state) if local_present && file.size >= state.have && state.have > 0 => state.have,
        _ => 0,
    };

    if have == 0 {
        // 全量下载
        let remote_path = format!("{home}/{}", file.rel);
        let bytes = sftp.read(remote_path).await.map_err(|e| e.to_string())?;
        ensure_parent(&local)?;
        write_atomic(&local, &bytes, file.mtime)?;
        stats.files_downloaded += 1;
        stats.bytes_downloaded += bytes.len() as u64;
        // 只镜像到最后一个完整行，尾部半行留待下次
        let have = last_newline_boundary(&bytes);
        states.insert(
            file.rel.clone(),
            FileState {
                size: file.size,
                mtime: file.mtime,
                have,
            },
        );
    } else {
        // 增量：从 have 处读到文件尾
        let remote_path = format!("{home}/{}", file.rel);
        let tail = read_from(sftp, &remote_path, have).await?;
        if !tail.is_empty() {
            append_complete_lines(&local, &tail, file.mtime)?;
            stats.bytes_downloaded += tail.len() as u64;
        }
        let local_size = std::fs::metadata(&local).map(|m| m.len()).unwrap_or(0);
        states.insert(
            file.rel.clone(),
            FileState {
                size: file.size,
                mtime: file.mtime,
                have: local_size,
            },
        );
    }
    Ok(())
}

async fn read_from(sftp: &SftpSession, remote_path: &str, offset: u64) -> Result<Vec<u8>, String> {
    let mut file = sftp.open(remote_path.to_string()).await.map_err(|e| e.to_string())?;
    file.seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e: std::io::Error| e.to_string())?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)
        .await
        .map_err(|e: std::io::Error| e.to_string())?;
    Ok(buffer)
}

/// 同步一个 SQLite 源的三件套。返回 false 表示本轮该源不可信
/// （主库读失败 / 有变更件读取失败），调用方应跳过它的历史替换。
async fn sync_sqlite_bundle(
    sftp: &SftpSession,
    home: &str,
    mirror_root: &Path,
    states: &mut HashMap<String, FileState>,
    db_rel: &str,
    stats: &mut SyncStats,
) -> bool {
    let parts = [
        db_rel.to_string(),
        format!("{db_rel}-wal"),
        format!("{db_rel}-shm"),
    ];

    // 1) 先取远端三件套属性
    let mut remote_parts: Vec<(String, u64, u32, Vec<u8>)> = Vec::new();
    for (index, rel) in parts.iter().enumerate() {
        let remote = format!("{home}/{rel}");
        let Ok(meta) = sftp.metadata(remote.clone()).await else {
            if index == 0 {
                return true; // 主库不存在 → 未安装该 agent，不算失败
            }
            continue; // side 文件可选
        };
        let size = meta.size.unwrap_or(0);
        let mtime = meta.mtime.unwrap_or(0);
        let local = mirror_root.join(rel.replace('\\', "/"));
        let changed = match states.get(rel) {
            // 本地文件缺失时即使元数据没变也必须重下。
            Some(state) => state.size != size || state.mtime != mtime || !local.is_file(),
            None => true,
        };
        if changed {
            let Ok(bytes) = sftp.read(remote).await else {
                return false; // 读取失败，本轮放弃，保留旧镜像
            };
            stats.files_downloaded += 1;
            stats.bytes_downloaded += bytes.len() as u64;
            remote_parts.push((rel.clone(), size, mtime, bytes));
        } else {
            remote_parts.push((rel.clone(), size, mtime, Vec::new())); // 未变，占位
        }
    }

    // 2) 变更件全部下载成功后：统一落盘（临时文件 + 原子替换）
    for (rel, size, mtime, bytes) in &remote_parts {
        let local = mirror_root.join(rel.replace('\\', "/"));
        if bytes.is_empty() {
            // 远端存在且未变：确保本地残留清理一致性留给 prune 阶段之外
            continue;
        }
        if ensure_parent(&local).is_err() {
            continue;
        }
        let _ = write_tmp(&local, bytes);
        let _ = finalize_tmp(&local, *mtime);
        states.insert(
            rel.clone(),
            FileState {
                size: *size,
                mtime: *mtime,
                have: *size,
            },
        );
    }

    // 3) 远端不存在的 side 文件：清理本地与状态
    for rel in parts.iter().skip(1) {
        let exists_remote = remote_parts.iter().any(|(r, ..)| r == rel);
        if !exists_remote {
            let local = mirror_root.join(rel.replace('\\', "/"));
            let _ = std::fs::remove_file(local);
            states.remove(rel);
        }
    }
    true
}

// ---------------------------------------------------------------------------
// 本地文件工具
// ---------------------------------------------------------------------------

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn last_newline_boundary(bytes: &[u8]) -> u64 {
    match bytes.iter().rposition(|b| *b == b'\n') {
        Some(pos) => (pos + 1) as u64,
        None => bytes.len() as u64,
    }
}

/// 临时写入 + 原子替换 + 设置远端 mtime。
fn write_atomic(path: &Path, bytes: &[u8], remote_mtime: u32) -> Result<(), String> {
    ensure_parent(path)?;
    write_tmp(path, bytes)?;
    finalize_tmp(path, remote_mtime)?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    name.push_str(".tsmp");
    path.with_file_name(name)
}

fn write_tmp(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(tmp_path(path), bytes).map_err(|e| e.to_string())
}

fn finalize_tmp(path: &Path, remote_mtime: u32) -> Result<(), String> {
    let tmp = tmp_path(path);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    if remote_mtime > 0 {
        if let Some(time) = std::time::SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(remote_mtime as u64))
        {
            if let Ok(file) = std::fs::File::options().write(true).open(path) {
                let _ = file.set_modified(time);
            }
        }
    }
    Ok(())
}

fn append_complete_lines(path: &Path, tail: &[u8], remote_mtime: u32) -> Result<(), String> {
    ensure_parent(path)?;
    let boundary = match tail.iter().rposition(|b| *b == b'\n') {
        Some(pos) => pos + 1,
        None => return Ok(()), // 尾部还没有完整行，留待下次
    };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&tail[..boundary]).map_err(|e| e.to_string())?;
    drop(file);
    // 必须把本地 mtime 拉回远端 mtime：增量追加后 mtime 会变成"现在"，
    // 看起来无害，但下次若按 mtime 判定"文件是否可能含窗口内记录"就会永远为真，
    // 增量窗口退化成全量重读。反过来（不更新）则会把仍在追加的文件误判成旧文件。
    if remote_mtime > 0 {
        if let Some(time) = std::time::SystemTime::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(remote_mtime as u64))
        {
            if let Ok(file) = std::fs::File::options().write(true).open(path) {
                let _ = file.set_modified(time);
            }
        }
    }
    Ok(())
}

/// 清理镜像中远端已删除的 jsonl。
fn prune_local(mirror_root: &Path, base: &str, seen: &std::collections::HashSet<String>) {
    let local_base = mirror_root.join(base);
    if !local_base.is_dir() {
        return;
    }
    let mut locals = Vec::new();
    collect_local_jsonl(&local_base, &mut locals);
    for local in locals {
        let rel = match local.strip_prefix(mirror_root) {
            Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };
        if !seen.contains(&rel) {
            let _ = std::fs::remove_file(&local);
        }
    }
}

fn collect_local_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_local_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;

    #[test]
    fn newline_boundary() {
        assert_eq!(last_newline_boundary(b"ab\ncd\n"), 6);
        assert_eq!(last_newline_boundary(b"ab\ncd"), 3); // 半行不取
        assert_eq!(last_newline_boundary(b"abc"), 3); // 无换行：整段（只有首行时保留）
    }

    #[test]
    fn epoch_is_created_then_reused() {
        let dir = TempDir::new();
        let (first, present) = read_or_create_epoch(dir.path());
        assert!(!present, "首次创建时标记原本不存在");
        assert!(dir.path().join(EPOCH_FILE).is_file());

        let (second, present) = read_or_create_epoch(dir.path());
        assert!(present);
        assert_eq!(first, second, "标记存在时必须复用同一个 epoch");
    }

    #[test]
    fn broken_epoch_marker_is_treated_as_missing() {
        let dir = TempDir::new();
        std::fs::write(dir.path().join(EPOCH_FILE), "{ broken").unwrap();
        let (_, present) = read_or_create_epoch(dir.path());
        assert!(!present, "损坏的标记等同于缓存被清空");
        let text = std::fs::read_to_string(dir.path().join(EPOCH_FILE)).unwrap();
        assert!(text.contains("epoch"));
    }

    #[test]
    fn append_updates_mtime_to_remote() {
        let dir = TempDir::new();
        let path = dir.path().join("a.jsonl");
        // 远端 mtime：2020-09-13 12:26:40 UTC
        let remote_mtime = 1_600_000_000u32;
        append_complete_lines(&path, b"line1\nline2\n", remote_mtime).unwrap();
        let modified = std::fs::metadata(&path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(
            modified, remote_mtime as u64,
            "增量追加后必须把本地 mtime 拉回远端 mtime，否则窗口判定会退化"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "line1\nline2\n");
    }

    #[test]
    fn append_skips_partial_tail() {
        let dir = TempDir::new();
        let path = dir.path().join("b.jsonl");
        append_complete_lines(&path, b"complete\npartial", 1_600_000_000).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "complete\n");
    }

    /// Linux 扫描计划 ↔ 同步规格双向守卫。
    /// 正向：计划里的源必须有规格，否则远端镜像永远缺它（detected=false，静默漏采）。
    /// 反向：有规格的源必须在计划里，否则文件白下载、解析层根本不读。
    #[test]
    fn linux_plan_and_sync_specs_cover_each_other() {
        use crate::parsing::token::LINUX_SCAN_PLAN;

        let mut synced: Vec<Source> = LINUX_JSONL_BASES
            .iter()
            .map(|spec| spec.source)
            .chain(LINUX_SQLITE_DBS.iter().map(|spec| spec.source))
            .chain(LINUX_PROBE_SPECS.iter().map(|spec| spec.source))
            .collect();
        synced.sort();
        synced.dedup();

        for source in LINUX_SCAN_PLAN {
            assert!(
                synced.contains(source),
                "Linux 计划里的 {source:?} 没有任何同步规格：远端镜像不会出现它"
            );
        }
        for source in &synced {
            assert!(
                LINUX_SCAN_PLAN.contains(source),
                "{source:?} 有同步规格但不在 LINUX_SCAN_PLAN：镜像白下载、解析层不会读"
            );
        }

        // WorkBuddy 必须是 JSONL 镜像（有真实 usage），且只镜像 projects/ 子目录
        let spec = LINUX_JSONL_BASES
            .iter()
            .find(|spec| spec.source == Source::WorkBuddy)
            .expect("WorkBuddy 必须出现在 LINUX_JSONL_BASES");
        assert_eq!(spec.base, ".workbuddy/projects");
        assert!(!spec.qwen_chats_only);
    }
}
