//! 主界面仪表盘：头部状态 + 双 Panel（Top20 模型横向柱状图 / 每日柱状图）。
//! 每个选项卡（合计或单服务器）共用此渲染；切换器命中区域经 Dashboard 返回。

use chrono::{DateTime, Utc};
use windows::Win32::Graphics::Gdi::HDC;

use super::chart;
use super::drawing::{draw_card, draw_text, format_relative};
use super::gdip::RectF;
use super::theme::{
    Theme, ACCENT, ACCENT_SOFT, MUTED_TEXT, SECONDARY_TEXT, TEXT,
};

/// Panel 1 的范围切换标签（顺序即显示顺序；命中按下标，天数见 MODEL_RANGE_DAYS）。
pub fn model_ranges() -> &'static [&'static str; 4] {
    &crate::i18n::texts().model_ranges
}
/// 对应天数（i64::MAX/4 = 全部历史，聚合层内部钳制防溢出）。
pub const MODEL_RANGE_DAYS: [i64; 4] = [i64::MAX / 4, 29, 6, 0];
/// Panel 1 的最大行数：面板高度恒定按此计算，行数不足时下方留白。
pub const TOP20_MAX_ROWS: usize = 20;
/// Panel 2 的范围切换标签（命中按下标，天数见 DAILY_RANGE_N）。
pub fn daily_ranges() -> &'static [&'static str; 2] {
    &crate::i18n::texts().daily_ranges
}
pub const DAILY_RANGE_N: [i64; 2] = [7, 30];

/// 固定头部高度（含上下内距）：可滚动 body 从 `top + header_height` 开始。
/// `paint` 与外部命中换算共用此函数，避免两处各算一遍而错位。
pub fn header_height(dpi: u32) -> i32 {
    let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
    s(12) + s(38) + s(4)
}

/// 内容区命中元素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelHit {
    ModelRange(usize),
    DailyRange(usize),
}

pub struct ContentView<'a> {
    pub g: &'a super::gdip::Graphics,
    pub theme: &'a mut Theme,
    pub hdc: HDC,
    pub width: i32,
    /// 视口顶部（= 选项卡条高度）：头部从这里开始，不随滚动移动。
    pub top: i32,
    /// 视口高度（= 客户区高 - 选项卡条高度）。
    pub height: i32,
    pub scroll: i32,
    pub now: DateTime<Utc>,
}

pub struct Dashboard {
    /// body 的可滚动总高度（不含头部）。
    pub content_height: i32,
    /// 命中区，坐标以 body 顶为原点（未减滚动）。
    pub hits: Vec<(RectF, PanelHit)>,
}

/// 渲染一个选项卡的完整内容。
/// `cards` 为预构造的状态卡片（错误/同步提示），(背景色, 前景色, 文本)。
/// 头部固定，其余内容在裁剪区内按 `scroll` 滚动。
#[allow(clippy::too_many_arguments)]
pub fn paint(
    view: &mut ContentView,
    title: &str,
    subtitle: &str,
    freshness: &str,
    cards: &[(u32, u32, String)],
    model_rows: &[(String, i64, i64)],
    daily: &[(String, i64)],
    model_total: i64,
    daily_total: i64,
    model_range: usize,
    daily_range: usize,
) -> Dashboard {
    let dpi = view.theme.dpi;
    let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
    let mut hits = Vec::new();
    let pad = s(12);
    let width = view.width;

    // ---- 固定头部（不滚动）----
    let header_y = view.top + pad;
    let header_h = s(38);
    draw_text(view.g, view.theme, view.hdc, title, 15, true, TEXT,
        RectF::new(pad as f32, header_y as f32, (width - pad * 2) as f32 * 0.6, s(20) as f32),
        super::drawing::TextAlign::LeftCenter);
    draw_text(view.g, view.theme, view.hdc, subtitle, 11, false, SECONDARY_TEXT,
        RectF::new(pad as f32, (header_y + s(19)) as f32, (width - pad * 2) as f32 * 0.6, s(16) as f32),
        super::drawing::TextAlign::LeftCenter);
    draw_text(view.g, view.theme, view.hdc, freshness, 11, false, MUTED_TEXT,
        RectF::new((width - pad) as f32 - (width - pad * 2) as f32 * 0.4, header_y as f32,
            (width - pad * 2) as f32 * 0.4, header_h as f32),
        super::drawing::TextAlign::RightCenter);

    let fixed_h = header_height(dpi);
    let body_top = view.top + fixed_h;
    let body_h = (view.height - fixed_h).max(0);

    // ---- 可滚动 body（裁剪，避免盖住头部/选项卡条）----
    view.g.set_clip_rect(RectF::new(
        0.0,
        body_top as f32,
        width as f32,
        body_h as f32,
    ));

    // content_y 以 body 顶为原点；screen_y 加上 top 与滚动偏移
    let scroll_off = view.scroll;
    let mut content_y = 0i32;
    let screen = move |content_y: i32| body_top + content_y - scroll_off;

    // ---- 状态卡片（错误 / 首次同步提示）----
    for (bg, fg, text) in cards {
        content_y += info_card(view, screen(content_y), *bg, *fg, text);
    }

    // ---- Panel 1：Token 用量 Top 20 ----
    content_y += s(4);
    let ranges1 = model_ranges();
    let seg_w = s(44) * ranges1.len() as i32;
    let y = screen(content_y);
    // 合计放标题行右端、切换器左侧；标题框宽度相应收窄
    let total_right = (width - pad - seg_w - s(8)) as f32;
    let title_right = draw_panel_total(view, model_total, total_right, y as f32, s(22) as f32);
    draw_text(view.g, view.theme, view.hdc, crate::i18n::texts().top20_title, 12, true, SECONDARY_TEXT,
        RectF::new(pad as f32, y as f32, (title_right - pad as f32).max(0.0), s(22) as f32),
        super::drawing::TextAlign::LeftCenter);
    for (rect, index) in chart::draw_segmented(view.g, view.theme, view.hdc, width - pad, y - s(2), ranges1, model_range) {
        hits.push((in_content(rect, body_top, view.scroll), PanelHit::ModelRange(index)));
    }
    content_y += s(26);

    // 面板高度恒定为 TOP20_MAX_ROWS 行：行数不足时顶端对齐、下方留白，
    // 保证切换选项卡时两个 Panel 的总高度不变
    let rows_area = s(20) * TOP20_MAX_ROWS as i32;
    let card_h = rows_area + s(16);
    let card_rect = RectF::new(pad as f32, screen(content_y) as f32, (width - pad * 2) as f32, card_h as f32);
    if visible(card_rect, body_top, body_h) {
        draw_card(view.g, card_rect, s(10) as f32);
        if model_rows.is_empty() {
            draw_text(view.g, view.theme, view.hdc, crate::i18n::texts().empty_data, 12, false, MUTED_TEXT,
                RectF::new(card_rect.x, card_rect.y + card_rect.h / 2.0 - s(11) as f32, card_rect.w, s(22) as f32),
                super::drawing::TextAlign::CenterCenter);
        } else {
            chart::draw_top_bars(view.g, view.theme, view.hdc,
                RectF::new(card_rect.x + s(10) as f32, card_rect.y + s(8) as f32,
                    card_rect.w - s(20) as f32, rows_area as f32),
                model_rows);
        }
    }
    content_y += card_h + s(12);

    // ---- Panel 2：每日 Token 用量 ----
    let ranges2 = daily_ranges();
    let seg_w2 = s(44) * ranges2.len() as i32;
    let y = screen(content_y);
    let total_right2 = (width - pad - seg_w2 - s(8)) as f32;
    let title_right2 = draw_panel_total(view, daily_total, total_right2, y as f32, s(22) as f32);
    draw_text(view.g, view.theme, view.hdc, crate::i18n::texts().daily_title, 12, true, SECONDARY_TEXT,
        RectF::new(pad as f32, y as f32, (title_right2 - pad as f32).max(0.0), s(22) as f32),
        super::drawing::TextAlign::LeftCenter);
    for (rect, index) in chart::draw_segmented(view.g, view.theme, view.hdc, width - pad, y - s(2), ranges2, daily_range) {
        hits.push((in_content(rect, body_top, view.scroll), PanelHit::DailyRange(index)));
    }
    content_y += s(26);

    // 剩余纵向空间补给每日图，避免窗口下半部大片空白（内容超出时不变）
    let base_chart_h = s(150);
    let spare = body_h - (content_y + base_chart_h + s(12) + s(10));
    let chart_h = if spare > 0 {
        base_chart_h + spare.min(s(300))
    } else {
        base_chart_h
    };
    let card2_rect = RectF::new(pad as f32, screen(content_y) as f32, (width - pad * 2) as f32, (chart_h + s(12)) as f32);
    if visible(card2_rect, body_top, body_h) {
        draw_card(view.g, card2_rect, s(10) as f32);
        if daily.is_empty() {
            draw_text(view.g, view.theme, view.hdc, crate::i18n::texts().empty_data, 12, false, MUTED_TEXT,
                RectF::new(card2_rect.x, card2_rect.y + card2_rect.h / 2.0 - s(11) as f32, card2_rect.w, s(22) as f32),
                super::drawing::TextAlign::CenterCenter);
        } else {
            chart::draw_daily_columns(view.g, view.theme, view.hdc,
                RectF::new(card2_rect.x + s(10) as f32, card2_rect.y + s(6) as f32,
                    card2_rect.w - s(20) as f32, chart_h as f32),
                daily);
        }
    }
    content_y += chart_h + s(12) + s(10);

    view.g.reset_clip();

    Dashboard {
        content_height: content_y,
        hits,
    }
}

/// 在面板标题行右端绘制「合计 <量>」（文案与单位随语言），返回合计文本左缘 x。
/// `right` / `y` / `h` 为物理像素（调用方已 `s()`）；标题框右缘应取返回值再留间距。
fn draw_panel_total(view: &mut ContentView, tokens: i64, right: f32, y: f32, h: f32) -> f32 {
    let text = format!(
        "{} {}",
        crate::i18n::texts().panel_total,
        super::drawing::format_tokens(tokens)
    );
    let (text_w, _) = super::drawing::measure_text(view.g, view.theme, view.hdc, &text, 12, false);
    let x = (right - text_w).max(0.0);
    // 布局框比实测宽 2px，避免最后一个像素被裁
    draw_text(view.g, view.theme, view.hdc, &text, 12, false, TEXT,
        RectF::new(x, y, text_w + 2.0, h),
        super::drawing::TextAlign::LeftCenter);
    x
}

/// 把绘制用的屏幕坐标矩形换算成 body 内容坐标（供命中测试）。
fn in_content(rect: RectF, body_top: i32, scroll: i32) -> RectF {
    RectF::new(
        rect.x,
        rect.y - body_top as f32 + scroll as f32,
        rect.w,
        rect.h,
    )
}

fn visible(rect: RectF, body_top: i32, body_h: i32) -> bool {
    rect.bottom() >= body_top as f32 && rect.y <= (body_top + body_h) as f32
}

/// 单行提示卡片（自动折两行）；`card_y` 为屏幕坐标，返回占用高度。
fn info_card(view: &mut ContentView, card_y: i32, bg: u32, fg: u32, text: &str) -> i32 {
    let dpi = view.theme.dpi;
    let s = move |v: i32| ((v as f64 * dpi as f64 / 96.0) + 0.5).floor() as i32;
    let pad = s(12);
    let inner = view.width - pad * 2 - s(20);
    let line_h = s(18) as f32;
    let (line1, line2) = wrap_two_lines(view, text, 12, inner as f32);
    let lines = if line2.is_empty() { 1 } else { 2 };
    let card_h = s(10) * 2 + lines * s(18);
    super::drawing::fill_rounded(view.g, bg, RectF::new(pad as f32, card_y as f32, (view.width - pad * 2) as f32, card_h as f32), s(8) as f32);
    let text_x = (pad + s(10)) as f32;
    draw_text(view.g, view.theme, view.hdc, &line1, 12, false, fg,
        RectF::new(text_x, card_y as f32 + s(10) as f32, inner as f32, line_h),
        super::drawing::TextAlign::LeftCenter);
    if !line2.is_empty() {
        draw_text(view.g, view.theme, view.hdc, &line2, 12, false, fg,
            RectF::new(text_x, card_y as f32 + s(10) as f32 + s(18) as f32, inner as f32, line_h),
            super::drawing::TextAlign::LeftCenter);
    }
    card_h + s(8)
}

/// 贪心折两行（超出省略）。
fn wrap_two_lines(view: &mut ContentView, text: &str, logical_px: i32, max_width: f32) -> (String, String) {
    let chars: Vec<char> = text.chars().collect();
    let mut line1_end = chars.len();
    for end in 1..=chars.len() {
        let candidate: String = chars[..end].iter().collect();
        let (width, _) = super::drawing::measure_text(view.g, view.theme, view.hdc, &candidate, logical_px, false);
        if width > max_width {
            line1_end = end - 1;
            break;
        }
    }
    let line1: String = chars[..line1_end].iter().collect();
    let rest: String = chars[line1_end..].iter().collect();
    if rest.is_empty() {
        return (line1, String::new());
    }
    let line2 = super::drawing::ellipsize(view.g, view.theme, view.hdc, &rest, logical_px, false, max_width);
    (line1, line2)
}

// 保留引用避免未使用告警
#[allow(dead_code)]
const _ACCENT: u32 = ACCENT;
#[allow(dead_code)]
const _ACCENT_SOFT: u32 = ACCENT_SOFT;
#[allow(dead_code)]
fn _now_fmt(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    format_relative(at, now)
}
