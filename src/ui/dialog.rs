//! 无边框对话框的公共设施：标题栏拖动/关闭、原生子控件（EDIT/BUTTON/
//! CHECKBOX/RADIO）创建与读写、控件字体。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::HMENU;
use windows::Win32::Graphics::Gdi::{
    CreateFontW, DeleteObject, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
    OUT_DEFAULT_PRECIS, HFONT,
};
use windows::Win32::UI::Controls::BST_CHECKED;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, GetDlgItem, SendMessageW, BM_GETCHECK, WM_GETTEXT, WM_GETTEXTLENGTH,
    WM_SETFONT, WM_SETTEXT,
};

/// 控件字体（微软雅黑，按 DPI 缩放；对话框销毁时删除）。
pub struct ControlFont(pub HFONT);

impl ControlFont {
    pub fn create(dpi: u32) -> Option<Self> {
        let height = -((12.0 * dpi as f64 / 96.0).round() as i32);
        unsafe {
            let face: Vec<u16> = "Microsoft YaHei UI"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let font = CreateFontW(
                height,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                PCWSTR(face.as_ptr()),
            );
            if font.is_invalid() {
                None
            } else {
                Some(ControlFont(font))
            }
        }
    }

    pub fn apply(&self, hwnd: HWND) {
        unsafe {
            let _ = SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(self.0 .0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
}

impl Drop for ControlFont {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

/// 创建原生子控件；`text` 为初始内容。`id` 经 hMenu 传入——对子控件而言
/// hMenu 就是控件 ID（WM_COMMAND 识别与 GetDlgItem 都依赖它）。
#[allow(clippy::too_many_arguments)]
pub fn create_child(
    parent: HWND,
    class: &str,
    text: &str,
    ex_style: windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE,
    style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    id: u16,
) -> Option<HWND> {
    let class_wide: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
    let text_wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        CreateWindowExW(
            ex_style,
            PCWSTR(class_wide.as_ptr()),
            PCWSTR(text_wide.as_ptr()),
            style,
            x,
            y,
            w,
            h,
            Some(parent),
            Some(HMENU(id as *mut core::ffi::c_void)),
            None,
            None,
        )
        .ok()
    }
}

pub fn get_text(control: HWND) -> String {
    unsafe {
        let length = SendMessageW(control, WM_GETTEXTLENGTH, None, None).0 as usize;
        if length == 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length + 1];
        let copied = SendMessageW(
            control,
            WM_GETTEXT,
            Some(WPARAM(buffer.len())),
            Some(LPARAM(buffer.as_mut_ptr() as isize)),
        )
        .0 as usize;
        let len = copied.min(length);
        String::from_utf16_lossy(&buffer[..len])
    }
}

pub fn set_text(control: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = SendMessageW(control, WM_SETTEXT, None, Some(LPARAM(wide.as_ptr() as isize)));
    }
}

pub fn is_checked(parent: HWND, id: i32) -> bool {
    unsafe {
        match GetDlgItem(Some(parent), id) {
            Ok(control) => (SendMessageW(control, BM_GETCHECK, None, None).0 as u32)
                == BST_CHECKED.0 as u32,
            Err(_) => false,
        }
    }
}

pub fn check(control: HWND, checked: bool) {
    unsafe {
        use windows::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED};
        use windows::Win32::UI::WindowsAndMessaging::BM_SETCHECK;
        let state = if checked {
            BST_CHECKED.0 as usize
        } else {
            BST_UNCHECKED.0 as usize
        };
        let _ = SendMessageW(control, BM_SETCHECK, Some(WPARAM(state)), None);
    }
}

pub fn enable(control: HWND, on: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
    unsafe {
        let _ = EnableWindow(control, on);
    }
}

/// 读取对话框内指定 ID 的控件文本。
pub fn get_item_text(parent: HWND, id: i32) -> String {
    unsafe {
        match GetDlgItem(Some(parent), id) {
            Ok(control) => get_text(control),
            Err(_) => String::new(),
        }
    }
}

/// 设置对话框内指定 ID 的控件文本。
pub fn set_item_text(parent: HWND, id: i32, text: &str) {
    unsafe {
        if let Ok(control) = GetDlgItem(Some(parent), id) {
            set_text(control, text);
        }
    }
}

/// 获取指定 ID 的控件句柄（不存在返回 None）。
pub fn item(parent: HWND, id: i32) -> Option<HWND> {
    unsafe { GetDlgItem(Some(parent), id).ok() }
}
