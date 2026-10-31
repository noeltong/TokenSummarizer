//! 模型别名对话框：把原始模型名映射为更友好的显示名（精确匹配、聚合合并）。
//! 列表 + 底部输入框添加/编辑；修改即时持久化（仅影响图表聚合，无需重启监控）。

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
    GetWindowRect, GWLP_USERDATA, PostMessageW, RegisterClassExW, SendMessageW,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, SWP_SHOWWINDOW, WNDCLASSEXW,
    WM_CREATE, WM_DESTROY,
    WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_NCCREATE, WM_NCLBUTTONDOWN, WM_PAINT,
    WM_PRINTCLIENT, HTCAPTION,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::super::app::WM_APP_DIALOG_CLOSED;
use super::super::config::AliasEntry;
use super::super::monitoring::state::AppState;
use super::button::{ButtonStyle, DrawnButton};
use super::dialog::{self, ControlFont};
use super::drawing::{draw_text, fill_rounded, stroke_rounded};
use super::gdip::{Graphics, RectF};
use super::theme::{
    Theme, ACCENT_SOFT, BORDER, CARD_BACKGROUND, GLYPH_CLOSE, GLYPH_DELETE, GLYPH_EDIT, HOVER,
    MUTED_TEXT, RED, RED_SOFT, SECONDARY_TEXT, TEXT, WINDOW_BACKGROUND,
};

const W_LOGICAL: i32 = 460;
const H_LOGICAL: i32 = 480;

// 布局（逻辑像素）
const PAD: i32 = 16;
const INPUT_LABEL_W: i32 = 84;
const INPUT1_X: i32 = PAD + INPUT_LABEL_W; // 原始模型名输入框
const INPUT1_W: i32 = 136;
const LABEL2_X: i32 = INPUT1_X + INPUT1_W + 12; // 显示别名标签
const LABEL2_W: i32 = 60;
const INPUT2_X: i32 = LABEL2_X + LABEL2_W;
const INPUT2_W: i32 = W_LOGICAL - PAD - INPUT2_X;
const ROW_H: i32 = 26;
const Y_INPUTS: i32 = 52;
const Y_ROWS: i32 = 106;
const ROW_STEP: i32 = 42;
const MAX_ROWS: usize = 7;

// 控件 ID
const ID_ALIAS_FROM: i32 = 4001;
const ID_ALIAS_TO: i32 = 4002;
const ID_BTN_ADD: i32 = 4101; // 添加 / 保存修改
const ID_BTN_CLOSE: i32 = 4102;

struct AliasState {
    parent: HWND,
    hwnd: HWND,
    state: Arc<AppState>,
    font: Option<ControlFont>,
    theme: Theme,
    /// (编辑图标, 删除图标, 条目下标)
    rows: Vec<(RectF, RectF, usize)>,
    /// 正在编辑的条目下标；None = 输入框用于新增。
    editing: Option<usize>,
    buttons: Vec<DrawnButton>,
    close_rect: RectF,
    title_rect: RectF,
    status_text: String,
}

unsafe fn userdata(hwnd: HWND) -> Option<&'static mut AliasState> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AliasState;
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
            lpszClassName: w!("TokenSummarizerAliasWnd"),
            ..Default::default()
        };
        RegisterClassExW(&wc) != 0
    }
}

pub fn open(parent: HWND, state: Arc<AppState>) {
    unsafe {
        let instance = GetModuleHandleW(None).ok().expect("GetModuleHandleW");
        let app = Box::new(AliasState {
            parent,
            hwnd: HWND::default(),
            state,
            font: None,
            theme: Theme::new(96),
            rows: Vec::new(),
            editing: None,
            buttons: Vec::new(),
            close_rect: RectF::default(),
            title_rect: RectF::default(),
            status_text: String::new(),
        });
        let title = crate::i18n::to_wide(crate::i18n::texts().alias_window_title);
        match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("TokenSummarizerAliasWnd"),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            200,
            160,
            100,
            100,
            Some(parent),
            None,
            Some(instance.into()),
            Some(Box::into_raw(app) as *mut core::ffi::c_void),
        ) {
            Ok(hwnd) => {
                let _ = SetForegroundWindow(hwnd);
            }
            Err(err) => crate::log_error!("别名对话框创建失败: {err}"),
        }
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => DefWindowProcW(hwnd, msg, wparam, lparam),
        WM_CREATE => {
            let create =
                &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
            let ptr = create.lpCreateParams as *mut AliasState;
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
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AliasState;
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

impl AliasState {
    unsafe fn create_controls(&mut self) {
        let s = |v: i32| {
            let dpi = self.theme.dpi;
            ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32
        };
        let hwnd = self.hwnd;
        let edit_style = windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(
            (windows::Win32::UI::WindowsAndMessaging::WS_CHILD
                | windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE
                | windows::Win32::UI::WindowsAndMessaging::WS_TABSTOP)
                .0
                | windows::Win32::UI::WindowsAndMessaging::ES_AUTOHSCROLL as u32,
        );
        for (id, x, w) in [
            (ID_ALIAS_FROM, INPUT1_X, INPUT1_W),
            (ID_ALIAS_TO, INPUT2_X, INPUT2_W),
        ] {
            if let Some(control) = dialog::create_child(
                hwnd,
                "EDIT",
                "",
                Default::default(),
                edit_style,
                s(x),
                s(Y_INPUTS),
                s(w),
                s(ROW_H),
                id as u16,
            ) {
                if let Some(font) = &self.font {
                    font.apply(control);
                }
            }
        }
        let texts = crate::i18n::texts();
        self.buttons = vec![
            DrawnButton::new(ID_BTN_ADD, texts.btn_add, W_LOGICAL - PAD - 88 * 2 - 10, H_LOGICAL - 56, 88, 30, ButtonStyle::Primary),
            DrawnButton::new(ID_BTN_CLOSE, texts.btn_close, W_LOGICAL - PAD - 88, H_LOGICAL - 56, 88, 30, ButtonStyle::Secondary),
        ];
    }

    unsafe fn on_button(&mut self, hwnd: HWND, id: i32) {
        match id {
            ID_BTN_ADD => self.save_entry(hwnd),
            ID_BTN_CLOSE => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
    }

    /// 添加或保存当前输入的映射。
    unsafe fn save_entry(&mut self, hwnd: HWND) {
        let from = dialog::get_item_text(hwnd, ID_ALIAS_FROM).trim().to_string();
        let to = dialog::get_item_text(hwnd, ID_ALIAS_TO).trim().to_string();
        if from.is_empty() {
            self.status_text = crate::i18n::texts().alias_from_empty_status.to_string();
            let _ = InvalidateRect(Some(hwnd), None, false);
            return;
        }
        let mut cfg = self
            .state
            .cfg
            .read()
            .map(|cfg| cfg.clone())
            .unwrap_or_default();
        if let Some(index) = self.editing {
            // 保存修改
            if let Some(entry) = cfg.model_aliases.get_mut(index) {
                entry.from = from.clone();
                entry.to = to.clone();
            }
            self.editing = None;
            if let Some(button) = self.buttons.iter_mut().find(|b| b.id == ID_BTN_ADD) {
                button.text = crate::i18n::texts().btn_add.to_string();
            }
        } else {
            // 新增：同 from 覆盖
            if let Some(existing) = cfg.model_aliases.iter_mut().find(|e| e.from == from) {
                existing.to = to;
            } else {
                cfg.model_aliases.push(AliasEntry { from, to });
            }
        }
        dialog::set_item_text(hwnd, ID_ALIAS_FROM, "");
        dialog::set_item_text(hwnd, ID_ALIAS_TO, "");
        self.status_text.clear();
        self.state.replace_config(cfg);
        let _ = InvalidateRect(Some(hwnd), None, false);
    }

    unsafe fn on_mouse_down(&mut self, hwnd: HWND, x: i32, y: i32) {
        if self.close_rect.contains(x, y) {
            let _ = DestroyWindow(hwnd);
            return;
        }
        let dpi = self.theme.dpi;
        let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
        for index in 0..self.buttons.len() {
            if self.buttons[index].hit(s, x, y) {
                let id = self.buttons[index].id;
                self.buttons[index].pressed = true;
                let _ = InvalidateRect(Some(hwnd), None, false);
                self.on_button(hwnd, id);
                return;
            }
        }
        if self.title_rect.contains(x, y) {
            let _ = ReleaseCapture();
            let _ = SendMessageW(hwnd, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), None);
            return;
        }
        for (edit_rect, delete_rect, index) in self.rows.clone() {
            if edit_rect.contains(x, y) {
                let cfg = self
                    .state
                    .cfg
                    .read()
                    .map(|cfg| cfg.clone())
                    .unwrap_or_default();
                if let Some(entry) = cfg.model_aliases.get(index) {
                    dialog::set_item_text(hwnd, ID_ALIAS_FROM, &entry.from);
                    dialog::set_item_text(hwnd, ID_ALIAS_TO, &entry.to);
                }
                self.editing = Some(index);
                if let Some(button) = self.buttons.iter_mut().find(|b| b.id == ID_BTN_ADD) {
                    button.text = crate::i18n::texts().btn_save_edit.to_string();
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                return;
            }
            if delete_rect.contains(x, y) {
                let mut cfg = self
                    .state
                    .cfg
                    .read()
                    .map(|cfg| cfg.clone())
                    .unwrap_or_default();

                if index < cfg.model_aliases.len() {
                    cfg.model_aliases.remove(index);
                }
                self.state.replace_config(cfg);
                self.editing = None;
                if let Some(button) = self.buttons.iter_mut().find(|b| b.id == ID_BTN_ADD) {
                    button.text = crate::i18n::texts().btn_add.to_string();
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
                return;
            }
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
        let bitmap = match CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)
        {
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
            draw_text(&g, &mut self.theme, mem_dc, texts.alias_window_title, 14, true, TEXT,
                RectF::new(s(PAD) as f32, 0.0, (width - s(60)) as f32, title_h as f32),
                super::drawing::TextAlign::LeftCenter);
            self.close_rect = RectF::new((width - s(34)) as f32, s(8) as f32, s(24) as f32, s(24) as f32);
            super::gdip::draw_glyph(&g, GLYPH_CLOSE, self.theme.icon_font(mem_dc, 10), self.close_rect, MUTED_TEXT);

            // 输入区标签
            draw_text(&g, &mut self.theme, mem_dc, texts.label_from, 12, false, SECONDARY_TEXT,
                RectF::new(s(PAD) as f32, s(Y_INPUTS) as f32, s(INPUT_LABEL_W) as f32, s(ROW_H) as f32),
                super::drawing::TextAlign::LeftCenter);
            draw_text(&g, &mut self.theme, mem_dc, texts.label_to, 12, false, SECONDARY_TEXT,
                RectF::new(s(LABEL2_X) as f32, s(Y_INPUTS) as f32, s(LABEL2_W) as f32, s(ROW_H) as f32),
                super::drawing::TextAlign::LeftCenter);
            // 扁平 EDIT 边框
            for (x, w) in [(INPUT1_X, INPUT1_W), (INPUT2_X, INPUT2_W)] {
                stroke_rounded(&g, BORDER, 1.0,
                    RectF::new(s(x) as f32, s(Y_INPUTS) as f32, s(w) as f32, s(ROW_H) as f32),
                    s(6) as f32);
            }

            // 映射列表
            let aliases = self
                .state
                .cfg
                .read()
                .map(|cfg| cfg.model_aliases.clone())
                .unwrap_or_default();
            draw_text(&g, &mut self.theme, mem_dc, &crate::i18n::mappings_count(aliases.len()), 12, true, SECONDARY_TEXT,
                RectF::new(s(PAD) as f32, s(Y_ROWS - 26) as f32, s(300) as f32, s(20) as f32),
                super::drawing::TextAlign::LeftCenter);
            self.rows.clear();
            let mut y = Y_ROWS;
            for (index, entry) in aliases.iter().take(MAX_ROWS).enumerate() {
                let row_rect = RectF::new(s(PAD) as f32, s(y) as f32, (width - s(PAD) * 2) as f32, s(34) as f32);
                fill_rounded(&g, CARD_BACKGROUND, row_rect, s(8) as f32);
                stroke_rounded(&g, BORDER, 1.0, row_rect, s(8) as f32);
                let from = super::drawing::ellipsize(&g, &mut self.theme, mem_dc, &entry.from, 12, false, s(160) as f32);
                let to = super::drawing::ellipsize(&g, &mut self.theme, mem_dc, &entry.to, 12, true, s(120) as f32);
                draw_text(&g, &mut self.theme, mem_dc, &from, 12, false, TEXT,
                    RectF::new(row_rect.x + s(10) as f32, row_rect.y, s(160) as f32, row_rect.h),
                    super::drawing::TextAlign::LeftCenter);
                draw_text(&g, &mut self.theme, mem_dc, "→", 12, false, MUTED_TEXT,
                    RectF::new(row_rect.x + s(170) as f32, row_rect.y, s(20) as f32, row_rect.h),
                    super::drawing::TextAlign::CenterCenter);
                draw_text(&g, &mut self.theme, mem_dc, &to, 12, true, TEXT,
                    RectF::new(row_rect.x + s(196) as f32, row_rect.y, s(120) as f32, row_rect.h),
                    super::drawing::TextAlign::LeftCenter);
                let btn = s(26);
                let edit_rect = RectF::new(row_rect.right() - btn as f32 * 2.0 - s(12) as f32, row_rect.y + s(4) as f32, btn as f32, btn as f32);
                let delete_rect = RectF::new(row_rect.right() - btn as f32 - s(8) as f32, row_rect.y + s(4) as f32, btn as f32, btn as f32);
                fill_rounded(&g, HOVER, edit_rect, s(6) as f32);
                super::gdip::draw_glyph(&g, GLYPH_EDIT, self.theme.icon_font(mem_dc, 11), edit_rect, SECONDARY_TEXT);
                fill_rounded(&g, RED_SOFT, delete_rect, s(6) as f32);
                super::gdip::draw_glyph(&g, GLYPH_DELETE, self.theme.icon_font(mem_dc, 11), delete_rect, RED);
                self.rows.push((edit_rect, delete_rect, index));
                y += ROW_STEP;
            }
            if aliases.len() > MAX_ROWS {
                draw_text(&g, &mut self.theme, mem_dc, &crate::i18n::aliases_more(aliases.len() - MAX_ROWS), 11, false, MUTED_TEXT,
                    RectF::new(s(PAD) as f32, s(y) as f32, s(300) as f32, s(18) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }
            if aliases.is_empty() {
                // 空态：用一张居中提示卡填充列表区，避免列表区留出大片无意义空白。
                let area = RectF::new(
                    s(PAD) as f32,
                    s(Y_ROWS) as f32,
                    (width - s(PAD) * 2) as f32,
                    (height - s(Y_ROWS) - s(84)).max(s(60)) as f32,
                );
                fill_rounded(&g, CARD_BACKGROUND, area, s(10) as f32);
                stroke_rounded(&g, BORDER, 1.0, area, s(10) as f32);
                draw_text(&g, &mut self.theme, mem_dc, texts.empty_aliases, 12, true, SECONDARY_TEXT,
                    RectF::new(area.x, area.y + area.h / 2.0 - s(22) as f32, area.w, s(20) as f32),
                    super::drawing::TextAlign::CenterCenter);
                draw_text(&g, &mut self.theme, mem_dc,
                    texts.empty_alias_hint, 11, false, MUTED_TEXT,
                    RectF::new(area.x, area.y + area.h / 2.0 + s(2) as f32, area.w, s(20) as f32),
                    super::drawing::TextAlign::CenterCenter);
            }

            // 状态文本
            if !self.status_text.is_empty() {
                draw_text(&g, &mut self.theme, mem_dc, &self.status_text, 11, false, RED,
                    RectF::new(s(PAD) as f32, (height - s(26)) as f32, (width - s(PAD) * 2) as f32, s(18) as f32),
                    super::drawing::TextAlign::LeftCenter);
            }

            // 自绘按钮
            for button in &self.buttons {
                button.paint(&g, &mut self.theme, mem_dc);
            }
        }
        let _ = BitBlt(hdc, 0, 0, width, height, Some(mem_dc), 0, 0, SRCCOPY);
        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen);
    }
}

// ACCENT_SOFT 用于提示卡片背景（此处保留引用避免告警）
#[allow(dead_code)]
const _ACCENT_SOFT: u32 = ACCENT_SOFT;
