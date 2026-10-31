//! 图表绘制：Top-N 横向柱状图与每日柱状图、分段切换器。
//! 坐标：布局参数用逻辑像素（调用方传 s() 闭包换算），绘制用物理像素。

use windows::Win32::Graphics::Gdi::HDC;

use super::drawing::{draw_text, fill_rounded, format_tokens, measure_text, stroke_rounded};
use super::gdip::{Graphics, Path, RectF, SolidBrush};
use super::theme::{
    Theme, ACCENT, ACCENT_SOFT, BORDER, CARD_BACKGROUND, MUTED_TEXT, SECONDARY_TEXT, TEXT, TRACK,
};

// ---------------------------------------------------------------------------
// 分段切换器
// ---------------------------------------------------------------------------

/// 绘制分段切换器（pill 轨道 + 活动白底段），返回每段命中矩形（物理像素）。
/// `right` / `y` 为物理像素（调用方已换算）；`seg_w` / `h` 在内部按逻辑像素换算。
pub fn draw_segmented(
    g: &Graphics,
    theme: &mut Theme,
    hdc: HDC,
    right: i32,
    y: i32,
    segments: &[&str],
    active: usize,
) -> Vec<(RectF, usize)> {
    let dpi = theme.dpi;
    let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
    let seg_w = s(44);
    let h = s(22);
    let total_w = seg_w * segments.len() as i32;
    let x = right - total_w;
    let rect = RectF::new(x as f32, y as f32, total_w as f32, h as f32);
    fill_rounded(g, TRACK, rect, h as f32 / 2.0);
    let mut hits = Vec::with_capacity(segments.len());
    for (index, label) in segments.iter().enumerate() {
        let seg_rect = RectF::new(
            (x + index as i32 * seg_w) as f32,
            rect.y,
            seg_w as f32,
            h as f32,
        );
        if index == active {
            fill_rounded(g, CARD_BACKGROUND, seg_rect, h as f32 / 2.0);
            stroke_rounded(g, BORDER, 1.0, seg_rect, h as f32 / 2.0);
        }
        let color = if index == active { ACCENT } else { SECONDARY_TEXT };
        draw_text(g, theme, hdc, label, 11, index == active, color, seg_rect, super::drawing::TextAlign::CenterCenter);
        hits.push((seg_rect, index));
    }
    hits
}

// ---------------------------------------------------------------------------
// Panel 1：Top-N 横向柱状图
// ---------------------------------------------------------------------------

/// 横向柱状图。`rows` 已降序（≤ 上游 take 的 20 条）；行高固定，
/// 行数不足时顶端对齐、下方留白（面板高度恒定，不随模型数变化）。
pub fn draw_top_bars(
    g: &Graphics,
    theme: &mut Theme,
    hdc: HDC,
    rect: RectF,
    rows: &[(String, i64, i64)],
) {
    let dpi = theme.dpi;
    let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
    if rows.is_empty() || rect.h <= 0.0 {
        return;
    }
    let row_h = s(20) as f32;
    let bar_h = (row_h - s(6) as f32).max(s(4) as f32);
    // 名字列按本面板最宽模型名自适应，柱轨道相应收窄（行数 ≤20，测量成本可忽略）。
    // 下限保持短名面板的版式稳定，上限钳住极端长名（超出仍走省略号）。
    let mut max_name_w = s(0);
    for (model, _, _) in rows.iter() {
        let (w, _) = measure_text(g, theme, hdc, model, 11, false);
        if (w.ceil() as i32) > max_name_w {
            max_name_w = w.ceil() as i32;
        }
    }
    let name_w = (max_name_w + s(8)).clamp(s(118), s(160)) as f32;
    // 数值列要容得下 "3555.4 万" 这类最宽文本，否则会被省略号截断
    let value_w = s(68) as f32;
    // 请求数列（弱化）在 token 数值左侧，也要容下 "9452.4 万次"；
    // 两列之间留缝，避免同时取最宽文本时贴在一起。
    let req_w = s(58) as f32;
    let col_gap = s(6) as f32;
    let track_x = rect.x + name_w;
    let track_w = (rect.right() - value_w - req_w - col_gap - track_x).max(s(20) as f32);
    let max = rows.first().map(|(_, tokens, _)| *tokens).unwrap_or(0).max(1);

    for (index, (model, tokens, requests)) in rows.iter().enumerate() {
        let row_y = rect.y + index as f32 * row_h;
        let bar_rect = RectF::new(
            track_x,
            row_y + (row_h - bar_h) / 2.0,
            track_w,
            bar_h,
        );
        // 轨道
        fill_rounded(g, TRACK, bar_rect, bar_h / 2.0);
        // 名称
        let name = super::drawing::ellipsize(g, theme, hdc, model, 11, false, name_w - s(6) as f32);
        draw_text(g, theme, hdc, &name, 11, false, TEXT,
            RectF::new(rect.x, row_y, name_w, row_h),
            super::drawing::TextAlign::LeftCenter);
        // 条（占比 ∝ 相对最大值）
        let ratio = (*tokens as f64 / max as f64).clamp(0.0, 1.0) as f32;
        if ratio > 0.004 {
            let fill = RectF::new(bar_rect.x, bar_rect.y, (bar_rect.w * ratio).max(bar_h), bar_rect.h);
            fill_rounded(g, ACCENT, fill, bar_h / 2.0);
        }
        // 请求数（弱化小字）：置于 token 数值左侧；排序与柱长仍只看 tokens
        let req_text =
            format!("{}{}", format_tokens(*requests), crate::i18n::texts().requests_suffix);
        draw_text(g, theme, hdc, &req_text, 9, false, MUTED_TEXT,
            RectF::new(rect.right() - value_w - col_gap - req_w, row_y, req_w, row_h),
            super::drawing::TextAlign::RightCenter);
        // 数值
        draw_text(g, theme, hdc, &format_tokens(*tokens), 11, false, SECONDARY_TEXT,
            RectF::new(rect.right() - value_w, row_y, value_w, row_h),
            super::drawing::TextAlign::RightCenter);
    }
}

// ---------------------------------------------------------------------------
// Panel 2：每日柱状图
// ---------------------------------------------------------------------------

/// 每日柱状图：`days` 旧→新；左侧数值轴 + 底部日期轴 + 网格线。
pub fn draw_daily_columns(
    g: &Graphics,
    theme: &mut Theme,
    hdc: HDC,
    rect: RectF,
    days: &[(String, i64)],
) {
    let dpi = theme.dpi;
    let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
    if days.is_empty() || rect.h <= 0.0 {
        return;
    }
    let axis_w = s(62) as f32; // 左侧数值标签（要容下 "5268.8 万"）
    let axis_h = s(16) as f32; // 底部日期标签
    // 顶部留出一行高度：最高柱（ratio=1）的数值标签画在柱顶之上，不留就会被裁掉。
    let top_pad = s(16) as f32;
    let plot = RectF::new(
        rect.x + axis_w,
        rect.y + top_pad,
        (rect.w - axis_w - s(4) as f32).max(s(10) as f32),
        (rect.h - axis_h - s(6) as f32 - top_pad).max(s(10) as f32),
    );
    let max = days.iter().map(|(_, v)| *v).max().unwrap_or(0).max(1);
    let n = days.len() as f32;
    let slot = plot.w / n;
    let bar_w = (slot * 0.62).clamp(s(3) as f32, s(34) as f32);

    // 网格线（4 条）+ 左侧刻度
    // 柱有标签时省略顶部刻度：顶部数值由最高柱的标签给出，否则同一行会出现两个相同数字。
    let bars_labelled = slot >= s(26) as f32;
    for quarter in 0..=3i32 {
        let ratio = quarter as f32 / 3.0;
        let gy = plot.bottom() - plot.h * ratio;
        if let Some(mut path) = Path::new() {
            path.add_rounded_rect(RectF::new(plot.x, gy, plot.w, 1.0), 0.0);
            if let Some(brush) = SolidBrush::new(TRACK) {
                g.fill_path(&brush, &path);
            }
        }
        if quarter == 3 && bars_labelled {
            continue;
        }
        let label_value = (max as f64 * ratio as f64) as i64;
        draw_text(g, theme, hdc, &format_tokens(label_value), 9, false, MUTED_TEXT,
            RectF::new(rect.x, gy - s(8) as f32, axis_w - s(4) as f32, s(16) as f32),
            super::drawing::TextAlign::RightCenter);
    }

    // 柱 + 日期
    let label_step = if days.len() > 8 { (days.len() + 5) / 6 } else { 1 };
    for (index, (day, tokens)) in days.iter().enumerate() {
        let ratio = (*tokens as f64 / max as f64).clamp(0.0, 1.0) as f32;
        let bar_h = (plot.h * ratio).max(if *tokens > 0 { s(2) as f32 } else { 0.0 });
        let bar_x = plot.x + index as f32 * slot + (slot - bar_w) / 2.0;
        let bar_rect = RectF::new(bar_x, plot.bottom() - bar_h, bar_w, bar_h);
        if bar_h > 0.0 {
            if let Some(mut path) = Path::new() {
                // 顶部圆角、底部直角：用圆角矩形近似（半径小于高度的一半）
                let radius = (bar_w / 2.0).min(s(3) as f32).min(bar_h / 2.0);
                path.add_rounded_rect(bar_rect, radius);
                if let Some(brush) = SolidBrush::new(ACCENT) {
                    g.fill_path(&brush, &path);
                }
            }
        }
        // 数值标签：空间足够且不越出绘图区顶部（含顶部预留）时标注
        let label_y = bar_rect.y - s(15) as f32;
        let show_value = bars_labelled && *tokens > 0 && label_y >= rect.y;
        if show_value {
            // 标签用整格宽居中，避免 "1572.4 万" 这类被截断成省略号
            let label_w = (slot - s(2) as f32).max(bar_w + s(12) as f32);
            draw_text(g, theme, hdc, &format_tokens(*tokens), 9, false, SECONDARY_TEXT,
                RectF::new(bar_x + bar_w / 2.0 - label_w / 2.0, label_y, label_w, s(14) as f32),
                super::drawing::TextAlign::CenterCenter);
        }
        // 日期标签（MM-DD）：标签框按整组宽度（足以容纳 "10-19"），但中心对齐到本柱，
        // 即不裁字也不偏移。单格宽度只有 ~12px 时，"10-19" 会被裁成单个字符。
        if index % label_step == 0 {
            let short = if day.len() >= 10 { &day[5..] } else { day.as_str() };
            let label_w = (slot * label_step as f32).max(slot);
            let center = plot.x + (index as f32 + 0.5) * slot;
            draw_text(g, theme, hdc, short, 9, false, MUTED_TEXT,
                RectF::new(center - label_w / 2.0, plot.bottom() + 1.0, label_w, axis_h),
                super::drawing::TextAlign::CenterCenter);
        }
    }
    let _ = ACCENT_SOFT; // 预留
}

/// 测量辅助（供面板标题等使用）。
pub fn text_width(g: &Graphics, theme: &mut Theme, hdc: HDC, text: &str, logical_px: i32) -> f32 {
    measure_text(g, theme, hdc, text, logical_px, false).0
}
