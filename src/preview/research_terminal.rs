//! 研究终端纸面装饰与封面；正文仍走研究报告的编号、公式和源码定位。

use super::{Metrics, layout::sheet, math_flow};
use crate::{export, models::DraftInput, theme};
use eframe::egui::{self, Color32, Stroke};
use std::ops::Range;

#[derive(Clone, Hash, Debug, PartialEq, Eq)]
pub(super) enum EntryKind {
    Part,
    Chapter,
    Section,
}

#[derive(Clone, Hash, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub kind: EntryKind,
    pub number: Option<String>,
    pub text: String,
    pub source: Range<usize>,
}

/// 与正文共用计数器，不从标题字面重新猜编号；部分分组不跨越附录。
pub(super) fn structure(located: &[export::LocatedBlock], markdown: &str) -> Vec<Entry> {
    use super::research::Kind;
    let mut entries = Vec::new();
    let mut part = 0;
    let mut backmatter = false;
    super::research::walk(located, markdown, |block, kind, _| {
        if let Some(section) = export::parse_research_marker(&markdown[block.range.clone()]) {
            backmatter = matches!(
                section,
                export::ResearchSection::ChangeLog | export::ResearchSection::References
            );
        }
        // mdx 不把版本变更记录、参考文献区的标题放进目录。
        if backmatter {
            return;
        }
        let (kind, number, text) = match kind {
            Kind::Part { text, .. } => {
                part += 1;
                (EntryKind::Part, Some(part.to_string()), text)
            }
            Kind::PartStar(text) => (EntryKind::Part, None, text),
            Kind::Chapter { number, text, .. } => (EntryKind::Chapter, Some(number), text),
            Kind::ChapterStar(text) => (EntryKind::Chapter, None, text),
            Kind::AbstractOpen => (EntryKind::Chapter, None, "摘要".into()),
            Kind::Section { number, text } if number.matches('.').count() == 1 => {
                (EntryKind::Section, Some(number), text)
            }
            Kind::SectionStar(text)
                if matches!(block.block, export::MarkdownBlock::Heading(3, _)) =>
            {
                (EntryKind::Section, None, text)
            }
            _ => return,
        };
        entries.push(Entry {
            kind,
            number,
            text,
            source: block.range.clone(),
        });
    });
    entries
}

pub(super) fn children<'a>(entries: &'a [Entry], source: &Range<usize>) -> Vec<&'a Entry> {
    entries
        .iter()
        .skip_while(|e| e.source != *source)
        .skip(1)
        .take_while(|e| {
            e.kind != EntryKind::Part
                && !e.number.as_deref().is_some_and(|n| {
                    e.kind == EntryKind::Chapter && n.chars().all(|c| c.is_ascii_uppercase())
                })
        })
        .filter(|e| e.kind == EntryKind::Chapter)
        .collect()
}

pub(super) fn is_toc(raw: &str) -> bool {
    raw.trim()
        .strip_prefix("<!--")
        .and_then(|s| s.strip_suffix("-->"))
        .is_some_and(|s| {
            matches!(
                s.trim()
                    .trim_start_matches(['[', '【'])
                    .trim_end_matches([']', '】'])
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "目录" | "toc" | "contents" | "tableofcontents"
            )
        })
}

fn text_block(ui: &mut egui::Ui, metrics: &Metrics, value: &str, pt: f32, width: f32) {
    let font = metrics.font(theme::FONT_HEITI, pt);
    let style = math_flow::FlowStyle::block((font.clone(), font), pt, metrics.pt(pt + 7.0), width);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = width;
    let mut bold_ranges = Vec::new();
    let slots = math_flow::append_with_math(ui, metrics, &mut job, value, &style, &mut bold_ranges);
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width, galley.size().y), egui::Sense::hover());
    ui.painter()
        .galley(rect.min, galley.clone(), theme::paper::ink());
    math_flow::paint_slots(ui.painter(), metrics, rect.min, &galley, &slots);
}

fn badge(ui: &mut egui::Ui, metrics: &Metrics, value: &str, size: f32, round: bool, pt: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::Vec2::splat(metrics.mm(size)), egui::Sense::hover());
    if round {
        ui.painter().circle_stroke(
            rect.center(),
            rect.width() / 2.0,
            Stroke::new(metrics.pt(1.0), accent()),
        );
    } else {
        ui.painter().rect_stroke(
            rect,
            0.0,
            Stroke::new(metrics.pt(1.0), accent()),
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        value,
        metrics.font(theme::FONT_RESEARCH_TERMINAL, pt),
        accent(),
    );
}

fn stamp(ui: &mut egui::Ui, metrics: &Metrics, value: &str) {
    let _ink = theme::paper::report_ink(theme::paper::ink_muted());
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(metrics.mm(7.0), metrics.mm(3.0)),
            egui::Sense::hover(),
        );
        ui.painter().hline(
            rect.x_range(),
            rect.center().y,
            Stroke::new(metrics.pt(0.7), accent()),
        );
        ui.add_space(metrics.mm(3.0));
        text_block(ui, metrics, value, 8.0, metrics.content - metrics.mm(12.0));
    });
}

pub(super) fn heading(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    title: &str,
    number: Option<&str>,
    prefix: Option<&str>,
    toc: bool,
) {
    ui.add_space(metrics.mm(8.0));
    stamp(
        ui,
        metrics,
        &if toc {
            "CONTENTS / 研究目录".into()
        } else if let Some(prefix) = prefix {
            format!("{prefix} / CHAPTER {}", number.unwrap_or("—"))
        } else {
            "RESEARCH / 研究报告".into()
        },
    );
    ui.add_space(metrics.mm(7.0));
    let id = number.map_or_else(
        || if toc { "BOM".into() } else { "※".into() },
        |n| {
            n.parse::<usize>()
                .map_or_else(|_| n.into(), |n| format!("{n:02}"))
        },
    );
    ui.horizontal_top(|ui| {
        badge(
            ui,
            metrics,
            &id,
            15.0,
            number.is_some(),
            if toc { 13.0 } else { 19.0 },
        );
        ui.add_space(metrics.mm(5.0));
        text_block(ui, metrics, title, 25.0, metrics.content - metrics.mm(21.0));
    });
    ui.add_space(metrics.mm(9.0));
    dashed_rule(ui, metrics);
    ui.add_space(metrics.mm(10.0));
}

pub(super) fn section(ui: &mut egui::Ui, metrics: &Metrics, text: &str) {
    let (number, title) = text
        .split_once(super::research::SECTION_GAP)
        .filter(|(n, _)| {
            n.contains('.')
                && n.chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase() || c == '.')
        })
        .unwrap_or(("", text));
    ui.add_space(metrics.mm(4.0));
    let level = number.matches('.').count();
    if level <= 1 {
        dashed_rule(ui, metrics);
        ui.add_space(metrics.mm(4.0));
    }
    {
        ui.horizontal_top(|ui| {
            let pt = if level <= 1 { 13.0 } else { 11.0 };
            let width = metrics
                .pt(pt * 0.65 * number.len() as f32)
                .max(metrics.mm(9.0))
                + metrics.mm(4.0);
            if !number.is_empty() {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(width, metrics.mm(if level <= 1 { 7.0 } else { 6.0 })),
                    egui::Sense::hover(),
                );
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    Stroke::new(metrics.pt(0.8), accent()),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    number,
                    metrics.font(theme::FONT_RESEARCH_TERMINAL, pt),
                    accent(),
                );
            }
            ui.add_space(metrics.mm(4.0));
            text_block(
                ui,
                metrics,
                title,
                if level <= 1 {
                    17.0
                } else if level == 2 {
                    15.0
                } else {
                    14.0
                },
                metrics.content - width - metrics.mm(5.0),
            );
        });
    }
    ui.add_space(metrics.mm(4.0));
}

fn dashed_rule(ui: &mut egui::Ui, metrics: &Metrics) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(metrics.content, metrics.mm(1.0)),
        egui::Sense::hover(),
    );
    let mut x = rect.left();
    while x < rect.right() {
        ui.painter().line_segment(
            [
                egui::pos2(x, rect.center().y),
                egui::pos2((x + metrics.mm(1.5)).min(rect.right()), rect.center().y),
            ],
            Stroke::new(metrics.pt(0.45), theme::paper::ink_faint()),
        );
        x += metrics.mm(2.5);
    }
}

/// 连续预览没有物理页码；目录仍按相同层级排，可点击回到对应源码。
pub(super) fn toc(
    ui: &mut egui::Ui,
    metrics: &Metrics,
    entries: &[Entry],
    clicked: &mut Option<Range<usize>>,
) {
    heading(ui, metrics, "目录", None, None, true);
    for e in entries {
        let response = ui
            .horizontal_top(|ui| {
                let section = e.kind == EntryKind::Section;
                if section {
                    ui.add_space(metrics.mm(14.0));
                }
                let width = if e.kind == EntryKind::Part {
                    22.0
                } else {
                    14.0
                };
                ui.allocate_ui(egui::vec2(metrics.mm(width), metrics.mm(5.0)), |ui| {
                    if e.kind == EntryKind::Part {
                        let _ink = theme::paper::report_ink(accent());
                        text_block(
                            ui,
                            metrics,
                            &format!("PART {}", e.number.as_deref().unwrap_or("—")),
                            10.0,
                            metrics.mm(width),
                        );
                    } else if section {
                        let _ink = theme::paper::report_ink(theme::paper::ink_muted());
                        text_block(
                            ui,
                            metrics,
                            e.number.as_deref().unwrap_or(""),
                            10.0,
                            metrics.mm(width),
                        );
                    } else {
                        badge(
                            ui,
                            metrics,
                            e.number.as_deref().unwrap_or("—"),
                            4.0,
                            true,
                            7.0,
                        );
                    }
                });
                text_block(
                    ui,
                    metrics,
                    &e.text,
                    12.0,
                    metrics.content - metrics.mm(width + if section { 15.0 } else { 1.0 }),
                );
            })
            .response;
        if ui
            .interact(
                response.rect,
                response.id.with("toc-target"),
                egui::Sense::click(),
            )
            .clicked()
        {
            *clicked = Some(e.source.clone());
        }
        if e.kind == EntryKind::Part {
            rule(ui, metrics);
        }
        ui.add_space(metrics.mm(if e.kind == EntryKind::Section {
            1.0
        } else {
            3.0
        }));
    }
    let _ink = theme::paper::report_ink(theme::paper::ink_muted());
    text_block(ui, metrics, "页码与引线见 PDF 排版", 8.0, metrics.content);
    ui.add_space(metrics.mm(12.0));
}

pub(super) fn part(ui: &mut egui::Ui, metrics: &Metrics, entry: &Entry, entries: &[Entry]) {
    ui.add_space(metrics.mm(8.0));
    stamp(
        ui,
        metrics,
        &format!("研究分部 / PART {}", entry.number.as_deref().unwrap_or("—")),
    );
    ui.add_space(metrics.mm(12.0));
    ui.horizontal(|ui| {
        badge(
            ui,
            metrics,
            &format!("P{}", entry.number.as_deref().unwrap_or("")),
            8.0,
            false,
            10.0,
        );
        ui.add_space(metrics.mm(4.0));
        let _ink = theme::paper::report_ink(theme::paper::ink_muted());
        let label = entry
            .number
            .as_deref()
            .and_then(|n| n.parse::<usize>().ok())
            .map(|n| format!("第{}部分", export::number_to_chinese(n)));
        text_block(
            ui,
            metrics,
            label.as_deref().unwrap_or("研究分部"),
            10.0,
            metrics.content - metrics.mm(13.0),
        );
    });
    ui.add_space(metrics.mm(7.0));
    text_block(ui, metrics, &entry.text, 30.0, metrics.content);
    ui.add_space(metrics.mm(12.0));
    stamp(ui, metrics, "章节明细 / CHAPTER LIST");
    rule(ui, metrics);
    for child in children(entries, &entry.source) {
        ui.add_space(metrics.mm(3.0));
        ui.horizontal_top(|ui| {
            badge(
                ui,
                metrics,
                child.number.as_deref().unwrap_or("—"),
                4.0,
                true,
                7.0,
            );
            ui.add_space(metrics.mm(6.0));
            text_block(
                ui,
                metrics,
                &child.text,
                12.0,
                metrics.content - metrics.mm(11.0),
            );
        });
        dashed_rule(ui, metrics);
    }
    ui.add_space(metrics.mm(25.0));
    if let Some(number) = &entry.number {
        let number = number
            .parse::<usize>()
            .map_or_else(|_| number.clone(), |n| format!("{n:02}"));
        if let Some((texture, aspect)) = hatched_number(ui.ctx(), &number) {
            let height = metrics.pt(100.0);
            let width = height * aspect;
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(metrics.content, height), egui::Sense::hover());
            ui.painter().image(
                texture.id(),
                egui::Rect::from_min_size(
                    egui::pos2(rect.right() - width, rect.top()),
                    egui::vec2(width, height),
                ),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            let galley = ui.painter().layout_no_wrap(
                number,
                metrics.font(theme::FONT_RESEARCH_TERMINAL, 100.0),
                theme::paper::ink_faint(),
            );
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(metrics.content, galley.size().y),
                egui::Sense::hover(),
            );
            ui.painter().galley(
                egui::pos2(rect.right() - galley.size().x, rect.top()),
                galley,
                theme::paper::ink_faint(),
            );
        }
    }
    ui.add_space(metrics.mm(15.0));
}

/// 字体轮廓生成斜线填充 SVG；缓存纹理，预览与 PDF 同样保留轮廓内部的网格空隙。
fn hatched_number(ctx: &egui::Context, number: &str) -> Option<(egui::TextureHandle, f32)> {
    use std::fmt::Write;
    static FONT: std::sync::LazyLock<Option<Vec<u8>>> = std::sync::LazyLock::new(|| {
        std::fs::read(crate::portable_runtime::find_font_dir()?.join("JetBrainsMono-Regular.ttf"))
            .ok()
    });
    let color = theme::paper::ink_muted();
    let key = egui::Id::new(("terminal-hatched-number", number, color));
    if let Some(hit) = ctx.data(|d| d.get_temp::<(egui::TextureHandle, f32)>(key)) {
        return Some(hit);
    }
    struct Outline(String);
    impl ttf_parser::OutlineBuilder for Outline {
        fn move_to(&mut self, x: f32, y: f32) {
            let _ = write!(self.0, "M{x},{y}");
        }
        fn line_to(&mut self, x: f32, y: f32) {
            let _ = write!(self.0, "L{x},{y}");
        }
        fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
            let _ = write!(self.0, "Q{x1},{y1} {x},{y}");
        }
        fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
            let _ = write!(self.0, "C{x1},{y1} {x2},{y2} {x},{y}");
        }
        fn close(&mut self) {
            self.0.push('Z');
        }
    }
    let face = ttf_parser::Face::parse(FONT.as_ref()?, 0).ok()?;
    let mut paths = String::new();
    let mut advance = 0.0_f32;
    let mut top = 0_i16;
    let mut bottom = 0_i16;
    for ch in number.chars() {
        let glyph = face.glyph_index(ch)?;
        let mut outline = Outline(String::new());
        let bounds = face.outline_glyph(glyph, &mut outline)?;
        top = top.max(bounds.y_max);
        bottom = bottom.min(bounds.y_min);
        let _ = write!(
            paths,
            "<path transform=\"translate({advance},0)\" d=\"{}\"/>",
            outline.0
        );
        advance += f32::from(face.glyph_hor_advance(glyph)?);
    }
    let height = f32::from(top - bottom) + 12.0;
    let width = advance + 12.0;
    let rgb = format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b());
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><defs><pattern id="h" width="70" height="70" patternUnits="userSpaceOnUse"><path d="M0,70 L70,0" fill="none" stroke="{rgb}" stroke-width="3"/></pattern></defs><g transform="translate(6,{}) scale(1,-1)" fill="url(#h)" stroke="{rgb}" stroke-width="5">{paths}</g></svg>"##,
        f32::from(top) + 6.0
    );
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &Default::default()).ok()?;
    let scale = 500.0 / height;
    let mut pixmap = resvg::tiny_skia::Pixmap::new((width * scale).ceil() as u32, 500)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let image = egui::ColorImage::from_rgba_premultiplied(
        [pixmap.width() as usize, pixmap.height() as usize],
        pixmap.data(),
    );
    let hit = (
        ctx.load_texture(
            format!("terminal-number/{number}"),
            image,
            egui::TextureOptions::LINEAR,
        ),
        width / height,
    );
    ctx.data_mut(|d| d.insert_temp(key, hit.clone()));
    Some(hit)
}

pub(super) fn accent() -> Color32 {
    if theme::paper::is_dark() {
        Color32::from_rgb(255, 189, 89)
    } else {
        Color32::from_rgb(181, 61, 48)
    }
}

pub(super) fn rule(ui: &mut egui::Ui, metrics: &Metrics) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(metrics.content, metrics.mm(2.0)),
        egui::Sense::hover(),
    );
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(metrics.pt(0.6), accent()),
    );
}

pub(super) fn paper(
    ui: &egui::Ui,
    metrics: &Metrics,
    rect: egui::Rect,
    index: egui::layers::ShapeIdx,
) {
    let grid = if theme::paper::is_dark() {
        Color32::from_rgb(28, 41, 49)
    } else {
        Color32::from_rgb(237, 240, 241)
    };
    let step = metrics.mm(5.0);
    let mut shapes = Vec::new();
    // 连续纸面只画视口附近的网格，长报告不会每帧生成数千条离屏线。
    let visible = rect.intersect(ui.clip_rect());
    for i in 0..=42 {
        let x = rect.left() + i as f32 * step;
        shapes.push(egui::Shape::line_segment(
            [
                egui::pos2(x, visible.top()),
                egui::pos2(x, visible.bottom()),
            ],
            Stroke::new(metrics.pt(0.2), grid),
        ));
    }
    let start = ((visible.top() - rect.top()) / step).max(0.0) as usize;
    let end = ((visible.bottom() - rect.top()) / step).max(0.0) as usize;
    for i in start..=end {
        let y = rect.top() + i as f32 * step;
        shapes.push(egui::Shape::line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(metrics.pt(0.2), grid),
        ));
    }
    shapes.push(egui::Shape::rect_stroke(
        rect.shrink(metrics.mm(15.0)),
        0.0,
        Stroke::new(metrics.pt(0.5), theme::paper::ink_faint()),
        egui::StrokeKind::Inside,
    ));
    ui.painter().set(index, egui::Shape::Vec(shapes));
}

pub(super) fn cover(ui: &mut egui::Ui, metrics: &Metrics, input: &DraftInput, markdown: &str) {
    sheet(ui, metrics, |ui| {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                metrics.content,
                metrics.page_height - 2.0 * metrics.margin_top,
            ),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        let meta = &input.research;
        let text = |y: f32, value: &str, pt: f32, color: Color32| {
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = metrics.content;
            job.append(
                value,
                0.0,
                egui::TextFormat {
                    font_id: metrics.font(theme::FONT_HEITI, pt),
                    color,
                    line_height: Some(metrics.pt(pt + 10.0)),
                    ..Default::default()
                },
            );
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            let bottom = y + galley.size().y / metrics.mm(1.0);
            painter.galley(rect.min + egui::vec2(0.0, metrics.mm(y)), galley, color);
            bottom
        };
        let ink = theme::paper::ink();
        text(4.0, "RESEARCH / 研究终端", 10.0, theme::paper::ink_muted());
        text(22.0, &meta.file_type, 14.0, accent());
        text(40.0, &meta.ident, 12.0, theme::paper::ink_muted());
        let title = export::research::cover_title(input, markdown)
            .unwrap_or_else(|| "【待核实：文件名称】".into());
        let title = export::title::cover_title_lines(&title).join("\n");
        let title_bottom = if math_flow::has_math(&title) {
            let font = metrics.font(theme::FONT_HEITI, 28.0);
            let style = math_flow::FlowStyle::block(
                (font.clone(), font),
                28.0,
                metrics.pt(40.0),
                metrics.content,
            );
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = metrics.content;
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
            let origin = rect.min + egui::vec2(0.0, metrics.mm(58.0));
            painter.galley(origin, galley.clone(), ink);
            math_flow::paint_slots(painter, metrics, origin, &galley, &slots);
            58.0 + galley.size().y / metrics.mm(1.0)
        } else {
            text(58.0, &title, 28.0, ink)
        };
        if !meta.original_title.is_empty() {
            text(title_bottom + 6.0, &meta.original_title, 12.0, ink);
        }
        let y = rect.top() + metrics.mm(118.0);
        painter.hline(rect.x_range(), y, Stroke::new(metrics.pt(1.0), accent()));
        text(
            128.0,
            "CONTENTS / 研究结构",
            10.0,
            theme::paper::ink_muted(),
        );
        let chapters = super::research::outline(markdown);
        for (i, entry) in chapters
            .iter()
            .filter(|entry| entry.level == 2)
            .take(5)
            .enumerate()
        {
            text(
                138.0 + i as f32 * 8.0,
                &format!("{}  {}", entry.number.as_deref().unwrap_or("—"), entry.text),
                12.0,
                ink,
            );
        }
        let family = mdx::cover::Family::of(&meta.file_type);
        let stage = if family.is_project() {
            format!(
                "立项论证 / 建设实施 / 技术实现 / 项目总结\n当前阶段：{}",
                meta.file_type
            )
        } else {
            meta.byline.clone()
        };
        text(192.0, &stage, 10.0, accent());
        text(214.0, &meta.institution, 16.0, ink);
        text(225.0, &mdx::cover::chinese_date(&meta.date), 12.0, ink);
        let security = mdx::cover::security_label(&meta.security);
        let security = if !security.is_empty() && !meta.security_years.is_empty() {
            format!("{security}★{}", meta.security_years)
        } else {
            security.into()
        };
        text(
            -10.0,
            &format!("{security}    {}    {}", meta.file_number, meta.version),
            9.0,
            theme::paper::ink_muted(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_details_keep_global_chapter_numbers_and_stop_before_appendices() {
        let markdown = "<!-- [部分] -->\n\n# 现状\n\n## 背景\n\n### 方法\n\n#### 样本\n\n# 对策\n\n## 实施\n\n<!-- [不编号] -->\n\n## 结束语\n\n<!-- [附录] -->\n\n## 数据表\n\n<!-- [版本变更记录] -->\n\n## 版本历史\n\n<!-- [参考文献] -->\n\n## 文献列表\n";
        let located = export::parse_markdown_located(markdown);
        let entries = structure(&located, markdown);
        let parts: Vec<_> = entries
            .iter()
            .filter(|e| e.kind == EntryKind::Part)
            .collect();
        assert_eq!(parts.len(), 2);
        let first = children(&entries, &parts[0].source);
        assert_eq!(
            first.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["背景"]
        );
        let second = children(&entries, &parts[1].source);
        assert_eq!(
            second
                .iter()
                .map(|e| (e.number.as_deref(), e.text.as_str()))
                .collect::<Vec<_>>(),
            [(Some("2"), "实施"), (None, "结束语")]
        );
        assert!(entries.iter().any(|e| e.number.as_deref() == Some("1.1")));
        assert!(!entries.iter().any(|e| e.text == "样本"));
        assert!(
            !entries
                .iter()
                .any(|e| e.text == "版本历史" || e.text == "文献列表")
        );
    }

    #[test]
    fn toc_marker_matches_export_variants_without_matching_body_text() {
        for marker in [
            "<!-- [目录] -->",
            "<!-- 【TOC】 -->",
            "<!-- contents -->",
            "<!-- tableofcontents -->",
        ] {
            assert!(is_toc(marker));
        }
        assert!(!is_toc("正文中的目录"));
        assert!(!is_toc("```\n<!-- [目录] -->\n```"));
    }
}
