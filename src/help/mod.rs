//! 应用内帮助：左侧目录 + 研究报告纸面，作为一个导航页。
//!
//! 手册本身就是一篇研究报告（正文见 [`content`]），纸面直接交给起草页同一个研究报告
//! 预览（[`preview::research_preview`]）去排，目录取同一份研究报告大纲
//! （[`preview::research_outline`]），图是正文里的 Mermaid 围栏。帮助这边不再有自己的
//! Markdown 渲染器：起草页能排的，帮助里就能用；帮助里看到的，就是用户写研究报告
//! 时会得到的样子。「生成 PDF」走研究报告的 Typst 导出，排好在 PDF 标签里打开，可打印。
//!
//! 入口有三处：菜单「使用帮助」、`F1`、设置页「上手指引」的按钮。是一格**标签**
//! （[`crate::app::NavPage::Help`]），不是浮窗：手册是要一页页读下去的长文，做成标签
//! 能跟稿件并排切换，还能随会话一起记住。

pub(crate) mod content;

use crate::models::NumberingConfig;
use crate::{preview, theme};
use eframe::egui;
use std::ops::Range;
use std::sync::OnceLock;

/// 左栏目录的固定宽度。够放下「第14章 AI 管理：模型、技能与润色预设」折成两行。
const TOC_WIDTH: f32 = 260.0;

/// 设置页「上手指引」跳到完整手册时停的那一章：章标题上挂的锚点。
pub(crate) const QUICKSTART_CHAPTER: &str = "chap:quickstart";

/// 帮助页的会话状态。
#[derive(Default)]
pub(crate) struct HelpState {
    /// 置位表示「请把帮助标签打开并切过去」。菜单项、`F1`、设置页的按钮都只置这个位，
    /// 真正开标签的是 app 层——`HelpState` 拿不到标签栏。
    pub(crate) request_open: bool,
    /// 置位表示「请在后台排一份 PDF」，由 app 层接手（它管后台线程与 PDF 标签）。
    pub(crate) request_pdf: bool,
    /// 后台正在排 PDF，按钮置灰。
    pub(crate) pdf_busy: bool,
    /// 下一帧要滚到的标题（源码字节范围）。只活一帧：留着的话纸面每帧都被拽回去。
    jump: Option<Range<usize>>,
    /// 目录里高亮的那一条：最近一次跳到的章。
    current: Option<Range<usize>>,
    /// 左栏搜索框里的关键词。
    query: String,
    /// 截图释放到图片目录的结果：`None` 是还没试过，`Some(Err)` 留着给抬头报错。
    images: Option<Result<(), String>>,
}

impl HelpState {
    /// 请求打开帮助标签并停在某一章；`anchor` 是章标题上的锚点，如 [`QUICKSTART_CHAPTER`]。
    pub(crate) fn open_chapter(&mut self, anchor: &str) {
        if let Some(entry) = entry_of_anchor(anchor) {
            self.jump = Some(entry.line.clone());
            self.current = Some(entry.line.clone());
        }
        self.request_open = true;
    }
}

/// 画帮助页。铺满调用方给的这块 `ui`，跟别的导航页一样占满标签内容区。
pub(crate) fn help_page(ui: &mut egui::Ui, state: &mut HelpState) {
    if state.images.is_none() {
        state.images = Some(content::install_images().map_err(|error| format!("{error:#}")));
    }
    let mut goto: Option<Range<usize>> = None;

    egui::Panel::left(egui::Id::new("help_toc_panel"))
        .resizable(false)
        .exact_size(TOC_WIDTH)
        .frame(
            egui::Frame::new()
                .fill(theme::surface_sunk())
                .inner_margin(egui::Margin::symmetric(10, 8)),
        )
        .show(ui, |ui| {
            toc_panel_ui(ui, state.current.as_ref(), &mut state.query, &mut goto)
        });
    egui::Panel::top(egui::Id::new("help_header_panel"))
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: 16,
            right: 14,
            top: 8,
            bottom: 8,
        }))
        .show(ui, |ui| header_ui(ui, state));
    // 取走：跳转只在这一帧生效，下一帧起纸面重新归用户的滚轮管。
    let jump = state.jump.take();
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(theme::surface_sunk()))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("help_paper")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let _ = preview::research_preview(
                        ui,
                        help_draft(),
                        content::markdown(),
                        preview::PreviewScale::zoom(None),
                        jump.as_ref(),
                        jump.is_some(),
                        &NumberingConfig::default(),
                        false,
                    );
                    ui.add_space(24.0);
                });
        });

    if let Some(line) = goto {
        state.current = Some(line.clone());
        state.jump = Some(line);
        ui.ctx().request_repaint();
    }
}

/// 抬头：书名、一句说明与「生成 PDF」。
fn header_ui(ui: &mut egui::Ui, state: &mut HelpState) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("公文助手使用帮助")
                .color(theme::text())
                .size(theme::font_sizes::HEADING)
                .strong(),
        );
        ui.label(
            egui::RichText::new(format!("V{} · F1 随时回到这里", env!("CARGO_PKG_VERSION")))
                .color(theme::text_muted())
                .size(theme::font_sizes::SMALL),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if state.pdf_busy {
                "正在排版…"
            } else {
                "生成 PDF"
            };
            if ui
                .add_enabled(
                    !state.pdf_busy,
                    theme::secondary_icon_button(theme::Icon::FileTypePdf, label),
                )
                .on_hover_text("按研究报告版式排成 PDF，在新标签里打开，可打印")
                .clicked()
            {
                state.request_pdf = true;
            }
        });
    });
    if let Some(Err(error)) = &state.images {
        ui.label(
            egui::RichText::new(format!(
                "界面截图没能放进图片目录，图会显示成占位框：{error}"
            ))
            .color(theme::warn())
            .size(theme::font_sizes::SMALL),
        );
    }
}

/// 左栏：搜索框 + 目录。目录列部分与章；当前章下面展开它的节，点了直接滚过去。
fn toc_panel_ui(
    ui: &mut egui::Ui,
    current: Option<&Range<usize>>,
    query: &mut String,
    goto: &mut Option<Range<usize>>,
) {
    let width = ui.available_width();
    ui.horizontal(|ui| {
        // 二十多章两百来个小节，翻目录不如直接搜。搜的是各节的标题与正文。
        ui.add(theme::field(query, "搜索帮助…", width - 36.0));
        if theme::icon_button_enabled(ui, !query.is_empty(), theme::Icon::X, "清空搜索").clicked()
        {
            query.clear();
        }
    });
    ui.add_space(6.0);

    let needle = query.trim().to_lowercase();
    let hits = (!needle.is_empty()).then(|| search(&needle));
    if let Some(hits) = &hits {
        ui.label(
            egui::RichText::new(if hits.is_empty() {
                "没有命中的章节".to_string()
            } else {
                format!("命中 {} 处", hits.len())
            })
            .color(theme::text_muted())
            .size(theme::font_sizes::SMALL),
        );
        ui.add_space(4.0);
    }

    let chapter = current.and_then(|line| chapter_of(line.start));
    egui::ScrollArea::vertical()
        .id_salt("help_toc")
        .auto_shrink([false, false])
        .show(ui, |ui| match &hits {
            Some(hits) => {
                for &index in hits {
                    entry_row(ui, &toc()[index], current, goto);
                }
            }
            None => {
                for (index, entry) in toc().iter().enumerate() {
                    let shown = match entry.kind {
                        EntryKind::Part | EntryKind::Chapter => true,
                        EntryKind::Section => chapter == chapter_of_index(index),
                    };
                    if shown {
                        entry_row(ui, entry, current, goto);
                    }
                }
                ui.add_space(12.0);
            }
        });
}

/// 目录里的一行。部分是分组标题，章整行可点、整行高亮，节缩一格、字小一号。
fn entry_row(
    ui: &mut egui::Ui,
    entry: &TocEntry,
    current: Option<&Range<usize>>,
    goto: &mut Option<Range<usize>>,
) {
    let selected = current == Some(&entry.line);
    let (text, size, indent, height) = match entry.kind {
        EntryKind::Part => {
            ui.add_space(8.0);
            (
                egui::RichText::new(&entry.label)
                    .color(theme::text_muted())
                    .strong(),
                theme::font_sizes::SMALL,
                0.0,
                24.0,
            )
        }
        EntryKind::Chapter => (
            egui::RichText::new(&entry.label).color(if selected {
                theme::accent()
            } else {
                theme::text_soft()
            }),
            theme::font_sizes::BODY,
            0.0,
            26.0,
        ),
        EntryKind::Section => (
            egui::RichText::new(&entry.label).color(if selected {
                theme::accent()
            } else {
                theme::text_muted()
            }),
            theme::font_sizes::SMALL,
            12.0,
            22.0,
        ),
    };
    // 纵向两端撑满、内容靠左：按钮整行可点，文字左对齐才好扫读（`add_sized` 会居中）。
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: indent as i8,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                let button = egui::Button::new(text.size(size))
                    .selected(selected && entry.kind == EntryKind::Chapter)
                    .frame_when_inactive(selected && entry.kind == EntryKind::Chapter)
                    .min_size(egui::vec2(0.0, height))
                    .wrap();
                if ui.add(button).clicked() {
                    *goto = Some(entry.line.clone());
                }
            });
        });
}

// ── 目录与搜索 ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EntryKind {
    Part,
    Chapter,
    Section,
}

/// 目录的一条：取自研究报告大纲，标签是纸面上的编号连同标题。
struct TocEntry {
    kind: EntryKind,
    label: String,
    /// 标题那一块的源码字节范围：跳转靠它。
    line: Range<usize>,
    /// 这一节的标题与正文（小写），供搜索。
    haystack: String,
}

/// 正文是编译期常量，目录只建一次。只收部分、章（含摘要、附录）与一级节，
/// 再往下的小节在纸面上看就够了。
fn toc() -> &'static [TocEntry] {
    static TOC: OnceLock<Vec<TocEntry>> = OnceLock::new();
    TOC.get_or_init(|| {
        let markdown = content::markdown();
        let outline = preview::research_outline(markdown);
        let entries: Vec<_> = outline
            .iter()
            .filter(|entry| (2..=3).contains(&entry.level))
            .collect();
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let number = entry.number.clone().unwrap_or_default();
                let kind = if entry.level == 3 {
                    EntryKind::Section
                } else if number.trim_end().ends_with("部分") {
                    EntryKind::Part
                } else {
                    EntryKind::Chapter
                };
                let end = entries
                    .get(index + 1)
                    .map_or(markdown.len(), |next| next.line.start);
                TocEntry {
                    kind,
                    label: format!("{number}{}", entry.text),
                    line: entry.line.clone(),
                    haystack: format!("{}\n{}", entry.text, &markdown[entry.line.start..end])
                        .to_lowercase(),
                }
            })
            .collect()
    })
}

/// 命中 `needle`（已小写）的章与节，按目录顺序。部分标题下面没有正文，不单列。
fn search(needle: &str) -> Vec<usize> {
    toc()
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.kind != EntryKind::Part && entry.haystack.contains(needle))
        .map(|(index, _)| index)
        .collect()
}

/// 第 `index` 条所属的章（目录下标）。
fn chapter_of_index(index: usize) -> Option<usize> {
    toc()[..=index]
        .iter()
        .rposition(|entry| entry.kind == EntryKind::Chapter)
}

/// 源码位置 `offset` 落在哪一章（目录下标）。
fn chapter_of(offset: usize) -> Option<usize> {
    let last = toc().iter().rposition(|entry| entry.line.start <= offset)?;
    chapter_of_index(last)
}

/// 标题上挂着 `{#anchor}` 的那一条。
fn entry_of_anchor(anchor: &str) -> Option<&'static TocEntry> {
    let markdown = content::markdown();
    let at = markdown.find(&format!("{{#{anchor}}}"))?;
    toc().iter().find(|entry| entry.line.contains(&at))
}

/// 帮助这篇研究报告的文档要素，建一次就不再动。
fn help_draft() -> &'static crate::models::DraftInput {
    static DRAFT: OnceLock<crate::models::DraftInput> = OnceLock::new();
    DRAFT.get_or_init(content::draft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn help_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/help")
    }

    /// 目录下的文件与 [`content::FILES`] 一一对应、顺序就是文件名升序——与
    /// 「从文件夹新建研究报告」的合并顺序一致，手册才能原样导入成稿件。
    #[test]
    fn 正文文件清单与目录一致() {
        let mut on_disk: Vec<String> = std::fs::read_dir(help_dir())
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".md"))
            .collect();
        on_disk.sort();
        let listed: Vec<&str> = content::FILES.iter().map(|(name, _)| *name).collect();
        assert_eq!(listed, on_disk);
    }

    #[test]
    fn 截图清单与目录一致且都被正文用上() {
        let on_disk: BTreeSet<String> = std::fs::read_dir(help_dir().join("images"))
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        let listed: BTreeSet<String> = content::IMAGES
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        assert_eq!(listed, on_disk);
        let markdown = content::markdown();
        for (name, bytes) in content::IMAGES {
            assert!(
                image::load_from_memory(bytes).is_ok(),
                "截图无法解码：{name}"
            );
            assert!(
                markdown.contains(&format!("](images/{name})")),
                "截图 {name} 打进了二进制却没有任何章节引用"
            );
        }
        let blocks =
            crate::export::parse_markdown_located_report(markdown, &NumberingConfig::default());
        for located in blocks {
            if let crate::export::MarkdownBlock::Image { src, .. } = located.block {
                let name = src.strip_prefix("images/").unwrap_or(&src);
                assert!(
                    content::IMAGES.iter().any(|(listed, _)| *listed == name),
                    "正文引用了没打包的图片 {src}"
                );
            }
        }
    }

    /// 正文里的 `{@id}` 都要有锚点：悬空的引用预览印 `??`，导出 PDF 直接中止。
    #[test]
    fn 交叉引用都有着落() {
        let markdown = content::markdown();
        let defined: BTreeSet<String> = preview::research_label_targets(markdown)
            .into_iter()
            .map(|target| target.id)
            .collect();
        for id in crate::export::crossref::crossref_ids(markdown) {
            assert!(defined.contains(id), "引用了不存在的锚点 {{@{id}}}");
        }
        assert!(defined.contains(QUICKSTART_CHAPTER));
    }

    /// 每张 Mermaid 图都要排得出来，且字号不被缩到下限以下。
    #[test]
    fn 示意图都能排出来() {
        let blocks = crate::export::parse_markdown_located_report(
            content::markdown(),
            &NumberingConfig::default(),
        );
        let mut count = 0;
        for located in blocks {
            if let crate::export::MarkdownBlock::Diagram { source, caption } = located.block {
                count += 1;
                let src = crate::mermaid::cache(
                    &source,
                    crate::mermaid::Style::Research,
                    crate::mermaid::Format::Png,
                )
                .unwrap_or_else(|error| panic!("图「{caption}」排不出来：{error:#}"));
                assert!(
                    crate::mermaid::cached_warning(&src).is_none(),
                    "图「{caption}」超出版心被缩小：{:?}",
                    crate::mermaid::cached_warning(&src)
                );
                assert!(!caption.is_empty(), "示意图都要有图题");
            }
        }
        assert!(count >= 10, "示意图应当都已改成 Mermaid，实际 {count} 张");
    }

    #[test]
    fn 目录列出部分章与附录() {
        let parts = toc().iter().filter(|e| e.kind == EntryKind::Part).count();
        let chapters = toc()
            .iter()
            .filter(|e| e.kind == EntryKind::Chapter)
            .count();
        assert_eq!(parts, 5, "五个部分");
        // 摘要 + 23 章 + 3 篇附录。
        assert_eq!(chapters, 27);
        assert!(toc().iter().any(|e| e.label.contains("附录A")));
        for (index, entry) in toc().iter().enumerate() {
            if entry.kind == EntryKind::Section {
                assert!(
                    chapter_of_index(index).is_some(),
                    "节「{}」前面没有章",
                    entry.label
                );
            }
        }
    }

    #[test]
    fn 搜索能按正文里的词命中章节() {
        assert!(!search("保密期限").is_empty(), "正文里的词应能搜到");
        assert!(search("这几个字正文里一定没有").is_empty());
    }

    #[test]
    fn 上手指引能跳到快速上手章() {
        let mut state = HelpState::default();
        state.open_chapter(QUICKSTART_CHAPTER);
        assert!(state.request_open);
        let line = state.jump.clone().expect("应当记下要跳去的标题");
        assert!(content::markdown()[line].contains("五分钟出第一份稿子"));
    }

    /// 离屏画两帧：纸面与目录都画得出来，跳转用完即清。
    #[test]
    fn 帮助页画得出来且跳转只活一帧() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        let mut state = HelpState {
            images: Some(Ok(())),
            ..HelpState::default()
        };
        state.open_chapter(QUICKSTART_CHAPTER);
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        crate::test_support::run_ui(&ctx, raw(), |ui| help_page(ui, &mut state));
        assert!(state.jump.is_none(), "跳转目标没清掉，纸面会被每帧拽回去");
        let output = crate::test_support::run_ui(&ctx, raw(), |ui| help_page(ui, &mut state));
        let drawn: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(drawn.iter().any(|text| text.contains("五分钟出第一份稿子")));
    }

    /// 帮助页离屏画成 `tmp/help-page.png`，改版后目视检查。默认忽略：
    ///
    /// ```text
    /// cargo test --locked help_page_snapshot -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn help_page_snapshot() {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        theme::configure_style(&ctx);
        theme::configure_icons(&ctx);
        let size = egui::vec2(1280.0, 820.0);
        // 第 1 章看目录与正文，第 8 章那一处看 Mermaid 图与代码块。
        for (name, anchor) in [("intro", "chap:intro"), ("code", "fig:demo-flow")] {
            let mut state = HelpState::default();
            let at = content::markdown()
                .find(&format!("{{#{anchor}}}"))
                .expect("锚点应当在正文里");
            let line = content::markdown()[..at].rfind("```text").unwrap_or(at);
            state.jump = Some(line..line + 1);
            let mut canvas = crate::ui_snapshot::Canvas::default();
            let raw = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            };
            // 头几帧在量面板、解码截图与 Mermaid 图，多跑几帧再出样张。
            for frame in 0..20 {
                if frame == 10 {
                    state.jump = Some(line..line + 1);
                }
                let mut output = ctx.run_ui(raw(), |ui| help_page(ui, &mut state));
                canvas.absorb(&mut output.textures_delta);
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            let output = ctx.run_ui(raw(), |ui| help_page(ui, &mut state));
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("tmp/help-page-{name}.png"));
            canvas.render(&ctx, output, size, theme::surface(), &path);
            println!("{}", path.display());
        }
    }

    /// 帮助手册整本排成 PDF，落到 `tmp/typst-samples/help/`，目视检查用。与「生成 PDF」
    /// 同一条路：截图与 Mermaid 图都在用户配置目录里取。依赖本机随包字体，默认忽略：
    ///
    /// ```text
    /// cargo test --locked help_pdf_sample -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn help_pdf_sample() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp/typst-samples/help");
        std::fs::create_dir_all(&dir).unwrap();
        content::install_images().unwrap();
        let path = dir.join("公文助手使用帮助.pdf");
        let outcome = crate::export::typst::research::write_pdf(
            &path,
            &content::draft(),
            content::markdown(),
            &NumberingConfig::default(),
        )
        .expect("帮助手册应当排得出 PDF");
        for warning in &outcome.warnings {
            println!("提示：{warning}");
        }
        println!("{}", path.display());
    }
}
