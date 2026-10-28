//! 把 Omarchy 的 `colors.toml` 换算成一整套界面 [`Theme`]。
//!
//! 换算原则（详见 `docs/omarchy-theme-design.md` 第 4.4 节）：
//!
//! 1. 只把键当色值用，不信键名里的明暗含义——`rose-pine` 的 `lighter_background`
//!    就比 `background` 暗。底色先按亮度排好再分层级。
//! 2. 卡片、编辑区（`surface`）直接用主题的 `background`，那是 Omarchy 终端里
//!    看到的颜色，一眼认得出来。
//! 3. 派生色在 OKLab 里混，深色主题上不发灰。
//! 4. 文字类颜色混完再过一道 WCAG 对比度，不够就往远离底色的方向推。

use super::LABEL;
use super::colors::Colors;
use crate::theme::{MdPalette, PaperFamily, Theme};
use eframe::egui::Color32;

/// 主文字对卡片、画布、沉底三种底色的最低对比度。
pub(crate) const TEXT_MIN: f32 = 7.0;
/// 次级文字与 Markdown 高亮里的文字色。
pub(crate) const SOFT_MIN: f32 = 4.5;
/// 弱化文字、强调色与语义色（大字、图形标准）。
pub(crate) const MUTED_MIN: f32 = 3.0;
/// 强调色实底上的文字（[`on_accent_for`] 取对比度最高的候选，测试按此验收）。
#[cfg(test)]
pub(crate) const ON_ACCENT_MIN: f32 = 4.5;

/// 主题缺某个色相键时的后备色，取自默认的 Claude 奶油。
const FALLBACK_RED: Color32 = Color32::from_rgb(0xB4, 0x46, 0x3C);
const FALLBACK_YELLOW: Color32 = Color32::from_rgb(0xA1, 0x63, 0x1C);
const FALLBACK_GREEN: Color32 = Color32::from_rgb(0x3C, 0x74, 0x53);
const FALLBACK_BLUE: Color32 = Color32::from_rgb(0x44, 0x66, 0x8A);
const FALLBACK_MAGENTA: Color32 = Color32::from_rgb(0x6B, 0x57, 0x8A);

/// 换算一份 Omarchy 配色。
pub(crate) fn derive(colors: &Colors) -> Theme {
    let bg = colors.get("background").unwrap_or(Color32::WHITE);
    let fg = colors.get("foreground").unwrap_or(Color32::BLACK);
    let dark = colors.dark.unwrap_or(luminance(bg) < 0.18);
    // 文字、强调色对比度不够时往这边推：深色主题推亮，明色主题推暗。
    let away = if dark { Color32::WHITE } else { Color32::BLACK };
    let hue = |key: &str, fallback: Color32| colors.get(key).unwrap_or(fallback);

    // ── 底色层级 ──
    // 比 background 暗的候选按亮度从高到低排，离 background 最近的在前。
    let mut deeper: Vec<Color32> = ["dark_background", "darker_background", "lighter_background"]
        .into_iter()
        .filter_map(|key| colors.get(key))
        .filter(|color| luminance(*color) < luminance(bg) && delta_e(*color, bg) >= 3.0)
        .collect();
    deeper.sort_by(|a, b| luminance(*b).total_cmp(&luminance(*a)));
    deeper.dedup_by(|later, earlier| delta_e(*later, *earlier) < 2.0);

    let (canvas, surface) = match deeper.first() {
        Some(&canvas) => (canvas, bg),
        // 纯黑一类的主题没有更暗的底：画布用原色，卡片往前景提一点。
        None if dark => (bg, mix(bg, fg, 0.06)),
        None => (mix(bg, fg, 0.04), bg),
    };
    let surface_sunk = match deeper.get(1) {
        Some(&sunk) if dark => sunk,
        _ if dark => mix(surface, fg, 0.04),
        _ => mix(canvas, fg, 0.04),
    };
    let surface_hover = mix(surface, fg, 0.08);
    let surface_active = colors
        .get("selection")
        .filter(|selection| {
            delta_e(*selection, surface) >= 6.0 && delta_e(*selection, surface_hover) >= 3.0
        })
        .unwrap_or_else(|| mix(surface, fg, 0.15));
    let border = mix(surface, fg, 0.12);
    let border_strong = mix(surface, fg, 0.24);

    // ── 文字 ──
    let grounds = [surface, canvas, surface_sunk];
    let text = ensure_contrast(fg, &grounds, TEXT_MIN, away);
    let text_soft = ensure_contrast(mix(fg, surface, 0.22), &grounds, SOFT_MIN, away);
    let text_muted = ensure_contrast(
        colors
            .get("dark_foreground")
            .unwrap_or_else(|| mix(fg, surface, 0.45)),
        &grounds,
        MUTED_MIN,
        away,
    );
    let heading = ensure_contrast(
        colors.get("bright_foreground").unwrap_or(fg),
        &grounds,
        TEXT_MIN,
        away,
    );

    // ── 强调色 ──
    let accent = ensure_contrast(
        colors.get("accent").unwrap_or(FALLBACK_BLUE),
        &[canvas, surface],
        MUTED_MIN,
        away,
    );
    let on_accent = on_accent_for(accent, bg, fg, colors.get("bright_foreground"));
    // 悬停、按下都往远离按钮字色的方向走，按钮字只会越来越清楚。
    let pressed_toward = if luminance(on_accent) > 0.5 {
        Color32::BLACK
    } else {
        Color32::WHITE
    };
    let accent_hover = mix(accent, pressed_toward, 0.10);
    let accent_active = mix(accent, pressed_toward, 0.22);
    let accent_soft = soft_ground(surface, accent);

    // ── 语义色 ──
    let semantic = |color: Color32| ensure_contrast(color, &[surface, canvas], MUTED_MIN, away);
    let warn = semantic(hue("yellow", FALLBACK_YELLOW));
    let danger = semantic(hue("red", FALLBACK_RED));
    let success = semantic(hue("green", FALLBACK_GREEN));
    let blue = hue("blue", FALLBACK_BLUE);
    let info = semantic(if delta_e(blue, accent) < 8.0 {
        colors.get("cyan").unwrap_or(blue)
    } else {
        blue
    });
    let warn_soft = mix(surface, warn, 0.16);
    let danger_soft = mix(surface, danger, 0.16);
    let success_soft = mix(surface, success, 0.16);

    // ── Markdown 审校区 ──
    let md_text = |color: Color32| ensure_contrast(color, &[surface], SOFT_MIN, away);
    let strong = md_text(
        colors
            .get("orange")
            .unwrap_or_else(|| hue("yellow", FALLBACK_YELLOW)),
    );
    let md = MdPalette {
        body: text,
        marker: text_muted,
        title: md_text(accent),
        heading,
        strong,
        strong_bg: mix(surface, strong, 0.14),
        bullet: accent,
        table_pipe: border_strong,
        table_rule: text_muted,
        table_cell: md_text(hue("cyan", blue)),
        comment: text_muted,
        comment_bg: mix(surface, fg, 0.05),
        todo: ensure_contrast(danger, &[surface, danger_soft], SOFT_MIN, away),
        todo_bg: danger_soft,
        code: md_text(hue("green", FALLBACK_GREEN)),
        quoted: md_text(hue("magenta", FALLBACK_MAGENTA)),
        anchor_bg: surface_active,
        search_bg: highlight_ground(surface, hue("yellow", FALLBACK_YELLOW), text),
    };

    // 纸面只承接明暗与冷暖，不吃强调色。
    let warm = i16::from(bg.r()) - i16::from(bg.b()) > 6;
    let dark_paper_family = if warm {
        PaperFamily::Sandalwood
    } else {
        PaperFamily::NightBlue
    };
    let paper_family = match (dark, warm) {
        (true, _) => dark_paper_family,
        (false, true) => PaperFamily::Parchment,
        (false, false) => PaperFamily::Original,
    };

    Theme {
        label: LABEL,
        dark,
        paper_family,
        dark_paper_family,
        canvas,
        surface,
        surface_sunk,
        surface_hover,
        surface_active,
        border,
        border_strong,
        text,
        text_soft,
        text_muted,
        accent,
        accent_hover,
        accent_active,
        accent_soft,
        on_accent,
        warn,
        warn_soft,
        danger,
        danger_soft,
        success,
        success_soft,
        info,
        md,
    }
}

/// 强调色实底上的字色：在白、主题底色、主题前景、亮前景里挑对比度最高的一个。
fn on_accent_for(accent: Color32, bg: Color32, fg: Color32, bright: Option<Color32>) -> Color32 {
    [Color32::WHITE, bg, fg, bright.unwrap_or(fg), Color32::BLACK]
        .into_iter()
        .max_by(|a, b| contrast(*a, accent).total_cmp(&contrast(*b, accent)))
        .unwrap_or(Color32::WHITE)
}

/// 强调色的淡底：混得越淡越不抢眼，但强调色压在上面仍要读得清（≥ 3:1）。
fn soft_ground(surface: Color32, accent: Color32) -> Color32 {
    [0.16, 0.12, 0.08, 0.04]
        .into_iter()
        .map(|amount| mix(surface, accent, amount))
        .find(|soft| contrast(accent, *soft) >= MUTED_MIN)
        .unwrap_or(surface)
}

/// 查找命中的衬底：黄色混得越浓越醒目，但压在上面的正文仍要 ≥ 4.5:1。
fn highlight_ground(surface: Color32, tint: Color32, text: Color32) -> Color32 {
    [0.28, 0.22, 0.16, 0.10]
        .into_iter()
        .map(|amount| mix(surface, tint, amount))
        .find(|ground| contrast(text, *ground) >= SOFT_MIN)
        .unwrap_or(surface)
}

/// 颜色对每种底色都要达到 `min`；不够就按 5% 一步往 `away` 混，直到达标。
pub(crate) fn ensure_contrast(
    color: Color32,
    grounds: &[Color32],
    min: f32,
    away: Color32,
) -> Color32 {
    let worst = |candidate: Color32| {
        grounds
            .iter()
            .map(|ground| contrast(candidate, *ground))
            .fold(f32::INFINITY, f32::min)
    };
    (0..=20)
        .map(|step| mix(color, away, step as f32 * 0.05))
        .find(|candidate| worst(*candidate) >= min)
        .unwrap_or(away)
}

// ── 色彩运算 ────────────────────────────────────────────────────────────────

fn to_linear(channel: u8) -> f32 {
    let v = f32::from(channel) / 255.0;
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let srgb = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round().clamp(0.0, 255.0) as u8
}

/// sRGB → OKLab。
fn oklab(color: Color32) -> [f32; 3] {
    let (r, g, b) = (
        to_linear(color.r()),
        to_linear(color.g()),
        to_linear(color.b()),
    );
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab → sRGB，超出色域的分量截断。
fn from_oklab([l, a, b]: [f32; 3]) -> Color32 {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    Color32::from_rgb(
        from_linear(4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3),
        from_linear(-1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3),
        from_linear(-0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3),
    )
}

/// 在 OKLab 里从 `from` 向 `to` 混 `amount`（0–1）。
pub(crate) fn mix(from: Color32, to: Color32, amount: f32) -> Color32 {
    let (a, b) = (oklab(from), oklab(to));
    let t = amount.clamp(0.0, 1.0);
    from_oklab([
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ])
}

/// 两色在 OKLab 里的距离，乘 100 后与常见的 ΔE 量级相当（2 左右刚能分辨）。
pub(crate) fn delta_e(a: Color32, b: Color32) -> f32 {
    let (a, b) = (oklab(a), oklab(b));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() * 100.0
}

/// WCAG 2.1 相对亮度。
pub(crate) fn luminance(color: Color32) -> f32 {
    0.2126 * to_linear(color.r()) + 0.7152 * to_linear(color.g()) + 0.0722 * to_linear(color.b())
}

/// WCAG 对比度，1.0（同色）到 21.0（黑对白）。
pub(crate) fn contrast(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}
