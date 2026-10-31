//! 浅色 Windows 11 风格主题（色板与 C# GPUObserver Theme.cs 一致），
//! 字体按 DPI 缓存；逻辑像素 @96dpi，经 scale() = round(v*dpi/96) 缩放。

use std::collections::HashMap;

use windows::Win32::Graphics::Gdi::HDC;

use super::gdip::Font;

// ARGB 颜色（0xAARRGGBB）
pub const WINDOW_BACKGROUND: u32 = 0xFFF7F8FA;
pub const CARD_BACKGROUND: u32 = 0xFFFFFFFF;
pub const BORDER: u32 = 0xFFE2E5EA;
pub const BORDER_STRONG: u32 = 0xFFD2D7DE;
pub const TEXT: u32 = 0xFF191C21;
pub const SECONDARY_TEXT: u32 = 0xFF626A76;
pub const MUTED_TEXT: u32 = 0xFF8F95A0;
pub const ACCENT: u32 = 0xFF2563EB;
pub const ACCENT_DARK: u32 = 0xFF1D4EBB;
pub const ACCENT_SOFT: u32 = 0xFFE8F0FE;
pub const PURPLE: u32 = 0xFF7C3AED;
pub const GREEN: u32 = 0xFF107C41;
pub const GREEN_SOFT: u32 = 0xFFE5F7ED;
pub const AMBER: u32 = 0xFFB45309;
pub const AMBER_SOFT: u32 = 0xFFFEF3C7;
pub const RED: u32 = 0xFFB91C1C;
pub const RED_SOFT: u32 = 0xFFFEE2E2;
pub const TRACK: u32 = 0xFFEBEEF3;
pub const HOVER: u32 = 0xFFEEF1F6;
pub const PRESSED: u32 = 0xFFE4E9F0;

pub const UI_FONT_FAMILY: &str = "Microsoft YaHei UI";
pub const ICON_FONT_FAMILY: &str = "Segoe MDL2 Assets";
pub const MONO_FONT_FAMILY: &str = "Consolas";

// Segoe MDL2 Assets 字形
pub const GLYPH_ADD: &str = "\u{E710}";
pub const GLYPH_SETTINGS: &str = "\u{E713}";
pub const GLYPH_REFRESH: &str = "\u{E72C}";
pub const GLYPH_WARNING: &str = "\u{E7BA}";
pub const GLYPH_ERROR: &str = "\u{E783}";
pub const GLYPH_CHECK: &str = "\u{E73E}";
pub const GLYPH_LEFT: &str = "\u{E76B}";
pub const GLYPH_RIGHT: &str = "\u{E76C}";
pub const GLYPH_EDIT: &str = "\u{E70F}";
pub const GLYPH_DELETE: &str = "\u{E74D}";
pub const GLYPH_FOLDER: &str = "\u{E8B7}";
pub const GLYPH_CLOSE: &str = "\u{E711}";

/// 用量/内存/相位颜色规则（与 C# Theme 一致）。
pub fn usage_color(percent: Option<f64>) -> u32 {
    match percent {
        None => MUTED_TEXT,
        Some(value) if value >= 90.0 => RED,
        Some(value) if value >= 70.0 => AMBER,
        Some(_) => ACCENT,
    }
}

pub fn memory_color(percent: Option<f64>) -> u32 {
    match percent {
        None => MUTED_TEXT,
        Some(value) if value >= 95.0 => RED,
        Some(value) if value >= 80.0 => AMBER,
        Some(_) => PURPLE,
    }
}

pub fn phase_color(phase: crate::model::Phase) -> u32 {
    use crate::model::Phase;
    match phase {
        Phase::Online => GREEN,
        Phase::Connecting => AMBER,
        Phase::Error => RED,
        Phase::Stopped => MUTED_TEXT,
    }
}

/// 主题上下文：DPI + 字体缓存。DPI 变化时重建。
pub struct Theme {
    pub dpi: u32,
    ui_fonts: HashMap<(i32, bool), Option<Font>>,
    icon_fonts: HashMap<i32, Option<Font>>,
}

impl Theme {
    pub fn new(dpi: u32) -> Self {
        Self {
            dpi,
            ui_fonts: HashMap::new(),
            icon_fonts: HashMap::new(),
        }
    }

    /// S(v) = round(v * dpi / 96)
    pub fn scale(&self, value: i32) -> i32 {
        ((value as f64 * self.dpi as f64 / 96.0) + 0.5).floor() as i32
    }

    /// 逻辑像素文本字体。`logical_px` 为 96dpi 下的字号。
    pub fn font(&mut self, hdc: HDC, logical_px: i32, bold: bool) -> &Font {
        let physical = self.scale(logical_px);
        self.ui_fonts
            .entry((logical_px, bold))
            .or_insert_with(|| Font::from_pixel_height(hdc, UI_FONT_FAMILY, physical, bold))
            .as_ref()
            .expect("字体创建失败在启动时已验证")
    }

    /// 图标字体（Segoe MDL2）。
    pub fn icon_font(&mut self, hdc: HDC, logical_px: i32) -> &Font {
        let physical = self.scale(logical_px);
        self.icon_fonts
            .entry(logical_px)
            .or_insert_with(|| Font::from_pixel_height(hdc, ICON_FONT_FAMILY, physical, false))
            .as_ref()
            .expect("系统字体创建失败（Win10/11 自带微软雅黑与 Segoe MDL2）")
    }
}
