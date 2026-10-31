//! GDI+ flat API 的最小 RAII 封装。
//! 全部文本绘制走 GDI+（DrawString/MeasureString），保证与绘制变换一致。

use std::sync::Once;

use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, CLEARTYPE_QUALITY, DEFAULT_CHARSET, HDC, HFONT, HGDIOBJ, LOGFONTW,
};
use windows::Win32::Graphics::GdiPlus::{
    GdipAddPathArc, GdipClosePathFigure, GdipCreateFontFromLogfontW, GdipCreateFromHDC,
    GdipCreatePath, GdipCreatePen1, GdipCreateSolidFill, GdipCreateStringFormat, GdipDeleteBrush,
    GdipDeleteFont, GdipDeleteGraphics, GdipDeletePath, GdipDeletePen, GdipDeleteStringFormat,
    GdipDrawPath, GdipDrawString, GdipFillPath, GdipMeasureString, GdipResetClip,
    GdipSetClipRect, GdipSetSmoothingMode,
    GdipSetTextRenderingHint, GdipSetStringFormatAlign, GdipSetStringFormatLineAlign,
    GdipSetStringFormatTrimming, GdiplusShutdown, GdiplusStartup, CombineModeReplace, FillMode,
    GpBrush, GpFont,
    GpGraphics, GpPath, GpPen, GpSolidFill, GpStringFormat,
    SmoothingModeAntiAlias, Status, StringAlignment, StringTrimming, TextRenderingHintClearTypeGridFit, Unit,
};

/// 进程级 GDI+ 初始化；程序退出时由 Drop 关闭。
pub struct GdiPlusGuard {
    token: usize,
}

static STARTUP: Once = Once::new();
static TOKEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl GdiPlusGuard {
    pub fn init() -> Option<Self> {
        unsafe {
            let mut token = 0usize;
            let mut result: Option<Self> = None;
            STARTUP.call_once(|| {
                let input = windows::Win32::Graphics::GdiPlus::GdiplusStartupInput {
                    GdiplusVersion: 2,
                    ..Default::default()
                };
                if GdiplusStartup(&mut token, &input, std::ptr::null_mut()) == Status(0) {
                    TOKEN.store(token, std::sync::atomic::Ordering::SeqCst);
                    result = Some(GdiPlusGuard { token });
                }
            });
            if result.is_none() {
                let existing = TOKEN.load(std::sync::atomic::Ordering::SeqCst);
                if existing != 0 {
                    result = Some(GdiPlusGuard { token: existing });
                }
            }
            result
        }
    }
}

impl Drop for GdiPlusGuard {
    fn drop(&mut self) {
        unsafe {
            GdiplusShutdown(self.token);
        }
    }
}

fn ok(status: Status) -> Result<(), Status> {
    if status == Status(0) {
        Ok(())
    } else {
        Err(status)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    fn raw(self) -> windows::Win32::Graphics::GdiPlus::RectF {
        windows::Win32::Graphics::GdiPlus::RectF {
            X: self.x,
            Y: self.y,
            Width: self.w,
            Height: self.h,
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        let (x, y) = (x as f32, y as f32);
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

pub struct Graphics {
    raw: *mut GpGraphics,
}

impl Graphics {
    pub fn from_hdc(hdc: HDC) -> Option<Self> {
        unsafe {
            let mut raw: *mut GpGraphics = std::ptr::null_mut();
            ok(GdipCreateFromHDC(hdc, &mut raw)).ok()?;
            Some(Graphics { raw })
        }
    }

    pub fn set_smoothing_antialias(&self) {
        unsafe {
            GdipSetSmoothingMode(self.raw, SmoothingModeAntiAlias);
        }
    }

    pub fn set_text_cleartype(&self) {
        unsafe {
            GdipSetTextRenderingHint(self.raw, TextRenderingHintClearTypeGridFit);
        }
    }

    /// 设置裁剪矩形（取代当前裁剪区）。与 `reset_clip` 成对使用，不要嵌套。
    pub fn set_clip_rect(&self, rect: RectF) {
        unsafe {
            let _ = GdipSetClipRect(
                self.raw,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                CombineModeReplace,
            );
        }
    }

    /// 恢复无裁剪。
    pub fn reset_clip(&self) {
        unsafe {
            let _ = GdipResetClip(self.raw);
        }
    }

    pub fn fill_path(&self, brush: &SolidBrush, path: &Path) {
        unsafe {
            GdipFillPath(self.raw, brush.as_raw(), path.as_raw());
        }
    }

    pub fn draw_path(&self, pen: &Pen, path: &Path) {
        unsafe {
            GdipDrawPath(self.raw, pen.as_raw(), path.as_raw());
        }
    }

    pub fn draw_string(
        &self,
        text: &str,
        font: &Font,
        layout: RectF,
        format: &StringFormat,
        brush: &SolidBrush,
    ) {
        if text.is_empty() || layout.w <= 0.0 || layout.h <= 0.0 {
            return;
        }
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            GdipDrawString(
                self.raw,
                PCWSTR(wide.as_ptr()),
                -1,
                font.as_raw(),
                &layout.raw(),
                format.as_raw(),
                brush.as_raw(),
            );
        }
    }

    pub fn measure_string(
        &self,
        text: &str,
        font: &Font,
        format: &StringFormat,
    ) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, 0.0);
        }
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mut bounds = windows::Win32::Graphics::GdiPlus::RectF::default();
            GdipMeasureString(
                self.raw,
                PCWSTR(wide.as_ptr()),
                -1,
                font.as_raw(),
                &RectF::new(0.0, 0.0, 1_000_000.0, 1_000_000.0).raw(),
                format.as_raw(),
                &mut bounds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            (bounds.Width, bounds.Height)
        }
    }
}

impl Drop for Graphics {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.raw);
        }
    }
}

pub struct SolidBrush {
    raw: *mut GpSolidFill,
}

impl SolidBrush {
    pub fn new(argb: u32) -> Option<Self> {
        unsafe {
            let mut raw: *mut GpSolidFill = std::ptr::null_mut();
            ok(GdipCreateSolidFill(argb, &mut raw)).ok()?;
            Some(SolidBrush { raw })
        }
    }

    fn as_raw(&self) -> *mut GpBrush {
        self.raw as *mut GpBrush
    }
}

impl Drop for SolidBrush {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteBrush(self.raw as *mut GpBrush);
        }
    }
}

pub struct Pen {
    raw: *mut GpPen,
}

impl Pen {
    pub fn new(argb: u32, width: f32) -> Option<Self> {
        unsafe {
            let mut raw: *mut GpPen = std::ptr::null_mut();
            ok(GdipCreatePen1(argb, width, Unit(2) /* UnitPixel */, &mut raw)).ok()?;
            Some(Pen { raw })
        }
    }

    fn as_raw(&self) -> *mut GpPen {
        self.raw
    }
}

impl Drop for Pen {
    fn drop(&mut self) {
        unsafe {
            GdipDeletePen(self.raw);
        }
    }
}

pub struct Path {
    raw: *mut GpPath,
}

impl Path {
    pub fn new() -> Option<Self> {
        unsafe {
            let mut raw: *mut GpPath = std::ptr::null_mut();
            ok(GdipCreatePath(FillMode(0) /* Alternate */, &mut raw)).ok()?;
            Some(Path { raw })
        }
    }

    pub fn add_arc(&mut self, x: f32, y: f32, w: f32, h: f32, start_angle: f32, sweep: f32) {
        unsafe {
            GdipAddPathArc(self.raw, x, y, w, h, start_angle, sweep);
        }
    }

    /// 圆角矩形路径（与 C# Drawing.RoundedRect 相同的四段弧）。
    pub fn add_rounded_rect(&mut self, bounds: RectF, radius: f32) {
        let diameter = (radius * 2.0).max(0.0);
        if diameter <= 0.5 || bounds.w <= diameter || bounds.h <= diameter {
            // 退化为矩形：四条直线近似（用 0.5px 弧）
            self.add_arc(bounds.x, bounds.y, 0.1, 0.1, 180.0, 90.0);
            self.add_arc(bounds.right() - 0.1, bounds.y, 0.1, 0.1, 270.0, 90.0);
            self.add_arc(bounds.right() - 0.1, bounds.bottom() - 0.1, 0.1, 0.1, 0.0, 90.0);
            self.add_arc(bounds.x, bounds.bottom() - 0.1, 0.1, 0.1, 90.0, 90.0);
        } else {
            self.add_arc(bounds.x, bounds.y, diameter, diameter, 180.0, 90.0);
            self.add_arc(bounds.right() - diameter, bounds.y, diameter, diameter, 270.0, 90.0);
            self.add_arc(bounds.right() - diameter, bounds.bottom() - diameter, diameter, diameter, 0.0, 90.0);
            self.add_arc(bounds.x, bounds.bottom() - diameter, diameter, diameter, 90.0, 90.0);
        }
        self.close_figure();
    }

    pub fn close_figure(&mut self) {
        unsafe {
            GdipClosePathFigure(self.raw);
        }
    }

    fn as_raw(&self) -> *mut GpPath {
        self.raw
    }
}

impl Drop for Path {
    fn drop(&mut self) {
        unsafe {
            GdipDeletePath(self.raw);
        }
    }
}

/// 文本布局格式：对齐 + 行对齐 + 截断 + 不换行。
pub struct StringFormat {
    raw: *mut GpStringFormat,
}

impl StringFormat {
    pub fn nowrap_left_center() -> Option<Self> {
        Self::build(StringAlignment(0), StringAlignment(1), true)
    }

    pub fn nowrap_left_top() -> Option<Self> {
        Self::build(StringAlignment(0), StringAlignment(0), true)
    }

    pub fn nowrap_center_center() -> Option<Self> {
        Self::build(StringAlignment(1), StringAlignment(1), true)
    }

    pub fn nowrap_right_center() -> Option<Self> {
        Self::build(StringAlignment(2), StringAlignment(1), true)
    }

    fn build(align: StringAlignment, line: StringAlignment, nowrap: bool) -> Option<Self> {
        unsafe {
            let mut raw: *mut GpStringFormat = std::ptr::null_mut();
            // NoWrap = 0x1000, NoClip = 0x4000
            let mut flags = 0x4000u32;
            if nowrap {
                flags |= 0x1000;
            }
            ok(GdipCreateStringFormat(flags as i32, 0, &mut raw)).ok()?;
            GdipSetStringFormatAlign(raw, align);
            GdipSetStringFormatLineAlign(raw, line);
            GdipSetStringFormatTrimming(raw, StringTrimming(3) /* EllipsisCharacter */);
            Some(StringFormat { raw })
        }
    }

    fn as_raw(&self) -> *mut GpStringFormat {
        self.raw
    }
}

impl Drop for StringFormat {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteStringFormat(self.raw);
        }
    }
}

/// GDI+ 字体（按逻辑像素创建，缓存于 Theme）。
pub struct Font {
    raw: *mut GpFont,
    gdi_font: Option<HFONT>, // 保持 HFONT 存活（GDI+ 引用其度量）
}

impl Font {
    /// `pixel_height` 为当前 DPI 下的物理像素高（调用方已 S() 缩放）。
    pub fn from_pixel_height(hdc: HDC, family: &str, pixel_height: i32, bold: bool) -> Option<Self> {
        unsafe {
            let mut logfont = LOGFONTW {
                lfHeight: -pixel_height,
                lfWeight: if bold { 700 } else { 400 },
                lfCharSet: DEFAULT_CHARSET,
                lfQuality: CLEARTYPE_QUALITY,
                ..Default::default()
            };
            let wide: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
            for (index, ch) in wide.iter().enumerate() {
                if index >= 31 {
                    break;
                }
                logfont.lfFaceName[index] = *ch;
            }
            let gdi_font = CreateFontIndirectW(&logfont);
            if gdi_font.is_invalid() {
                return None;
            }
            let old = windows::Win32::Graphics::Gdi::SelectObject(hdc, HGDIOBJ(gdi_font.0));
            let mut raw: *mut GpFont = std::ptr::null_mut();
            let result = ok(GdipCreateFontFromLogfontW(hdc, &logfont, &mut raw));
            let _ = windows::Win32::Graphics::Gdi::SelectObject(hdc, old);
            if result.is_err() {
                let _ = DeleteObject(HGDIOBJ(gdi_font.0));
                return None;
            }
            Some(Font {
                raw,
                gdi_font: Some(gdi_font),
            })
        }
    }

    pub fn as_raw(&self) -> *mut GpFont {
        self.raw
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteFont(self.raw);
            if let Some(font) = self.gdi_font {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
    }
}

/// 单色填充图标的简易绘制：用字符字体渲染 Segoe MDL2 图标。
pub fn draw_glyph(
    graphics: &Graphics,
    glyph: &str,
    font: &Font,
    layout: RectF,
    color: u32,
) {
    if let Some(brush) = SolidBrush::new(color) {
        if let Some(format) = StringFormat::nowrap_center_center() {
            graphics.draw_string(glyph, font, layout, &format, &brush);
        }
    }
}

/// 供托盘图标使用的 ARGB 像素画布。
pub struct ArgbCanvas {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>, // 0xAARRGGBB，自上而下
}

impl ArgbCanvas {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height) as usize],
        }
    }
}
