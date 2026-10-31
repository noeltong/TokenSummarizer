//! Windows DPAPI（CurrentUser 作用域）加解密。
//! 凭据只能在原 Windows 用户环境下解密；换用户或换机器后需重新输入。

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HLOCAL;
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

/// 加密为 base64 字符串；失败返回 None（UI 层提示保存失败）。
pub fn protect(plain: &str) -> Option<String> {
    let bytes = plain.as_bytes();
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let result = CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        let Ok(()) = result else {
            return None;
        };
        let encoded = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let text = BASE64.encode(encoded);
        free_blob(output);
        Some(text)
    }
}

/// 解密 base64 密文；失败（密文损坏 / 非本机本用户）返回 None。
pub fn unprotect(encoded: &str) -> Option<String> {
    let bytes = BASE64.decode(encoded.trim()).ok()?;
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        let result = CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        let Ok(()) = result else {
            return None;
        };
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let text = String::from_utf8_lossy(plain).into_owned();
        free_blob(output);
        Some(text)
    }
}

unsafe fn free_blob(blob: CRYPT_INTEGER_BLOB) {
    if !blob.pbData.is_null() {
        let _ = windows::Win32::Foundation::LocalFree(Some(HLOCAL(blob.pbData as _)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_unprotect_roundtrip() {
        let cipher = protect("s3cret-密码").expect("DPAPI 加密应成功");
        assert_ne!(cipher, "s3cret-密码");
        assert_eq!(unprotect(&cipher).as_deref(), Some("s3cret-密码"));
    }

    #[test]
    fn unprotect_garbage_returns_none() {
        assert!(unprotect("not-valid-base64!!!").is_none());
        assert!(unprotect("aGVsbG8=").is_none());
    }
}
