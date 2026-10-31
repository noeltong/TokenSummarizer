//! 可选的 HKCU Run 自启动（不需要管理员权限）。

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE,
};

const VALUE_NAME: PCWSTR = w!("TokenSummarizer");

fn run_subkey() -> Vec<u16> {
    "Software\\Microsoft\\Windows\\CurrentVersion\\Run"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    unsafe {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let command = format!("\"{}\"", exe.display());
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = to_byte_slice(&wide);
            with_key(KEY_SET_VALUE, |key| {
                check(
                    RegSetValueExW(
                        key,
                        VALUE_NAME,
                        None,
                        REG_VALUE_TYPE(REG_SZ.0),
                        Some(bytes),
                    ),
                    "写入自启动项失败",
                )
            })
        } else {
            with_key(KEY_QUERY_VALUE, |key| {
                // 值不存在时删除也算成功（幂等）
                let _ = RegDeleteValueW(key, VALUE_NAME);
                Ok(())
            })
        }
    }
}

pub fn is_enabled() -> bool {
    match (query_value(), std::env::current_exe().ok()) {
        (Some(text), Some(exe)) => {
            let plain = text.replace('"', "");
            plain.eq_ignore_ascii_case(&exe.to_string_lossy())
        }
        _ => false,
    }
}

fn query_value() -> Option<String> {
    unsafe {
        let mut key = HKEY::default();
        check(
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(run_subkey().as_ptr()),
                None,
                KEY_QUERY_VALUE,
                &mut key,
            ),
            "",
        )
        .ok()?;
        let mut buffer = [0u8; 1024];
        let mut size = buffer.len() as u32;
        let result = RegQueryValueExW(
            key,
            VALUE_NAME,
            None,
            None,
            Some(buffer.as_mut_ptr()),
            Some(&mut size),
        );
        let _ = RegCloseKey(key);
        if result != ERROR_SUCCESS || size < 2 {
            return None;
        }
        let wide: Vec<u16> = buffer[..size as usize]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        Some(
            String::from_utf16_lossy(&wide)
                .trim_end_matches('\0')
                .to_string(),
        )
    }
}

unsafe fn with_key(access: REG_SAM_FLAGS, body: impl FnOnce(HKEY) -> Result<(), String>) -> Result<(), String> {
    let mut key = HKEY::default();
    check(
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(run_subkey().as_ptr()),
            None,
            access,
            &mut key,
        ),
        "打开注册表 Run 项失败",
    )?;
    let result = body(key);
    let _ = RegCloseKey(key);
    result
}

fn check(code: WIN32_ERROR, context: &str) -> Result<(), String> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else if context.is_empty() {
        Err(format!("注册表操作失败（错误码 {}）", code.0))
    } else {
        Err(format!("{context}（错误码 {}）", code.0))
    }
}

unsafe fn to_byte_slice(wide: &[u16]) -> &[u8] {
    std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2)
}
