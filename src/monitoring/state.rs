//! 跨线程共享状态：后台采集任务写，UI 线程读；配置变更原子持久化。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use uuid::Uuid;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app::WM_APP_UPDATE;
use crate::config::AppConfig;
use crate::model::ServerData;
use crate::store::TokenStore;

pub struct AppState {
    pub cfg: RwLock<AppConfig>,
    pub servers: RwLock<Vec<ServerData>>,
    /// 本机（非 SSH）coding agent 的 token 快照："本机" 选项卡与合计用。
    pub local: RwLock<Option<LocalSnapshot>>,
    /// 本地历史库（平台 × agent × 模型 × 日期）。所有采集循环共用。
    pub store: Arc<TokenStore>,
    hwnd: AtomicUsize,
    save_lock: Mutex<()>,
}

/// 本机扫描结果。
#[derive(Debug, Clone)]
pub struct LocalSnapshot {
    pub token: crate::model::TokenSnapshot,
    pub at: chrono::DateTime<chrono::Utc>,
}

impl AppState {
    pub fn new(cfg: AppConfig, store: Arc<TokenStore>) -> Self {
        let servers = cfg.servers.iter().map(|s| ServerData::new(s.id)).collect();
        Self {
            cfg: RwLock::new(cfg),
            servers: RwLock::new(servers),
            local: RwLock::new(None),
            store,
            hwnd: AtomicUsize::new(0),
            save_lock: Mutex::new(()),
        }
    }

    pub fn set_local_data(&self, snapshot: crate::model::TokenSnapshot) {
        self.set_local_snapshot(snapshot, chrono::Utc::now());
    }

    /// 带时间戳发布本机快照：从历史库恢复时用"上次成功时间"，
    /// 刚扫完时用当前时间。
    pub fn set_local_snapshot(
        &self,
        snapshot: crate::model::TokenSnapshot,
        at: chrono::DateTime<chrono::Utc>,
    ) {
        if let Ok(mut local) = self.local.write() {
            *local = Some(LocalSnapshot {
                token: snapshot,
                at,
            });
        }
    }

    pub fn local_data(&self) -> Option<LocalSnapshot> {
        self.local.read().ok().and_then(|local| local.clone())
    }

    pub fn set_hwnd(&self, hwnd: HWND) {
        self.hwnd.store(hwnd.0 as usize, Ordering::SeqCst);
    }

    fn hwnd(&self) -> Option<HWND> {
        let value = self.hwnd.load(Ordering::SeqCst);
        if value == 0 {
            None
        } else {
            Some(HWND(value as *mut _))
        }
    }

    /// 后台线程安全地请求 UI 重绘。
    pub fn post_update(&self) {
        if let Some(hwnd) = self.hwnd() {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_UPDATE, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn update_server(&self, id: Uuid, mutate: impl FnOnce(&mut ServerData)) {
        if let Ok(mut servers) = self.servers.write() {
            if let Some(server) = servers.iter_mut().find(|server| server.id == id) {
                mutate(server);
            }
        }
    }

    pub fn server_config(&self, id: Uuid) -> Option<crate::config::ServerConfig> {
        self.cfg
            .read()
            .ok()?
            .servers
            .iter()
            .find(|server| server.id == id)
            .cloned()
    }

    /// 整体替换配置（设置保存 / 服务器增删改）：更新内存 + 原子写盘。
    /// ServerData 按 id 保留已有数据，新服务器初始化为空。
    pub fn replace_config(&self, new_cfg: AppConfig) {
        let _guard = self.save_lock.lock().ok();
        if let Ok(mut cfg) = self.cfg.write() {
            *cfg = new_cfg.clone();
        }
        if let Ok(mut servers) = self.servers.write() {
            servers.retain(|server| new_cfg.servers.iter().any(|s| s.id == server.id));
            for server_cfg in &new_cfg.servers {
                if !servers.iter().any(|server| server.id == server_cfg.id) {
                    servers.push(ServerData::new(server_cfg.id));
                }
            }
            servers.sort_by_key(|server| {
                new_cfg
                    .servers
                    .iter()
                    .position(|s| s.id == server.id)
                    .unwrap_or(usize::MAX)
            });
        }
        let path = crate::util::paths::config_path();
        if let Err(err) = crate::config::save(&path, &new_cfg) {
            crate::log_error!("配置保存失败: {err}");
        }
    }

    /// 首次连接后持久化主机密钥指纹（TOFU）。
    pub fn set_host_fingerprint(&self, id: Uuid, fingerprint: String) {
        let updated = {
            let Ok(mut cfg) = self.cfg.write() else {
                return;
            };
            let Some(server) = cfg.servers.iter_mut().find(|server| server.id == id) else {
                return;
            };
            if server.host_key_fingerprint_sha256.as_deref() == Some(fingerprint.as_str()) {
                None
            } else {
                server.host_key_fingerprint_sha256 = Some(fingerprint);
                Some(cfg.clone())
            }
        };
        if let Some(cfg) = updated {
            let _guard = self.save_lock.lock().ok();
            let path = crate::util::paths::config_path();
            if let Err(err) = crate::config::save(&path, &cfg) {
                crate::log_error!("主机密钥指纹保存失败: {err}");
            }
        }
    }

    /// 清空所有服务器的镜像缓存（设置中"清空缓存"）。
    pub fn clear_cache() -> std::io::Result<()> {
        let dir = crate::util::paths::cache_dir();
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        std::fs::create_dir_all(&dir)
    }
}
