//! 高层绘制助手：圆角卡片、文本、状态点、用量条与格式化。
//! 所有坐标为物理像素（调用方经 theme.scale 换算）。

use chrono::{DateTime, Utc};
use windows::Win32::Graphics::Gdi::HDC;

use super::gdip::{Graphics, Path, RectF, SolidBrush, StringFormat};
use super::theme::{Theme, BORDER, CARD_BACKGROUND, TRACK};

pub fn fill_rounded(g: &Graphics, color: u32, bounds: RectF, radius: f32) {
    let Some(mut path) = Path::new() else { return };
    path.add_rounded_rect(bounds, radius);
    if let Some(brush) = SolidBrush::new(color) {
        g.fill_path(&brush, &path);
    }
}

pub fn stroke_rounded(g: &Graphics, color: u32, width: f32, bounds: RectF, radius: f32) {
    let Some(mut path) = Path::new() else { return };
    path.add_rounded_rect(
        RectF::new(bounds.x + 0.5, bounds.y + 0.5, bounds.w - 1.0, bounds.h - 1.0),
        radius,
    );
    if let Some(pen) = super::gdip::Pen::new(color, width) {
        g.draw_path(&pen, &path);
    }
}

pub fn draw_card(g: &Graphics, bounds: RectF, radius: f32) {
    fill_rounded(g, CARD_BACKGROUND, bounds, radius);
    stroke_rounded(g, BORDER, 1.0, bounds, radius);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    LeftCenter,
    LeftTop,
    CenterCenter,
    RightCenter,
}

pub fn draw_text(
    g: &Graphics,
    theme: &mut Theme,
    hdc: HDC,
    text: &str,
    logical_px: i32,
    bold: bool,
    color: u32,
    bounds: RectF,
    align: TextAlign,
) {
    if text.is_empty() || bounds.w <= 0.0 || bounds.h <= 0.0 {
        return;
    }
    let font = theme.font(hdc, logical_px, bold);
    let Some(format) = (match align {
        TextAlign::LeftCenter => StringFormat::nowrap_left_center(),
        TextAlign::LeftTop => StringFormat::nowrap_left_top(),
        TextAlign::CenterCenter => StringFormat::nowrap_center_center(),
        TextAlign::RightCenter => StringFormat::nowrap_right_center(),
    }) else {
        return;
    };
    let Some(brush) = SolidBrush::new(color) else { return };
    // 单行显示：把换行替换为空格（与 C# Drawing.DrawText 一致）
    let normalized = if text.contains('\n') || text.contains('\r') {
        text.replace(['\n', '\r'], " ")
    } else {
        text.to_string()
    };
    g.draw_string(&normalized, font, bounds, &format, &brush);
}

pub fn draw_multiline(
    g: &Graphics,
    theme: &mut Theme,
    hdc: HDC,
    text: &str,
    logical_px: i32,
    color: u32,
    bounds: RectF,
    line_height: f32,
) {
    let font = theme.font(hdc, logical_px, false);
    let Some(format) = StringFormat::nowrap_left_top() else { return };
    let Some(brush) = SolidBrush::new(color) else { return };
    let mut y = bounds.y;
    for line in text.split('\n') {
        if y + line_height > bounds.bottom() {
            break;
        }
        let rect = RectF::new(bounds.x, y, bounds.w, line_height);
        g.draw_string(line, font, rect, &format, &brush);
        y += line_height;
    }
}

pub fn measure_text(g: &Graphics, theme: &mut Theme, hdc: HDC, text: &str, logical_px: i32, bold: bool) -> (f32, f32) {
    let font = theme.font(hdc, logical_px, bold);
    match StringFormat::nowrap_left_center() {
        Some(format) => g.measure_string(text, font, &format),
        None => (0.0, 0.0),
    }
}

/// 二分省略（与 C# Drawing.Ellipsize 一致）。
pub fn ellipsize(
    g: &Graphics,
    theme: &mut Theme,
    hdc: HDC,
    text: &str,
    logical_px: i32,
    bold: bool,
    max_width: f32,
) -> String {
    if max_width <= 0.0 || text.is_empty() {
        return text.to_string();
    }
    let (width, _) = measure_text(g, theme, hdc, text, logical_px, bold);
    if width <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let chars: Vec<char> = text.chars().collect();
    let mut low: i64 = 0;
    let mut high: i64 = chars.len() as i64;
    while low < high {
        let mid = (low + high + 1) / 2;
        let candidate: String = chars[..mid as usize].iter().collect::<String>() + ellipsis;
        let (candidate_width, _) = measure_text(g, theme, hdc, &candidate, logical_px, bold);
        if candidate_width <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    if low <= 0 {
        return ellipsis.to_string();
    }
    chars[..low as usize].iter().collect::<String>() + ellipsis
}

pub fn draw_status_dot(g: &Graphics, center_x: f32, center_y: f32, diameter: f32, color: u32) {
    let rect = RectF::new(
        center_x - diameter / 2.0,
        center_y - diameter / 2.0,
        diameter,
        diameter,
    );
    if let Some(mut path) = Path::new() {
        // 圆形：用两段半圆弧
        path.add_arc(rect.x, rect.y, diameter, diameter, 0.0, 180.0);
        path.add_arc(rect.x, rect.y, diameter, diameter, 180.0, 180.0);
        path.close_figure();
        if let Some(brush) = SolidBrush::new(color) {
            g.fill_path(&brush, &path);
        }
    }
}

/// 用量条：轨道 + 填充 + 右侧百分比文字由调用方绘制。
pub fn draw_usage_bar(
    g: &Graphics,
    bounds: RectF,
    percent: f64,
    fill_color: u32,
) {
    fill_rounded(g, TRACK, bounds, bounds.h / 2.0);
    let ratio = (percent / 100.0).clamp(0.0, 1.0) as f32;
    if ratio > 0.005 {
        let fill = RectF::new(bounds.x, bounds.y, bounds.w * ratio, bounds.h);
        fill_rounded(g, fill_color, fill, bounds.h / 2.0);
    }
}

// ---------------------------------------------------------------------------
// 格式化
// ---------------------------------------------------------------------------

/// token 数：中文 1.2 万 / 3.45 亿 / 9876；英文 1.2K / 12.3M / 3.45B。
pub fn format_tokens(value: i64) -> String {
    let abs = value.unsigned_abs();
    if crate::i18n::lang() == crate::config::Language::En {
        if abs >= 1_000_000_000 {
            format!("{:.2}B", value as f64 / 1e9)
        } else if abs >= 1_000_000 {
            format!("{:.1}M", value as f64 / 1e6)
        } else if abs >= 1_000 {
            format!("{:.1}K", value as f64 / 1e3)
        } else {
            format!("{value}")
        }
    } else if abs >= 100_000_000 {
        format!("{:.2} 亿", value as f64 / 1e8)
    } else if abs >= 10_000 {
        format!("{:.1} 万", value as f64 / 1e4)
    } else {
        format!("{value}")
    }
}

/// 积分：1,252 / 4.717。
pub fn format_credits(value: f64) -> String {
    if value.abs() >= 1000.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

pub fn format_memory_mib(mib: Option<i64>) -> String {
    let Some(mib) = mib else {
        return "—".to_string();
    };
    if mib < 0 {
        return "—".to_string();
    }
    let gib = mib as f64 / 1024.0;
    if gib >= 100.0 {
        format!("{gib:.0} GB")
    } else {
        format!("{gib:.1} GB")
    }
}

pub fn format_percent(percent: Option<f64>) -> String {
    match percent {
        None => "—".to_string(),
        Some(value) => format!("{value:.0}%"),
    }
}

pub fn format_relative(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let elapsed = now.signed_duration_since(timestamp);
    let seconds = elapsed.num_seconds();
    if crate::i18n::lang() == crate::config::Language::En {
        if seconds < 60 {
            "just now".to_string()
        } else if seconds < 3600 {
            format!("{}m ago", seconds / 60)
        } else {
            format!("{}h ago", seconds / 3600)
        }
    } else if seconds < 60 {
        "刚刚".to_string()
    } else if seconds < 3600 {
        format!("{} 分钟前", seconds / 60)
    } else {
        format!("{} 小时前", seconds / 3600)
    }
}

pub fn format_clock(timestamp: DateTime<Utc>) -> String {
    use chrono::Local;
    timestamp
        .with_timezone(&Local)
        .format("%H:%M:%S")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;
    use crate::i18n;

    #[test]
    fn token_formatting() {
        let _guard = i18n::test_lock().lock().unwrap();
        i18n::set_language(Language::Zh);
        assert_eq!(format_tokens(9876), "9876");
        assert_eq!(format_tokens(12_345), "1.2 万");
        assert_eq!(format_tokens(345_000_000), "3.45 亿");
        i18n::set_language(Language::En);
        assert_eq!(format_tokens(987), "987");
        assert_eq!(format_tokens(1234), "1.2K");
        assert_eq!(format_tokens(12_345_678), "12.3M");
        assert_eq!(format_tokens(3_450_000_000), "3.45B");
        i18n::set_language(Language::Zh);
    }

    #[test]
    fn memory_formatting() {
        assert_eq!(format_memory_mib(None), "—");
        assert_eq!(format_memory_mib(Some(0)), "0.0 GB");
        assert_eq!(format_memory_mib(Some(1024)), "1.0 GB");
        assert_eq!(format_memory_mib(Some(200_704)), "196 GB");
    }

    #[test]
    fn percent_formatting() {
        assert_eq!(format_percent(None), "—");
        assert_eq!(format_percent(Some(87.4)), "87%");
    }

    #[test]
    fn relative_time() {
        let _guard = i18n::test_lock().lock().unwrap();
        let now = Utc::now();
        i18n::set_language(Language::Zh);
        assert_eq!(format_relative(now, now), "刚刚");
        assert_eq!(format_relative(now - chrono::Duration::seconds(30), now), "刚刚");
        assert_eq!(format_relative(now - chrono::Duration::minutes(5), now), "5 分钟前");
        assert_eq!(format_relative(now - chrono::Duration::hours(3), now), "3 小时前");
        i18n::set_language(Language::En);
        assert_eq!(format_relative(now, now), "just now");
        assert_eq!(format_relative(now - chrono::Duration::minutes(5), now), "5m ago");
        assert_eq!(format_relative(now - chrono::Duration::hours(3), now), "3h ago");
        i18n::set_language(Language::Zh);
    }
}
