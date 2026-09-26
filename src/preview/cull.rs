//! 只排看得见的块：滚出视野很远的正文块沿用上一次量到的高度占位，不再排版。
//!
//! 预览每帧从头画一遍。长稿的正文块有几百上千个，每块都要避头尾断行、两端对齐、
//! 逐行排版，60 页的稿子每帧好几毫秒，还随篇幅线性增长——鼠标一动、滚动动画的
//! 每一帧都要付一遍，而屏幕上同时只看得见一两页。
//!
//! 做法：每块画完记下它占了多高（连同它在页边号栏里记下的行），按「块内容 +
//! 影响排版的上下文」做键存起来。下一帧轮到这块时，如果按记下的高度它整个落在
//! 可视区上下各放宽一屏之外，就只占位、不排版；可视区附近的块照常排、顺手更新高度。
//!
//! 为什么不会画错：
//! - 看得见的块永远是现排的，屏幕上的每一个字都是本帧的真实结果；
//! - 跳过的块只贡献一个高度。键里含块内容、标题编号计数器、缩放与版心宽度，
//!   这些不变高度就不变；字体设置之类没进键的变化，块一滚进视野就被重新量准；
//! - 标题编号在跳过时照样推进（调用方负责），页边行号按记下的行补回，导航刻度
//!   要回查的标题矩形也照样登记。

use super::Metrics;
use super::gutter::CachedRow;
use super::memo;
use eframe::egui;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::sync::{Arc, Mutex};

/// 缓存条目超过这个数就清掉很久没用过的。
const PRUNE_ABOVE: usize = 4096;
/// 清理时保留最近这么多帧里用过的条目。
const KEEP_PASSES: u64 = 600;

/// 一块上次排版的结果。
struct Measured {
    height: f32,
    rows: Vec<CachedRow>,
    /// 最近一次用到它的帧号，清理时看它。
    used: u64,
}

#[derive(Default)]
struct Heights {
    blocks: HashMap<u64, Measured>,
}

/// 一次预览绘制里的裁剪器：对每个正文块决定现排还是占位。
pub(crate) struct Cull {
    heights: Arc<Mutex<Heights>>,
    /// 纵向上值得真排的范围：可视区上下各放宽一屏。
    window: egui::Rangef,
    /// 影响所有块排版的上下文（缩放、版心、字面、号栏开关）。
    salt: u64,
    pass: u64,
}

impl Cull {
    /// `flavor` 区分版式（公文 / 研究报告），同样内容的块在两种版式里高度不同。
    pub(crate) fn new(ui: &egui::Ui, metrics: &Metrics, flavor: &'static str) -> Self {
        let ctx = ui.ctx();
        let heights = ctx.data_mut(|data| {
            data.get_temp_mut_or_default::<Arc<Mutex<Heights>>>(egui::Id::new(
                "gw-preview-block-heights",
            ))
            .clone()
        });
        let pass = ctx.cumulative_pass_nr();
        if let Ok(mut heights) = heights.lock()
            && heights.blocks.len() > PRUNE_ABOVE
        {
            heights
                .blocks
                .retain(|_, block| block.used + KEEP_PASSES >= pass);
        }
        let clip = ui.clip_rect();
        let margin = clip.height().max(200.0);
        let salt = memo::key((
            flavor,
            metrics.scale.to_bits(),
            metrics.content.to_bits(),
            metrics.line.to_bits(),
            metrics.body_family,
            metrics.body_pt.to_bits(),
            metrics.line_numbers_on(),
            ctx.pixels_per_point().to_bits(),
        ));
        Self {
            heights,
            window: egui::Rangef::new(clip.top() - margin, clip.bottom() + margin),
            salt,
            pass,
        }
    }

    /// 画一个正文块，或者在它远离视野时只占位。返回这一帧是否真的画了。
    ///
    /// - `key`：块内容与影响它排版的上下文（标题编号计数器等），由调用方给；
    /// - `source`：块在源码中的范围。行号的源码范围相对它的起点记，前面的字
    ///   增删、块整体挪位也能复用；
    /// - `keep`：这一帧必须真画（锚点落在块里要滚过去、要铺底色）；
    /// - `heading`：跳过时仍要登记的标题矩形 id 范围，导航刻度靠它回查位置。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn block(
        &self,
        ui: &mut egui::Ui,
        metrics: &Metrics,
        key: impl Hash,
        source: &Range<usize>,
        keep: bool,
        heading: Option<&Range<usize>>,
        draw: impl FnOnce(&mut egui::Ui),
    ) -> bool {
        let key = memo::key((self.salt, key));
        let origin = ui.cursor().min;
        if !keep && let Some((height, rows)) = self.cached(key, origin.y) {
            metrics.gutter_replay(&rows, origin, source.start);
            ui.add_space(height);
            if let Some(range) = heading {
                // 与 `layout::clickable` 同一个 id：导航按它回查标题的版面位置。
                ui.interact(
                    egui::Rect::from_min_size(origin, egui::vec2(metrics.content, height)),
                    egui::Id::new(("gw-preview-block", range.start, range.end)),
                    egui::Sense::hover(),
                );
            }
            return false;
        }
        let mark = metrics.gutter_mark();
        draw(ui);
        let height = ui.cursor().min.y - origin.y;
        let rows = metrics.gutter_capture(mark, origin, source.start);
        if let Ok(mut heights) = self.heights.lock() {
            heights.blocks.insert(
                key,
                Measured {
                    height,
                    rows,
                    used: self.pass,
                },
            );
        }
        true
    }

    /// 键为 `key` 的块若有记录、且按记录的高度摆在 `top` 处整个落在视野窗口
    /// 之外，返回它的高度与号栏行；否则返回 None（要现排）。
    fn cached(&self, key: u64, top: f32) -> Option<(f32, Vec<CachedRow>)> {
        let mut heights = self.heights.lock().ok()?;
        let block = heights.blocks.get_mut(&key)?;
        let bottom = top + block.height;
        if bottom >= self.window.min && top <= self.window.max {
            return None;
        }
        block.used = self.pass;
        Some((block.height, block.rows.clone()))
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{DraftInput, NumberingConfig, TemplateKind};
    use crate::preview::{PreviewScale, official_preview};
    use crate::theme;
    use crate::units::UnitDisplay;
    use crate::visual_diff::ElementMarks;
    use eframe::egui;

    /// 40 节、每节三段的长稿：可视区只装得下其中一小截。
    fn long_markdown() -> String {
        let paragraph = "为进一步推进服务事项标准化、规范化、便利化，全面掌握各单位年度工作进展，请结合实际报送年度标准化建设情况。";
        let mut parts = vec!["# 关于报送标准化建设情况的函".to_string()];
        for section in 1..=40 {
            parts.push(format!("## 专项工作{section}"));
            for index in 1..=3 {
                parts.push(format!("第{section}节第{index}段。{paragraph}"));
            }
        }
        parts.join("\n\n")
    }

    struct Frame {
        texts: Vec<String>,
        content_height: f32,
    }

    /// 在 800×900 的窗口里、滚到 `offset` 处画一帧公文预览。
    fn frame(
        ctx: &egui::Context,
        input: &DraftInput,
        markdown: &str,
        offset: f32,
        line_numbers: bool,
    ) -> Frame {
        let vocabulary = Vec::new();
        let display = UnitDisplay::new(&vocabulary);
        let numbering = NumberingConfig::default();
        let mut content_height = 0.0;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 900.0),
                )),
                ..Default::default()
            },
            |ui| {
                let scrolled = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(offset)
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let _ = official_preview(
                            ui,
                            input,
                            &display,
                            markdown,
                            PreviewScale::zoom(Some(1.0)),
                            None,
                            false,
                            &numbering,
                            line_numbers,
                            &ElementMarks::default(),
                        );
                    });
                content_height = scrolled.content_size.y;
            },
        );
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_texts(&clipped.shape, &mut texts);
        }
        Frame {
            texts,
            content_height,
        }
    }

    fn collect_texts(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Vec(inner) => {
                inner.iter().for_each(|shape| collect_texts(shape, out));
            }
            egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_string()),
            _ => {}
        }
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        ctx
    }

    fn contains(frame: &Frame, needle: &str) -> bool {
        frame.texts.iter().any(|text| text.contains(needle))
    }

    #[test]
    fn far_blocks_are_skipped_but_keep_their_height() {
        let ctx = context();
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let markdown = long_markdown();
        // 第一帧还没有记录，整篇照排；之后远处的块只占位。
        let full = frame(&ctx, &input, &markdown, 0.0, false);
        let culled = frame(&ctx, &input, &markdown, 0.0, false);
        assert!(contains(&full, "第40节第3段"), "首帧应整篇排版");
        assert!(!contains(&culled, "第40节第3段"), "远处的块不该再排版");
        assert!(contains(&culled, "第1节第1段"), "看得见的块照常排");
        assert!(
            (full.content_height - culled.content_height).abs() < 0.5,
            "占位高度应与真排一致：{} vs {}",
            full.content_height,
            culled.content_height
        );
    }

    #[test]
    fn research_reports_skip_far_blocks_too() {
        let ctx = context();
        let input = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..Default::default()
        };
        let markdown = long_markdown();
        let full = frame(&ctx, &input, &markdown, 0.0, false);
        let culled = frame(&ctx, &input, &markdown, 0.0, false);
        assert!(contains(&full, "第40节第3段"), "首帧应整篇排版");
        assert!(!contains(&culled, "第40节第3段"), "远处的块不该再排版");
        assert!(
            (full.content_height - culled.content_height).abs() < 0.5,
            "占位高度应与真排一致：{} vs {}",
            full.content_height,
            culled.content_height
        );
    }

    #[test]
    fn skipped_headings_still_advance_the_numbering() {
        let ctx = context();
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let markdown = long_markdown();
        let top = frame(&ctx, &input, &markdown, 0.0, false);
        let height = top.content_height;
        // 缓存已有全部块；滚到文末时前面三十几节都是跳过的。
        let _ = frame(&ctx, &input, &markdown, 0.0, false);
        let bottom = frame(&ctx, &input, &markdown, height - 900.0, false);
        assert!(!contains(&bottom, "第1节第1段"), "文首应已跳过");
        assert!(
            contains(&bottom, "四十、专项工作40"),
            "跳过的标题照样占号，末节仍是「四十、」：{:?}",
            bottom.texts
        );
        // 导航刻度要回查的标题矩形：首节虽然跳过了，仍登记在案。
        let first = markdown.find("## 专项工作1\n").unwrap();
        let range = first..first + "## 专项工作1".len();
        assert!(
            ctx.read_response(egui::Id::new(("gw-preview-block", range.start, range.end)))
                .is_some(),
            "跳过的标题也要登记矩形"
        );
    }

    #[test]
    fn line_numbers_match_a_full_layout() {
        let input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let markdown = long_markdown();
        let numbers = |frame: &Frame| {
            frame
                .texts
                .iter()
                .filter(|text| !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()))
                .cloned()
                .collect::<Vec<_>>()
        };
        let ctx = context();
        let height = frame(&ctx, &input, &markdown, 0.0, true).content_height;
        // 全新上下文滚到文末画第一帧：没有记录，整篇真排，号码是标准答案。
        let fresh = context();
        let full = frame(&fresh, &input, &markdown, height - 900.0, true);
        // 已有记录的上下文滚到文末：前面的块全跳过，号码靠补回的行接着数。
        let culled = frame(&ctx, &input, &markdown, height - 900.0, true);
        assert!(!contains(&culled, "第1节第1段"), "文首应已跳过");
        assert!(!numbers(&full).is_empty());
        assert_eq!(numbers(&full), numbers(&culled));
    }
}
