//! 对话框自绘圆角按钮：主按钮主题蓝、次按钮灰白、危险按钮红底。
//! 与主面板同一套色板/字体；命中检测在对话框的 WM_LBUTTONDOWN 中查
//! `rect.contains`。坐标为逻辑像素，绘制时按 DPI 换算。

use windows::Win32::Graphics::Gdi::HDC;

use super::drawing::{draw_text, fill_rounded, stroke_rounded};
use super::gdip::RectF;
use super::theme::{Theme, ACCENT, ACCENT_DARK, BORDER_STRONG, CARD_BACKGROUND, HOVER, RED, RED_SOFT, TEXT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
}

#[derive(Debug, Clone)]
pub struct DrawnButton {
    pub id: i32,
    pub text: String,
    /// 逻辑像素布局。
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub style: ButtonStyle,
    pub pressed: bool,
    /// false 时不绘制也不命中（如"浏览…"在密码模式下隐藏）。
    pub visible: bool,
}

impl DrawnButton {
    pub fn new(id: i32, text: &str, x: i32, y: i32, w: i32, h: i32, style: ButtonStyle) -> Self {
        Self {
            id,
            text: text.to_string(),
            x,
            y,
            w,
            h,
            style,
            pressed: false,
            visible: true,
        }
    }

    fn rect(&self, s: impl Fn(i32) -> i32) -> RectF {
        RectF::new(s(self.x) as f32, s(self.y) as f32, s(self.w) as f32, s(self.h) as f32)
    }

    /// 命中检测（物理像素）；不可见按钮不命中。
    pub fn hit(&self, s: impl Fn(i32) -> i32, x: i32, y: i32) -> bool {
        self.visible && self.rect(s).contains(x, y)
    }

    /// 绘制按钮；`pressed` 时颜色加深。
    pub fn paint(&self, g: &super::gdip::Graphics, theme: &mut Theme, hdc: HDC) {
        if !self.visible {
            return;
        }
        let dpi = theme.dpi;
        let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
        let rect = self.rect(s);
        let radius = s(6) as f32;
        let (bg, fg, border) = match self.style {
            ButtonStyle::Primary => {
                if self.pressed {
                    (ACCENT_DARK, 0xFFFFFFFF, ACCENT_DARK)
                } else {
                    (ACCENT, 0xFFFFFFFF, ACCENT)
                }
            }
            ButtonStyle::Danger => {
                if self.pressed {
                    (RED, 0xFFFFFFFF, RED)
                } else {
                    (RED_SOFT, RED, RED)
                }
            }
            ButtonStyle::Secondary => {
                if self.pressed {
                    (HOVER, TEXT, BORDER_STRONG)
                } else {
                    (CARD_BACKGROUND, TEXT, BORDER_STRONG)
                }
            }
        };
        fill_rounded(g, bg, rect, radius);
        stroke_rounded(g, border, 1.0, rect, radius);
        draw_text(
            g,
            theme,
            hdc,
            &self.text,
            12,
            self.style == ButtonStyle::Primary,
            fg,
            rect,
            super::drawing::TextAlign::CenterCenter,
        );
    }
}
