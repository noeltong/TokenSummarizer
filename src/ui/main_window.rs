//! 托盘气泡主窗口：右下角定位、失焦/Esc 关闭、选项卡（合计 + 本机 + 各服务器）
//! + 双 Panel 仪表盘 + 滚动；处理托盘回调与后台更新消息。

use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint,
    GetDC, GetMonitorInfoW, HDC, InvalidateRect, MonitorFromPoint, ReleaseDC, SelectObject,
    SRCCOPY,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos,
    GetMessageW, GetWindowLongPtrW, GetWindowRect, IsWindowVisible, KillTimer, LoadCursorW,
    PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, CW_USEDEFAULT,
    GWLP_USERDATA,
    HICON, IDC_ARROW, MSG, SWP_HIDEWINDOW, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SW_HIDE, WM_ACTIVATE, WM_CREATE, WM_DESTROY, WM_DPICHANGED,
    WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_PRINTCLIENT,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCREATE, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};

use super::super::app::tray::{self, Tray, TrayCommand};
use super::super::app::{
    WM_APP_DIALOG_CLOSED, WM_APP_OPEN_ADD_SERVER, WM_APP_OPEN_ALIASES, WM_APP_OPEN_SETTINGS,
    WM_APP_SELECT_TAB, WM_APP_SHOW, WM_APP_TRAY, WM_APP_UPDATE,
};
use super::super::monitoring::state::AppState;
use super::super::monitoring::UiCommand;
use super::content::{self, PanelHit};
use super::gdip::RectF;
use super::tab_strip::{self, TabElement, TabLayout};
use super::theme::{Theme, BORDER, WINDOW_BACKGROUND};

pub const POPUP_CLASS: &str = "TokenSummarizerPopupWnd";
pub const WINDOW_W_LOGICAL: i32 = 470;
pub const WINDOW_H_LOGICAL: i32 = 790;

/// Explorer 重建任务栏时广播的消息 ID（运行时注册）。
static TASKBAR_CREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub struct PopupApp {
    pub state: Arc<AppState>,
    pub cmd_tx: tokio::sync::mpsc::UnboundedSender<UiCommand>,
    pub tray: Option<Tray>,
    theme: Theme,
    /// 0 = 合计（全部服务器合并），1.. = 服务器索引 + 1。
    pub visible_tab: usize,
    tab_scroll: usize,
    model_range: usize,
    daily_range: usize,
    scroll: i32,
    content_height: i32,
    strip_h: i32,
    viewport_h: i32,
    content_w: i32,
    tab_layout: TabLayout,
    content_hits: Vec<(RectF, PanelHit)>,
    thumb_drag_offset: Option<i32>,
    suppress_deactivate: bool,
    first_run_handled: bool,
}

unsafe fn userdata(hwnd: HWND) -> Option<&'static mut PopupApp> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PopupApp;
    if ptr.is_null() {
        None
    } else {
        Some(&mut *ptr)
    }
}

unsafe fn app_of(hwnd: HWND) -> Option<&'static mut PopupApp> {
    userdata(hwnd)
}

pub fn register_class() -> bool {
    unsafe {
        let instance = GetModuleHandleW(None).ok().expect("GetModuleHandleW");
        let big_icon = tray::load_big_app_icon().unwrap_or_default();
        let small_icon = tray::load_app_icon(16).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: Default::default(),
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: w!("TokenSummarizerPopupWnd"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: big_icon,
            hIconSm: small_icon,
            ..Default::default()
        };
        RegisterClassExW(&wc) != 0
    }
}

/// 创建隐藏的主窗口；返回句柄。窗口销毁时释放 PopupApp。
pub fn create(
    state: Arc<AppState>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<UiCommand>,
) -> windows::core::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let app = Box::new(PopupApp {
            state,
            cmd_tx,
            tray: None,
            theme: Theme::new(96),
            visible_tab: 0,
            tab_scroll: 0,
            model_range: 0,
            daily_range: 0,
            scroll: 0,
            content_height: 0,
            strip_h: 0,
            viewport_h: 0,
            content_w: 0,
            tab_layout: TabLayout::default(),
            content_hits: Vec::new(),
            thumb_drag_offset: None,
            suppress_deactivate: false,
            first_run_handled: false,
        });
        let title = crate::i18n::to_wide(crate::i18n::texts().app_title);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("TokenSummarizerPopupWnd"),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            Some(Box::into_raw(app) as *mut core::ffi::c_void),
        )?;
        Ok(hwnd)
    }
}

pub unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => DefWindowProcW(hwnd, msg, wparam, lparam),
        WM_CREATE => {
            let create =
                &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            let app = create.lpCreateParams as *mut PopupApp;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
            let app = &mut *app;
            let dpi = GetDpiForWindow(hwnd);
            app.theme = Theme::new(dpi);
            let w = app.theme.scale(WINDOW_W_LOGICAL);
            let h = app.theme.scale(WINDOW_H_LOGICAL);
            let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_HIDEWINDOW | SWP_NOMOVE);
            // Explorer 重建任务栏时重建托盘图标
            let taskbar_msg = RegisterWindowMessageW(w!("TaskbarCreated"));
            TASKBAR_CREATED.store(taskbar_msg, Ordering::Release);
            apply_rounded_corners(hwnd);
            LRESULT(0)
        }
        WM_DPICHANGED => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    let dpi = (wparam.0 & 0xFFFF) as u32;
                    app.theme = super::theme::Theme::new(dpi);
                    let w = app.theme.scale(WINDOW_W_LOGICAL);
                    let h = app.theme.scale(WINDOW_H_LOGICAL);
                    let (x, y) = app.position_bottom_right(hwnd);
                    let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER);
                    apply_rounded_corners(hwnd);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut PopupApp;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            // PrintWindow/WM_PRINT 转发的客户区绘制（供截图与 DWM 使用）
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    let hdc = HDC(wparam.0 as *mut _);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
                    if width > 0 && height > 0 {
                        app.render(hwnd, hdc, width, height);
                    }
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe {
                if let Some(app) = app_of(hwnd) {
                    app.paint(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_TIMER => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    // 托盘图标缺失（启动早于 Explorer 等）时重试创建
                    app.ensure_tray(hwnd);
                    if IsWindowVisible(hwnd).as_bool() {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            unsafe {
                let low = (wparam.0 & 0xFFFF) as u32;
                if low == 0 && IsWindowVisible(hwnd).as_bool() {
                    if let Some(app) = userdata(hwnd) {
                        if !app.suppress_deactivate {
                            app.hide(hwnd);
                        }
                    }
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_KEYDOWN => {
            if (wparam.0 as u16) == (VK_ESCAPE.0 as u16) {
                unsafe {
                    if let Some(app) = userdata(hwnd) {
                        app.hide(hwnd);
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    let x = (lparam.0 & 0xFFFF) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                    app.on_mouse_down(hwnd, x, y);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    if app.thumb_drag_offset.is_some() {
                        let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                        app.on_thumb_drag(hwnd, y);
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    if app.thumb_drag_offset.take().is_some() {
                        let _ = ReleaseCapture();
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    let delta = ((wparam.0 >> 16) as i16) as i32;
                    app.scroll_wheel(delta);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        msg if msg == TASKBAR_CREATED.load(Ordering::Acquire) && msg != 0 => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    app.tray = None; // 旧图标随任务栏销毁
                    app.ensure_tray(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_APP_UPDATE | WM_APP_SHOW | WM_APP_TRAY | WM_APP_DIALOG_CLOSED | WM_APP_OPEN_ADD_SERVER
        | WM_APP_OPEN_ALIASES | WM_APP_OPEN_SETTINGS | WM_APP_SELECT_TAB => {
            handle_app_message(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn handle_app_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(app) = userdata(hwnd) else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match msg {
            WM_APP_UPDATE => {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            WM_APP_SHOW => {
                if !IsWindowVisible(hwnd).as_bool() {
                    app.show(hwnd);
                } else {
                    app.hide(hwnd);
                }
            }
            WM_APP_TRAY => {
                let event = tray::tray_event(lparam);
                const WM_LBUTTONUP_EVT: u32 = 0x0202;
                const WM_RBUTTONUP_EVT: u32 = 0x0205;
                const WM_CONTEXTMENU_EVT: u32 = 0x007B;
                match event {
                    WM_LBUTTONUP_EVT => {
                        if IsWindowVisible(hwnd).as_bool() {
                            app.hide(hwnd);
                        } else {
                            app.show(hwnd);
                        }
                    }
                    WM_RBUTTONUP_EVT | WM_CONTEXTMENU_EVT => {
                        let Some(tray) = app.tray.as_ref() else {
                            return LRESULT(0);
                        };
                        match tray.show_menu() {
                            Some(TrayCommand::OpenPanel) => app.show(hwnd),
                            Some(TrayCommand::RefreshNow) => {
                                let _ = app.cmd_tx.send(UiCommand::RefreshAll);
                            }
                            Some(TrayCommand::Settings) => app.open_settings(hwnd),
                            Some(TrayCommand::Exit) => {
                                let _ = DestroyWindow(hwnd);
                            }
                            None => {}
                        }
                    }
                    _ => {}
                }
            }
            WM_APP_DIALOG_CLOSED => {
                app.suppress_deactivate = false;
                // 语言可能在设置里切换了：同步托盘 tooltip 与窗口标题（面板文案每帧重算，无需额外处理）
                app.refresh_language_chrome(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            WM_APP_OPEN_ADD_SERVER => {
                app.open_server_edit(hwnd, None);
            }
            WM_APP_OPEN_ALIASES => {
                app.suppress_deactivate = true;
                super::alias_edit::open(hwnd, app.state.clone());
            }
            WM_APP_OPEN_SETTINGS => {
                app.open_settings(hwnd);
            }
            WM_APP_SELECT_TAB => {
                let index = wparam.0 as usize;
                if index < app.tab_count() {
                    app.visible_tab = index;
                    app.scroll = 0;
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            _ => {}
        }
        LRESULT(0)
    }
}

impl PopupApp {
    // ------------------------------------------------------------------
    // 显示 / 隐藏 / 定位
    // ------------------------------------------------------------------

    pub fn show(&mut self, hwnd: HWND) {
        unsafe {
            crate::log_info!("面板 show 被调用");
            self.ensure_tray(hwnd);
            let (x, y) = self.position_bottom_right(hwnd);
            let _ = SetWindowPos(
                hwnd,
                Some(windows::Win32::UI::WindowsAndMessaging::HWND_TOPMOST),
                x,
                y,
                0,
                0,
                SWP_SHOWWINDOW | SWP_NOSIZE,
            );
            let _ = SetForegroundWindow(hwnd);
            let _ = SetTimer(Some(hwnd), 1, 1000, None);
            // DWM 偏好可能因窗口重建/换显示器失效，显示后重放一次
            apply_rounded_corners(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
            // 首次运行且无服务器 → 打开"添加服务器"
            if !self.first_run_handled {
                self.first_run_handled = true;
                let empty = self
                    .state
                    .cfg
                    .read()
                    .map(|cfg| cfg.servers.is_empty())
                    .unwrap_or(true);
                if empty {
                    self.open_server_edit(hwnd, None);
                }
            }
        }
    }

    pub fn hide(&mut self, hwnd: HWND) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
            let _ = KillTimer(Some(hwnd), 1);
        }
    }

    fn ensure_tray(&mut self, hwnd: HWND) {
        if self.tray.is_none() {
            self.tray = Tray::create(hwnd);
        }
    }

    /// 语言切换后刷新托盘 tooltip 与窗口标题（对话框关闭消息触发）。
    fn refresh_language_chrome(&mut self, hwnd: HWND) {
        unsafe {
            if let Some(tray) = self.tray.as_mut() {
                tray.set_tip_text(crate::i18n::texts().app_title);
            }
            let title = crate::i18n::to_wide(crate::i18n::texts().app_title);
            let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
        }
    }

    /// 固定定位到工作区右下角（任务栏上方靠右）。
    fn position_bottom_right(&self, hwnd: HWND) -> (i32, i32) {
        unsafe {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let mut area = RECT {
                left: 0,
                top: 0,
                right: 1600,
                bottom: 900,
            };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                area = info.rcWork;
            }
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;
            let margin = self.theme.scale(10);
            let x = area.right - w - margin;
            let y = area.bottom - h - margin;
            (x.max(area.left), y.max(area.top))
        }
    }

    // ------------------------------------------------------------------
    // 子窗口
    // ------------------------------------------------------------------

    fn open_settings(&mut self, hwnd: HWND) {
        self.suppress_deactivate = true;
        super::settings::open(hwnd, self.state.clone(), self.cmd_tx.clone());
    }

    fn open_server_edit(&mut self, hwnd: HWND, edit: Option<crate::config::ServerConfig>) {
        self.suppress_deactivate = true;
        super::server_edit::open(hwnd, self.state.clone(), self.cmd_tx.clone(), edit);
    }

    // ------------------------------------------------------------------
    // 滚动
    // ------------------------------------------------------------------

    /// 可滚动 body 的顶部（= 选项卡条底部 + 固定头部高度）。
    fn body_top(&self) -> i32 {
        self.strip_h + content::header_height(self.theme.dpi)
    }

    /// 可滚动 body 的高度（视口高 - 固定头部）。
    fn body_height(&self) -> i32 {
        (self.viewport_h - content::header_height(self.theme.dpi)).max(0)
    }

    fn max_scroll(&self) -> i32 {
        (self.content_height - self.body_height()).max(0)
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0, self.max_scroll());
    }

    fn scroll_wheel(&mut self, delta: i32) {
        let step = self.theme.scale(90);
        self.scroll -= delta.signum() * step;
        self.clamp_scroll();
    }

    /// 滚动条叠在右侧内距里（不占布局，卡片左右内距对称）。
    fn scrollbar_geometry(&self) -> Option<(RectF, RectF)> {
        if self.max_scroll() <= 0 {
            return None;
        }
        let body_h = self.body_height();
        if body_h <= 0 {
            return None;
        }
        let w = self.theme.scale(6) as f32;
        let track = RectF::new(
            (self.content_w - self.theme.scale(10)) as f32,
            self.body_top() as f32,
            w,
            body_h as f32,
        );
        let ratio = self.scroll as f32 / self.max_scroll().max(1) as f32;
        let thumb_h = (body_h as f32 * body_h as f32 / self.content_height.max(1) as f32)
            .max(self.theme.scale(24) as f32)
            .min(track.h);
        let movable = track.h - thumb_h;
        let thumb = RectF::new(track.x, track.y + movable * ratio, w, thumb_h);
        Some((track, thumb))
    }

    fn on_thumb_drag(&mut self, hwnd: HWND, y: i32) {
        let Some((track, thumb)) = self.scrollbar_geometry() else {
            return;
        };
        let Some(offset) = self.thumb_drag_offset else {
            return;
        };
        let movable = track.h - thumb.h;
        if movable <= 0.0 {
            return;
        }
        let new_top = (y as f32 - offset as f32).clamp(track.y, track.y + movable);
        self.scroll = ((new_top - track.y) / movable * self.max_scroll() as f32) as i32;
        self.clamp_scroll();
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn on_mouse_down(&mut self, hwnd: HWND, x: i32, y: i32) {
        // 选项卡条
        for (rect, element) in self.tab_layout.hits.clone() {
            if rect.contains(x, y) {
                match element {
                    TabElement::Tab(index) => {
                        self.visible_tab = index;
                        self.scroll = 0;
                    }
                    TabElement::ScrollLeft => {
                        self.tab_scroll = tab_strip::scroll_step(
                            false,
                            self.tab_scroll,
                            self.tab_count(),
                        );
                    }
                    TabElement::ScrollRight => {
                        self.tab_scroll =
                            tab_strip::scroll_step(true, self.tab_scroll, self.tab_count());
                    }
                    TabElement::Add => {
                        self.open_server_edit(hwnd, None);
                    }
                    TabElement::Refresh => {
                        let _ = self.cmd_tx.send(UiCommand::RefreshAll);
                    }
                    TabElement::Settings => {
                        self.open_settings(hwnd);
                    }
                }
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                return;
            }
        }
        // 面板范围切换（命中区是 body 内容坐标，需要换算；头部区域不参与）
        if y >= self.body_top() {
            let content_y = y - self.body_top() + self.scroll;
            for (rect, hit) in self.content_hits.clone() {
                if rect.contains(x, content_y) {
                    match hit {
                        PanelHit::ModelRange(index) => self.model_range = index,
                        PanelHit::DailyRange(index) => self.daily_range = index,
                    }
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    return;
                }
            }
        }
        // 滚动条
        if let Some((track, thumb)) = self.scrollbar_geometry() {
            // 细滚动条加一点横向容差
            let slop = self.theme.scale(3) as f32;
            let hit = |rect: RectF| {
                x as f32 >= rect.x - slop
                    && (x as f32) < rect.right() + slop
                    && rect.contains(x, y)
            };
            if hit(thumb) {
                self.thumb_drag_offset = Some(y - thumb.y as i32);
                unsafe {
                    let _ = SetCapture(hwnd);
                }
                return;
            }
            if hit(track) {
                let page = self.body_height().max(1);
                self.scroll += if (y as f32) < thumb.y { -page } else { page };
                self.clamp_scroll();
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
        }
    }

    // 选项卡总数（合计 + 本机 + 服务器）。
    fn tab_count(&self) -> usize {
        self.server_count() + 2
    }

    fn server_count(&self) -> usize {
        self.state
            .cfg
            .read()
            .map(|cfg| cfg.servers.len())
            .unwrap_or(0)
    }

    // ------------------------------------------------------------------
    // 绘制
    // ------------------------------------------------------------------

    unsafe fn paint(&mut self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        if hdc.is_invalid() {
            return;
        }
        let dpi = GetDpiForWindow(hwnd);
        if dpi != self.theme.dpi {
            self.theme = Theme::new(dpi);
        }
        let mut rect = RECT::default();
        let _ = GetClientRect(hwnd, &mut rect);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width > 0 && height > 0 {
            self.render(hwnd, hdc, width, height);
        }
        let _ = EndPaint(hwnd, &ps);
    }

    /// 渲染客户区到任意 DC（WM_PAINT 与 WM_PRINTCLIENT 共用）。
    unsafe fn render(&mut self, hwnd: HWND, hdc: HDC, width: i32, height: i32) {
        // 双缓冲
        let screen = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap =
            match CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) => bitmap,
                Err(_) => {
                    let _ = DeleteDC(mem_dc);
                    let _ = ReleaseDC(None, screen);
                    return;
                }
            };
        let old_bitmap = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

        if let Some(g) = super::gdip::Graphics::from_hdc(mem_dc) {
            g.set_smoothing_antialias();
            g.set_text_cleartype();
            super::drawing::fill_rounded(
                &g,
                WINDOW_BACKGROUND,
                RectF::new(0.0, 0.0, width as f32, height as f32),
                0.0,
            );
            if let Some(pen) = super::gdip::Pen::new(BORDER, 1.0) {
                if let Some(mut path) = super::gdip::Path::new() {
                    path.add_rounded_rect(
                        RectF::new(0.5, 0.5, width as f32 - 1.0, height as f32 - 1.0),
                        corner_radius_px(self.theme.dpi),
                    );
                    g.draw_path(&pen, &path);
                }
            }

            // 选项卡：合计 + 各服务器
            let (labels, phases, server_datas) = self.tab_view_data();
            if self.visible_tab >= labels.len() {
                self.visible_tab = labels.len().saturating_sub(1);
            }
            self.tab_layout = tab_strip::paint(
                &g,
                &mut self.theme,
                mem_dc,
                width,
                &labels,
                &phases,
                self.visible_tab,
                self.tab_scroll,
            );
            let strip_h = self.theme.scale(tab_strip::STRIP_H_LOGICAL);
            self.strip_h = strip_h;
            self.viewport_h = height - strip_h;
            // 不再为滚动条预留横向空间：卡片左右内距对称（各 12），滚动条叠在右侧
            self.content_w = width;
            // 按上一帧的内容高度先钳制，避免本帧画出越界位置
            self.clamp_scroll();

            let (title, subtitle, freshness, cards, snapshot_opt) =
                self.dashboard_data(&server_datas);

            let (model_rows, model_total, daily, daily_total) = {
                let cfg = self.state.cfg.read().unwrap_or_else(|err| err.into_inner());
                let aliases = cfg.alias_pairs();
                let tpc = cfg.tokens_per_credit;
                match &snapshot_opt {
                    Some(snapshot) => {
                        let all_models = snapshot.model_totals(
                            content::MODEL_RANGE_DAYS[self.model_range.min(3)],
                            Utc::now(),
                            &aliases,
                            tpc,
                        );
                        // 合计在截断与过滤前求和（含未标注模型：真实用量，只是不进列表）
                        let model_total = all_models.iter().map(|(_, total, _)| *total).sum();
                        let model_rows: Vec<_> = all_models
                            .into_iter()
                            .filter(|(name, _, _)| name != crate::parsing::token::EMPTY_MODEL)
                            .take(content::TOP20_MAX_ROWS)
                            .collect();
                        let daily = snapshot.daily_totals(
                            content::DAILY_RANGE_N[self.daily_range.min(1)],
                            Utc::now(),
                            tpc,
                        );
                        let daily_total = daily.iter().map(|(_, total)| *total).sum();
                        (model_rows, model_total, daily, daily_total)
                    }
                    None => (Vec::new(), 0, Vec::new(), 0),
                }
            };

            let mut view = content::ContentView {
                g: &g,
                theme: &mut self.theme,
                hdc: mem_dc,
                width: self.content_w,
                top: strip_h,
                height: self.viewport_h,
                scroll: self.scroll,
                now: Utc::now(),
            };
            let dashboard = content::paint(
                &mut view,
                &title,
                &subtitle,
                &freshness,
                &cards,
                &model_rows,
                &daily,
                model_total,
                daily_total,
                self.model_range,
                self.daily_range,
            );
            let prev_content_height = self.content_height;
            self.content_height = dashboard.content_height;
            self.content_hits = dashboard.hits;
            if self.content_height != prev_content_height {
                // 内容高度变化会改变滚动条是否出现，需要再刷一帧
                let _ = InvalidateRect(Some(hwnd), None, false);
            }

            // 滚动条（叠在右侧内距内，不遮挡卡片）
            if let Some((track, thumb)) = self.scrollbar_geometry() {
                super::drawing::fill_rounded(&g, super::theme::TRACK, track, track.w / 2.0);
                super::drawing::fill_rounded(&g, super::theme::BORDER_STRONG, thumb, thumb.w / 2.0);
            }
        }
        let before = self.scroll;
        self.clamp_scroll();
        if self.scroll != before {
            // 内容变短导致回弹，下一帧按新位置重绘
            let _ = InvalidateRect(Some(hwnd), None, false);
        }

        let _ = BitBlt(hdc, 0, 0, width, height, Some(mem_dc), 0, 0, SRCCOPY);
        let _ = SelectObject(mem_dc, old_bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen);
    }

    /// 选项卡数据：labels[0]="合计"，labels[1]="本机"，其后为各服务器；相位聚合。
    fn tab_view_data(&self) -> (Vec<String>, Vec<super::super::model::Phase>, Vec<ServerView>) {
        let cfg = self
            .state
            .cfg
            .read()
            .unwrap_or_else(|err| err.into_inner());
        let servers_state = self.state.servers.read().unwrap_or_else(|err| err.into_inner());
        let texts = crate::i18n::texts();
        let mut labels = vec![texts.tab_totals.to_string(), texts.tab_local.to_string()];
        let mut phases = vec![crate::model::Phase::Online]; // 本机恒在线
        let mut views = Vec::new();
        let mut any_online = false;
        let mut any_error = false;
        for server in &cfg.servers {
            labels.push(server.display_label());
            let data = servers_state.iter().find(|s| s.id == server.id);
            let phase = data.map(|d| d.phase).unwrap_or(crate::model::Phase::Stopped);
            match phase {
                crate::model::Phase::Online => any_online = true,
                crate::model::Phase::Error => any_error = true,
                _ => {}
            }
            phases.push(phase);
            views.push(ServerView {
                label: server.display_label(),
                user_host: server.display_user_host(),
                data: data.cloned(),
            });
        }
        // 合计标签的相位：本机在线 + 服务器聚合
        let total_phase = if any_online {
            crate::model::Phase::Online
        } else if any_error {
            crate::model::Phase::Error
        } else {
            crate::model::Phase::Connecting
        };
        phases.insert(0, total_phase);
        (labels, phases, views)
    }

    /// 当前选项卡的仪表盘输入（标题/副标题/新鲜度/状态卡片/快照）。
    #[allow(clippy::type_complexity)]
    fn dashboard_data(
        &self,
        views: &[ServerView],
    ) -> (
        String,
        String,
        String,
        Vec<(u32, u32, String)>,
        Option<crate::model::TokenSnapshot>,
    ) {
        use crate::model::{Phase, TokenSnapshot};
        let now = Utc::now();
        if self.visible_tab == 0 {
            // 合计：本机 + 全部服务器快照合并
            let mut snapshots: Vec<TokenSnapshot> = Vec::new();
            let mut latest: Option<chrono::DateTime<Utc>> =
                self.state.local_data().map(|local| local.at);
            if let Some(local) = self.state.local_data() {
                snapshots.push(local.token);
            }
            for view in views {
                if let Some(data) = &view.data {
                    if let Some(token) = &data.token {
                        snapshots.push(token.clone());
                    }
                    if let Some(at) = data.token_at {
                        latest = Some(latest.map_or(at, |prev| prev.max(at)));
                    }
                }
            }
            let online = views
                .iter()
                .filter(|view| view.data.as_ref().map(|d| d.phase) == Some(Phase::Online))
                .count();
            let freshness = match latest {
                Some(at) => format!("Token {}", super::drawing::format_relative(at, now)),
                None => crate::i18n::texts().syncing.to_string(),
            };
            let subtitle = crate::i18n::subtitle_servers(views.len(), online, views.len());
            let snapshot_opt = if snapshots.is_empty() {
                None
            } else {
                Some(TokenSnapshot::merge_all(snapshots.iter()))
            };
            return (
                crate::i18n::texts().title_totals.to_string(),
                subtitle,
                freshness,
                Vec::new(),
                snapshot_opt,
            );
        }
        if self.visible_tab == 1 {
            // 本机
            let title = crate::i18n::texts().tab_local.to_string();
            let subtitle = format!(
                "{}\\{}",
                std::env::var("COMPUTERNAME").unwrap_or_default(),
                std::env::var("USERNAME").unwrap_or_default()
            );
            let freshness = match self.state.local_data() {
                Some(local) => format!("Token {}", super::drawing::format_relative(local.at, now)),
                None => crate::i18n::texts().scanning.to_string(),
            };
            let snapshot_opt = self.state.local_data().map(|local| local.token);
            return (title, subtitle, freshness, Vec::new(), snapshot_opt);
        }
        // 单服务器
        let Some(view) = views.get(self.visible_tab - 2) else {
            return (
                crate::i18n::texts().unknown.to_string(),
                String::new(),
                String::new(),
                Vec::new(),
                None,
            );
        };
        let mut cards: Vec<(u32, u32, String)> = Vec::new();
        let mut snapshot_opt = None;
        if let Some(data) = &view.data {
            if let Some(error) = &data.error {
                cards.push((
                    super::theme::RED_SOFT,
                    super::theme::RED,
                    crate::i18n::conn_error(error),
                ));
            }
            if let Some(token) = &data.token {
                snapshot_opt = Some(token.clone());
            } else if let Some(err) = &data.token_error {
                cards.push((
                    super::theme::AMBER_SOFT,
                    super::theme::AMBER,
                    crate::i18n::collect_failed(err),
                ));
            } else if data.token_syncing {
                cards.push((
                    super::theme::ACCENT_SOFT,
                    super::theme::ACCENT,
                    crate::i18n::texts().first_sync_hint.to_string(),
                ));
            }
        }
        let freshness = match view.data.as_ref().and_then(|d| d.token_at) {
            Some(at) => format!("Token {}", super::drawing::format_relative(at, now)),
            None => crate::i18n::texts().syncing.to_string(),
        };
        (
            view.label.clone(),
            view.user_host.clone(),
            freshness,
            cards,
            snapshot_opt,
        )
    }
}

/// 选项卡渲染用的服务器视图。
struct ServerView {
    label: String,
    user_host: String,
    data: Option<crate::model::ServerData>,
}

/// 消息循环（主线程）。
pub fn run_message_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
    }
}

// 保留导入引用
#[allow(dead_code)]
fn _icon_type(_i: HICON) {}

/// 自绘卡片/窗口的圆角半径（逻辑像素）。
pub const CORNER_RADIUS_LOGICAL: i32 = 8;

/// 逻辑半径 → 物理像素。
pub fn corner_radius_px(dpi: u32) -> f32 {
    ((CORNER_RADIUS_LOGICAL as f64 * dpi as f64 / 96.0) + 0.5).floor() as f32
}

/// Win11 圆角（DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND）。
/// Win10 上 DWM 返回 E_INVALIDARG，回退 SetWindowRgn 圆角区域。
/// 返回 true 表示 DWM 生效（此时不要再设窗口区域，否则会盖掉 DWM 的圆角）。
pub fn apply_rounded_corners(hwnd: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWM_WINDOW_CORNER_PREFERENCE,
        DWMWCP_ROUND,
    };
    unsafe {
        // 属性 ID 与偏好值不是同一个常量：33 = 属性 ID，值是 DWMWCP_ROUND(2)。
        let preference = DWMWCP_ROUND;
        let applied = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const DWM_WINDOW_CORNER_PREFERENCE as *const core::ffi::c_void,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
        .is_ok();
        if applied {
            return true;
        }
        apply_round_region(hwnd);
        false
    }
}

/// Win10 回退：整窗圆角区域（DWM 不可用时用 SetWindowRgn 裁剪）。
unsafe fn apply_round_region(hwnd: HWND) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return;
        }
        let radius = corner_radius_px(GetDpiForWindow(hwnd)) as i32;
        let diameter = (radius * 2).max(2);
        // 右/下边界不含，必须 +1 才覆盖整窗
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
        if region.is_invalid() {
            return;
        }
        // 成功时区域归系统所有，不能再 DeleteObject；失败才释放。
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
}

