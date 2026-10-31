//! WSL 发现：枚举发行版、定位 Linux home、缓存与失败退避。
//!
//! **禁止调用 `wsl.exe`**：本机安全策略会直接拦掉它，而且进程调用比读注册表
//! 慢一个数量级。发行版列表从 `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss`
//! 读；home 目录通过 `\\wsl$\<distro>\...` 直接访问（WSL 未运行时会自动拉起，
//! 也可能失败 —— 失败只降级为告警）。
//!
//! WSL 里的 agent 数据与 Linux 完全同构，因此扫描规则复用 `LINUX_SCAN_PLAN`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 注册表重新枚举的间隔：装/删发行版不需要秒级感知。
const DISCOVERY_TTL: Duration = Duration::from_secs(600);
/// 连续失败达到该次数后进入退避。
const FAILURE_THRESHOLD: u32 = 2;
const FAILURE_BACKOFF: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone)]
pub struct WslDistro {
    pub name: String,
    /// 注册表里的 `BasePath`（`...\LocalState`），仅诊断用，**不是 home**。
    pub base_path: Option<String>,
    pub default_uid: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct WslHome {
    pub distro: String,
    pub user: String,
    /// Linux 侧路径，如 `/home/noeltong`。
    pub unix_home: String,
    /// `\\wsl$\<distro>\home\<user>`。
    pub root: PathBuf,
}

impl WslHome {
    pub fn label(&self) -> String {
        format!("WSL {} · {}", self.distro, self.user)
    }
}

/// 枚举已安装的 WSL 发行版。任何一步失败都返回 Err（调用方降级为告警）。
pub fn enumerate_distros() -> Result<Vec<WslDistro>, String> {
    imp::enumerate()
}

/// 解析 `/etc/passwd`：按 uid 找 (用户名, home)。
pub fn parse_passwd_home(passwd: &str, uid: u32) -> Option<(String, String)> {
    for line in passwd.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() < 7 {
            continue;
        }
        if fields[2].trim().parse::<u32>().ok() != Some(uid) {
            continue;
        }
        let home = fields[5].trim();
        if home.is_empty() {
            return None;
        }
        return Some((fields[0].to_string(), home.to_string()));
    }
    None
}

/// 解析 `/etc/wsl.conf` 的 `[user] default=<name>`。
pub fn parse_wsl_conf_default_user(text: &str) -> Option<String> {
    let mut in_user_section = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_user_section = line[1..line.len() - 1].trim().eq_ignore_ascii_case("user");
            continue;
        }
        if !in_user_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("default") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// 把 Linux 绝对路径转成发行版根下的 Windows 路径。
/// `/mnt/...` 是 Windows 盘挂载点，扫它等于把本机数据算两遍，直接拒绝。
pub fn unix_home_to_path(prefix: &Path, unix_home: &str) -> Option<PathBuf> {
    let trimmed = unix_home.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    if trimmed == "/" || trimmed.starts_with("/mnt/") || trimmed == "/mnt" {
        return None;
    }
    let mut path = prefix.to_path_buf();
    for part in trimmed.trim_start_matches('/').split('/') {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        path.push(part);
    }
    Some(path)
}

/// 从 `/etc/passwd` 找某个用户名对应的 home。
fn passwd_home_of_user(passwd: &str, user: &str) -> Option<(String, String)> {
    for line in passwd.lines() {
        let fields: Vec<&str> = line.trim().split(':').collect();
        if fields.len() < 7 || fields[0] != user {
            continue;
        }
        let home = fields[5].trim();
        if !home.is_empty() {
            return Some((fields[0].to_string(), home.to_string()));
        }
    }
    None
}

/// 在发行版根下解析 home（顺序见模块文档）。
fn resolve_homes(prefix: &Path, distro: &str, default_uid: Option<u32>) -> Vec<WslHome> {
    let passwd = std::fs::read_to_string(prefix.join("etc").join("passwd")).unwrap_or_default();
    let mut found: Option<(String, String)> = None;

    if let Some(uid) = default_uid {
        found = parse_passwd_home(&passwd, uid);
    }
    if found.is_none() {
        if let Ok(conf) = std::fs::read_to_string(prefix.join("etc").join("wsl.conf")) {
            if let Some(user) = parse_wsl_conf_default_user(&conf) {
                found = passwd_home_of_user(&passwd, &user);
            }
        }
    }
    if found.is_none() {
        // 退化：枚举 /home/*，优先与 Windows 用户名同名
        let mut candidates: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(prefix.join("home")) {
            for entry in entries.flatten() {
                if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                    if let Some(name) = entry.file_name().to_str() {
                        candidates.push(name.to_string());
                    }
                }
            }
        }
        if !candidates.is_empty() {
            candidates.sort();
            let windows_user = std::env::var("USERNAME").unwrap_or_default();
            let chosen = candidates
                .iter()
                .find(|name| name.eq_ignore_ascii_case(&windows_user))
                .cloned()
                .unwrap_or_else(|| candidates[0].clone());
            found = Some((chosen.clone(), format!("/home/{chosen}")));
        } else if prefix.join("root").is_dir() {
            found = Some(("root".to_string(), "/root".to_string()));
        }
    }

    let Some((user, unix_home)) = found else {
        return Vec::new();
    };
    let Some(root) = unix_home_to_path(prefix, &unix_home) else {
        return Vec::new();
    };
    vec![WslHome {
        distro: distro.to_string(),
        user,
        unix_home,
        root,
    }]
}

/// 发行版根：先试 `\\wsl$`，再试 `\\wsl.localhost`。
fn distro_prefix(distro: &str) -> Option<PathBuf> {
    for base in ["\\\\wsl$", "\\\\wsl.localhost"] {
        let path = PathBuf::from(base).join(distro);
        if path.is_dir() {
            return Some(path);
        }
    }
    None
}

/// 发行版列表 + home 的发现缓存。
pub struct WslCache {
    last_refresh: Option<Instant>,
    distros: Vec<WslDistro>,
    homes: Vec<WslHome>,
    failures: HashMap<String, (u32, Instant)>,
    last_error: Option<String>,
}

impl Default for WslCache {
    fn default() -> Self {
        Self::new()
    }
}

impl WslCache {
    pub fn new() -> Self {
        Self {
            last_refresh: None,
            distros: Vec::new(),
            homes: Vec::new(),
            failures: HashMap::new(),
            last_error: None,
        }
    }

    /// 重新枚举（TTL 内且非强制时直接返回缓存）。
    pub fn refresh(&mut self, force: bool) {
        if !force {
            if let Some(at) = self.last_refresh {
                if at.elapsed() < DISCOVERY_TTL {
                    return;
                }
            }
        }
        self.last_refresh = Some(Instant::now());
        match enumerate_distros() {
            Ok(distros) => {
                let mut homes = Vec::new();
                for distro in &distros {
                    let Some(prefix) = distro_prefix(&distro.name) else {
                        crate::log_info!("WSL {} 当前不可访问（跳过）", distro.name);
                        continue;
                    };
                    homes.extend(resolve_homes(&prefix, &distro.name, distro.default_uid));
                }
                self.homes = homes;
                self.distros = distros;
                self.last_error = None;
            }
            Err(err) => {
                crate::log_error!("WSL 枚举失败: {err}");
                self.last_error = Some(err);
                self.distros.clear();
                self.homes.clear();
            }
        }
    }

    pub fn homes(&self) -> &[WslHome] {
        &self.homes
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn distro_names(&self) -> Vec<String> {
        self.distros.iter().map(|d| d.name.clone()).collect()
    }

    /// 该发行版是否处于退避期（连续失败太多次）。
    pub fn in_backoff(&self, distro: &str) -> bool {
        match self.failures.get(distro) {
            Some((count, until)) => *count >= FAILURE_THRESHOLD && until.elapsed() < FAILURE_BACKOFF,
            None => false,
        }
    }

    pub fn note_success(&mut self, distro: &str) {
        self.failures.remove(distro);
    }

    pub fn note_failure(&mut self, distro: &str) {
        let entry = self.failures.entry(distro.to_string()).or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();
    }
}

#[cfg(windows)]
mod imp {
    use super::WslDistro;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER,
        KEY_READ, KEY_WOW64_64KEY, REG_DWORD, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };

    const LXSS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Lxss";

    pub fn enumerate() -> Result<Vec<WslDistro>, String> {
        unsafe {
            let mut root = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(wide(LXSS).as_ptr()),
                None,
                KEY_READ | KEY_WOW64_64KEY,
                &mut root,
            ) != ERROR_SUCCESS
            {
                // 没装 WSL：注册表项不存在是正常情况
                return Ok(Vec::new());
            }

            let mut distros = Vec::new();
            let mut index = 0u32;
            loop {
                let mut name = [0u16; 256];
                let mut length = name.len() as u32;
                let status = RegEnumKeyExW(
                    root,
                    index,
                    Some(windows::core::PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                );
                if status == ERROR_NO_MORE_ITEMS {
                    break;
                }
                if status != ERROR_SUCCESS {
                    break;
                }
                index += 1;
                let subkey = String::from_utf16_lossy(&name[..length as usize]);
                if let Some(distro) = read_distro(root, &subkey) {
                    distros.push(distro);
                }
            }
            let _ = RegCloseKey(root);
            distros.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(distros)
        }
    }

    fn read_distro(root: HKEY, subkey: &str) -> Option<WslDistro> {
        unsafe {
            let mut key = HKEY::default();
            if RegOpenKeyExW(
                root,
                PCWSTR(wide(subkey).as_ptr()),
                None,
                KEY_READ,
                &mut key,
            ) != ERROR_SUCCESS
            {
                return None;
            }
            let name = read_string(key, "DistributionName");
            let base_path = read_string(key, "BasePath");
            let default_uid = read_dword(key, "DefaultUid");
            let _ = RegCloseKey(key);
            let name = name.filter(|name| !name.trim().is_empty())?;
            Some(WslDistro {
                name,
                base_path,
                default_uid,
            })
        }
    }

    /// 读 REG_SZ / REG_EXPAND_SZ。
    fn read_string(key: HKEY, value: &str) -> Option<String> {
        unsafe {
            for kind in [REG_SZ, REG_EXPAND_SZ] {
                let mut buffer = [0u8; 2048];
                let mut size = buffer.len() as u32;
                let status = RegQueryValueExW(
                    key,
                    PCWSTR(wide(value).as_ptr()),
                    None,
                    Some(&mut REG_VALUE_TYPE(kind.0)),
                    Some(buffer.as_mut_ptr()),
                    Some(&mut size),
                );
                if status != ERROR_SUCCESS || size < 4 {
                    continue;
                }
                let units = size as usize / 2;
                let slice =
                    std::slice::from_raw_parts(buffer.as_ptr() as *const u16, units.min(1024));
                let text = String::from_utf16_lossy(slice)
                    .trim_end_matches('\0')
                    .to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
            None
        }
    }

    fn read_dword(key: HKEY, value: &str) -> Option<u32> {
        unsafe {
            let mut data = 0u32;
            let mut size = std::mem::size_of::<u32>() as u32;
            let status = RegQueryValueExW(
                key,
                PCWSTR(wide(value).as_ptr()),
                None,
                Some(&mut REG_VALUE_TYPE(REG_DWORD.0)),
                Some(&mut data as *mut u32 as *mut u8),
                Some(&mut size),
            );
            (status == ERROR_SUCCESS).then_some(data)
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::WslDistro;

    pub fn enumerate() -> Result<Vec<WslDistro>, String> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testutil::TempDir;
    use std::fs;

    #[test]
    fn passwd_lookup_by_uid() {
        let text = "root:x:0:0:root:/root:/bin/bash\nnoel:x:1000:1000::/home/noel:/bin/bash\n";
        assert_eq!(
            parse_passwd_home(text, 1000),
            Some(("noel".to_string(), "/home/noel".to_string()))
        );
        assert_eq!(parse_passwd_home(text, 0).unwrap().1, "/root");
        assert!(parse_passwd_home(text, 4242).is_none());
    }

    #[test]
    fn wsl_conf_default_user() {
        let text = "# comment\n[boot]\nsystemd=true\n\n[user]\ndefault = noel\n";
        assert_eq!(parse_wsl_conf_default_user(text), Some("noel".to_string()));
        let text = "[boot]\nsystemd=true\n";
        assert_eq!(parse_wsl_conf_default_user(text), None);
        // 引号包裹也要认
        assert_eq!(
            parse_wsl_conf_default_user("[user]\ndefault=\"noel\"\n"),
            Some("noel".to_string())
        );
    }

    #[test]
    fn rejects_mnt_and_root_homes() {
        let prefix = Path::new("\\\\wsl$\\Ubuntu");
        assert!(unix_home_to_path(prefix, "/mnt/c/Users/x").is_none());
        assert!(unix_home_to_path(prefix, "/").is_none());
        assert!(unix_home_to_path(prefix, "relative/home").is_none());
        assert_eq!(
            unix_home_to_path(prefix, "/home/noel").unwrap(),
            Path::new("\\\\wsl$\\Ubuntu\\home\\noel")
        );
    }

    #[test]
    fn resolves_home_from_passwd_then_wsl_conf_then_listing() {
        let temp = TempDir::new();
        let prefix = temp.path();
        fs::create_dir_all(prefix.join("etc")).unwrap();
        fs::create_dir_all(prefix.join("home/alice")).unwrap();
        fs::create_dir_all(prefix.join("home/bob")).unwrap();
        fs::write(
            prefix.join("etc/passwd"),
            "root:x:0:0::/root:/bin/sh\nalice:x:1000:1000::/home/alice:/bin/sh\nbob:x:1001:1001::/home/bob:/bin/sh\n",
        )
        .unwrap();

        // 1) DefaultUid 命中
        let homes = resolve_homes(prefix, "Ubuntu", Some(1001));
        assert_eq!(homes.len(), 1);
        assert_eq!(homes[0].user, "bob");
        assert_eq!(homes[0].unix_home, "/home/bob");

        // 2) wsl.conf 指定 default 用户
        fs::write(prefix.join("etc/wsl.conf"), "[user]\ndefault=alice\n").unwrap();
        let homes = resolve_homes(prefix, "Ubuntu", None);
        assert_eq!(homes[0].user, "alice");

        // 3) 都没有 → 枚举 /home/*（排序后取第一个）
        fs::remove_file(prefix.join("etc/wsl.conf")).unwrap();
        fs::remove_file(prefix.join("etc/passwd")).unwrap();
        let homes = resolve_homes(prefix, "Ubuntu", None);
        assert_eq!(homes.len(), 1);
        assert!(homes[0].unix_home == "/home/alice" || homes[0].unix_home == "/home/bob");
    }

    #[test]
    fn falls_back_to_root_when_no_home_user() {
        let temp = TempDir::new();
        let prefix = temp.path();
        fs::create_dir_all(prefix.join("root")).unwrap();
        let homes = resolve_homes(prefix, "Debian", None);
        assert_eq!(homes.len(), 1);
        assert_eq!(homes[0].unix_home, "/root");
    }

    #[test]
    fn backoff_after_repeated_failures() {
        let mut cache = WslCache::new();
        assert!(!cache.in_backoff("Ubuntu"));
        cache.note_failure("Ubuntu");
        assert!(!cache.in_backoff("Ubuntu"), "单次失败不进入退避");
        cache.note_failure("Ubuntu");
        assert!(cache.in_backoff("Ubuntu"));
        cache.note_success("Ubuntu");
        assert!(!cache.in_backoff("Ubuntu"), "成功后立即解除退避");
    }
}
