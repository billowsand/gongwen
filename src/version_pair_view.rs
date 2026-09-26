//! 只读的历史版本对照视图：统一正文 diff、要素变化与花脸稿预览。
//!
//! 组件只接收两份快照和渲染配置，不依赖起草页或稿件库。历史版本不可变，按
//!「稿件 id + 旧版本号 + 新版本号」缓存一次 diff、花脸稿与联动表。

use crate::diff::{self, ContentSnapshot, ManuscriptDiff};
use crate::diff_view::{self, DiffViewState};
use crate::preview;
use crate::redline::{self, RedlineDoc};
use crate::units::UnitDisplay;
use crate::version_link::ChangeLinks;
use eframe::egui;
use std::ops::Range;

/// 版本对的新侧。历史版本用版本号；工作区内容用「正文 + 文档要素 + 备注」的
/// 内容哈希——内容一变（改字、换要素、改备注）哈希就变，缓存自然失效，
/// 不会与历史版本撞键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PairSide {
    /// 某个已提交的历史版本。
    Version(i64),
    /// 当前工作区内容，带内容哈希。
    Working(u64),
}

/// 历史快照的稳定身份。旧版本号为空表示 v1 左侧为空白稿。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct VersionPairKey {
    pub(crate) manuscript_id: i64,
    /// 旧侧版本号；None 表示 v1 左侧为空白稿。旧侧永远是已提交版本
    /// （工作区只可能出现在新侧）。
    pub(crate) old_version_number: Option<i64>,
    pub(crate) new_side: PairSide,
}

struct PairCache {
    key: VersionPairKey,
    report: ManuscriptDiff,
    redline: RedlineDoc,
    links: ChangeLinks,
    new: ContentSnapshot,
}

/// 一对历史版本的只读绘制状态与缓存。
#[derive(Default)]
pub(crate) struct VersionPairViewState {
    cache: Option<PairCache>,
    view: DiffViewState,
    preview_target: Option<Range<usize>>,
    preview_hover: Option<usize>,
    preview_scroll: bool,
    /// 拖动分隔条时冻结右栏版面的缩放状态（见 `preview::freeze`）。
    preview_freeze: preview::ScaleFreeze,
}

impl VersionPairViewState {
    /// 当前缓存是否已对应这对版本。
    pub(crate) fn matches(&self, key: VersionPairKey) -> bool {
        self.cache.as_ref().is_some_and(|cache| cache.key == key)
    }

    /// 计算或复用一对版本的所有对照结果。
    pub(crate) fn set_pair(
        &mut self,
        key: VersionPairKey,
        old: &ContentSnapshot,
        new: &ContentSnapshot,
        display: &UnitDisplay,
    ) {
        if self.matches(key) {
            return;
        }
        let report = diff::manuscript_diff(old, new);
        let redline = redline::build_with_inputs(
            &old.content_markdown,
            &new.content_markdown,
            &old.snapshot,
            &new.snapshot,
            display,
        );
        let links = ChangeLinks::new(&report.body, &old.content_markdown, &redline.spans);
        self.cache = Some(PairCache {
            key,
            report,
            redline,
            links,
            new: new.clone(),
        });
        self.view.reset();
        self.preview_target = None;
        self.preview_hover = None;
        self.preview_scroll = false;
    }

    pub(crate) fn redline(&self) -> Option<&RedlineDoc> {
        self.cache.as_ref().map(|cache| &cache.redline)
    }

    /// 新版快照：导出这对版本的花脸稿时，版式要素取它的 `DraftInput`。
    pub(crate) fn new_snapshot(&self) -> Option<&ContentSnapshot> {
        self.cache.as_ref().map(|cache| &cache.new)
    }

    pub(crate) fn step(&mut self, forward: bool) {
        let total = self
            .cache
            .as_ref()
            .map_or(0, |cache| cache.report.body.changed_count);
        self.view.step(forward, total);
        self.preview_scroll = total > 0;
        self.preview_target = None;
    }

    /// 画出左侧只读统一 diff 与右侧花脸稿。返回双击正文时要求外层定位的新版源码范围。
    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        panel_id: egui::Id,
        old_label: &str,
        new_label: &str,
        display: &UnitDisplay,
        numbering: &crate::models::NumberingConfig,
    ) -> Option<Range<usize>> {
        let total = self
            .cache
            .as_ref()
            .map_or(0, |cache| cache.report.body.changed_count);
        // 先判断 Shift+F7。egui 的 consume_key 会让无修饰键分支也匹配带 Shift 的按键。
        let previous =
            ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::F7));
        let next = !previous
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F7));
        if previous {
            self.step(false);
        } else if next {
            self.step(true);
        }
        let Some(cache) = self.cache.as_ref() else {
            ui.weak("请选择两个历史版本进行对照。");
            return None;
        };
        let new = &cache.new;
        let cache_key = cache.key;
        let report = &cache.report;
        let redline = &cache.redline;
        let links = &cache.links;
        let view = &mut self.view;
        let preview_target = &mut self.preview_target;
        let preview_hover = &mut self.preview_hover;
        let preview_scroll = &mut self.preview_scroll;
        let preview_freeze = &mut self.preview_freeze;
        let focus = (total > 0).then(|| view.focus().min(total - 1));
        let hover_from_preview = *preview_hover;

        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("{old_label} → {new_label}"));
            crate::theme::chip(
                ui,
                &format!("共 {} 处变更", report.total()),
                crate::theme::accent(),
                crate::theme::accent_soft(),
            );
            if total > 0 {
                ui.label(format!(
                    "正文第 {} / {total} 处",
                    view.focus().min(total - 1) + 1
                ));
                if crate::theme::icon_button(ui, crate::theme::Icon::ArrowUp, "上一处（Shift+F7）")
                    .clicked()
                {
                    view.step(false, total);
                    *preview_scroll = true;
                    *preview_target = None;
                }
                if crate::theme::icon_button(ui, crate::theme::Icon::ArrowDown, "下一处（F7）")
                    .clicked()
                {
                    view.step(true, total);
                    *preview_scroll = true;
                    *preview_target = None;
                }
            }
            ui.checkbox(&mut view.only_changes, "折叠未改动")
                .on_hover_text("关掉后左栏未改动的段落也全量显示；右栏预览始终是全文");
        });
        ui.separator();

        let scroll = std::mem::take(preview_scroll);
        let mut left_click = None;
        let mut left_context = None;
        let mut edit_source = None;

        let mut left_hover = None;
        egui::Panel::left(panel_id)
            .default_size(ui.available_width() * 0.45)
            .size_range(280.0..=1400.0)
            .frame(egui::Frame::new().inner_margin(egui::Margin {
                right: 8,
                ..egui::Margin::ZERO
            }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("version_pair_left", cache_key))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if report.is_empty() {
                            ui.weak(format!("{old_label} 与 {new_label} 一致，没有差异。"));
                        }
                        if !report.fields.is_empty() {
                            egui::CollapsingHeader::new(format!(
                                "文档要素变化（{} 项）",
                                report.fields.len()
                            ))
                            .default_open(true)
                            .show(ui, |ui| {
                                diff_view::field_changes_table(
                                    ui,
                                    &report.fields,
                                    old_label,
                                    new_label,
                                );
                            });
                        }
                        if let Some(notes) = &report.notes {
                            egui::CollapsingHeader::new("备注变化")
                                .default_open(false)
                                .show(ui, |ui| {
                                    diff_view::field_changes_table(
                                        ui,
                                        std::slice::from_ref(notes),
                                        old_label,
                                        new_label,
                                    );
                                });
                        }
                        let output =
                            diff_view::unified_body_ui(ui, &report.body, view, hover_from_preview);
                        left_hover = output.hovered_change;
                        left_click = output.clicked_change;
                        left_context = output.clicked_context;
                        edit_source = output
                            .edit_source
                            .or_else(|| links.new_source(output.edit_change?));
                    });
            });

        let anchor = left_hover
            .and_then(|change| links.marked_range(change))
            .or_else(|| preview_target.clone())
            .or_else(|| focus.and_then(|change| links.marked_range(change)));
        let (clicked_preview, hovered_preview) = egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                egui::ScrollArea::both()
                    .id_salt(("version_pair_preview", cache_key))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // 拖分隔条时沿用落定的版面，只做层变换（见 `preview::freeze`）。
                        let output = preview::show_frozen(ui, preview_freeze, None, |ui, scale| {
                            preview::official_preview(
                                ui,
                                &new.snapshot,
                                display,
                                &redline.markdown,
                                scale,
                                anchor.as_ref(),
                                scroll,
                                numbering,
                                false,
                                &redline.elements,
                            )
                        })
                        .inner;
                        let hovered = preview::hovered_source(ui.ctx());
                        ui.add_space(12.0);
                        (output.clicked, hovered)
                    })
                    .inner
            })
            .inner;

        let hover = hovered_preview
            .as_ref()
            .and_then(|range| links.change_at(range));
        if hover != *preview_hover {
            *preview_hover = hover;
            ui.ctx().request_repaint();
        }
        if let Some(change) = left_click {
            view.set_focus(change, false);
            *preview_scroll = true;
            *preview_target = None;
        }
        if let Some(source) = &left_context {
            *preview_target = links.marked_for_new_source(source);
            *preview_scroll = preview_target.is_some();
            // 左栏点中的这一行本身也描出来，与预览里标亮的那段对应。
            view.locate_context(source.clone(), false);
        }
        if let Some(clicked) = clicked_preview {
            match links.change_at(&clicked) {
                Some(change) => {
                    view.set_focus(change, true);
                    *preview_target = None;
                    *preview_scroll = true;
                }
                // 点在没改动的段落上：预览里标亮它，左栏滚到同一段并描出来。
                None => {
                    if let Some(source) = links.new_source_for_marked(&clicked) {
                        view.locate_context(source, true);
                    }
                    *preview_target = Some(clicked);
                    *preview_scroll = true;
                }
            }
            ui.ctx().request_repaint();
        }
        edit_source
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DraftInput, NumberingConfig, TemplateKind};
    use crate::theme;
    use eframe::egui;

    const OLD: &str = "# 测试通知\n\n请于八月十日前报送材料。\n\n保留段落。\n\n删除段落。";
    const NEW: &str = "# 测试通知\n\n请于八月十五日前报送材料。\n\n保留段落。\n\n新增段落。";

    struct Harness {
        ctx: egui::Context,
        old: ContentSnapshot,
        new: ContentSnapshot,
        display: UnitDisplay<'static>,
        state: VersionPairViewState,
        key: VersionPairKey,
        clock: f64,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            theme::configure_icons(&ctx);
            theme::configure_fonts(&ctx, &crate::models::FontConfig::default());
            let old = ContentSnapshot::new(
                DraftInput {
                    kind: TemplateKind::OfficialLetter,
                    ..Default::default()
                },
                OLD.into(),
                String::new(),
            );
            let new = ContentSnapshot::new(old.snapshot.clone(), NEW.into(), String::new());
            Self {
                ctx,
                old,
                new,
                display: UnitDisplay::new(&[]),
                state: VersionPairViewState::default(),
                key: VersionPairKey {
                    manuscript_id: 1,
                    old_version_number: Some(1),
                    new_side: PairSide::Version(2),
                },
                clock: 0.0,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.clock += 0.05;
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                time: Some(self.clock),
                events,
                ..Default::default()
            };
            self.ctx.clone().run_ui(raw, |ui| {
                self.state
                    .set_pair(self.key, &self.old, &self.new, &self.display);
                let _ = self.state.show(
                    ui,
                    egui::Id::new(("version_pair_test", self.key)),
                    "v1 · 送审稿",
                    "v2 · 定稿",
                    &self.display,
                    &NumberingConfig::default(),
                );
            })
        }

        fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]);
        }
    }

    fn text(output: &egui::FullOutput) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(shape) => Some(shape.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn text_in_column(output: &egui::FullOutput, right: bool) -> String {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                eframe::egui::epaint::Shape::Text(shape) if (shape.pos.x >= 800.0) == right => {
                    Some(shape.galley.text().to_string())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn historical_pair_view_draws_both_read_only_panes_and_links_every_change() {
        let mut harness = Harness::new();
        let output = harness.frame(Vec::new());
        let rendered = text(&output);
        assert!(rendered.contains("请于八月十日前报送材料。"), "{rendered}");
        assert!(
            rendered.contains("请于八月十五日前报送材料。"),
            "{rendered}"
        );
        assert!(rendered.contains("新增段落。"), "{rendered}");
        let left = text_in_column(&output, false);
        let right = text_in_column(&output, true);
        assert!(
            left.contains("请于八月十日前报送材料。"),
            "左栏旧值：{left}"
        );
        assert!(
            left.contains("请于八月十五日前报送材料。"),
            "左栏新值：{left}"
        );
        let cache = harness.state.cache.as_ref().expect("已缓存版本 diff");
        let fragments = crate::preview::marks::body_sequence(&cache.redline.markdown);
        assert!(
            fragments
                .iter()
                .any(|(_, kind)| *kind == crate::export::RedlineKind::Deleted)
        );
        assert!(
            fragments
                .iter()
                .any(|(_, kind)| *kind == crate::export::RedlineKind::Added)
        );
        for (fragment, kind) in fragments
            .iter()
            .filter(|(_, kind)| *kind != crate::export::RedlineKind::Same)
        {
            assert!(
                right.contains(fragment),
                "预览缺少 {kind:?} 片段 {fragment:?}：{right}"
            );
        }

        let report = &cache.report;
        let redline = &cache.redline;
        let links = &cache.links;
        let mut checked = 0;
        for (index, block) in report
            .body
            .blocks
            .iter()
            .filter_map(|block| match block {
                diff::DiffBlock::Changed(change) => Some(change),
                diff::DiffBlock::Unchanged(_) => None,
            })
            .enumerate()
        {
            // 空行变化按既有规则只在代码 diff 中显示，视觉预览没有标记范围。
            if block.role == diff::BlockRole::Blank {
                continue;
            }
            assert!(
                links.marked_range(index).is_some(),
                "正文变更 {index} 在预览无落点"
            );
            assert_eq!(
                links.change_at(&links.marked_range(index).unwrap()),
                Some(index)
            );
            checked += 1;
        }
        assert!(checked >= 2, "正文修改与新增都应有预览落点");
        assert!(!rendered.chars().any(crate::export::is_redline_sentinel));
        assert_eq!(harness.old.content_markdown, OLD);
        assert_eq!(harness.new.content_markdown, NEW);
        assert!(!redline.is_empty());

        let total = report.body.changed_count;
        assert!(total >= 2);
        assert_eq!(harness.state.view.focus(), 0);
        harness.key(egui::Key::F7, egui::Modifiers::NONE);
        assert_eq!(harness.state.view.focus(), 1);
        for _ in 0..total {
            harness.key(egui::Key::F7, egui::Modifiers::NONE);
        }
        assert_eq!(harness.state.view.focus(), 1, "F7 正向首尾循环");
        harness.key(egui::Key::F7, egui::Modifiers::SHIFT);
        assert_eq!(harness.state.view.focus(), 0, "Shift+F7 反向首尾循环");
        harness.key(egui::Key::F7, egui::Modifiers::SHIFT);
        assert_eq!(
            harness.state.view.focus(),
            total - 1,
            "Shift+F7 反向首尾循环"
        );
        // 快照只以不可变引用进入组件；文本事件和点击均不能改写历史两侧。
        harness.frame(vec![egui::Event::Text("不应写入历史".into())]);
        assert_eq!(harness.old.content_markdown, OLD);
        assert_eq!(harness.new.content_markdown, NEW);
    }
}
