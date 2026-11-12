//! 维基阅读版式。复用 Markdown 切块与字形，页面层级、引用和滚动由维基管理。

use super::{LinkTarget, link_display_text};
use crate::export::{self, ColumnAlign, LineAlign, MarkdownBlock, QuoteLineKind};
use crate::models::{DiagramTheme, NumberingConfig};
use crate::preview::memo;
use crate::theme;
use eframe::egui::{self, FontFamily, FontId, TextFormat, text::LayoutJob};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, OnceLock},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BodyTarget {
    Page(String),
    Source(i64),
    Missing,
}

pub(super) fn page_title(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(28.0)
            .family(FontFamily::Name(theme::FONT_BIAOSONG.into()))
            .color(theme::text()),
    );
}

pub(super) fn scroll_pane<R>(
    ui: &mut egui::Ui,
    id: impl egui::AsIdSalt,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let width = ui.available_width().max(1.0);
    let height = ui.available_height().max(1.0);
    ui.scope(|ui| {
        let clip = ui.clip_rect();
        ui.set_clip_rect(clip.intersect(egui::Rect::from_x_y_ranges(
            ui.max_rect().x_range(),
            clip.y_range(),
        )));
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_width(width)
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, content)
            .inner
    })
    .inner
}

pub(super) fn source_pane(
    ui: &mut egui::Ui,
    id: impl egui::AsIdSalt,
    markdown: &mut dyn egui::TextBuffer,
) {
    ui.push_id(id, |ui| {
        scroll_pane(ui, "scroll", |ui| {
            let font = egui::TextStyle::Monospace.resolve(ui.style());
            let output = egui::TextEdit::multiline(markdown)
                .id_salt("markdown")
                .font(font.clone())
                .margin(egui::Margin {
                    left: 44,
                    right: 8,
                    top: 8,
                    bottom: 8,
                })
                .desired_width(f32::INFINITY)
                .show(ui);
            crate::draft_page::paint_editor_line_numbers(ui, &output, font.size - 2.0, font.family);
        });
    });
}

struct Link {
    raw: crate::wiki::RawLink,
    source: Range<usize>,
}

struct ParsedBody {
    prefix: String,
    markdown: String,
    links: Vec<Link>,
    blocks: Vec<export::LocatedBlock>,
    diagrams: OnceLock<Arc<HashMap<String, DiagramSlot>>>,
}

impl ParsedBody {
    fn new(body: &str) -> Self {
        // 先保护内链，再切表格和标题。别名分隔符 | 和标题里的 Markdown
        // 符号都留给维基处理，且解析结果与目标页当前名称无关。
        let mut prefix = String::from("WIKILINKQ");
        while body.contains(&prefix) {
            prefix.push('Q');
        }
        let mut markdown = String::with_capacity(body.len());
        let mut links = Vec::new();
        let mut end = 0;
        for (source, raw) in crate::wiki::wikilink_spans(body) {
            markdown.push_str(&body[end..source.start]);
            markdown.push_str(&format!("{prefix}{}Z", links.len()));
            end = source.end;
            links.push(Link { raw, source });
        }
        markdown.push_str(&body[end..]);
        let blocks = export::parse_markdown_located_report(&markdown, &NumberingConfig::default());
        Self {
            prefix,
            markdown,
            links,
            blocks,
            diagrams: OnceLock::new(),
        }
    }

    fn parts<'a>(&self, text: &'a str) -> impl Iterator<Item = (&'a str, Option<&Link>)> {
        let mut parts = text.split(&self.prefix);
        let first = parts.next().unwrap_or_default();
        std::iter::once((first, None)).chain(parts.map(|part| {
            if let Some((index, rest)) = part.split_once('Z')
                && let Ok(index) = index.parse::<usize>()
                && let Some(link) = self.links.get(index)
            {
                return (rest, Some(link));
            }
            (part, None)
        }))
    }
}

type DiagramSlot = OnceLock<Result<Diagram, String>>;

struct Diagram {
    texture: egui::TextureHandle,
    warning: Option<String>,
}

fn start_diagrams(
    ui: &egui::Ui,
    blocks: &[export::LocatedBlock],
    theme: DiagramTheme,
) -> Arc<HashMap<String, DiagramSlot>> {
    let diagrams = Arc::new(
        blocks
            .iter()
            .filter_map(|block| match &block.block {
                MarkdownBlock::Diagram { source, .. } => Some((source.clone(), DiagramSlot::new())),
                _ => None,
            })
            .collect::<HashMap<_, _>>(),
    );
    let sources = diagrams.keys().cloned().collect::<Vec<_>>();
    let pending = Arc::downgrade(&diagrams);
    let ctx = ui.ctx().clone();
    std::thread::spawn(move || {
        for source in sources {
            if pending.strong_count() == 0 {
                break;
            }
            let result = (|| -> anyhow::Result<_> {
                let (bytes, warning) = crate::mermaid::wiki_preview(&source, theme)?;
                let rgba = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?
                    .into_rgba8();
                let pixels = egui::ColorImage::from_rgba_unmultiplied(
                    [rgba.width() as usize, rgba.height() as usize],
                    rgba.as_raw(),
                );
                Ok((pixels, warning))
            })();
            let Some(diagrams) = pending.upgrade() else {
                break;
            };
            let result = result
                .map(|(pixels, warning)| Diagram {
                    texture: ctx.load_texture("wiki-diagram", pixels, egui::TextureOptions::LINEAR),
                    warning,
                })
                .map_err(|error| error.to_string());
            let _ = diagrams[&source].set(result);
            ctx.request_repaint();
        }
    });
    diagrams
}

struct Document<'a> {
    source: &'a str,
    parsed: &'a ParsedBody,
    targets: &'a HashMap<String, LinkTarget>,
    refs: &'a [(i64, String)],
    clicked: Option<BodyTarget>,
    diagram_theme: DiagramTheme,
}

#[derive(Clone, Copy)]
enum Style {
    Paragraph,
    Heading(u8),
    Caption,
    Quote,
    Table(bool),
    Code,
}

impl Style {
    fn format(self, bold: bool) -> TextFormat {
        let (size, leading, family, color) = match self {
            Self::Paragraph => (17.0, 27.0, theme::FONT_SONGTI, theme::text()),
            Self::Heading(1 | 2) => (22.0, 30.0, theme::FONT_HEITI, theme::text()),
            Self::Heading(_) => (18.0, 26.0, theme::FONT_HEITI, theme::text()),
            Self::Caption => (13.0, 21.0, theme::FONT_HEITI, theme::text_soft()),
            Self::Quote => (16.0, 25.0, theme::FONT_KAITI, theme::text()),
            Self::Table(true) => (14.0, 21.0, theme::FONT_HEITI, theme::text()),
            Self::Table(false) => (14.0, 21.0, theme::FONT_SONGTI, theme::text()),
            Self::Code => (14.0, 21.0, theme::EDITOR_FONT_FAMILY, theme::text_soft()),
        };
        TextFormat {
            font_id: FontId::new(
                size,
                FontFamily::Name(if bold { theme::FONT_HEITI } else { family }.into()),
            ),
            line_height: Some(leading),
            color,
            ..Default::default()
        }
    }
}

struct Hit {
    chars: Range<usize>,
    target: BodyTarget,
    hint: String,
}

pub(super) fn show_page_body(
    ui: &mut egui::Ui,
    body: &str,
    targets: &HashMap<String, LinkTarget>,
    refs: &[(i64, String)],
) -> Option<BodyTarget> {
    let diagram_theme = crate::mermaid::current_theme();
    let parsed = memo::memo(
        ui.ctx(),
        "wiki-body",
        memo::key((body, diagram_theme as u8)),
        || ParsedBody::new(body),
    );
    let mut doc = Document {
        source: body,
        parsed: &parsed,
        targets,
        refs,
        clicked: None,
        diagram_theme,
    };
    let width = ui.available_width();
    let inset = ((width - 720.0) / 2.0)
        .clamp(0.0, 127.0)
        .max((width * 0.05).min(24.0));
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(inset.min(127.0) as i8, 14))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            for (index, block) in parsed.blocks.iter().enumerate() {
                ui.push_id(index, |ui| doc.block(ui, block));
            }
            ui.add_space(12.0);
        });
    doc.clicked
}

impl Document<'_> {
    fn block(&mut self, ui: &mut egui::Ui, located: &export::LocatedBlock) {
        match &located.block {
            MarkdownBlock::Title(text) | MarkdownBlock::Heading(_, text) => {
                let raw = &self.parsed.markdown[located.range.clone()];
                let (level, heading) = export::parse_heading(raw.trim()).unwrap_or((2, text));
                ui.add_space(if level <= 2 { 9.0 } else { 3.0 });
                self.line(ui, heading, Style::Heading(level), "", egui::Align::LEFT);
                if level <= 2 {
                    ui.separator();
                }
            }
            MarkdownBlock::Paragraph(text) => {
                self.line(ui, text, Style::Paragraph, "　　", egui::Align::LEFT);
            }
            MarkdownBlock::OrderedListItem { number, text } => {
                self.line(
                    ui,
                    text,
                    Style::Paragraph,
                    &format!("{number}.  "),
                    egui::Align::LEFT,
                );
            }
            MarkdownBlock::Aligned { align, text } => {
                let align = match align {
                    LineAlign::Center => egui::Align::Center,
                    LineAlign::Right => egui::Align::RIGHT,
                };
                self.line(ui, text, Style::Paragraph, "", align);
            }
            MarkdownBlock::Quote { boxed, lines } => {
                let bar = theme::border_strong();
                let response = theme::panel(theme::surface_sunk(), 12)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 3.0;
                        if let Some(boxed) = boxed {
                            self.line(
                                ui,
                                &boxed.heading(),
                                Style::Heading(3),
                                "",
                                egui::Align::LEFT,
                            );
                        } else {
                            ui.weak("引文");
                        }
                        for (index, line) in lines.iter().enumerate() {
                            ui.push_id(index, |ui| {
                                let (style, prefix, align) = match line.kind {
                                    QuoteLineKind::Paragraph => {
                                        (Style::Quote, "", egui::Align::LEFT)
                                    }
                                    QuoteLineKind::ListItem => {
                                        (Style::Quote, "•  ", egui::Align::LEFT)
                                    }
                                    QuoteLineKind::Source => {
                                        (Style::Caption, "", egui::Align::RIGHT)
                                    }
                                };
                                self.line(ui, &line.text, style, prefix, align);
                            });
                        }
                    })
                    .response;
                ui.painter().vline(
                    response.rect.left() + 1.0,
                    response.rect.y_range().shrink(4.0),
                    egui::Stroke::new(2.0, bar),
                );
            }
            MarkdownBlock::Table {
                rows,
                header_rows,
                aligns,
                ..
            } => {
                let count = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let width = ui.available_width().max(72.0 * count as f32);
                        ui.set_width(width);
                        for (row, cells) in rows.iter().enumerate() {
                            let header = row < *header_rows;
                            ui.push_id(row, |ui| {
                                egui::Frame::new()
                                    .fill(if header || row % 2 == 0 {
                                        theme::surface_sunk()
                                    } else {
                                        theme::surface()
                                    })
                                    .inner_margin(egui::Margin::symmetric(8, 4))
                                    .show(ui, |ui| {
                                        ui.columns(count, |columns| {
                                            for (index, (column, cell)) in
                                                columns.iter_mut().zip(cells).enumerate()
                                            {
                                                let align = match aligns.get(index) {
                                                    Some(ColumnAlign::Center) => {
                                                        egui::Align::Center
                                                    }
                                                    Some(ColumnAlign::Right) => egui::Align::RIGHT,
                                                    _ => egui::Align::LEFT,
                                                };
                                                self.line(
                                                    column,
                                                    cell,
                                                    Style::Table(header),
                                                    "",
                                                    align,
                                                );
                                            }
                                        });
                                    });
                            });
                        }
                    });
            }
            MarkdownBlock::Code { .. } => {
                let raw = &self.parsed.markdown[located.range.clone()];
                let mut literal = String::new();
                for (text, link) in self.parsed.parts(raw) {
                    if let Some(link) = link {
                        literal.push_str(&self.source[link.source.clone()]);
                    }
                    literal.push_str(text);
                }
                theme::panel(theme::surface_sunk(), 10).show(ui, |ui| {
                    self.line(ui, &literal, Style::Code, "", egui::Align::LEFT);
                });
            }
            MarkdownBlock::Diagram { source, caption } => {
                let diagrams = self
                    .parsed
                    .diagrams
                    .get_or_init(|| start_diagrams(ui, &self.parsed.blocks, self.diagram_theme));
                match diagrams.get(source).and_then(OnceLock::get) {
                    Some(Ok(diagram)) => {
                        let width = ui.available_width();
                        ui.vertical_centered(|ui| {
                            ui.add(
                                egui::Image::from_texture(&diagram.texture)
                                    .fit_to_original_size(1.0)
                                    .max_width(width),
                            );
                        });
                        if let Some(warning) = &diagram.warning {
                            ui.colored_label(theme::warn(), warning);
                        }
                    }
                    Some(Err(error)) => {
                        ui.colored_label(theme::warn(), error);
                    }
                    None => {
                        ui.weak("正在排版流程图…");
                    }
                }
                if !caption.is_empty() {
                    self.line(ui, caption, Style::Caption, "", egui::Align::Center);
                }
            }
            MarkdownBlock::Image { alt, src } => {
                crate::preview::wiki_figure(ui, alt, src);
                if !alt.is_empty() {
                    self.line(ui, alt, Style::Caption, "", egui::Align::Center);
                }
            }
            MarkdownBlock::Html(_) | MarkdownBlock::Marker(_) => {}
        }
    }

    fn line(
        &mut self,
        ui: &mut egui::Ui,
        text: &str,
        style: Style,
        prefix: &str,
        align: egui::Align,
    ) {
        let width = ui.available_width().max(1.0);
        let mut job = LayoutJob {
            halign: align,
            wrap: egui::text::TextWrapping {
                max_width: width,
                ..Default::default()
            },
            ..Default::default()
        };
        job.append(prefix, 0.0, style.format(false));
        let mut hits = Vec::new();
        if matches!(style, Style::Code) {
            job.append(text, 0.0, style.format(false));
        } else {
            // 先切行内加粗，再取内链。这样加粗跨过内链时仍是完整的一段。
            for segment in export::inline_segments(text) {
                for (text, link) in self.parsed.parts(&segment.text) {
                    let format = style.format(segment.bold);
                    if let Some(link) = link {
                        let label = link_display_text(&link.raw, self.targets);
                        let target = match self.targets.get(&link.raw.target) {
                            Some(LinkTarget::Page { uuid, .. })
                            | Some(LinkTarget::Node {
                                guide: Some(uuid), ..
                            }) => BodyTarget::Page(uuid.clone()),
                            _ => BodyTarget::Missing,
                        };
                        let (color, hint) = if target == BodyTarget::Missing {
                            (theme::warn(), format!("待建页 · {}", link.raw.target))
                        } else {
                            (theme::accent(), format!("打开 · {label}"))
                        };
                        append_hit(
                            &mut job,
                            &mut hits,
                            label,
                            format.clone(),
                            color,
                            target,
                            hint,
                        );
                    }
                    self.append_citations(&mut job, &mut hits, text, &format);
                }
            }
        }
        let galley = ui.painter().layout_job(job);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, galley.size().y), egui::Sense::hover());
        let origin = egui::pos2(
            align.align_size_within_range(0.0, rect.x_range()).min,
            rect.top(),
        );
        if !ui.is_rect_visible(rect) {
            return;
        }
        ui.painter().galley(origin, galley.clone(), theme::text());
        let mut row_start = 0;
        for (index, row) in galley.rows.iter().enumerate() {
            let row_end = row_start + row.glyphs.len();
            for hit in &hits {
                let start = row_start.max(hit.chars.start);
                let end = row_end.min(hit.chars.end);
                if start >= end {
                    continue;
                }
                let left = row.x_offset(egui::text::CharIndex(start - row_start));
                let right = row.x_offset(egui::text::CharIndex(end - row_start));
                let hit_rect = egui::Rect::from_min_max(
                    origin + row.pos.to_vec2() + egui::vec2(left, 0.0),
                    origin + row.pos.to_vec2() + egui::vec2(right, row.size.y),
                );
                if ui
                    .interact(
                        hit_rect,
                        response.id.with((index, hit.chars.start)),
                        egui::Sense::click(),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(&hit.hint)
                    .clicked()
                {
                    self.clicked = Some(hit.target.clone());
                }
            }
            row_start = row_end + usize::from(row.ends_with_newline);
        }
    }

    fn append_citations(
        &self,
        job: &mut LayoutJob,
        hits: &mut Vec<Hit>,
        text: &str,
        format: &TextFormat,
    ) {
        let mut end = 0;
        for (start, _) in text.match_indices("〔E") {
            let after = start + "〔E".len();
            let Some((number, _)) = text[after..].split_once('〕') else {
                break;
            };
            let Ok(id) = number.parse::<i64>() else {
                continue;
            };
            let Some((_, note)) = self.refs.iter().find(|(source, _)| *source == id) else {
                continue;
            };
            job.append(&text[end..start], 0.0, format.clone());
            let badge = TextFormat {
                font_id: FontId::new(12.0, FontFamily::Proportional),
                background: theme::surface_active(),
                ..format.clone()
            };
            append_hit(
                job,
                hits,
                &format!(" E{id} "),
                badge,
                theme::accent(),
                BodyTarget::Source(id),
                format!("依据 E{id} · {note}"),
            );
            end = after + number.len() + '〕'.len_utf8();
        }
        job.append(&text[end..], 0.0, format.clone());
    }
}

fn append_hit(
    job: &mut LayoutJob,
    hits: &mut Vec<Hit>,
    label: &str,
    mut format: TextFormat,
    color: egui::Color32,
    target: BodyTarget,
    hint: String,
) {
    let start = job.text.chars().count();
    format.color = color;
    format.underline = egui::Stroke::new(1.0, color);
    job.append(label, 0.0, format);
    hits.push(Hit {
        chars: start..start + label.chars().count(),
        target,
        hint,
    });
}
