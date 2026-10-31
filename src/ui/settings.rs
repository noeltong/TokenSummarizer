//! 设置窗口：刷新间隔、通知与自启开关、每积分 Token 数、服务器管理
//! （添加/编辑/删除）、测试通知与清空缓存。自绘框架 + 原生 EDIT/CHECKBOX/BUTTON。

use std::sync::Arc;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint,
    GetDC, InvalidateRect, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HDC, HGDIOBJ, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    GetWindowRect, PostMessageW, RegisterClassExW, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, WNDCLASSEXW, WM_CREATE, WM_DESTROY, WM_ERASEBKGND,
    WM_COMMAND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_NCCREATE, WM_PAINT, WM_NCLBUTTONDOWN,
    WM_PRINTCLIENT, BN_CLICKED,
    ES_NUMBER,
    GWLP_USERDATA, HTCAPTION, SWP_SHOWWINDOW, WS_CHILD,

    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_TABSTOP, WS_VISIBLE,
};
use uuid::Uuid;

use super::super::app::{autostart, WM_APP_DIALOG_CLOSED};
use super::super::monitoring::state::AppState;
use super::super::monitoring::UiCommand;
use super::button::{ButtonStyle, DrawnButton};
use super::dialog::{self, ControlFont};
use super::drawing::{draw_text, fill_rounded, stroke_rounded};
use super::gdip::{Graphics, RectF};
use super::theme::{
    Theme, ACCENT, ACCENT_SOFT, BORDER, CARD_BACKGROUND, GLYPH_CLOSE, GLYPH_DELETE, GLYPH_EDIT,
    HOVER, MUTED_TEXT, RED, RED_SOFT, SECONDARY_TEXT, TEXT, WINDOW_BACKGROUND,
};

const W_LOGICAL: i32 = 440;
const H_LOGICAL: i32 = 720;

// 固定布局（逻辑像素 @96）
const PAD: i32 = 16;
const LABEL_W: i32 = 150;
const EDIT_X: i32 = 170;
const EDIT_W: i32 = 96;
const ROW_H: i32 = 26;
const Y_TK: i32 = 52;
const Y_TPC: i32 = 88;
const Y_CHK_AUTOSTART: i32 = 126;
const Y_LANG: i32 = 158;
const Y_BTN_ROW: i32 = 194;
const Y_SERVER_SECTION: i32 = 240;
const Y_SERVER_ROWS: i32 = 266;
const ROW_STEP: i32 = 46;
const MAX_ROWS: usize = 7;
const Y_BOTTOM_BUTTONS: i32 = H_LOGICAL - 64;

// 控件 ID
const ID_TK_INTERVAL: i32 = 2002;
const ID_TPC: i32 = 2003;
const ID_CHK_AUTOSTART: i32 = 2102;
const ID_RADIO_LANG_ZH: i32 = 2103;
const ID_RADIO_LANG_EN: i32 = 2104;
const ID_BTN_ALIASES: i32 = 2201;
const ID_BTN_CLEAR_CACHE: i32 = 2203;
const ID_BTN_SAVE: i32 = 2301;
const ID_BTN_CANCEL: i32 = 2302;

struct SettingsState {
    parent: HWND,
    hwnd: HWND,
    state: Arc<AppState>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<UiCommand>,
    font: Option<ControlFont>,
    theme: Theme,
    rows: Vec<(RectF, RectF, Uuid)>, // (编辑按钮, 删除按钮, 服务器 id)
    /// 自绘按钮（模型别名/清空缓存/保存/取消）。
    drawn_buttons: Vec<super::button::DrawnButton>,
    add_server_rect: RectF,
    close_rect: RectF,
    title_rect: RectF,
    status_text: String,
}

unsafe fn userdata(hwnd: HWND) -> Option<&'static mut SettingsState> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
    if ptr.is_null() {
        None
    } else {
        Some(&mut *ptr)
    }
}

pub fn register_class() -> bool {
    unsafe {
        let instance = GetModuleHandleW(None).ok().expect("GetModuleHandleW");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: w!("TokenSummarizerSettingsWnd"),
            ..Default::default()
        };
        RegisterClassExW(&wc) != 0
    }
}

pub fn open(parent: HWND, state: Arc<AppState>, cmd_tx: tokio::sync::mpsc::UnboundedSender<UiCommand>) {
    unsafe {
        let instance = GetModuleHandleW(None).ok().expect("GetModuleHandleW");
        let (cfg, sender_state, sender_cmd) = {
            let cfg = state.cfg.read().map(|cfg| cfg.clone()).unwrap_or_default();
            (cfg, state, cmd_tx)
        };
        let app = Box::new(SettingsState {
            parent,
            hwnd: HWND::default(),
            state: sender_state,
            cmd_tx: sender_cmd,
            font: None,
            theme: Theme::new(96),
            rows: Vec::new(),
            drawn_buttons: Vec::new(),
            add_server_rect: RectF::default(),
            close_rect: RectF::default(),
            title_rect: RectF::default(),
            status_text: String::new(),
        });
        let _ = cfg;
        let title = crate::i18n::to_wide(crate::i18n::texts().settings_title);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("TokenSummarizerSettingsWnd"),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            160,
            120,
            100,
            100,
            Some(parent),
            None,
            Some(instance.into()),
            Some(Box::into_raw(app) as *mut core::ffi::c_void),
        )
        .ok();
        if let Some(hwnd) = hwnd {
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => DefWindowProcW(hwnd, msg, wparam, lparam),
        WM_CREATE => {
            let create = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            let ptr = create.lpCreateParams as *mut SettingsState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
            let app = &mut *ptr;
            app.hwnd = hwnd;
            let dpi = GetDpiForWindow(hwnd);
            app.theme = Theme::new(dpi);
            let w = app.theme.scale(W_LOGICAL);
            let h = app.theme.scale(H_LOGICAL);
            app.font = ControlFont::create(dpi);
            app.create_controls();
            // 居中于父窗口
            let mut parent_rect = RECT::default();
            let _ = GetWindowRect(app.parent, &mut parent_rect);
            let x = (parent_rect.left + parent_rect.right) / 2 - w / 2;
            let y = (parent_rect.top + parent_rect.bottom) / 2 - h / 2;
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_SHOWWINDOW);
            // 尺寸确定后再设圆角/区域，否则 Win10 回退会按初始 100×100 裁剪
            super::window::apply_rounded_corners(hwnd);
            LRESULT(0)
        }
        WM_PRINTCLIENT => {
            unsafe {
                if let Some(app) = userdata(hwnd) {
                    let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut _);
                    let mut rect = RECT::default();
                    let _ = GetClientRect(hwnd, &mut rect);
                    let (width, height) = (rect.right, rect.bottom);
                    if width > 0 && height > 0 {
                        app.render(hdc, width, height);
                    }
                }
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            if let Some(app) = userdata(hwnd) {
                app.paint(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            if code == BN_CLICKED {
                if let Some(app) = userdata(hwnd) {
                    app.on_button(hwnd, id);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            if let Some(app) = userdata(hwnd) {
                let x = (lparam.0 & 0xFFFF) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
                app.on_mouse_down(hwnd, x, y);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if (wparam.0 as u16) == (VK_ESCAPE.0 as u16) {
                unsafe { let _ = DestroyWindow(hwnd); };
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SettingsState;
            if !ptr.is_null() {
                let app = Box::from_raw(ptr);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let parent = app.parent;
                drop(app);
                let _ = PostMessageW(Some(parent), WM_APP_DIALOG_CLOSED, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

impl SettingsState {
    unsafe fn create_controls(&mut self) {
        let cfg = self.state.cfg.read().map(|cfg| cfg.clone()).unwrap_or_default();
        let dpi = self.theme.dpi;
        let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
        let hwnd = self.hwnd;

        let mk_edit = |id: i32, text: String, numeric: bool, y: i32| {
            let mut style = (WS_CHILD | WS_VISIBLE | WS_TABSTOP).0
                | windows::Win32::UI::WindowsAndMessaging::ES_AUTOHSCROLL as u32;
            if numeric {
                style |= ES_NUMBER as u32;
            }
            // 扁平化：不用 WS_EX_CLIENTEDGE 立体边框，边框由父窗口绘制
            if let Some(control) = dialog::create_child(
                hwnd,
                "EDIT",
                &text,
                Default::default(),
                windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(style),
                s(EDIT_X),
                s(y),
                s(EDIT_W),
                s(ROW_H),
                id as u16,
            ) {
                if let Some(font) = &self.font {
                    font.apply(control);
                }
            }
        };
        mk_edit(
            ID_TK_INTERVAL,
            cfg.token_refresh_seconds.to_string(),
            true,
            Y_TK,
        );
        mk_edit(
            ID_TPC,
            format!("{}", cfg.tokens_per_credit),
            false,
            Y_TPC,
        );

        if let Some(check) = dialog::create_child(
            hwnd,
            "BUTTON",
            crate::i18n::texts().chk_autostart,
            Default::default(),
            windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                (WS_CHILD | WS_VISIBLE).0
                    | windows::Win32::UI::WindowsAndMessaging::BS_AUTOCHECKBOX as u32,
            ),
            s(PAD),
            s(Y_CHK_AUTOSTART),
            s(320),
            s(24),
            ID_CHK_AUTOSTART as u16,
        ) {
            if let Some(font) = &self.font {
                font.apply(check);
            }
            if cfg.start_with_windows {
                dialog::check(check, true);
            }
        }

        // 语言：两枚单选（语言名永远用各自语言书写）
        for (id, text, x, checked) in [
            (
                ID_RADIO_LANG_ZH,
                crate::i18n::LANG_NAME_ZH,
                EDIT_X,
                cfg.language == crate::config::Language::Zh,
            ),
            (
                ID_RADIO_LANG_EN,
                crate::i18n::LANG_NAME_EN,
                EDIT_X + 96,
                cfg.language == crate::config::Language::En,
            ),
        ] {
            if let Some(control) = dialog::create_child(
                hwnd,
                "BUTTON",
                text,
                Default::default(),
                windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
                    (WS_CHILD | WS_VISIBLE).0
                        | windows::Win32::UI::WindowsAndMessaging::BS_AUTORADIOBUTTON as u32,
                ),
                s(x),
                s(Y_LANG),
                s(88),
                s(24),
                id as u16,
            ) {
                if let Some(font) = &self.font {
                    font.apply(control);
                }
                if checked {
                    dialog::check(control, true);
                }
            }
        }

        // 按钮全部自绘，命中区在 paint/on_mouse_down 中处理
        let texts = crate::i18n::texts();
        self.drawn_buttons = vec![
            DrawnButton::new(ID_BTN_ALIASES, texts.btn_aliases, PAD, Y_BTN_ROW, 110, 28, ButtonStyle::Secondary),
            DrawnButton::new(ID_BTN_CLEAR_CACHE, texts.btn_clear_cache, PAD + 120, Y_BTN_ROW, 110, 28, ButtonStyle::Secondary),
            DrawnButton::new(ID_BTN_SAVE, texts.btn_save, W_LOGICAL - PAD - 88, Y_BOTTOM_BUTTONS, 88, 30, ButtonStyle::Primary),
            DrawnButton::new(ID_BTN_CANCEL, texts.btn_cancel, W_LOGICAL - PAD - 88 * 2 - 10, Y_BOTTOM_BUTTONS, 88, 30, ButtonStyle::Secondary),
        ];
    }

    unsafe fn on_button(&mut self, hwnd: HWND, id: i32) {
        match id {
            ID_BTN_SAVE => self.save(hwnd),
            ID_BTN_CANCEL => { let _ = DestroyWindow(hwnd); }
            ID_BTN_ALIASES => {
                let parent = self.parent;
                let state = self.state.clone();
                super::alias_edit::open(parent, state);
            }
            ID_BTN_CLEAR_CACHE => {
                match AppState::clear_cache() {
                    Ok(()) => {
                        self.status_text = crate::i18n::texts().cache_cleared.to_string()
                    }
                    Err(err) => self.status_text = crate::i18n::clear_failed(&err.to_string()),
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            _ => {}
        }
    }

    unsafe fn save(&mut self, hwnd: HWND) {
        let tk = dialog::get_item_text(hwnd, ID_TK_INTERVAL).trim().parse::<u32>().unwrap_or(3600);
        let tpc = dialog::get_item_text(hwnd, ID_TPC).trim().parse::<f64>().unwrap_or(20000.0);
        let autostart_on = dialog::is_checked(hwnd, ID_CHK_AUTOSTART);
        let language = if dialog::is_checked(hwnd, ID_RADIO_LANG_EN) {
            crate::config::Language::En
        } else {
            crate::config::Language::Zh
        };

        let mut cfg = self.state.cfg.read().map(|cfg| cfg.clone()).unwrap_or_default();
        cfg.token_refresh_seconds = tk;
        cfg.tokens_per_credit = tpc;
        cfg.start_with_windows = autostart_on;
        cfg.language = language;
        cfg.clamp_refresh_values();

        if let Err(err) = autostart::set_enabled(autostart_on) {
            self.status_text = crate::i18n::autostart_failed(&err);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return;
        }
        // 先切全局语言再落盘：关闭窗口后主界面/托盘立即按新语言重绘
        crate::i18n::set_language(language);
        self.state.replace_config(cfg);
        let _ = self.cmd_tx.send(UiCommand::RestartMonitors);
        let _ = DestroyWindow(hwnd);
    }

    unsafe fn on_mouse_down(&mut self, hwnd: HWND, x: i32, y: i32) {
        if self.close_rect.contains(x, y) {
            let _ = DestroyWindow(hwnd);
            return;
        }
        if self.title_rect.contains(x, y) {
            let _ = ReleaseCapture();
            let _ = SendMessageW(
                hwnd,
                WM_NCLBUTTONDOWN,
                Some(WPARAM(HTCAPTION as usize)),
                None,
            );
            return;
        }
        // 自绘按钮命中
        let dpi = self.theme.dpi;
        let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
        for index in 0..self.drawn_buttons.len() {
            if self.drawn_buttons[index].hit(s, x, y) {
                let id = self.drawn_buttons[index].id;
                self.drawn_buttons[index].pressed = true;
                let _ = InvalidateRect(Some(hwnd), None, false);
                self.on_button(hwnd, id);
                return;
            }
        }
        if self.title_rect.contains(x, y) {
            let _ = ReleaseCapture();
            let _ = SendMessageW(
                hwnd,
                WM_NCLBUTTONDOWN,
                Some(WPARAM(HTCAPTION as usize)),
                None,
            );
            return;
        }
        for (edit_rect, delete_rect, id) in self.rows.clone() {
            if edit_rect.contains(x, y) {
                let server = self
                    .state
                    .cfg
                    .read()
                    .ok()
                    .and_then(|cfg| cfg.servers.iter().find(|s| s.id == id).cloned());
                let parent = self.parent;
                let (state, cmd_tx) = (self.state.clone(), self.cmd_tx.clone());
                let _ = DestroyWindow(hwnd);
                super::server_edit::open(parent, state, cmd_tx, server);
                return;
            }
            if delete_rect.contains(x, y) {
                let mut cfg = self.state.cfg.read().ok().map(|c| c.clone()).unwrap_or_default();
                cfg.servers.retain(|s| s.id != id);
                self.state.replace_config(cfg);
                let _ = self.cmd_tx.send(UiCommand::RestartMonitors);
                let _ = InvalidateRect(Some(hwnd), None, false);
                return;
            }
        }
        if self.add_server_rect.contains(x, y) {
            let parent = self.parent;
            let (state, cmd_tx) = (self.state.clone(), self.cmd_tx.clone());
            let _ = DestroyWindow(hwnd);
            super::server_edit::open(parent, state, cmd_tx, None);
        }
    }

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
        let width = rect.right;
        let height = rect.bottom;
        if width > 0 && height > 0 {
            self.render(hdc, width, height);
        }
        let _ = EndPaint(hwnd, &ps);
    }

    /// 渲染客户区到任意 DC（WM_PAINT 与 WM_PRINTCLIENT 共用）。
    unsafe fn render(&mut self, hdc: HDC, width: i32, height: i32) {

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
        let bitmap = match CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) => bitmap,
            Err(_) => {
                let _ = DeleteDC(mem_dc);
                let _ = ReleaseDC(None, screen);
                return;
            }
        };
        let old = SelectObject(mem_dc, HGDIOBJ(bitmap.0));

        if let Some(g) = Graphics::from_hdc(mem_dc) {
            g.set_smoothing_antialias();
            g.set_text_cleartype();
            let dpi = self.theme.dpi;
        let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
            fill_rounded(&g, WINDOW_BACKGROUND, RectF::new(0.0, 0.0, width as f32, height as f32), 0.0);
            if let Some(pen) = super::gdip::Pen::new(BORDER, 1.0) {
                if let Some(mut path) = super::gdip::Path::new() {
                    path.add_rounded_rect(RectF::new(0.5, 0.5, width as f32 - 1.0, height as f32 - 1.0), super::window::corner_radius_px(dpi));
                    g.draw_path(&pen, &path);
                }
            }

            // 标题栏
            let title_h = s(40);
            self.title_rect = RectF::new(0.0, 0.0, (width - s(40)) as f32, title_h as f32);
            let texts = crate::i18n::texts();
            draw_text(&g, &mut self.theme, mem_dc, texts.settings_title, 14, true, TEXT,
                RectF::new(s(PAD) as f32, 0.0, (width - s(60)) as f32, title_h as f32),
                super::drawing::TextAlign::LeftCenter);
            self.close_rect = RectF::new((width - s(34)) as f32, s(8) as f32, s(24) as f32, s(24) as f32);
            super::gdip::draw_glyph(&g, GLYPH_CLOSE, self.theme.icon_font(mem_dc, 10), self.close_rect, MUTED_TEXT);

            // 表单标签
            for (label, y) in [
                (texts.label_refresh, Y_TK),
                (texts.label_tpc, Y_TPC),
                (texts.label_language, Y_LANG),
            ] {
                draw_text(&g, &mut self.theme, mem_dc, label, 12, false, SECONDARY_TEXT,
                    RectF::new(s(PAD) as f32, s(y) as f32, s(LABEL_W) as f32, s(ROW_H) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }

            // 服务器列表
            let servers = self.state.cfg.read().map(|cfg| cfg.servers.clone()).unwrap_or_default();
            draw_text(&g, &mut self.theme, mem_dc, texts.section_servers, 12, true, SECONDARY_TEXT,
                RectF::new(s(PAD) as f32, s(Y_SERVER_SECTION) as f32, s(200) as f32, s(20) as f32),
                super::drawing::TextAlign::LeftCenter);
            self.rows.clear();
            let mut y = Y_SERVER_ROWS;
            for server in servers.iter().take(MAX_ROWS) {
                let row_rect = RectF::new(s(PAD) as f32, s(y) as f32, (width - s(PAD) * 2) as f32, s(40) as f32);
                fill_rounded(&g, CARD_BACKGROUND, row_rect, s(8) as f32);
                stroke_rounded(&g, BORDER, 1.0, row_rect, s(8) as f32);
                draw_text(&g, &mut self.theme, mem_dc, &server.display_label(), 12, true, TEXT,
                    RectF::new(row_rect.x + s(10) as f32, row_rect.y + s(4) as f32, row_rect.w - s(120) as f32, s(16) as f32),
                    super::drawing::TextAlign::LeftCenter);
                draw_text(&g, &mut self.theme, mem_dc, &server.display_user_host(), 11, false, MUTED_TEXT,
                    RectF::new(row_rect.x + s(10) as f32, row_rect.y + s(20) as f32, row_rect.w - s(120) as f32, s(16) as f32),
                    super::drawing::TextAlign::LeftCenter);
                let btn = s(28);
                let edit_rect = RectF::new(row_rect.right() - btn as f32 * 2.0 - s(12) as f32, row_rect.y + s(6) as f32, btn as f32, btn as f32);
                let delete_rect = RectF::new(row_rect.right() - btn as f32 - s(8) as f32, row_rect.y + s(6) as f32, btn as f32, btn as f32);
                fill_rounded(&g, HOVER, edit_rect, s(6) as f32);
                super::gdip::draw_glyph(&g, GLYPH_EDIT, self.theme.icon_font(mem_dc, 12), edit_rect, SECONDARY_TEXT);
                fill_rounded(&g, RED_SOFT, delete_rect, s(6) as f32);
                super::gdip::draw_glyph(&g, GLYPH_DELETE, self.theme.icon_font(mem_dc, 12), delete_rect, RED);
                self.rows.push((edit_rect, delete_rect, server.id));
                y += ROW_STEP;
            }
            if servers.len() > MAX_ROWS {
                draw_text(&g, &mut self.theme, mem_dc, &crate::i18n::servers_more(servers.len() - MAX_ROWS), 11, false, MUTED_TEXT,
                    RectF::new(s(PAD) as f32, s(y) as f32, s(300) as f32, s(18) as f32),
                    super::drawing::TextAlign::LeftCenter);
                y += 20;
            }
            self.add_server_rect = RectF::new(s(PAD) as f32, s(y + 4) as f32, s(120) as f32, s(30) as f32);
            fill_rounded(&g, ACCENT_SOFT, self.add_server_rect, s(6) as f32);
            draw_text(&g, &mut self.theme, mem_dc, texts.btn_add_server, 12, false, ACCENT,
                self.add_server_rect, super::drawing::TextAlign::CenterCenter);

            // 页脚注释：贴底对齐（仅当服务器列表未占满时才有位置，避免与「添加服务器」交叠）
            let hint_y = Y_BOTTOM_BUTTONS - 36;
            if y + 4 + 30 + 12 <= hint_y {
                draw_text(&g, &mut self.theme, mem_dc,
                    texts.footer_cache_hint, 11, false, MUTED_TEXT,
                    RectF::new(s(PAD) as f32, s(hint_y) as f32, (width - s(PAD) * 2) as f32, s(18) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }

            // 页脚分隔线：让列表下方的空白读作「列表预留空间」，而不是漏画的内容。
            if let Some(pen) = super::gdip::Pen::new(BORDER, 1.0) {
                if let Some(mut path) = super::gdip::Path::new() {
                    path.add_rounded_rect(
                        RectF::new(s(PAD) as f32, s(Y_BOTTOM_BUTTONS - 14) as f32, (width - s(PAD) * 2) as f32, 1.0),
                        0.0,
                    );
                    g.draw_path(&pen, &path);
                }
            }

            // 状态文本（与底部按钮同一行左侧，避免纵向抢位）
            if !self.status_text.is_empty() {
                let status_w = (W_LOGICAL - PAD * 2 - 88 * 2 - 18) as f32;
                draw_text(&g, &mut self.theme, mem_dc, &self.status_text, 11, false, MUTED_TEXT,
                    RectF::new(s(PAD) as f32, s(Y_BOTTOM_BUTTONS) as f32, s(status_w as i32) as f32, s(30) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }

            // 自绘按钮（绘制顺序在状态文本之后，保证在顶层）
            for button in &self.drawn_buttons {
                button.paint(&g, &mut self.theme, mem_dc);
            }

            // 扁平 EDIT 的 1px 细边框
            for y in [Y_TK, Y_TPC] {
                let edit_rect = RectF::new(
                    s(EDIT_X) as f32,
                    s(y) as f32,
                    s(EDIT_W) as f32,
                    s(ROW_H) as f32,
                );
                stroke_rounded(&g, BORDER, 1.0, edit_rect, s(6) as f32);
            }
        }
        let _ = BitBlt(hdc, 0, 0, width, height, Some(mem_dc), 0, 0, SRCCOPY);
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen);
    }
}
