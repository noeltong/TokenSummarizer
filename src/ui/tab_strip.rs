//! 服务器选项卡条：每台服务器一个标签（状态圆点 + 名称），过多时左右箭头
//! 翻页，右侧 "+" 添加；右上角为 范围切换/刷新/设置 图标。

use windows::Win32::Graphics::Gdi::HDC;

use super::drawing::{
    draw_status_dot, draw_text, ellipsize, fill_rounded, measure_text, stroke_rounded,
};
use super::gdip::{Graphics, RectF};
use super::theme::{
    phase_color, Theme, ACCENT, BORDER, BORDER_STRONG, GLYPH_ADD, GLYPH_LEFT,
    GLYPH_RIGHT, GLYPH_REFRESH, GLYPH_SETTINGS, HOVER, MUTED_TEXT, SECONDARY_TEXT, TEXT, TRACK,
    WINDOW_BACKGROUND,
};
use crate::model::Phase;

pub const STRIP_H_LOGICAL: i32 = 46;

/// 标签文字的垂直光学补偿（逻辑 px）。中英文共享基线但视觉重心不同：
/// 实测 96dpi 下 CJK 墨迹中心比卡片几何中心高 ~1.5px，纯 ASCII（服务器名/
/// 数字）低 ~0.5px，两类相差 2px。反向各移 1px 使两类标签视觉对齐。
/// 判断必须用原始标签（省略号 "…" 是非 ASCII，会翻转分类）。
fn tab_text_dy(label: &str) -> i32 {
    if label.chars().all(|c| c.is_ascii()) { -1 } else { 1 }
}

/// 选项卡条内可点击元素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabElement {
    Tab(usize),
    ScrollLeft,
    ScrollRight,
    Add,

    Refresh,
    Settings,
}

#[derive(Debug, Default, Clone)]
pub struct TabLayout {
    pub hits: Vec<(RectF, TabElement)>,
}

/// 绘制选项卡条；返回布局（含全部命中区域）。`width` 为物理像素。
pub fn paint(
    g: &Graphics,
    theme: &mut Theme,
    hdc: HDC,
    width: i32,
    labels: &[String],
    phases: &[Phase],
    visible: usize,
    tab_scroll: usize,
) -> TabLayout {
    let dpi = theme.dpi;
    let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
    let mut layout = TabLayout::default();
    let strip_h = s(STRIP_H_LOGICAL);

    fill_rounded(g, WINDOW_BACKGROUND, RectF::new(0.0, 0.0, width as f32, strip_h as f32), 0.0);
    if let Some(pen) = super::gdip::Pen::new(BORDER, 1.0) {
        if let Some(mut path) = super::gdip::Path::new() {
            path.add_rounded_rect(
                RectF::new(0.5, strip_h as f32 - 1.5, width as f32 - 1.0, 1.0),
                0.0,
            );
            g.draw_path(&pen, &path);
        }
    }

    // 右侧控件区（从右往左）：设置 / 刷新
    let icon = s(28);
    let pad = s(8);
    let mut x_right = width - pad;
    let icon_y = (strip_h - icon) / 2;

    let settings_rect = RectF::new((x_right - icon) as f32, icon_y as f32, icon as f32, icon as f32);
    x_right -= icon;
    let refresh_rect = RectF::new((x_right - icon - pad) as f32, icon_y as f32, icon as f32, icon as f32);
    x_right -= icon + pad;
    layout.hits.push((refresh_rect, TabElement::Refresh));
    layout.hits.push((settings_rect, TabElement::Settings));

    // 左侧 tab 区；起始 x 与内容区卡片左缘（content::paint 的 pad=12）对齐
    let left_limit = s(12);
    let arrow_w = s(22);
    // 箭头盒比 28px 图标盒矮，垂直居中须单独算，否则中心比 tab 卡片/"+"高 3px
    let arrow_y = (strip_h - arrow_w) / 2;
    let tabs_area_right = x_right - s(10);
    let plus_size = s(24);
    let tab_x_max = tabs_area_right - arrow_w - plus_size - s(10);

    let dot = s(7);
    let tab_pad = s(10);
    let gap = s(4);
    let mut widths = Vec::with_capacity(labels.len());
    for label in labels {
        let (tw, _) = measure_text(g, theme, hdc, label, 13, true);
        widths.push(dot + s(6) + tw.ceil() as i32 + tab_pad * 2);
    }

    // 计算可见窗口
    let mut x = left_limit;
    let mut last_visible = tab_scroll.min(labels.len().saturating_sub(1));
    for (index, width_px) in widths.iter().enumerate().skip(tab_scroll) {
        if x + width_px > tab_x_max {
            break;
        }
        last_visible = index;
        x += width_px + gap;
    }
    let arrow_left = tab_scroll > 0;
    let arrow_right = labels.len() > 0 && last_visible + 1 < labels.len();

    let mut tab_start = left_limit;
    if arrow_left {
        let rect = RectF::new(tab_start as f32, arrow_y as f32, arrow_w as f32, arrow_w as f32);
        super::gdip::draw_glyph(g, GLYPH_LEFT, theme.icon_font(hdc, 10), rect, MUTED_TEXT);
        layout.hits.push((rect, TabElement::ScrollLeft));
        tab_start += arrow_w + s(2);
    }

    // "+" 按钮
    let plus_rect = RectF::new(
        (tabs_area_right - plus_size) as f32,
        ((strip_h - plus_size) / 2) as f32,
        plus_size as f32,
        plus_size as f32,
    );
    fill_rounded(g, TRACK, plus_rect, plus_size as f32 / 2.0);
    super::gdip::draw_glyph(g, GLYPH_ADD, theme.icon_font(hdc, 12), plus_rect, ACCENT);
    layout.hits.push((plus_rect, TabElement::Add));

    // tabs
    let visible = visible.min(last_visible);
    let tab_h = s(32);
    let tab_y = (strip_h - tab_h) / 2;
    let mut x = tab_start;
    for index in tab_scroll..=last_visible {
        if index >= labels.len() {
            break;
        }
        let width_px = widths[index];
        let tab_rect = RectF::new(x as f32, tab_y as f32, width_px as f32, tab_h as f32);
        let active = index == visible;
        if active {
            fill_rounded(g, super::theme::ACCENT_SOFT, tab_rect, s(8) as f32);
            stroke_rounded(g, BORDER_STRONG, 1.0, tab_rect, s(8) as f32);
        } else {
            fill_rounded(g, HOVER, tab_rect, s(8) as f32);
        }
        let dot_cx = tab_rect.x + tab_pad as f32 + dot as f32 / 2.0;
        let dot_cy = tab_rect.y + tab_rect.h / 2.0;
        let color = phase_color(*phases.get(index).unwrap_or(&Phase::Stopped));
        draw_status_dot(g, dot_cx, dot_cy, dot as f32, color);
        let text_x = dot_cx + dot as f32 / 2.0 + s(6) as f32;
        let text_w = tab_rect.right() - text_x - tab_pad as f32;
        let label = ellipsize(g, theme, hdc, &labels[index], 13, true, text_w);
        let text_y = tab_rect.y as i32 + s(tab_text_dy(&labels[index]));
        draw_text(
            g, theme, hdc, &label, 13, true,
            if active { TEXT } else { SECONDARY_TEXT },
            RectF::new(text_x, text_y as f32, text_w, tab_rect.h),
            super::drawing::TextAlign::LeftCenter,
        );
        layout.hits.push((tab_rect, TabElement::Tab(index)));
        x += width_px + gap;
    }
    if arrow_right {
        let rect = RectF::new(tab_x_max as f32, arrow_y as f32, arrow_w as f32, arrow_w as f32);
        super::gdip::draw_glyph(g, GLYPH_RIGHT, theme.icon_font(hdc, 10), rect, MUTED_TEXT);
        layout.hits.push((rect, TabElement::ScrollRight));
    }

    // 右上图标（在 tab 之后绘制避免被覆盖）
    super::gdip::draw_glyph(g, GLYPH_REFRESH, theme.icon_font(hdc, 13), refresh_rect, SECONDARY_TEXT);
    super::gdip::draw_glyph(g, GLYPH_SETTINGS, theme.icon_font(hdc, 13), settings_rect, SECONDARY_TEXT);

    layout
}

/// 根据当前可见 tab 与滚动位置计算下一滚动偏移。
pub fn scroll_step(forward: bool, current: usize, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    if forward {
        (current + 1).min(count - 1)
    } else {
        current.saturating_sub(1)
    }
}
