//! 封面排版。
//!
//! 由 src/preview/research.rs 拆分而来：本文件是模块 `preview::research::cover`，与其它子模块共享
//! `preview::research` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export;
use crate::models::DraftInput;
use crate::preview::layout::sheet;
use crate::preview::research::report_title;
use crate::preview::{Metrics, math_flow};
use crate::theme;
use egui::Align;

/// 封面：照 mdx `cover::layout` 的毫米坐标整页绝对定位，与研究报告模板
/// 封面、Word 的图文框封面是同一张网格。题名换几行都不推动落款。
pub(crate) fn cover_sheet(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    input: &DraftInput,
    markdown: &str,
) {
    use mdx::cover::{self, Family, layout as l};

    let meta = &input.research;
    let family = Family::of(&meta.file_type);
    // 题名的正主是文档要素的「文件名称」（导出时写进 frontmatter）；留空
    // 才回退到正文区的 `#`，与 mdx 的 `cover.title.or(emitter.report_title())`
    // 一条口径。两处都空才出待核实占位。
    let body_title = report_title(markdown);
    let title = match (input.title_hint.trim(), body_title.as_deref()) {
        ("", Some(title)) => title,
        ("", None) => "【待核实：文件名称】",
        (hint, _) => hint,
    };
    // 密级★保密期限：公开件不标；年限为空时不出星。
    let security = match (
        cover::security_label(&meta.security),
        meta.security_years.trim(),
    ) {
        ("", _) => String::new(),
        (level, "") => level.to_string(),
        (level, years) => format!("{level}★{years}"),
    };
    let doc_type = match meta.file_type.trim() {
        "" => "研究报告",
        doc_type => doc_type,
    };

    sheet(ui, metrics, |ui| {
        let height = (metrics.page_height - metrics.margin_top * 2.0).max(0.0);
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
        let page = rect.min - egui::vec2(metrics.margin_left, metrics.margin_top);
        let at = |x: f32, y: f32| page + egui::vec2(metrics.mm(x), metrics.mm(y));
        let painter = ui.painter();
        let ink = theme::paper::ink();
        let faint = theme::paper::ink_faint();
        let center_x = l::PAGE_WIDTH / 2.0;

        // 一段字：`x` 是锚点（居中时为中线，左对齐时为左边），`y` 是顶边。
        // 返回画完后的底边（mm），题名块靠它往下接。
        let text = |spec: CoverText<'_>, x: f32, y: f32| -> f32 {
            let font = metrics.font(spec.family, spec.pt);
            let size = font.size;
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = metrics.mm(spec.wrap);
            job.halign = spec.align;
            job.append(
                spec.text,
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: spec.color.unwrap_or(ink),
                    line_height: Some(size * spec.leading),
                    extra_letter_spacing: size * spec.tracking,
                    italics: spec.italic,
                    ..Default::default()
                },
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let bottom = y + galley.size().y / metrics.mm(1.0);
            painter.galley(at(x, y), galley, ink);
            bottom
        };
        let bar = |x: f32, y: f32, w: f32, h: f32, color: egui::Color32| {
            painter.rect_filled(
                egui::Rect::from_min_max(at(x, y), at(x + w, y + h)),
                0.0,
                color,
            );
        };

        // 密级（左）与编号（右）。
        if !security.is_empty() {
            text(
                CoverText::new(&security, theme::FONT_HEITI, l::META_PT).left(),
                l::SIDE,
                l::META_TOP,
            );
        }
        let number = meta.file_number.trim();
        if !number.is_empty() {
            text(
                CoverText::new(&format!("编号：{number}"), theme::FONT_HEITI, l::META_PT).right(),
                l::PAGE_WIDTH - l::SIDE,
                l::META_TOP,
            );
        }

        // 文种：黑体小二，字距半字。字距加在每个字后面，把整行往右推了半字，
        // 这里左移回来，让字面居中。
        let tracking = l::TYPE_PT * l::TYPE_TRACKING_EM * 0.3528 / 2.0;
        text(
            CoverText::new(doc_type, theme::FONT_HEITI, l::TYPE_PT).tracking(l::TYPE_TRACKING_EM),
            center_x - tracking,
            l::TYPE_TOP,
        );
        let ident = meta.ident.trim();
        if !ident.is_empty() {
            text(
                CoverText::new(ident, theme::FONT_SONGTI, l::IDENT_PT),
                center_x,
                l::IDENT_TOP,
            );
        }

        // 反线：项目类单线，研究类一粗一细。
        bar(l::SIDE, l::RULE_TOP, l::TEXT_WIDTH, l::RULE_THICK, ink);
        if !family.is_project() {
            bar(
                l::SIDE,
                l::RULE_TOP + l::RULE_THICK + l::RULE_GAP,
                l::TEXT_WIDTH,
                l::RULE_THIN,
                ink,
            );
        }

        // 题名、稿次、外文原题顺排。题名按公文标题的断行规矩分行，与导出的
        // PDF、Word 用同一个函数，断在同一处。
        let title = export::title::cover_title_lines(title).join("\n");
        let spec =
            CoverText::new(&title, theme::FONT_BIAOSONG, l::TITLE_PT).leading(l::TITLE_LEADING);
        let mut bottom = if math_flow::has_math(&title) {
            // 题名里有行内公式：公式占位排进同一个 galley，再按占位贴图，
            // 与 PDF 封面一样排成公式。
            let font = metrics.font(spec.family, spec.pt);
            let wrap = metrics.mm(spec.wrap);
            let line = font.size * spec.leading;
            let style = math_flow::FlowStyle::block((font.clone(), font), spec.pt, line, wrap);
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = wrap;
            job.halign = spec.align;
            let mut bold_ranges = Vec::new();
            let slots = math_flow::append_with_math(
                ui,
                metrics,
                &mut job,
                &title,
                &style,
                &mut bold_ranges,
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let origin = at(center_x, l::TITLE_TOP);
            painter.galley(origin, galley.clone(), ink);
            for chars in bold_ranges {
                crate::preview::layout::paint_synthetic_bold_galley(
                    painter, origin, &galley, chars,
                );
            }
            math_flow::paint_slots(painter, metrics, origin, &galley, &slots);
            l::TITLE_TOP + galley.size().y / metrics.mm(1.0)
        } else {
            text(spec, center_x, l::TITLE_TOP)
        };
        if let Some(version) = cover::version_mark(&meta.version) {
            bottom = text(
                CoverText::new(&version, theme::FONT_SONGTI, l::VERSION_PT),
                center_x,
                bottom + l::TITLE_GAP,
            );
        }
        let original = meta.original_title.trim();
        if !original.is_empty() && !family.is_project() {
            text(
                CoverText::new(original, theme::FONT_SONGTI, l::ORIGINAL_PT)
                    .leading(1.35)
                    .italic(),
                center_x,
                bottom + l::TITLE_GAP,
            );
        }

        match family.stage() {
            // 项目类：四格阶段条，当前格黑色粗线，其余灰色细线。
            Some(stage) => {
                let cell = (l::TEXT_WIDTH - 3.0 * l::STAGE_GAP) / 4.0;
                for (index, name) in cover::PROJECT_STAGES.iter().enumerate() {
                    let x = l::SIDE + index as f32 * (cell + l::STAGE_GAP);
                    let on = index == stage;
                    let (line, color) = if on {
                        (l::STAGE_LINE_ON, ink)
                    } else {
                        (l::STAGE_LINE, faint)
                    };
                    bar(x, l::STAGE_TOP, cell, line, color);
                    text(
                        CoverText::new(name, theme::FONT_HEITI, l::STAGE_PT)
                            .color(color)
                            .tracking(0.12),
                        x + cell / 2.0,
                        l::STAGE_TOP + l::STAGE_LINE_ON + 2.4,
                    );
                }
            }
            // 研究类：署名行。
            None => {
                let byline = meta.byline.trim();
                if !byline.is_empty() {
                    text(
                        CoverText::new(byline, theme::FONT_SONGTI, l::BYLINE_PT),
                        center_x,
                        l::BYLINE_TOP,
                    );
                }
            }
        }

        // 落款：单位黑体三号，日期宋体小三、汉字数字。
        let institution = match meta.institution.trim() {
            "" => "【待核实：撰写单位】",
            institution => institution,
        };
        let bottom = text(
            CoverText::new(institution, theme::FONT_HEITI, l::ORG_PT),
            center_x,
            l::ORG_TOP,
        );
        let date = meta.date.trim();
        if !date.is_empty() {
            text(
                CoverText::new(&cover::chinese_date(date), theme::FONT_SONGTI, l::DATE_PT),
                center_x,
                bottom + l::DATE_GAP,
            );
        }
    });
}

/// 封面上一段字的排法。缺省居中、宽度为版心、单倍行距、黑色。
pub(crate) struct CoverText<'a> {
    text: &'a str,
    family: &'a str,
    pt: f32,
    align: Align,
    wrap: f32,
    leading: f32,
    tracking: f32,
    italic: bool,
    color: Option<egui::Color32>,
}

impl<'a> CoverText<'a> {
    pub(crate) fn new(text: &'a str, family: &'a str, pt: f32) -> Self {
        Self {
            text,
            family,
            pt,
            align: Align::Center,
            wrap: mdx::cover::layout::TEXT_WIDTH - 10.0,
            leading: 1.3,
            tracking: 0.0,
            italic: false,
            color: None,
        }
    }

    pub(crate) fn left(mut self) -> Self {
        self.align = Align::LEFT;
        self
    }

    pub(crate) fn right(mut self) -> Self {
        self.align = Align::RIGHT;
        self
    }

    pub(crate) fn leading(mut self, leading: f32) -> Self {
        self.leading = leading;
        self
    }

    pub(crate) fn tracking(mut self, em: f32) -> Self {
        self.tracking = em;
        self
    }

    pub(crate) fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub(crate) fn color(mut self, color: egui::Color32) -> Self {
        self.color = Some(color);
        self
    }
}
