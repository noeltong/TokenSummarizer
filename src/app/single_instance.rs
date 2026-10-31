//! 单实例：命名互斥体；重复启动时唤起已有实例后退出。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};

use super::WM_APP_SHOW;

pub enum SingleInstanceResult {
    /// 本进程是第一个实例；结构体存活期间互斥体保持占用。
    Primary(SingleInstance),
    /// 已有实例运行，已向其发送显示消息。
    AlreadyRunning,
}

pub struct SingleInstance {
    _mutex: HANDLE,
}

pub fn acquire(window_class: &str) -> SingleInstanceResult {
    unsafe {
        let name: Vec<u16> = "Local\\TokenSummarizerSingleInstance"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let Ok(mutex) = CreateMutexW(None, false, PCWSTR(name.as_ptr())) else {
            // 无法创建互斥体时放行，避免完全无法启动
            return SingleInstanceResult::Primary(SingleInstance { _mutex: HANDLE::default() });
        };
        if GetLastError().0 == windows::Win32::Foundation::ERROR_ALREADY_EXISTS.0 {
            let _ = CloseHandle(mutex);
            raise_existing(window_class);
            return SingleInstanceResult::AlreadyRunning;
        }

        SingleInstanceResult::Primary(SingleInstance { _mutex: mutex })
    }
}

fn raise_existing(window_class: &str) {
    unsafe {
        let class: Vec<u16> = window_class.encode_utf16().chain(std::iter::once(0)).collect();
        if let Ok(hwnd) = FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()) {
            let _ = PostMessageW(Some(hwnd), WM_APP_SHOW, WPARAM(0), LPARAM(0));
        }
    }
}

impl SingleInstance {
    /// 让出互斥体（用于测试）。
    pub fn release(self) {
        unsafe {
            if !self._mutex.is_invalid() {
                let _ = CloseHandle(self._mutex);
            }
        }
    }
}
