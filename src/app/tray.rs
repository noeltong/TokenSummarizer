//! 系统托盘：优先加载 exe 内嵌的 app.ico 资源（多尺寸，Shell 按需取用），
//! 失败回退运行时 GDI+ 绘制。左键消息由窗口过程分发；右键菜单
//! （打开面板/立即刷新/设置/退出）。

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, DestroyIcon, DestroyMenu, GetCursorPos,
    GetSystemMetrics, LoadImageW, PostMessageW, SetForegroundWindow, TrackPopupMenu, HICON,
    IMAGE_ICON, LR_DEFAULTCOLOR, MF_SEPARATOR, MF_STRING, SM_CXICON, SM_CXSMICON,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use super::WM_APP_TRAY;

/// winresource 嵌入的第一个图标资源 ID。
pub const APP_ICON_RESOURCE_ID: usize = 1;

pub const TRAY_ID: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    OpenPanel,
    RefreshNow,
    Settings,
    Exit,
}

pub struct Tray {
    hwnd: HWND,
    data: NOTIFYICONDATAW,
    icon: HICON,
}

impl Tray {
    pub fn create(hwnd: HWND) -> Option<Self> {
        let small = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
        let icon = load_app_icon(small).or_else(|| create_app_icon(small))?;
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: icon,
            ..Default::default()
        };
        set_tip(&mut data, crate::i18n::texts().app_title);
        unsafe {
            if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
                let _ = DestroyIcon(icon);
                return None;
            }
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
        Some(Tray { hwnd, data, icon })
    }

    /// 语言切换后刷新 tooltip（NIM_MODIFY）。
    pub fn set_tip_text(&mut self, text: &str) {
        set_tip(&mut self.data, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &self.data);
        }
    }

    /// 显示右键菜单并返回所选命令；点击菜单外返回 None。
    pub fn show_menu(&self) -> Option<TrayCommand> {
        unsafe {
            let menu = CreatePopupMenu().ok()?;
            let texts = crate::i18n::texts();
            let open: Vec<u16> = crate::i18n::to_wide(texts.menu_open);
            let refresh: Vec<u16> = crate::i18n::to_wide(texts.menu_refresh);
            let settings: Vec<u16> = crate::i18n::to_wide(texts.menu_settings);
            let exit: Vec<u16> = crate::i18n::to_wide(texts.menu_exit);
            let _ = AppendMenuW(menu, MF_STRING, 0x1001usize, PCWSTR(open.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, 0x1002usize, PCWSTR(refresh.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, 0x1003usize, PCWSTR(settings.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, 0x1004usize, PCWSTR(exit.as_ptr()));

            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let _ = SetForegroundWindow(self.hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN,
                cursor.x,
                cursor.y,
                None,
                self.hwnd,
                None,
            );
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            match command.0 {
                0x1001 => Some(TrayCommand::OpenPanel),
                0x1002 => Some(TrayCommand::RefreshNow),
                0x1003 => Some(TrayCommand::Settings),
                0x1004 => Some(TrayCommand::Exit),
                _ => None,
            }
        }
    }

    fn remove(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data);
            let _ = DestroyIcon(self.icon);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
    }
}

fn set_tip(data: &mut NOTIFYICONDATAW, text: &str) {
    let tip: Vec<u16> = text.encode_utf16().take(127).collect();
    data.szTip = [0; 128];
    data.szTip[..tip.len()].copy_from_slice(&tip);
}

/// WM_APP_TRAY 事件的 lParam 判定（NOTIFYICON_VERSION_4：LOWORD=事件）。
pub fn tray_event(lparam: LPARAM) -> u32 {
    (lparam.0 & 0xFFFF) as u32
}

/// 从 exe 内嵌资源加载应用图标（app.ico 含多尺寸，Shell 挑最接近的）。
pub fn load_app_icon(size: i32) -> Option<HICON> {
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let handle = LoadImageW(
            Some(instance.into()),
            PCWSTR(APP_ICON_RESOURCE_ID as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        )
        .ok()?;
        Some(HICON(handle.0))
    }
}

/// 窗口类用的大图标（任务栏 / Alt-Tab）。
pub fn load_big_app_icon() -> Option<HICON> {
    let size = unsafe { GetSystemMetrics(SM_CXICON) }.clamp(16, 256);
    load_app_icon(size)
}

/// 运行时生成回退图标：主题蓝圆角方块 + 白色 "T"。
pub fn create_app_icon(size: i32) -> Option<HICON> {
    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size, // 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(screen_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .ok()?;
        let old = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

        if let Some(graphics) = crate::ui::gdip::Graphics::from_hdc(mem_dc) {
            graphics.set_smoothing_antialias();
            graphics.set_text_cleartype();
            let full = crate::ui::gdip::RectF::new(0.0, 0.0, size as f32, size as f32);
            crate::ui::drawing::fill_rounded(
                &graphics,
                crate::ui::theme::ACCENT,
                full,
                size as f32 * 0.22,
            );
            if let Some(font) = crate::ui::gdip::Font::from_pixel_height(
                mem_dc,
                crate::ui::theme::UI_FONT_FAMILY,
                (size as f32 * 0.64) as i32,
                true,
            ) {
                crate::ui::gdip::draw_glyph(&graphics, "T", &font, full, 0xFFFFFFFF);
            }
        }

        let mask = CreateBitmap(size, size, 1, 1, None);
        if mask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem_dc);
            let _ = ReleaseDC(None, screen_dc);
            return None;
        }
        let icon_info = windows::Win32::UI::WindowsAndMessaging::ICONINFO {
            fIcon: windows::core::BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: bitmap,
        };
        let icon = CreateIconIndirect(&icon_info).ok();

        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);
        icon
    }
}
