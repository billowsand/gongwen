//! 审校区的查找/替换、正则展开与源码跳转状态。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::find`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::diff;
use crate::theme;
use crate::app::{warn};
use crate::diff_view::{DiffViewState};
use std::ops::{Range};
use std::path::{PathBuf};
use eframe::egui;
use crate::draft_page::{DraftPage, PreviewAnchor, editor_id};

/// 文件按钮不能在借用 `self` 的过程中直接改 `self`，先记下来再执行。
pub(crate) enum FileAction {
    Open(PathBuf),
    Reveal(PathBuf),
}

/// 起草页横幅：当前编辑内容来自哪一版本。
pub(crate) struct LoadedVersion {
    pub(crate) manuscript_id: i64,
    pub(crate) version_number: i64,
    pub(crate) name: String,
}

/// 起草页"版本对照"模式的状态：左侧基准版本 + 视图状态 + diff 缓存。
#[derive(Default)]
pub(crate) struct DraftDiffState {
    /// 左侧基准版本号；None 表示跟最新版比（每次提交后自动跟上）。
    pub(crate) base: Option<i64>,
    pub(crate) view: DiffViewState,
    /// 上一次算出的 diff 及其输入指纹。这个模式每帧都要渲染，
    /// 内容没动就直接复用，长稿才不会边打字边重算。
    pub(crate) cache: Option<(u64, diff::ManuscriptDiff)>,
}

/// 审校区的查找/替换条。匹配范围每帧按当前正文重新计算，避免编辑或替换后保存
/// 已经过期的字节位置。
#[derive(Default)]
pub(crate) struct MarkdownFindState {
    pub(crate) open: bool,
    pub(crate) query: String,
    pub(crate) replacement: String,
    pub(crate) case_sensitive: bool,
    pub(crate) regex: bool,
    pub(crate) current: usize,
    pub(crate) focus_query: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum FindAction {
    Previous,
    Next,
    Replace,
    ReplaceAll,
}

pub(crate) fn find_query_id() -> egui::Id {
    egui::Id::new("gw-markdown-find-query")
}

/// 返回非重叠匹配的 UTF-8 字节范围，正好可以安全交给 `String::replace_range`。
#[cfg(test)]
pub(crate) fn markdown_matches(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    markdown_matches_mode(text, query, case_sensitive, false).unwrap_or_default()
}

/// 按普通文本或正则表达式返回非重叠匹配；正则无效时把编译错误交给界面展示。
pub(crate) fn markdown_matches_mode(
    text: &str,
    query: &str,
    case_sensitive: bool,
    regex_mode: bool,
) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let pattern = if regex_mode {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let regex = regex::RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|error| error.to_string())?;
    Ok(regex
        .find_iter(text)
        .map(|matched| matched.start()..matched.end())
        .collect())
}

pub(crate) fn expanded_replacement(
    text: &str,
    query: &str,
    replacement: &str,
    case_sensitive: bool,
    regex_mode: bool,
    range: &Range<usize>,
) -> String {
    if !regex_mode {
        return replacement.to_string();
    }
    let Ok(regex) = regex::RegexBuilder::new(query)
        .case_insensitive(!case_sensitive)
        .build()
    else {
        return replacement.to_string();
    };
    let Some(captures) = regex.captures_at(text, range.start) else {
        return replacement.to_string();
    };
    if captures.get(0).map(|matched| matched.range()) != Some(range.clone()) {
        return replacement.to_string();
    }
    let mut expanded = String::new();
    captures.expand(replacement, &mut expanded);
    expanded
}

/// 把编辑框的光标挪到源码的 `offset` 字节处，聚焦并滚动到可见位置。
pub(crate) fn jump_to_source(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    offset: usize,
) {
    // egui 的光标按字符计，源码范围按字节计，这里换算一次；
    // 位置退到最近的字符边界，避免正文已改动时从半个汉字中间切开。
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let index = text[..offset].chars().count();
    let cursor = egui::text::CCursor::new(index);
    let mut state = output.state.clone();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(cursor)));
    state.store(ui.ctx(), editor_id());
    ui.ctx()
        .memory_mut(|memory| memory.request_focus(editor_id()));
    // egui 只在光标“变化”时自动滚动，程序设定的光标不算，只能自己滚。
    let rect = output
        .galley
        .pos_from_cursor(cursor)
        .translate(output.galley_pos.to_vec2());
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
}

/// 在插入点的一侧要补几个换行，才能让插入的块级内容独占段落。
///
/// `before` 为真时看的是插入点之前那半段（补在它后面），否则看之后那半段。
/// 那一侧已经是空的（插在文首/文末）就什么都不补。
pub(crate) fn blank_line_padding(side: &str, before: bool) -> &'static str {
    if side.is_empty() {
        return "";
    }
    let (one, two) = if before {
        (side.ends_with('\n'), side.ends_with("\n\n"))
    } else {
        (side.starts_with('\n'), side.starts_with("\n\n"))
    };
    match (two, one) {
        (true, _) => "",
        (false, true) => "\n",
        (false, false) => "\n\n",
    }
}

/// 选中查找命中的完整字节范围，并把当前命中滚动到编辑器中央。
pub(crate) fn select_source_range(
    ui: &mut egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    text: &str,
    range: Range<usize>,
) {
    if range.is_empty()
        || range.end > text.len()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return;
    }
    let start = egui::text::CCursor::new(text[..range.start].chars().count());
    let end = egui::text::CCursor::new(text[..range.end].chars().count());
    let mut state = output.state.clone();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(start, end)));
    state.store(ui.ctx(), editor_id());
    let rect = output
        .galley
        .pos_from_cursor(end)
        .translate(output.galley_pos.to_vec2());
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
}

impl DraftPage<'_> {
    /// 审校区顶栏：显示方式切换、缩放、以及作用于当前稿件的几个动作。
    /// VS Code 风格的紧凑查找/替换条：Enter / Shift+Enter 在命中间移动，
    /// 当前命中同时选中源码，并在公文预览中标亮它所在的版式块。
    pub(crate) fn markdown_find_ui(&mut self, ui: &mut egui::Ui) {
        if ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.close_markdown_find();
            return;
        }

        let match_result = markdown_matches_mode(
            &self.doc.generated_markdown,
            &self.doc.markdown_find.query,
            self.doc.markdown_find.case_sensitive,
            self.doc.markdown_find.regex,
        );
        let regex_error = match_result.as_ref().err().cloned();
        let matches = match_result.unwrap_or_default();
        if matches.is_empty() {
            self.doc.markdown_find.current = 0;
        } else {
            self.doc.markdown_find.current = self.doc.markdown_find.current.min(matches.len() - 1);
        }
        let anchored = self
            .doc
            .preview_anchor
            .as_ref()
            .and_then(|anchor| anchor.range_in(&self.doc.generated_markdown));
        if !matches.is_empty() && anchored.as_ref() != matches.get(self.doc.markdown_find.current) {
            self.select_find_match(matches.get(self.doc.markdown_find.current).cloned());
        }

        let mut action = None;
        let mut query_changed = false;
        let mut close = false;
        ui.horizontal(|ui| {
            ui.strong("查找");
            let input_width = (ui.available_width() - 250.0).clamp(120.0, 300.0);
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.doc.markdown_find.query)
                    .id(find_query_id())
                    .desired_width(input_width)
                    .hint_text("输入要查找的文字"),
            );
            if self.doc.markdown_find.focus_query {
                response.request_focus();
                self.doc.markdown_find.focus_query = false;
            }
            query_changed = response.changed();
            if response.has_focus() {
                if ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter))
                {
                    action = Some(FindAction::Previous);
                } else if ui
                    .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                {
                    action = Some(FindAction::Next);
                }
            }

            let count = if self.doc.markdown_find.query.is_empty() {
                "无结果".to_string()
            } else if matches.is_empty() {
                "0 个结果".to_string()
            } else {
                format!("{} / {}", self.doc.markdown_find.current + 1, matches.len())
            };
            ui.label(egui::RichText::new(count).color(theme::text_muted()));
            if theme::icon_button_enabled(
                ui,
                !matches.is_empty(),
                theme::Icon::ArrowUp,
                "上一个匹配（Shift+Enter）",
            )
            .clicked()
            {
                action = Some(FindAction::Previous);
            }
            if theme::icon_button_enabled(
                ui,
                !matches.is_empty(),
                theme::Icon::ArrowDown,
                "下一个匹配（Enter）",
            )
            .clicked()
            {
                action = Some(FindAction::Next);
            }
            let case_response = ui
                .add(
                    egui::Button::new("Aa")
                        .small()
                        .selected(self.doc.markdown_find.case_sensitive),
                )
                .on_hover_text("区分大小写");
            if case_response.clicked() {
                self.doc.markdown_find.case_sensitive = !self.doc.markdown_find.case_sensitive;
                query_changed = true;
            }
            let regex_response = ui
                .add(
                    egui::Button::new(".*")
                        .small()
                        .selected(self.doc.markdown_find.regex),
                )
                .on_hover_text("使用正则表达式；替换内容支持 $1 和 $name 捕获组");
            if regex_response.clicked() {
                self.doc.markdown_find.regex = !self.doc.markdown_find.regex;
                query_changed = true;
            }
            if theme::icon_button(ui, theme::Icon::X, "关闭查找（Esc）").clicked() {
                close = true;
            }
        });
        if let Some(error) = &regex_error {
            ui.colored_label(warn(), format!("正则表达式无效：{error}"));
        }
        ui.horizontal(|ui| {
            ui.strong("替换");
            let input_width = (ui.available_width() - 220.0).clamp(120.0, 300.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.doc.markdown_find.replacement)
                    .desired_width(input_width)
                    .hint_text("输入替换文字"),
            );
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("替换"))
                .clicked()
            {
                action = Some(FindAction::Replace);
            }
            if ui
                .add_enabled(!matches.is_empty(), egui::Button::new("全部替换"))
                .clicked()
            {
                action = Some(FindAction::ReplaceAll);
            }
        });

        if close {
            self.close_markdown_find();
            return;
        }
        if query_changed {
            self.doc.markdown_find.current = 0;
            let refreshed = markdown_matches_mode(
                &self.doc.generated_markdown,
                &self.doc.markdown_find.query,
                self.doc.markdown_find.case_sensitive,
                self.doc.markdown_find.regex,
            )
            .unwrap_or_default();
            self.select_find_match(refreshed.first().cloned());
            return;
        }
        if let Some(action) = action {
            self.run_find_action(action, matches);
        }
    }

    pub(crate) fn close_markdown_find(&mut self) {
        self.doc.markdown_find.open = false;
        self.doc.pending_source_selection = None;
        self.doc.pending_render_jump = false;
        self.doc.preview_anchor = None;
    }

    pub(crate) fn select_find_match(&mut self, range: Option<Range<usize>>) {
        self.doc.pending_source_jump = None;
        let Some(range) = range else {
            self.doc.preview_anchor = None;
            self.doc.pending_source_selection = None;
            self.doc.pending_render_jump = false;
            return;
        };
        self.doc.pending_source_selection = Some(range.clone());
        self.doc.pending_render_jump = true;
        self.doc.preview_anchor =
            self.doc
                .generated_markdown
                .get(range.clone())
                .map(|text| PreviewAnchor {
                    range,
                    text: text.to_owned(),
                });
    }

    pub(crate) fn run_find_action(&mut self, action: FindAction, matches: Vec<Range<usize>>) {
        if matches.is_empty() {
            return;
        }
        match action {
            FindAction::Previous => {
                self.doc.markdown_find.current = if self.doc.markdown_find.current == 0 {
                    matches.len() - 1
                } else {
                    self.doc.markdown_find.current - 1
                };
                self.select_find_match(matches.get(self.doc.markdown_find.current).cloned());
            }
            FindAction::Next => {
                self.doc.markdown_find.current =
                    (self.doc.markdown_find.current + 1) % matches.len();
                self.select_find_match(matches.get(self.doc.markdown_find.current).cloned());
            }
            FindAction::Replace => {
                let range = matches[self.doc.markdown_find.current].clone();
                let replacement = self.find_replacement(&range);
                self.doc
                    .generated_markdown
                    .replace_range(range.clone(), &replacement);
                let next_offset = range.start + replacement.len();
                let refreshed = markdown_matches_mode(
                    &self.doc.generated_markdown,
                    &self.doc.markdown_find.query,
                    self.doc.markdown_find.case_sensitive,
                    self.doc.markdown_find.regex,
                )
                .unwrap_or_default();
                self.doc.markdown_find.current = refreshed
                    .iter()
                    .position(|candidate| candidate.start >= next_offset)
                    .unwrap_or(0);
                self.select_find_match(refreshed.get(self.doc.markdown_find.current).cloned());
                *self.status = "已替换当前匹配。".into();
            }
            FindAction::ReplaceAll => {
                let count = matches.len();
                if self.doc.markdown_find.regex {
                    let pattern = self.doc.markdown_find.query.clone();
                    if let Ok(regex) = regex::RegexBuilder::new(&pattern)
                        .case_insensitive(!self.doc.markdown_find.case_sensitive)
                        .build()
                    {
                        self.doc.generated_markdown = regex
                            .replace_all(
                                &self.doc.generated_markdown,
                                self.doc.markdown_find.replacement.as_str(),
                            )
                            .into_owned();
                    }
                } else {
                    for range in matches.into_iter().rev() {
                        self.doc
                            .generated_markdown
                            .replace_range(range, &self.doc.markdown_find.replacement);
                    }
                }
                self.doc.markdown_find.current = 0;
                self.select_find_match(None);
                *self.status = format!("已替换 {count} 处匹配。");
            }
        }
    }

    pub(crate) fn find_replacement(&self, range: &Range<usize>) -> String {
        expanded_replacement(
            &self.doc.generated_markdown,
            &self.doc.markdown_find.query,
            &self.doc.markdown_find.replacement,
            self.doc.markdown_find.case_sensitive,
            self.doc.markdown_find.regex,
            range,
        )
    }
}
