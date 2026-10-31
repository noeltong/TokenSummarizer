//! 添加/编辑服务器对话框：名称、主机/端口/用户名、密码或私钥认证、
//! 通知开关、测试连接、重新信任主机密钥。自绘框架 + 原生控件。

use std::sync::Arc;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint,
    GetDC, InvalidateRect, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HDC, HGDIOBJ, PAINTSTRUCT, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    GetWindowRect, GWLP_USERDATA, PostMessageW, RegisterClassExW, SendMessageW,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WNDCLASSEXW, WM_APP,
    WM_COMMAND, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_NCCREATE,
    WM_NCLBUTTONDOWN, WM_PAINT, WM_PRINTCLIENT, BN_CLICKED, ES_AUTOHSCROLL, ES_PASSWORD,
    ES_NUMBER, HTCAPTION,
    SWP_SHOWWINDOW, WS_CHILD, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP, WS_TABSTOP, WS_VISIBLE, SW_HIDE, SW_SHOW, WINDOW_STYLE,
};

use super::super::app::WM_APP_DIALOG_CLOSED;
use super::super::config::{self, AuthType, ServerConfig};
use super::super::monitoring::state::AppState;
use super::super::monitoring::UiCommand;
use super::super::ssh::tester::TestReport;
use super::dialog::{self, ControlFont};
use super::drawing::{draw_multiline, draw_text, fill_rounded, stroke_rounded};
use super::gdip::{Graphics, RectF};
use super::theme::{
    Theme, ACCENT_SOFT, BORDER, CARD_BACKGROUND, GLYPH_CLOSE, GREEN,
    GREEN_SOFT, MUTED_TEXT, RED, RED_SOFT, SECONDARY_TEXT, TEXT, WINDOW_BACKGROUND,
};

const W_LOGICAL: i32 = 440;
const H_LOGICAL: i32 = 520;

/// 测试连接完成（wParam = Box<TestReport>）。
pub const WM_APP_TEST_DONE: u32 = WM_APP + 16;

// 布局（逻辑像素）
const PAD: i32 = 16;
const LABEL_X: i32 = PAD;
const LABEL_W: i32 = 86;
const FIELD_X: i32 = 110;
const FIELD_W: i32 = 300;
const ROW_H: i32 = 26;
const Y_NAME: i32 = 52;
const Y_HOST: i32 = 88;
const Y_USER: i32 = 124;
const Y_AUTH: i32 = 158;
const Y_SECRET: i32 = 188;
const Y_PASSPHRASE: i32 = 224;
const Y_TEST_ROW: i32 = 296;
const Y_RESULT: i32 = 334;
const RESULT_H: i32 = 72;
const Y_BOTTOM_BUTTONS: i32 = H_LOGICAL - 64;

// 控件 ID
const ID_NAME: i32 = 3001;
const ID_HOST: i32 = 3002;
const ID_PORT: i32 = 3003;
const ID_USER: i32 = 3004;
const ID_RADIO_PASSWORD: i32 = 3101;
const ID_RADIO_KEY: i32 = 3102;
const ID_PASSWORD: i32 = 3201;
const ID_KEY_PATH: i32 = 3202;
const ID_BTN_BROWSE: i32 = 3203;
const ID_PASSPHRASE: i32 = 3204;
const ID_BTN_TEST: i32 = 3401;
const ID_BTN_RETRUST: i32 = 3402;
const ID_BTN_SAVE: i32 = 3501;
const ID_BTN_CANCEL: i32 = 3502;

struct EditState {
    parent: HWND,
    hwnd: HWND,
    state: Arc<AppState>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<UiCommand>,
    edit: Option<ServerConfig>,
    font: Option<ControlFont>,
    theme: Theme,
    auth_password: bool,
    testing: bool,
    test_report: Option<TestReport>,
    retrust_clicked: bool,
    /// 自绘按钮（保存/取消/测试/浏览/重新信任）。
    drawn_buttons: Vec<super::button::DrawnButton>,
    close_rect: RectF,
    title_rect: RectF,
}

unsafe fn userdata(hwnd: HWND) -> Option<&'static mut EditState> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut EditState;
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
            lpszClassName: w!("TokenSummarizerEditWnd"),
            ..Default::default()
        };
        RegisterClassExW(&wc) != 0
    }
}

pub fn open(
    parent: HWND,
    state: Arc<AppState>,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<UiCommand>,
    edit: Option<ServerConfig>,
) {
    unsafe {
        let instance = GetModuleHandleW(None).ok().expect("GetModuleHandleW");
        let app = Box::new(EditState {
            parent,
            hwnd: HWND::default(),
            state,
            cmd_tx,
            auth_password: edit
                .as_ref()
                .map(|server| server.auth_type == AuthType::Password)
                .unwrap_or(true),
            edit,
            font: None,
            theme: Theme::new(96),
            testing: false,
            test_report: None,
            retrust_clicked: false,
            drawn_buttons: Vec::new(),
            close_rect: RectF::default(),
            title_rect: RectF::default(),
        });
        let title = crate::i18n::to_wide(crate::i18n::texts().server_window_title);
        let _ = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("TokenSummarizerEditWnd"),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            200,
            140,
            100,
            100,
            Some(parent),
            None,
            Some(instance.into()),
            Some(Box::into_raw(app) as *mut core::ffi::c_void),
        )
        .ok()
        .map(|hwnd| {
            let _ = SetForegroundWindow(hwnd);
        });
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => DefWindowProcW(hwnd, msg, wparam, lparam),
        WM_CREATE => {
            let create = &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            let ptr = create.lpCreateParams as *mut EditState;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
            let app = &mut *ptr;
            app.hwnd = hwnd;
            let dpi = GetDpiForWindow(hwnd);
            app.theme = Theme::new(dpi);
            let w = app.theme.scale(W_LOGICAL);
            let h = app.theme.scale(H_LOGICAL);
            app.font = ControlFont::create(dpi);
            app.create_controls();
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
        WM_APP_TEST_DONE => {
            if wparam.0 != 0 {
                let report = Box::from_raw(wparam.0 as *mut TestReport);
                if let Some(app) = userdata(hwnd) {
                    app.testing = false;
                    app.test_report = Some(*report);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut EditState;
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

impl EditState {
    unsafe fn create_controls(&mut self) {
        let dpi = self.theme.dpi;
        let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
        let hwnd = self.hwnd;
        let edit_cfg = self.edit.as_ref();

        let mk_edit = |id: i32, text: &str, x: i32, y: i32, w: i32, password: bool, numeric: bool| {
            let mut style = (WS_CHILD | WS_VISIBLE | WS_TABSTOP).0 | ES_AUTOHSCROLL as u32;
            if password {
                style |= ES_PASSWORD as u32;
            }
            if numeric {
                style |= ES_NUMBER as u32;
            }
            // 扁平化：不用 WS_EX_CLIENTEDGE，边框由父窗口绘制
            if let Some(control) = dialog::create_child(
                hwnd, "EDIT", text, Default::default(), WINDOW_STYLE(style),
                s(x), s(y), s(w), s(ROW_H), id as u16,
            ) {
                if let Some(font) = &self.font {
                    font.apply(control);
                }
            }
        };
        let mk_radio = |id: i32, text: &str, x: i32, y: i32, checked: bool| {
            if let Some(control) = dialog::create_child(
                hwnd, "BUTTON", text, Default::default(),
                WINDOW_STYLE(
                    (WS_CHILD | WS_VISIBLE).0 | windows::Win32::UI::WindowsAndMessaging::BS_AUTORADIOBUTTON as u32,
                ),
                s(x), s(y), s(88), s(ROW_H), id as u16,
            ) {
                if let Some(font) = &self.font {
                    font.apply(control);
                }
                if checked {
                    dialog::check(control, true);
                }
            }
        };

        mk_edit(ID_NAME, edit_cfg.and_then(|s| s.name.as_deref()).unwrap_or(""), FIELD_X, Y_NAME, FIELD_W, false, false);
        mk_edit(ID_HOST, edit_cfg.map(|s| s.host.as_str()).unwrap_or(""), FIELD_X, Y_HOST, 170, false, false);
        mk_edit(ID_PORT, &edit_cfg.map(|s| s.port.to_string()).unwrap_or_else(|| "22".into()), 342, Y_HOST, 68, false, true);
        mk_edit(ID_USER, edit_cfg.map(|s| s.username.as_str()).unwrap_or(""), FIELD_X, Y_USER, FIELD_W, false, false);

        let texts = crate::i18n::texts();
        mk_radio(ID_RADIO_PASSWORD, texts.radio_password, FIELD_X, Y_AUTH, self.auth_password);
        mk_radio(ID_RADIO_KEY, texts.radio_key, FIELD_X + 96, Y_AUTH, !self.auth_password);

        mk_edit(ID_PASSWORD, "", FIELD_X, Y_SECRET, FIELD_W, true, false);
        mk_edit(ID_KEY_PATH, edit_cfg.and_then(|s| s.private_key_path.as_deref()).unwrap_or(""), FIELD_X, Y_SECRET, 210, false, false);
        mk_edit(ID_PASSPHRASE, "", FIELD_X, Y_PASSPHRASE, FIELD_W, true, false);

        // 按钮全部自绘（含"浏览…""重新信任"）；"重新信任"仅编辑已固定指纹的服务器时出现
        let mut buttons = vec![
            super::button::DrawnButton::new(ID_BTN_BROWSE, texts.btn_browse, 330, Y_SECRET - 1, 80, 28, super::button::ButtonStyle::Secondary),
            super::button::DrawnButton::new(ID_BTN_TEST, texts.btn_test, PAD, Y_TEST_ROW, 100, 28, super::button::ButtonStyle::Secondary),
            super::button::DrawnButton::new(ID_BTN_SAVE, texts.btn_save, W_LOGICAL - PAD - 88, Y_BOTTOM_BUTTONS, 88, 30, super::button::ButtonStyle::Primary),
            super::button::DrawnButton::new(ID_BTN_CANCEL, texts.btn_cancel, W_LOGICAL - PAD - 88 * 2 - 10, Y_BOTTOM_BUTTONS, 88, 30, super::button::ButtonStyle::Secondary),
        ];
        if self.edit.is_some() && edit_cfg.map(|s| s.host_key_fingerprint_sha256.is_some()).unwrap_or(false) {
            buttons.push(super::button::DrawnButton::new(ID_BTN_RETRUST, texts.btn_retrust, PAD + 110, Y_TEST_ROW, 150, 28, super::button::ButtonStyle::Danger));
        }
        self.drawn_buttons = buttons;

        self.apply_auth_visibility(hwnd);
    }

    unsafe fn apply_auth_visibility(&mut self, hwnd: HWND) {
        // 原生控件（EDIT）显隐
        let items = [
            (ID_PASSWORD, self.auth_password),
            (ID_KEY_PATH, !self.auth_password),
            (ID_PASSPHRASE, !self.auth_password),
        ];
        for (id, show) in items {
            if let Ok(control) = windows::Win32::UI::WindowsAndMessaging::GetDlgItem(Some(hwnd), id) {
                let _ = ShowWindow(control, if show { SW_SHOW } else { SW_HIDE });
            }
        }
        // 自绘按钮："浏览…"仅私钥模式可见
        for button in &mut self.drawn_buttons {
            if button.id == ID_BTN_BROWSE {
                button.visible = !self.auth_password;
            }
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
    }

    /// 从表单构造服务器配置草稿（含明文密码/口令，供测试连接）。
    unsafe fn draft(&mut self, hwnd: HWND) -> ServerConfig {
        let name = dialog::get_item_text(hwnd, ID_NAME);
        let host = dialog::get_item_text(hwnd, ID_HOST).trim().to_string();
        let port: u16 = dialog::get_item_text(hwnd, ID_PORT).trim().parse().unwrap_or(22);
        let username = dialog::get_item_text(hwnd, ID_USER).trim().to_string();
        let password_plain = dialog::get_item_text(hwnd, ID_PASSWORD);
        let key_path = dialog::get_item_text(hwnd, ID_KEY_PATH).trim().to_string();
        let passphrase_plain = dialog::get_item_text(hwnd, ID_PASSPHRASE);
        let mut server = match &self.edit {
            Some(existing) => existing.clone(),
            None => ServerConfig {
                id: uuid::Uuid::new_v4(),
                name: None,
                host: String::new(),
                port: 22,
                username: String::new(),
                auth_type: AuthType::Password,
                protected_password: None,
                private_key_path: None,
                protected_key_passphrase: None,
                host_key_fingerprint_sha256: None,
            },
        };
        server.name = if name.trim().is_empty() { None } else { Some(name.trim().to_string()) };
        server.host = host;
        server.port = port;
        server.username = username;
        server.auth_type = if self.auth_password { AuthType::Password } else { AuthType::PrivateKey };
        if self.auth_password {
            if !password_plain.is_empty() {
                server.protected_password = config::secret::protect(&password_plain);
            }
            server.private_key_path = None;
            server.protected_key_passphrase = None;
        } else {
            server.private_key_path = if key_path.is_empty() { None } else { Some(key_path) };
            if !passphrase_plain.is_empty() {
                server.protected_key_passphrase = config::secret::protect(&passphrase_plain);
            }
            server.protected_password = None;
        }
        if self.retrust_clicked {
            server.host_key_fingerprint_sha256 = None;
        }
        server
    }

    unsafe fn on_button(&mut self, hwnd: HWND, id: i32) {
        match id {
            ID_RADIO_PASSWORD => {
                self.auth_password = true;
                self.apply_auth_visibility(hwnd);
            }
            ID_RADIO_KEY => {
                self.auth_password = false;
                self.apply_auth_visibility(hwnd);
            }
            ID_BTN_BROWSE => self.browse_key(hwnd),
            ID_BTN_TEST => self.run_test(hwnd),
            ID_BTN_RETRUST => {
                self.retrust_clicked = true;
                for button in &mut self.drawn_buttons {
                    if button.id == ID_BTN_RETRUST {
                        button.visible = false;
                    }
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            ID_BTN_SAVE => self.save(hwnd),
            ID_BTN_CANCEL => { let _ = DestroyWindow(hwnd); }
            _ => {}
        }
    }

    unsafe fn browse_key(&mut self, hwnd: HWND) {
        let mut file = [0u16; 520];
        let initial = dialog::get_item_text(hwnd, ID_KEY_PATH);
        let wide: Vec<u16> = initial.encode_utf16().collect();
        let len = wide.len().min(500);
        file[..len].copy_from_slice(&wide[..len]);
        // 过滤器（双 NUL 结尾）
        let filter: Vec<u16> = format!("{}\0*.*\0\0", crate::i18n::texts().filter_key_files)
            .encode_utf16()
            .collect();
        let mut ofn = OPENFILENAMEW {
            lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: hwnd,
            lpstrFilter: PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(file.as_mut_ptr()),
            nMaxFile: file.len() as u32,
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
            ..Default::default()
        };
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let len = file.iter().position(|c| *c == 0).unwrap_or(file.len());
            let path = String::from_utf16_lossy(&file[..len]);
            dialog::set_item_text(hwnd, ID_KEY_PATH, &path);
        }
    }

    unsafe fn run_test(&mut self, hwnd: HWND) {
        if self.testing {
            return;
        }
        self.testing = true;
        self.test_report = None;
        let _ = InvalidateRect(Some(hwnd), None, false);
        let draft = self.draft(hwnd);
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel::<TestReport>();
        let _ = self.cmd_tx.send(UiCommand::TestConnection {
            server: draft,
            fingerprint_override: None,
            reply: reply_tx,
        });
        let target = hwnd.0 as usize;
        std::thread::spawn(move || {
            // 阻塞等待测试结果（测试本身有 10s 连接超时）
            let report = match reply_rx.blocking_recv() {
                Ok(report) => report,
                Err(_) => return,
            };
            let boxed = Box::into_raw(Box::new(report)) as usize;
            let target_hwnd = HWND(target as *mut _);
            let _ = PostMessageW(Some(target_hwnd), WM_APP_TEST_DONE, WPARAM(boxed), LPARAM(0));
        });
    }

    unsafe fn save(&mut self, hwnd: HWND) {
        let mut draft = self.draft(hwnd);
        if draft.host.trim().is_empty() || draft.username.trim().is_empty() {
            self.test_report = None;
            let _ = InvalidateRect(Some(hwnd), None, false);
            return;
        }
        // 测试连接时若为首次（或重新信任），持久化观察到的指纹
        if let Some(report) = &self.test_report {
            if let Some(seen) = &report.seen_fingerprint {
                if draft.host_key_fingerprint_sha256.is_none() {
                    draft.host_key_fingerprint_sha256 = Some(seen.clone());
                }
            }
        }
        let mut cfg = self.state.cfg.read().map(|cfg| cfg.clone()).unwrap_or_default();
        match cfg.servers.iter_mut().find(|s| s.id == draft.id) {
            Some(existing) => *existing = draft,
            None => cfg.servers.push(draft),
        }
        self.state.replace_config(cfg);
        let _ = self.cmd_tx.send(UiCommand::RestartMonitors);
        let _ = DestroyWindow(hwnd);
    }

    unsafe fn on_mouse_down(&mut self, hwnd: HWND, x: i32, y: i32) {
        if self.close_rect.contains(x, y) {
            let _ = DestroyWindow(hwnd);
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
            let _ = SendMessageW(hwnd, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), None);
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

            let is_edit = self.edit.is_some();
            let texts = crate::i18n::texts();
            let title = if is_edit { texts.title_edit_server } else { texts.title_add_server };
            let title_h = s(40);
            self.title_rect = RectF::new(0.0, 0.0, (width - s(40)) as f32, title_h as f32);
            draw_text(&g, &mut self.theme, mem_dc, title, 14, true, TEXT,
                RectF::new(s(PAD) as f32, 0.0, (width - s(60)) as f32, title_h as f32),
                super::drawing::TextAlign::LeftCenter);
            self.close_rect = RectF::new((width - s(34)) as f32, s(8) as f32, s(24) as f32, s(24) as f32);
            super::gdip::draw_glyph(&g, GLYPH_CLOSE, self.theme.icon_font(mem_dc, 10), self.close_rect, MUTED_TEXT);

            let labels: [(&str, i32); 3] = [(texts.label_name, Y_NAME), (texts.label_host, Y_HOST), (texts.label_user, Y_USER)];
            for (label, y) in labels {
                draw_text(&g, &mut self.theme, mem_dc, label, 12, false, SECONDARY_TEXT,
                    RectF::new(s(LABEL_X) as f32, s(y) as f32, s(LABEL_W) as f32, s(ROW_H) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }
            draw_text(&g, &mut self.theme, mem_dc, texts.label_port, 12, false, SECONDARY_TEXT,
                RectF::new(s(296) as f32, s(Y_HOST) as f32, s(40) as f32, s(ROW_H) as f32),
                super::drawing::TextAlign::LeftCenter);
            draw_text(&g, &mut self.theme, mem_dc, texts.label_auth, 12, false, SECONDARY_TEXT,
                RectF::new(s(LABEL_X) as f32, s(Y_AUTH) as f32, s(LABEL_W) as f32, s(ROW_H) as f32),
                super::drawing::TextAlign::LeftCenter);
            draw_text(&g, &mut self.theme, mem_dc,
                if self.auth_password { texts.radio_password } else { texts.label_key_path },
                12, false, SECONDARY_TEXT,
                RectF::new(s(LABEL_X) as f32, s(Y_SECRET) as f32, s(LABEL_W) as f32, s(ROW_H) as f32),
                super::drawing::TextAlign::LeftCenter);
            // 私钥口令只在私钥认证模式下有意义；密码模式下控件已隐藏，标签也要一并跳过。
            if !self.auth_password {
                draw_text(&g, &mut self.theme, mem_dc, texts.label_passphrase, 12, false, SECONDARY_TEXT,
                    RectF::new(s(LABEL_X) as f32, s(Y_PASSPHRASE) as f32, s(LABEL_W) as f32, s(ROW_H) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }

            // 测试结果卡片
            let result_rect = RectF::new(s(PAD) as f32, s(Y_RESULT) as f32, (width - s(PAD) * 2) as f32, s(RESULT_H) as f32);
            if self.testing {
                fill_rounded(&g, ACCENT_SOFT, result_rect, s(8) as f32);
                draw_text(&g, &mut self.theme, mem_dc, texts.testing_now, 12, false, TEXT,
                    RectF::new(result_rect.x + s(10) as f32, result_rect.y + s(8) as f32, result_rect.w - s(20) as f32, s(20) as f32),
                    super::drawing::TextAlign::LeftCenter);
            } else if let Some(report) = &self.test_report {
                let (bg, fg) = if report.ok { (GREEN_SOFT, GREEN) } else { (RED_SOFT, RED) };
                fill_rounded(&g, bg, result_rect, s(8) as f32);
                let headline = report.headline();
                let mut details = String::new();
                for (name, ok, detail) in &report.steps {
                    details.push_str(&format!("{} {}: {}\n", if *ok { "[OK]" } else { "[FAIL]" }, name, detail));
                }
                draw_text(&g, &mut self.theme, mem_dc, &headline, 12, true, fg,
                    RectF::new(result_rect.x + s(10) as f32, result_rect.y + s(6) as f32, result_rect.w - s(20) as f32, s(20) as f32),
                    super::drawing::TextAlign::LeftCenter);
                draw_multiline(&g, &mut self.theme, mem_dc, details.trim_end(), 11, SECONDARY_TEXT,
                    RectF::new(result_rect.x + s(10) as f32, result_rect.y + s(28) as f32, result_rect.w - s(20) as f32, s(RESULT_H - 32) as f32),
                    s(16) as f32);
            } else {
                fill_rounded(&g, CARD_BACKGROUND, result_rect, s(8) as f32);
                stroke_rounded(&g, BORDER, 1.0, result_rect, s(8) as f32);
                let hint = if is_edit {
                    texts.hint_edit
                } else {
                    texts.hint_add
                };
                draw_text(&g, &mut self.theme, mem_dc, hint, 11, false, MUTED_TEXT,
                    RectF::new(result_rect.x + s(10) as f32, result_rect.y + s(6) as f32, result_rect.w - s(20) as f32, s(RESULT_H - 12) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }

            // 自绘按钮（顶层）
            for button in &self.drawn_buttons {
                button.paint(&g, &mut self.theme, mem_dc);
            }

            // 扁平 EDIT 的 1px 细边框（只画可见项）
            let visible_edits = [
                (ID_NAME, FIELD_X, Y_NAME, FIELD_W),
                (ID_HOST, FIELD_X, Y_HOST, 170),
                (ID_PORT, 342, Y_HOST, 68),
                (ID_USER, FIELD_X, Y_USER, FIELD_W),
                (ID_PASSWORD, FIELD_X, Y_SECRET, FIELD_W),
                (ID_KEY_PATH, FIELD_X, Y_SECRET, 210),
                (ID_PASSPHRASE, FIELD_X, Y_PASSPHRASE, FIELD_W),
            ];
            for (id, x, y, w) in visible_edits {
                let shown = match id {
                    ID_PASSWORD => self.auth_password,
                    ID_KEY_PATH | ID_PASSPHRASE => !self.auth_password,
                    _ => true,
                };
                if shown {
                    let edit_rect = RectF::new(s(x) as f32, s(y) as f32, s(w) as f32, s(ROW_H) as f32);
                    stroke_rounded(&g, BORDER, 1.0, edit_rect, s(6) as f32);
                }
            }
        }
        let _ = BitBlt(hdc, 0, 0, width, height, Some(mem_dc), 0, 0, SRCCOPY);
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen);
    }
}
