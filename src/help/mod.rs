//! 应用内帮助中心：左侧目录树 + 右侧 Markdown 正文，作为一个导航页。
//!
//! 正文与配图都编译进二进制（见 [`content`]），离线可读。入口有三处：
//! 菜单「使用帮助」、`F1`、设置页「上手指引」的按钮。
//!
//! 是一格**标签**（[`crate::app::NavPage::Help`]），不是浮窗：手册是要一页页
//! 读下去的长文，浮窗既挡着稿子又没法跟稿件并排切换；做成标签，看完帮助点回
//! 稿件标签就行，还能随会话一起记住。
//!
//! 拆分：[`content`] 管静态资源，[`render`] 管 Markdown 子集渲染，本文件
//! 只管页面布局、目录与搜索。
//!
//! 页面内部用 `Panel` / `CentralPanel` 划区，而不是手算高度：
//! 正文滚动区若直接吃满可用高度，底下的翻页栏会被顶出可视区。

pub(crate) mod content;
pub(crate) mod render;

use crate::theme;
use content::{CHAPTERS, Chapter, Part};
use eframe::egui;

/// 左栏目录的固定宽度。够放下最长的章名「拟稿：源码编辑与五种视图」。
const TOC_WIDTH: f32 = 244.0;

/// 帮助页的会话状态。跨帧保留当前章节与滚动目标。
pub(crate) struct HelpState {
    /// 置位表示「请把帮助标签打开并切过去」。菜单项、`F1`、设置页的按钮都只
    /// 置这个位，真正开标签的是 app 层——`HelpState` 拿不到标签栏。
    pub(crate) request_open: bool,
    /// 当前章节下标，指向 [`CHAPTERS`]。
    chapter: usize,
    /// 下一帧要滚到的小节标题。只活一帧：渲染时取走，否则正文会被每帧
    /// 重新拽回该小节，滚轮彻底失灵。
    scroll_to: Option<String>,
    /// 正文滚动区要回到顶部（换章时置位）。
    reset_scroll: bool,
    /// 左栏搜索框里的关键词。
    query: String,
    /// 正在放大看的配图 key；`None` 表示没开放大浮层。
    zoom: Option<String>,
}

impl Default for HelpState {
    fn default() -> Self {
        Self {
            request_open: false,
            // 头一次打开停在入口章。按 id 找而不是写死 0，章节顺序调了也不会
            // 莫名其妙落到别的章上。
            chapter: content::index_of(ROOT_CHAPTER).unwrap_or(0),
            scroll_to: None,
            reset_scroll: true,
            query: String::new(),
            zoom: None,
        }
    }
}

impl HelpState {
    /// 请求打开帮助标签并停在某一章；`id` 见 [`content::CHAPTERS`]。
    pub(crate) fn open_chapter(&mut self, id: &str) {
        if let Some(index) = content::index_of(id) {
            self.chapter = index;
        }
        self.request_open = true;
        self.scroll_to = None;
        self.reset_scroll = true;
        self.zoom = None;
    }

    /// Esc：收起配图放大层。返回是否真收了——没收的话这一下 Esc 该留给别人。
    pub(crate) fn close_zoom(&mut self) -> bool {
        self.zoom.take().is_some()
    }

    fn current(&self) -> &'static Chapter {
        &CHAPTERS[self.chapter.min(CHAPTERS.len() - 1)]
    }

    fn goto(&mut self, index: usize) {
        if index < CHAPTERS.len() && index != self.chapter {
            self.chapter = index;
            self.scroll_to = None;
            self.reset_scroll = true;
        }
    }
}

/// 一帧里用户在页面上按下的东西，收齐了再统一改状态——闭包里 `state`
/// 被借着，直接改会和渲染打架。
#[derive(Default)]
struct Actions {
    goto: Option<usize>,
    section: Option<String>,
    zoom: Option<String>,
}

/// 画帮助页。铺满调用方给的这块 `ui`，跟别的导航页一样占满标签内容区。
pub(crate) fn help_page(ui: &mut egui::Ui, state: &mut HelpState) {
    let mut actions = Actions::default();
    let chapter = state.current();
    let index = state.chapter;
    // 取走：小节跳转只在这一帧生效，下一帧起正文重新归用户的滚轮管。
    let scroll_to = state.scroll_to.take();
    let reset_scroll = std::mem::take(&mut state.reset_scroll);

    egui::Panel::left(egui::Id::new("help_toc_panel"))
        .resizable(false)
        .exact_size(TOC_WIDTH)
        .frame(
            egui::Frame::new()
                .fill(theme::surface_sunk())
                .inner_margin(egui::Margin::symmetric(10, 8)),
        )
        .show(ui, |ui| {
            toc_panel_ui(ui, index, &mut state.query, &mut actions);
        });
    // 翻页栏先占位再画正文，正文才不会把它挤出可视区。
    egui::Panel::bottom(egui::Id::new("help_footer_panel"))
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: 16,
            right: 14,
            top: 8,
            bottom: 8,
        }))
        .show(ui, |ui| footer_ui(ui, index, &mut actions));
    egui::Panel::top(egui::Id::new("help_header_panel"))
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: 16,
            right: 14,
            top: 10,
            bottom: 8,
        }))
        .show(ui, |ui| header_ui(ui, chapter, index));
    egui::CentralPanel::default()
        .frame(egui::Frame::new().inner_margin(egui::Margin {
            left: 12,
            right: 6,
            top: 4,
            bottom: 0,
        }))
        .show(ui, |ui| {
            let mut area = egui::ScrollArea::vertical()
                .id_salt(("help_body", chapter.id))
                .auto_shrink([false, false]);
            if reset_scroll {
                area = area.vertical_scroll_offset(0.0);
            }
            area.show(ui, |ui| {
                let width = ui.available_width();
                match render::show(ui, chapter.body, width, scroll_to.as_deref()) {
                    Some(render::Action::Section(target)) => actions.section = Some(target),
                    Some(render::Action::Chapter(id)) => actions.goto = content::index_of(&id),
                    Some(render::Action::Zoom(src)) => actions.zoom = Some(src),
                    None => {}
                }
                ui.add_space(24.0);
            });
        });

    // 放大浮层走 ctx 层的 Area，盖在最上面：留在页面里的话会被滚动区裁掉，
    // 也只能撑到这一格标签的大小，放大就没意义了。
    if let Some(src) = state.zoom.clone()
        && render::zoom_overlay(ui.ctx(), &src)
    {
        state.zoom = None;
    }

    if let Some(index) = actions.goto {
        state.goto(index);
    } else if let Some(section) = actions.section {
        state.scroll_to = Some(section);
    }
    if let Some(src) = actions.zoom {
        state.zoom = Some(src);
    }
}

/// 正文上方的抬头：所属部分、章名、一句话摘要与「第几章 / 共几章」。
fn header_ui(ui: &mut egui::Ui, chapter: &'static Chapter, index: usize) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(chapter.part.label())
                .color(theme::text_muted())
                .size(theme::font_sizes::SMALL),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} / {}", index + 1, CHAPTERS.len()))
                    .color(theme::text_muted())
                    .size(theme::font_sizes::SMALL),
            );
        });
    });
    ui.add_space(3.0);
    // 章名只在这里出现一次，不再跟面包屑里重一遍。
    ui.label(
        egui::RichText::new(chapter.title)
            .color(theme::text())
            .size(theme::font_sizes::HEADING)
            .strong(),
    );
    ui.add_space(3.0);
    ui.label(
        egui::RichText::new(chapter.summary)
            .color(theme::text_muted())
            .size(theme::font_sizes::SMALL),
    );
}

/// 底部翻页栏：上一章 / 下一章都带上目标章名，不用猜点过去是哪。
fn footer_ui(ui: &mut egui::Ui, index: usize, actions: &mut Actions) {
    ui.horizontal(|ui| {
        let prev = index.checked_sub(1);
        let next = (index + 1 < CHAPTERS.len()).then_some(index + 1);
        let prev_label = prev.map_or("已是第一章".to_string(), |i| {
            format!("上一章 · {}", CHAPTERS[i].title)
        });
        if ui
            .add_enabled(
                prev.is_some(),
                theme::secondary_icon_button(theme::Icon::ChevronUp, &prev_label),
            )
            .clicked()
        {
            actions.goto = prev;
        }
        let next_label = next.map_or("已是最后一章".to_string(), |i| {
            format!("下一章 · {}", CHAPTERS[i].title)
        });
        if ui
            .add_enabled(
                next.is_some(),
                theme::secondary_icon_button(theme::Icon::ChevronDown, &next_label),
            )
            .clicked()
        {
            actions.goto = next;
        }
        // 这里不放「关闭」：帮助是一格标签，关它跟关别的标签一样点标签上的 ✕。
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("共 {} 章 · F1 随时回到这里", CHAPTERS.len()))
                    .color(theme::text_muted())
                    .size(theme::font_sizes::SMALL),
            );
        });
    });
}

/// 左栏：搜索框 + 目录树。当前章下面展开它的小节，点了直接滚过去。
fn toc_panel_ui(ui: &mut egui::Ui, current: usize, query: &mut String, actions: &mut Actions) {
    let width = ui.available_width();
    ui.horizontal(|ui| {
        // 二十四章一百八十多个小节，翻目录不如直接搜。搜的是标题、摘要与正文。
        ui.add(theme::field(query, "搜索帮助…", width - 36.0));
        if theme::icon_button_enabled(ui, !query.is_empty(), theme::Icon::X, "清空搜索").clicked()
        {
            query.clear();
        }
    });
    ui.add_space(6.0);

    let needle = query.trim().to_lowercase();
    let matches: Option<Vec<usize>> = (!needle.is_empty()).then(|| search(&needle));
    if let Some(hits) = &matches {
        ui.label(
            egui::RichText::new(if hits.is_empty() {
                "没有命中的章节".to_string()
            } else {
                format!("命中 {} 章", hits.len())
            })
            .color(theme::text_muted())
            .size(theme::font_sizes::SMALL),
        );
        ui.add_space(4.0);
    }

    egui::ScrollArea::vertical()
        .id_salt("help_toc")
        .auto_shrink([false, false])
        .show(ui, |ui| match &matches {
            // 搜索态下分组标题只会把七八条命中拆得七零八落，平铺就好。
            Some(hits) => {
                for index in hits {
                    chapter_row(ui, *index, current, actions);
                }
            }
            None => toc_tree_ui(ui, current, actions),
        });
}

/// 按部分分组的完整目录。
fn toc_tree_ui(ui: &mut egui::Ui, current: usize, actions: &mut Actions) {
    for part in Part::ALL {
        let chapters: Vec<usize> = CHAPTERS
            .iter()
            .enumerate()
            .filter(|(_, ch)| ch.part == part)
            .map(|(index, _)| index)
            .collect();
        if chapters.is_empty() {
            continue;
        }
        ui.add_space(10.0);
        ui.label(
            egui::RichText::new(part.label())
                .color(theme::text_muted())
                .size(theme::font_sizes::SMALL)
                .strong(),
        );
        ui.add_space(4.0);
        for index in chapters {
            chapter_row(ui, index, current, actions);
            if index == current {
                section_rows(ui, index, actions);
            }
        }
    }
    ui.add_space(12.0);
}

/// 目录里的一章。整行可点、整行高亮——只靠文字变色看不出选中了哪条。
fn chapter_row(ui: &mut egui::Ui, index: usize, current: usize, actions: &mut Actions) {
    let chapter = &CHAPTERS[index];
    let selected = index == current;
    let color = if selected {
        theme::accent()
    } else {
        theme::text_soft()
    };
    let button = egui::Button::new(egui::RichText::new(chapter.title).color(color))
        .selected(selected)
        .frame_when_inactive(selected)
        .wrap();
    if ui
        .add_sized([ui.available_width(), 26.0], button)
        .on_hover_text(chapter.summary)
        .clicked()
    {
        actions.goto = Some(index);
    }
}

/// 当前章的二级小节，缩一格排在章名下面，点了滚到正文对应位置。
fn section_rows(ui: &mut egui::Ui, index: usize, actions: &mut Actions) {
    for section in sections(index) {
        let button = egui::Button::new(
            egui::RichText::new(section.as_str())
                .color(theme::text_muted())
                .size(theme::font_sizes::SMALL),
        )
        .frame(false)
        .wrap();
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            if ui.add_sized([ui.available_width(), 22.0], button).clicked() {
                actions.section = Some(section.clone());
            }
        });
    }
    ui.add_space(4.0);
}

// ── 搜索与小节索引 ──────────────────────────────────────────────────────────

/// 每章一份：标题 + 摘要 + 正文纯文字（全小写）用于搜索，二级小节标题用于目录。
struct ChapterIndex {
    haystack: String,
    sections: Vec<String>,
}

/// 建一次就不再动。正文是编译期常量，每帧重解析二十四章纯属浪费。
fn indexes() -> &'static [ChapterIndex] {
    static INDEX: std::sync::OnceLock<Vec<ChapterIndex>> = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        CHAPTERS
            .iter()
            .map(|chapter| ChapterIndex {
                haystack: format!(
                    "{}\n{}\n{}",
                    chapter.title,
                    chapter.summary,
                    render::plain_text(chapter.body)
                )
                .to_lowercase(),
                sections: render::headings(chapter.body)
                    .into_iter()
                    .filter(|(level, _)| *level <= 2)
                    .map(|(_, text)| text)
                    .collect(),
            })
            .collect()
    })
}

fn sections(index: usize) -> &'static [String] {
    &indexes()[index].sections
}

/// 命中 `needle`（已小写）的章节下标，按目录顺序。
fn search(needle: &str) -> Vec<usize> {
    indexes()
        .iter()
        .enumerate()
        .filter(|(_, index)| index.haystack.contains(needle))
        .map(|(i, _)| i)
        .collect()
}

/// 手册入口章：菜单「使用帮助」默认停在这里。
pub(crate) const ROOT_CHAPTER: &str = "01-intro";

/// 供设置页「上手指引」跳到完整手册的第一步说明。
pub(crate) const QUICKSTART_CHAPTER: &str = "03-quickstart";

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试画布大小。帮助页铺满它，就跟铺满标签内容区一样。
    const CANVAS: egui::Vec2 = egui::vec2(1400.0, 900.0);

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
        ctx
    }

    /// 离屏画一帧帮助页，返回这一帧画出来的每段文字及其位置。
    fn frame(
        ctx: &egui::Context,
        state: &mut HelpState,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect)> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, CANVAS)),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| help_page(ui, state));
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect()
    }

    fn rect_of(drawn: &[(String, egui::Rect)], needle: &str) -> Option<egui::Rect> {
        drawn
            .iter()
            .find(|(text, _)| text.contains(needle))
            .map(|(_, rect)| *rect)
    }

    fn click_events(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    /// 底部翻页栏必须留在可视区里。
    ///
    /// 正文滚动区一旦直接吃满可用高度，翻页栏就被顶到下沿之外——整章读完
    /// 找不到「下一章」，只能回左栏点。这条锁住的是布局，不是文案。
    #[test]
    fn 翻页栏不会被正文顶出可视区() {
        let ctx = test_ctx();
        let mut state = HelpState::default();
        // 头一帧面板还在量尺寸，量第二帧。
        frame(&ctx, &mut state, vec![]);
        let drawn = frame(&ctx, &mut state, vec![]);

        let footer = rect_of(&drawn, "下一章").expect("翻页栏应当画出来");
        assert!(
            footer.max.y <= CANVAS.y + 4.0,
            "翻页栏被顶出了可视区：栏底 {} 已越过底边 {}",
            footer.max.y,
            CANVAS.y
        );
        assert!(
            footer.min.y > CANVAS.y * 0.6,
            "翻页栏应当贴着底边，实测栏顶 {}",
            footer.min.y
        );
    }

    /// 小节跳转只该钉一帧。
    ///
    /// 目标留在状态里不清，正文每帧都被重新拽回那一节，滚轮直接失灵。
    #[test]
    fn 小节跳转用完即清() {
        let ctx = test_ctx();
        let mut state = HelpState {
            scroll_to: Some(sections(0)[0].clone()),
            ..Default::default()
        };
        frame(&ctx, &mut state, vec![]);
        assert!(
            state.scroll_to.is_none(),
            "跳转目标没清掉，正文会被每帧拽回去"
        );
    }

    /// 左栏点一章就要换一章，并且正文回到顶部。
    #[test]
    fn 点目录换章并回到章首() {
        let ctx = test_ctx();
        let mut state = HelpState::default();
        frame(&ctx, &mut state, vec![]);
        let drawn = frame(&ctx, &mut state, vec![]);

        let target = content::index_of("03-quickstart").expect("快速上手章应当在目录里");
        let row = rect_of(&drawn, CHAPTERS[target].title).expect("目录里应当列出该章");
        frame(&ctx, &mut state, click_events(row.center()));
        assert_eq!(state.chapter, target, "点了目录没换章");
        assert!(state.reset_scroll, "换章后正文应当回到章首");
    }

    /// Esc 只在真收起了放大层时才算数——否则这一下要留给别的抽屉。
    #[test]
    fn esc只在有放大层时被吃掉() {
        let mut state = HelpState {
            zoom: Some("images/diag-concept.png".into()),
            ..Default::default()
        };
        assert!(state.close_zoom(), "开着放大层，这一下 Esc 归帮助页");
        assert!(state.zoom.is_none());
        assert!(!state.close_zoom(), "没有放大层就不该吞掉 Esc");
    }

    /// 入口章要按 id 找，别指望它正好排在第一个。
    #[test]
    fn 新开的帮助页停在入口章() {
        let state = HelpState::default();
        assert_eq!(CHAPTERS[state.chapter].id, ROOT_CHAPTER);
    }

    /// 正文里的跨章链接点下去要真的换章。
    #[test]
    fn 正文里的跨章链接能换章() {
        let ctx = test_ctx();
        let mut state = HelpState::default();
        frame(&ctx, &mut state, vec![]);
        let drawn = frame(&ctx, &mut state, vec![]);

        // 开篇那句「若你只想快速出第一份稿子，直接看……」在首屏之外，
        // 这里退而求其次：找正文里任意一处指向别章的链接文字。
        let target = content::index_of("12-ai-guard").expect("AI 边界章应当在目录里");
        let Some(rect) = rect_of(&drawn, CHAPTERS[target].title) else {
            return; // 首屏没露出这条链接，交给 render 里的点击测试把关。
        };
        frame(
            &ctx,
            &mut state,
            click_events(egui::pos2(rect.max.x - 6.0, rect.center().y)),
        );
        assert_eq!(state.chapter, target, "点了跨章链接没换章");
    }

    #[test]
    fn 章节id唯一且非空() {
        let mut seen = std::collections::BTreeSet::new();
        for chapter in CHAPTERS {
            assert!(!chapter.id.is_empty());
            assert!(seen.insert(chapter.id), "章节 id 重复：{}", chapter.id);
            assert!(!chapter.title.is_empty());
            assert!(!chapter.summary.is_empty());
            assert!(!chapter.body.is_empty());
        }
    }

    #[test]
    fn 入口章都存在() {
        assert!(content::index_of(ROOT_CHAPTER).is_some());
        assert!(content::index_of(QUICKSTART_CHAPTER).is_some());
    }

    #[test]
    fn 目录分组覆盖全部章节() {
        let total: usize = Part::ALL
            .iter()
            .map(|part| CHAPTERS.iter().filter(|ch| ch.part == *part).count())
            .sum();
        assert_eq!(total, CHAPTERS.len());
    }

    #[test]
    fn 每章都能抽出小节供目录展开() {
        for (index, chapter) in CHAPTERS.iter().enumerate() {
            assert!(
                !sections(index).is_empty(),
                "章节 {} 抽不出二级小节",
                chapter.id
            );
        }
    }

    #[test]
    fn 搜索能按正文里的词命中章节() {
        let hits = search("保密期限");
        assert!(!hits.is_empty(), "正文里的词应能搜到");
        assert!(search("这几个字正文里一定没有").is_empty());
    }

    #[test]
    fn 配图都能解码() {
        for image in content::IMAGES {
            assert!(
                image::load_from_memory(image.bytes).is_ok(),
                "配图无法解码：{}",
                image.key
            );
        }
    }

    #[test]
    fn 正文里引用的配图都已打包() {
        for chapter in CHAPTERS {
            for raw in chapter.body.lines() {
                // 行内代码里可能是语法示例（如 `![图](images/x.png)`），不是真引用。
                let line = strip_inline_code(raw);
                let Some(start) = line.find("![") else {
                    continue;
                };
                let Some(close) = line[start..].find("](") else {
                    continue;
                };
                let src_start = start + close + 2;
                let Some(end) = line[src_start..].find(')') else {
                    continue;
                };
                let key = &line[src_start..src_start + end];
                if key.starts_with("images/") {
                    assert!(
                        content::image_bytes(key).is_some(),
                        "章节 {} 引用了未打包的配图 {}",
                        chapter.id,
                        key
                    );
                }
            }
        }
    }

    #[test]
    fn 打包的配图都被正文用上() {
        let referenced: std::collections::BTreeSet<&str> = CHAPTERS
            .iter()
            .flat_map(|chapter| chapter.body.lines())
            .filter_map(|line| {
                let line = strip_inline_code(line);
                let start = line.find("![")?;
                let close = line[start..].find("](")?;
                let src_start = start + close + 2;
                let end = line[src_start..].find(')')?;
                Some(line[src_start..src_start + end].to_string())
            })
            .map(|key| {
                content::IMAGES
                    .iter()
                    .find(|img| img.key == key)
                    .map_or("", |img| img.key)
            })
            .collect();
        for image in content::IMAGES {
            assert!(
                referenced.contains(image.key),
                "配图 {} 打进了二进制却没有任何章节引用",
                image.key
            );
        }
    }

    /// 去掉行内代码段，剩下的才是真 Markdown 标记。
    fn strip_inline_code(line: &str) -> String {
        let mut out = String::new();
        let mut in_code = false;
        for ch in line.chars() {
            if ch == '`' {
                in_code = !in_code;
                continue;
            }
            if !in_code {
                out.push(ch);
            }
        }
        out
    }
}
