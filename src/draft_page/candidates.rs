//! 起草页候选区：写稿时从正文暂时移走、以后可能还要用的文字。
//!
//! 选中正文右键「移入候选区」，文字离开正文进入本篇的候选区；需要时在候选区
//! 里只挑一部分（拖选、双击选一句、三击选一段）插回正文光标处，用掉的部分随即
//! 从候选里删去。候选区按稿件各一份，只随稿件保存，不进版本、不进离线同步
//! （存储见 `manuscript::candidates`）。
//!
//! 内存里的条目是准数，稿件库只是它的落盘副本：每次改动整组重写一遍。还没入库
//! 的新稿先把候选留在内存里，等它有了稿件库 id 再一起写进去。

use crate::draft_page::markdown::byte_at_char;
use crate::draft_page::{DraftPage, PreviewMode, editor_id, editor_selection, navigator};
use crate::manuscript::ManuscriptStore;
use crate::manuscript::candidates::CandidateRecord;
use crate::models::{NumberingConfig, TemplateKind};
use chrono::Datelike;
use eframe::egui;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::time::{Duration, Instant};

/// 在候选里打字时，停手这么久才落盘，免得每敲一个字写一次库。
const TYPING_SAVE_DELAY: Duration = Duration::from_millis(1200);

/// 候选区里的一条。`key` 只在本次运行里区分条目，不落盘。
#[derive(Debug, Clone)]
pub(crate) struct Candidate {
    pub(crate) key: u64,
    pub(crate) record: CandidateRecord,
}

/// 一篇稿件的候选区状态。
#[derive(Debug, Default)]
pub(crate) struct CandidateState {
    /// 最新的排在最前。
    pub(crate) items: Vec<Candidate>,
    /// 这批条目属于哪条稿件库记录；与会话的 `manuscript_id` 不一致时要重新
    /// 载入，或者把新稿攒下的条目写进刚建好的记录。
    loaded_for: Option<i64>,
    /// 有改动还没落盘。
    dirty: bool,
    /// 正在候选里打字：停手一段时间后再落盘。为 None 表示下一帧就写。
    typing_since: Option<Instant>,
    next_key: u64,
    /// 候选区面板是否展开。
    pub(crate) open: bool,
    /// 右栏正在看的那一条。
    pub(crate) selected: Option<u64>,
    /// 最近一次移入、插回或删除的撤销点。
    pub(crate) undo: Option<CandidateUndo>,
    /// 正文里各一级小节的标题，分组排序用；正文不变就不重算。
    pub(crate) sections: SectionCache,
}

/// 撤销点：操作前的正文与候选，外加操作后的正文用来判断还能不能撤。
#[derive(Debug)]
pub(crate) struct CandidateUndo {
    pub(crate) label: &'static str,
    markdown_before: String,
    markdown_after: String,
    items_before: Vec<Candidate>,
    selected_before: Option<u64>,
    cursor_before: Option<usize>,
    at: Instant,
}

/// 撤销入口保留多久。
pub(crate) const UNDO_WINDOW: Duration = Duration::from_secs(30);

impl CandidateUndo {
    /// 正文在操作之后又被改过，就不能再整篇退回去了，否则会吞掉后来的修改。
    pub(crate) fn valid_for(&self, markdown: &str) -> bool {
        self.markdown_after == markdown && self.at.elapsed() < UNDO_WINDOW
    }

    pub(crate) fn remaining(&self) -> Duration {
        UNDO_WINDOW.saturating_sub(self.at.elapsed())
    }
}

/// 正文里各一级小节的 `(标题文字, 带编号的字面)`，按正文顺序。
#[derive(Debug, Default)]
pub(crate) struct SectionCache {
    fingerprint: u64,
    numbering: Option<NumberingConfig>,
    kind: Option<TemplateKind>,
    pub(crate) tops: Vec<(String, String)>,
}

impl SectionCache {
    pub(crate) fn refresh(
        &mut self,
        markdown: &str,
        numbering: &NumberingConfig,
        kind: TemplateKind,
    ) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        markdown.hash(&mut hasher);
        let fingerprint = hasher.finish();
        if self.fingerprint == fingerprint
            && self.numbering == Some(*numbering)
            && self.kind == Some(kind)
        {
            return;
        }
        self.fingerprint = fingerprint;
        self.numbering = Some(*numbering);
        self.kind = Some(kind);
        let entries = navigator::collect_entries(markdown, numbering, kind);
        self.tops = top_entries(&entries)
            .map(|entry| {
                (
                    crate::export::plain_text(&entry.text),
                    navigator::label_text(entry),
                )
            })
            .collect();
    }
}

impl CandidateState {
    pub(crate) fn position(&self, key: u64) -> Option<usize> {
        self.items.iter().position(|item| item.key == key)
    }

    /// 标记有改动。`typing` 为真时延后落盘，否则下一帧就写。
    pub(crate) fn mark_dirty(&mut self, typing: bool) {
        self.dirty = true;
        self.typing_since = typing.then(Instant::now);
    }

    pub(crate) fn next_key(&mut self) -> u64 {
        self.next_key += 1;
        self.next_key
    }

    /// 让内存与稿件库对齐：换了记录就载入，新稿刚入库就把攒下的条目写进去，
    /// 有改动就落盘。`force` 为真时不等打字停手（切标签、关窗前用）。
    /// 返回落盘失败的原因。
    pub(crate) fn sync(
        &mut self,
        manuscript_id: Option<i64>,
        store: Option<&mut ManuscriptStore>,
        force: bool,
    ) -> Option<String> {
        let Some(id) = manuscript_id else {
            // 还没入库：条目只在内存里。
            return None;
        };
        let store = store?;
        if self.loaded_for != Some(id) {
            let adopt_unsaved = self.loaded_for.is_none() && !self.items.is_empty();
            self.loaded_for = Some(id);
            if adopt_unsaved {
                self.dirty = true;
                self.typing_since = None;
            } else {
                self.dirty = false;
                self.typing_since = None;
                self.selected = None;
                self.undo = None;
                match store.load_candidates(id) {
                    Ok(records) => {
                        self.items = records
                            .into_iter()
                            .map(|record| Candidate {
                                key: self.next_key(),
                                record,
                            })
                            .collect();
                    }
                    Err(error) => {
                        self.items.clear();
                        return Some(format!("读取候选区失败：{error:#}"));
                    }
                }
            }
        }
        if !self.dirty {
            return None;
        }
        if !force
            && self
                .typing_since
                .is_some_and(|since| since.elapsed() < TYPING_SAVE_DELAY)
        {
            return None;
        }
        let records: Vec<CandidateRecord> =
            self.items.iter().map(|item| item.record.clone()).collect();
        match store.save_candidates(id, &records) {
            Ok(()) => {
                self.dirty = false;
                self.typing_since = None;
                None
            }
            Err(error) => {
                // 过一会儿再试，别每帧都撞一次。
                self.typing_since = Some(Instant::now());
                Some(format!("保存候选区失败：{error:#}"))
            }
        }
    }

    /// 还在等打字停手的改动：界面要继续刷新，到点才能写下去。
    pub(crate) fn waiting_to_save(&self) -> bool {
        self.dirty && self.typing_since.is_some()
    }
}

impl DraftPage<'_> {
    /// 每帧调用：候选区与稿件库对齐。
    pub(crate) fn sync_candidates(&mut self, ctx: &egui::Context) {
        let error =
            self.doc
                .candidates
                .sync(self.doc.manuscript_id, self.store.as_deref_mut(), false);
        if let Some(error) = error {
            *self.status = error;
        }
        if self.doc.candidates.waiting_to_save() {
            ctx.request_repaint_after(TYPING_SAVE_DELAY);
        }
    }

    /// 有源码编辑框的显示方式才能移入、插回候选。
    pub(crate) fn candidates_available(&self) -> bool {
        matches!(
            self.doc.preview_mode,
            PreviewMode::Source | PreviewMode::Split
        )
    }

    /// 执行编辑框右键菜单里选中的动作。
    pub(crate) fn run_editor_menu_action(&mut self, ctx: &egui::Context, action: EditorMenuAction) {
        let selection = editor_selection(ctx, &self.doc.generated_markdown);
        match action {
            EditorMenuAction::Copy => {
                if let Some(range) = selection.filter(|range| !range.is_empty()) {
                    ctx.copy_text(self.doc.generated_markdown[range].to_string());
                }
            }
            EditorMenuAction::Cut => {
                if self.doc.read_only() {
                    return;
                }
                if let Some(range) = selection.filter(|range| !range.is_empty()) {
                    ctx.copy_text(self.doc.generated_markdown[range.clone()].to_string());
                    self.doc.generated_markdown.replace_range(range.clone(), "");
                    self.doc.pending_source_jump = Some(range.start);
                }
            }
            EditorMenuAction::Paste => {
                if self.doc.read_only() {
                    return;
                }
                // 剪贴板只能由窗口层读：把焦点还给编辑框，再让它照 Ctrl+V 粘贴。
                ctx.memory_mut(|memory| memory.request_focus(editor_id()));
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            EditorMenuAction::MoveToCandidates => self.move_selection_to_candidates(ctx),
        }
    }

    /// 操作前留一份撤销点的原料：正文、候选、选中条目与正文光标。
    fn undo_snapshot(&self, ctx: &egui::Context) -> UndoSnapshot {
        UndoSnapshot {
            markdown: self.doc.generated_markdown.clone(),
            items: self.doc.candidates.items.clone(),
            selected: self.doc.candidates.selected,
            cursor: editor_selection(ctx, &self.doc.generated_markdown).map(|range| range.start),
        }
    }

    /// 操作做完后落下撤销点，并安排落盘。
    fn commit_candidate_change(&mut self, label: &'static str, before: UndoSnapshot) {
        let candidates = &mut self.doc.candidates;
        candidates.undo = Some(CandidateUndo {
            label,
            markdown_before: before.markdown,
            markdown_after: self.doc.generated_markdown.clone(),
            items_before: before.items,
            selected_before: before.selected,
            cursor_before: before.cursor,
            at: Instant::now(),
        });
        candidates.mark_dirty(false);
    }

    /// 把正文里选中的文字移入候选区。
    pub(crate) fn move_selection_to_candidates(&mut self, ctx: &egui::Context) {
        if self.doc.read_only() || !self.candidates_available() {
            return;
        }
        let Some(range) =
            editor_selection(ctx, &self.doc.generated_markdown).filter(|range| !range.is_empty())
        else {
            *self.status = "先在正文里选中要移入候选区的文字。".into();
            return;
        };
        let text = clean_candidate_text(&self.doc.generated_markdown[range.clone()]);
        if text.is_empty() {
            *self.status = "选中的只有空白，没有移入候选区。".into();
            return;
        }
        let before = self.undo_snapshot(ctx);
        let entries = navigator::collect_entries(
            &self.doc.generated_markdown,
            &self.config.numbering,
            self.doc.draft.kind,
        );
        let (section, section_label, subsection) = section_at(&entries, range.start);
        let (updated, cursor) = cut_range(&self.doc.generated_markdown, range);
        self.doc.generated_markdown = updated;
        self.doc.pending_source_jump = Some(cursor);
        let candidates = &mut self.doc.candidates;
        let key = candidates.next_key();
        candidates.items.insert(
            0,
            Candidate {
                key,
                record: CandidateRecord {
                    text,
                    section,
                    section_label,
                    subsection,
                    created_at: chrono::Local::now().to_rfc3339(),
                },
            },
        );
        candidates.selected = Some(key);
        let count = candidates.items.len();
        self.commit_candidate_change("已移入候选区", before);
        *self.status = format!("已移入候选区（共 {count} 条）。");
    }

    /// 把一条候选插回正文光标处：`part` 为 None 时整条插入。插进去的部分随即
    /// 从候选里删去，整条用完就去掉这一条。
    pub(crate) fn insert_candidate(
        &mut self,
        ctx: &egui::Context,
        key: u64,
        part: Option<Range<usize>>,
    ) {
        if self.doc.read_only() {
            return;
        }
        let Some(index) = self.doc.candidates.position(key) else {
            return;
        };
        let text = &self.doc.candidates.items[index].record.text;
        let part = part
            .filter(|range| !range.is_empty() && range.end <= text.len())
            .unwrap_or(0..text.len());
        let piece = text[part.clone()].trim_matches(['\n', '\r']).to_string();
        if piece.trim().is_empty() {
            *self.status = "选中的只有空白，没有插入。".into();
            return;
        }
        let Some(target) = editor_selection(ctx, &self.doc.generated_markdown) else {
            *self.status = "先在正文里点一下要插入的位置。".into();
            return;
        };
        let before = self.undo_snapshot(ctx);
        self.doc
            .generated_markdown
            .replace_range(target.clone(), &piece);
        self.doc.pending_source_jump = Some(target.start + piece.len());
        self.take_candidate_part(index, part);
        self.commit_candidate_change("已插入正文", before);
        *self.status = format!("已把 {} 字插入正文。", visible_chars(&piece));
    }

    /// 从候选里删去一部分；`part` 为 None 时删除整条。
    pub(crate) fn delete_candidate(
        &mut self,
        ctx: &egui::Context,
        key: u64,
        part: Option<Range<usize>>,
    ) {
        if self.doc.read_only() {
            return;
        }
        let Some(index) = self.doc.candidates.position(key) else {
            return;
        };
        let before = self.undo_snapshot(ctx);
        let len = self.doc.candidates.items[index].record.text.len();
        let part = part.filter(|range| !range.is_empty() && range.end <= len);
        let whole = part.is_none();
        self.take_candidate_part(index, part.unwrap_or(0..len));
        let label = if whole {
            "已删除这条候选"
        } else {
            "已删去选中的部分"
        };
        self.commit_candidate_change(label, before);
        *self.status = format!("{label}。");
    }

    /// 清空本篇候选区。
    pub(crate) fn clear_candidates(&mut self, ctx: &egui::Context) {
        if self.doc.read_only() || self.doc.candidates.items.is_empty() {
            return;
        }
        let before = self.undo_snapshot(ctx);
        self.doc.candidates.items.clear();
        self.doc.candidates.selected = None;
        self.commit_candidate_change("已清空候选区", before);
        *self.status = "已清空候选区。".into();
    }

    /// 撤销最近一次候选区操作：正文与候选一起退回去。
    pub(crate) fn undo_candidates(&mut self) {
        let Some(undo) = self.doc.candidates.undo.take() else {
            return;
        };
        if !undo.valid_for(&self.doc.generated_markdown) {
            return;
        }
        self.doc.generated_markdown = undo.markdown_before;
        self.doc.pending_source_jump = undo.cursor_before;
        let candidates = &mut self.doc.candidates;
        candidates.items = undo.items_before;
        candidates.selected = undo.selected_before;
        candidates.mark_dirty(false);
        *self.status = "已撤销。".into();
    }

    /// 删去第 `index` 条候选里的 `part`；剩下的只有空白就去掉整条，并把
    /// 选中挪到相邻的一条上。
    fn take_candidate_part(&mut self, index: usize, part: Range<usize>) {
        let candidates = &mut self.doc.candidates;
        let rest = take_part(&candidates.items[index].record.text, part);
        if rest.is_empty() {
            candidates.items.remove(index);
            candidates.selected = candidates
                .items
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|i| candidates.items.get(i)))
                .map(|item| item.key);
        } else {
            candidates.items[index].record.text = rest;
        }
    }
}

/// 操作前的状态，见 [`DraftPage::undo_snapshot`]。
struct UndoSnapshot {
    markdown: String,
    items: Vec<Candidate>,
    selected: Option<u64>,
    cursor: Option<usize>,
}

/// 源码编辑框右键菜单里的动作。菜单在编辑框借着正文时弹出，动作要等借用
/// 结束后再做，所以先记下来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditorMenuAction {
    Cut,
    Copy,
    Paste,
    MoveToCandidates,
}

/// 某个编辑框本帧绘制前的选区。
pub(crate) fn selection_before_show(
    ctx: &egui::Context,
    id: egui::Id,
) -> Option<egui::text::CCursorRange> {
    egui::TextEdit::load_state(ctx, id).and_then(|state| state.cursor.char_range())
}

/// 右键时保住编辑框的选区。
///
/// egui 的编辑框在任何鼠标键按下时都会把选区收成指针处的一点，右键也不例外，
/// 菜单弹出来时选中的文字已经没了。所以右键按住到松开的这几帧里，把绘制前的
/// 选区原样放回去。没有选区时照常让光标落到右键处。
pub(crate) fn keep_selection_on_secondary(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    before: Option<egui::text::CCursorRange>,
    id: egui::Id,
) {
    let secondary = ui.input(|input| {
        input.pointer.button_down(egui::PointerButton::Secondary)
            || input
                .pointer
                .button_released(egui::PointerButton::Secondary)
    });
    if secondary
        && let Some(before) = before.filter(|range| !range.is_empty())
        && output.state.cursor.char_range() != Some(before)
    {
        let mut state = output.state.clone();
        state.cursor.set_char_range(Some(before));
        state.store(ui.ctx(), id);
        ui.ctx().request_repaint();
    }
}

/// 源码编辑框的右键菜单。
pub(crate) fn editor_context_menu(
    ui: &egui::Ui,
    output: &egui::text_edit::TextEditOutput,
    before: Option<egui::text::CCursorRange>,
    editable: bool,
) -> Option<EditorMenuAction> {
    keep_selection_on_secondary(ui, output, before, editor_id());
    let has_selection =
        selection_before_show(ui.ctx(), editor_id()).is_some_and(|range| !range.is_empty());
    let mut action = None;
    output.response.context_menu(|ui| {
        ui.set_min_width(200.0);
        if menu_entry(
            ui,
            editable && has_selection,
            "剪切",
            &crate::theme::primary_shortcut("X"),
        ) {
            action = Some(EditorMenuAction::Cut);
        }
        if menu_entry(
            ui,
            has_selection,
            "复制",
            &crate::theme::primary_shortcut("C"),
        ) {
            action = Some(EditorMenuAction::Copy);
        }
        if menu_entry(ui, editable, "粘贴", &crate::theme::primary_shortcut("V")) {
            action = Some(EditorMenuAction::Paste);
        }
        ui.separator();
        // 加词与查码都只带选中的文字过去，不改正文；请求经临时存储交给输入法浮窗。
        let ime_requests = [
            (
                "加入词表",
                crate::theme::primary_shortcut("Shift+A"),
                "ime-add-word",
            ),
            ("查编码", String::new(), "ime-lookup"),
        ];
        for (label, shortcut, request) in ime_requests {
            if menu_entry(ui, has_selection, label, &shortcut) {
                if let Some(range) = selection_before_show(ui.ctx(), editor_id()) {
                    let sorted = range.sorted_cursors();
                    let text: String = output
                        .galley
                        .job
                        .text
                        .chars()
                        .skip(sorted[0].index.0)
                        .take(sorted[1].index.0 - sorted[0].index.0)
                        .collect();
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(egui::Id::new(request), text));
                }
                ui.close();
            }
        }
        if menu_entry(
            ui,
            editable && has_selection,
            "移入候选区",
            &crate::theme::primary_shortcut("Shift+X"),
        ) {
            action = Some(EditorMenuAction::MoveToCandidates);
        }
        if action.is_some() {
            ui.close();
        }
    });
    action
}

/// 右键菜单里带快捷键提示的一项。
pub(crate) fn menu_entry(ui: &mut egui::Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    ui.add_enabled(
        enabled,
        crate::theme::menu_text_item(label).shortcut_text(shortcut),
    )
    .clicked()
}

/// 编辑框当前选区换算成 `text` 里的字节范围。
pub(crate) fn selection_bytes(
    ctx: &egui::Context,
    id: egui::Id,
    text: &str,
) -> Option<Range<usize>> {
    let range = selection_before_show(ctx, id)?;
    let primary = byte_at_char(text, range.primary.index.0);
    let secondary = byte_at_char(text, range.secondary.index.0);
    Some(primary.min(secondary)..primary.max(secondary))
}

/// 不计空白的字数。
pub(crate) fn visible_chars(text: &str) -> usize {
    text.chars().filter(|ch| !ch.is_whitespace()).count()
}

/// 候选文字去掉首尾的空行和行尾空白，段内换行原样保留。
fn clean_candidate_text(text: &str) -> String {
    text.trim_start_matches(['\n', '\r']).trim_end().to_string()
}

/// 从候选里删去一段后剩下的文字：切口两侧的空行照正文的规矩收拾，首尾空白去掉。
fn take_part(text: &str, part: Range<usize>) -> String {
    clean_candidate_text(&cut_range(text, part).0)
}

/// 从正文里切走一段，并收拾切口两侧的空行：整段移走后留下的多个空行并成
/// 一个，切到文首时不留开头的空行。返回新正文与切口处的光标位置。
pub(crate) fn cut_range(text: &str, range: Range<usize>) -> (String, usize) {
    let head = &text[..range.start];
    let tail = &text[range.end..];
    let head_core = head.trim_end_matches('\n');
    let tail_core = tail.trim_start_matches('\n');
    let head_newlines = head.len() - head_core.len();
    let total = head_newlines + (tail.len() - tail_core.len());
    let keep = if head_core.is_empty() {
        0
    } else if tail_core.is_empty() {
        total.min(1)
    } else {
        total.min(2)
    };
    let mut result = String::with_capacity(head_core.len() + keep + tail_core.len());
    result.push_str(head_core);
    result.extend(std::iter::repeat_n('\n', keep));
    result.push_str(tail_core);
    (result, head_core.len() + head_newlines.min(keep))
}

/// 句末标点。省略号不算：「等等……」常在句中。
fn is_sentence_end(ch: char) -> bool {
    matches!(ch, '。' | '！' | '？' | '；' | '!' | '?' | ';')
}

/// 紧跟在句末标点后面、仍属于这一句的收尾符号。
fn is_closing(ch: char) -> bool {
    matches!(
        ch,
        '”' | '’' | '」' | '』' | '）' | ')' | '》' | '〉' | '】' | '"' | '\''
    )
}

/// `index` 处是不是一句的收尾：句末标点本身，或跟在句末标点后面的收尾符号。
fn closes_sentence(chars: &[char], index: usize) -> bool {
    let mut at = index;
    while is_closing(chars[at]) {
        if at == 0 {
            return false;
        }
        at -= 1;
    }
    is_sentence_end(chars[at])
}

/// 双击选句：`index`（字符下标）所在的那一句，按字符下标返回。
///
/// 句子从上一个句末标点（或段首）之后开始，到下一个句末标点连同紧跟的引号、
/// 括号为止；换行也是边界。开头的空白不算进去。
pub(crate) fn sentence_range(text: &str, index: usize) -> Range<usize> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return 0..0;
    }
    let mut pos = index.min(chars.len() - 1);
    if chars[pos] == '\n' {
        // 点在行尾：算作前面那一句。
        if pos == 0 || chars[pos - 1] == '\n' {
            return index..index;
        }
        pos -= 1;
    }
    // 点在句末引号上：往回退到句末标点，算作这一句。
    while pos > 0 && is_closing(chars[pos]) && closes_sentence(&chars, pos - 1) {
        pos -= 1;
    }
    let mut start = pos;
    while start > 0 {
        let previous = chars[start - 1];
        if previous == '\n' || closes_sentence(&chars, start - 1) {
            break;
        }
        start -= 1;
    }
    while start < pos && chars[start].is_whitespace() {
        start += 1;
    }
    let mut end = pos;
    while end < chars.len() && chars[end] != '\n' {
        let ch = chars[end];
        end += 1;
        if is_sentence_end(ch) {
            while end < chars.len() && is_closing(chars[end]) {
                end += 1;
            }
            break;
        }
    }
    start..end
}

/// 正文里的一级标题：最高的编号层级（公文的「一、」，研究报告的章），附件标题也算。
fn top_entries(entries: &[navigator::NavEntry]) -> impl Iterator<Item = &navigator::NavEntry> {
    let top_level = entries
        .iter()
        .filter(|entry| entry.level >= 2)
        .map(|entry| entry.level)
        .min();
    entries
        .iter()
        .filter(move |entry| entry.is_attachment_title || Some(entry.level) == top_level)
}

/// 某个位置所在的小节：`(分组用的一级标题文字, 一级标题字面, 更深一级的最近标题字面)`。
/// 第一个标题之前的内容三者都为空。
fn section_at(entries: &[navigator::NavEntry], byte: usize) -> (String, String, String) {
    let before = entries
        .iter()
        .take_while(|entry| entry.line.start <= byte)
        .collect::<Vec<_>>();
    let tops = top_entries(entries)
        .map(|entry| entry.line.start)
        .collect::<Vec<_>>();
    let top = before
        .iter()
        .rposition(|entry| tops.contains(&entry.line.start));
    let (section, section_label) = top.map_or_else(Default::default, |index| {
        (
            crate::export::plain_text(&before[index].text),
            navigator::label_text(before[index]),
        )
    });
    let subsection = before
        .iter()
        .enumerate()
        .rev()
        .find(|(index, entry)| top.is_none_or(|top| *index > top) && entry.level >= 2)
        .map(|(_, entry)| navigator::label_text(entry))
        .unwrap_or_default();
    (section, section_label, subsection)
}

/// 候选区左栏的一组：同一个一级小节下移入的条目。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CandidateGroup {
    /// 分组键，即一级标题文字；第一个标题之前的内容为空串。
    pub(crate) key: String,
    /// 显示的字面：小节还在正文里就用当前编号，否则用移入时的字面。
    pub(crate) label: String,
    /// 这个小节已经不在正文里了。
    pub(crate) missing: bool,
    /// 组内条目在 `items` 里的下标，保持原有先后（新的在前）。
    pub(crate) members: Vec<usize>,
}

/// 按来源小节分组：开头一组在最前，其余按小节在当前正文里的顺序；已经不在
/// 正文里的小节排在最后，按最新一条移入的先后。
pub(crate) fn group_candidates(
    items: &[Candidate],
    tops: &[(String, String)],
) -> Vec<CandidateGroup> {
    let mut groups: Vec<CandidateGroup> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let key = &item.record.section;
        if let Some(group) = groups.iter_mut().find(|group| &group.key == key) {
            group.members.push(index);
            continue;
        }
        let current = tops.iter().find(|(text, _)| text == key);
        let label = match current {
            _ if key.is_empty() => "开头".to_string(),
            Some((_, label)) => label.clone(),
            None if item.record.section_label.is_empty() => key.clone(),
            None => item.record.section_label.clone(),
        };
        groups.push(CandidateGroup {
            key: key.clone(),
            label,
            missing: !key.is_empty() && current.is_none(),
            members: vec![index],
        });
    }
    let rank = |group: &CandidateGroup| {
        if group.key.is_empty() {
            0
        } else {
            tops.iter()
                .position(|(text, _)| *text == group.key)
                .map_or(usize::MAX, |position| position + 1)
        }
    };
    // 稳定排序：不在正文里的几组保持最新一条的先后。
    groups.sort_by_key(rank);
    groups
}

/// 条目说明里的时间：今天、昨天只写钟点，更早的写日期。
pub(crate) fn time_label(created_at: &str) -> String {
    let Ok(time) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return String::new();
    };
    let time = time.with_timezone(&chrono::Local);
    let today = chrono::Local::now().date_naive();
    let date = time.date_naive();
    if date == today {
        time.format("今天 %H:%M").to_string()
    } else if today.pred_opt() == Some(date) {
        time.format("昨天 %H:%M").to_string()
    } else if date.year() == today.year() {
        time.format("%-m月%-d日").to_string()
    } else {
        time.format("%Y年%-m月%-d日").to_string()
    }
}

/// 条目说明里的篇幅：表格按行数，多段注明段数，其余只写字数。
pub(crate) fn size_label(text: &str) -> String {
    let table_rows = text
        .lines()
        .filter(|line| {
            crate::draft_page::is_table_source_line(line)
                && !crate::draft_page::is_table_separator_line(line)
        })
        .count();
    if table_rows >= 2 {
        return format!("表格 {table_rows} 行");
    }
    let paragraphs = text
        .split("\n\n")
        .filter(|paragraph| !paragraph.trim().is_empty())
        .count();
    let chars = visible_chars(text);
    if paragraphs > 1 {
        format!("{paragraphs} 段 {chars} 字")
    } else {
        format!("{chars} 字")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NumberingConfig, TemplateKind};

    fn cut(text: &str, picked: &str) -> (String, usize) {
        let start = text.find(picked).unwrap();
        cut_range(text, start..start + picked.len())
    }

    #[test]
    fn cutting_inside_a_line_leaves_the_rest_untouched() {
        let (text, cursor) = cut("甲句。乙句。丙句。\n", "乙句。");
        assert_eq!(text, "甲句。丙句。\n");
        assert_eq!(cursor, "甲句。".len());
    }

    #[test]
    fn cutting_a_whole_paragraph_merges_the_blank_lines() {
        let source = "第一段。\n\n第二段。\n\n第三段。\n";
        // 三击选段会连着行尾换行一起选中。
        let (text, cursor) = cut(source, "第二段。\n");
        assert_eq!(text, "第一段。\n\n第三段。\n");
        assert_eq!(&text[cursor..], "第三段。\n");
        let (text, _) = cut(source, "第二段。");
        assert_eq!(text, "第一段。\n\n第三段。\n");
    }

    #[test]
    fn cutting_at_the_edges_leaves_no_stray_blank_lines() {
        let (text, cursor) = cut("第一段。\n\n第二段。\n", "第一段。");
        assert_eq!(text, "第二段。\n");
        assert_eq!(cursor, 0);
        let (text, _) = cut("第一段。\n\n第二段。\n", "第二段。\n");
        assert_eq!(text, "第一段。\n");
        let (text, _) = cut("第一段。\n\n第二段。", "\n\n第二段。");
        assert_eq!(text, "第一段。");
    }

    #[test]
    fn candidate_text_keeps_inner_line_breaks_only() {
        assert_eq!(
            clean_candidate_text("\n\n甲。\n\n乙。  \n\n"),
            "甲。\n\n乙。"
        );
    }

    #[test]
    fn section_is_the_top_level_heading_with_the_nearest_subheading() {
        let markdown = "# 关于某事的报告\n\n开头一段。\n\n## 主要做法\n\n### 强化统筹\n\n正文甲。\n\n## 下一步打算\n\n正文乙。\n";
        let entries = navigator::collect_entries(
            markdown,
            &NumberingConfig::default(),
            TemplateKind::PlainDocument,
        );
        let at = |picked: &str| section_at(&entries, markdown.find(picked).unwrap());
        assert_eq!(
            at("开头一段"),
            (String::new(), String::new(), String::new())
        );
        assert_eq!(
            at("正文甲"),
            (
                "主要做法".into(),
                "一、主要做法".into(),
                "（一）强化统筹".into()
            )
        );
        assert_eq!(
            at("正文乙"),
            ("下一步打算".into(), "二、下一步打算".into(), String::new())
        );
    }

    /// 在 `text` 里点 `at` 这个字，返回选中的那一句。
    fn sentence(text: &str, at: &str) -> String {
        let byte = text.find(at).unwrap();
        let index = text[..byte].chars().count();
        let range = sentence_range(text, index);
        text.chars()
            .skip(range.start)
            .take(range.end - range.start)
            .collect()
    }

    #[test]
    fn double_click_selects_the_whole_sentence_past_commas() {
        let text = "成立工作专班，每月召开一次调度会。各单位确定一名联络员，负责信息报送。专班办公室设在综合处";
        assert_eq!(sentence(text, "每月"), "成立工作专班，每月召开一次调度会。");
        assert_eq!(
            sentence(text, "联络员"),
            "各单位确定一名联络员，负责信息报送。"
        );
        // 最后一句没有句号，也选到段尾。
        assert_eq!(sentence(text, "综合处"), "专班办公室设在综合处");
        // 点在句号上算这一句。
        assert_eq!(sentence(text, "。各"), "成立工作专班，每月召开一次调度会。");
    }

    #[test]
    fn sentence_keeps_closing_quotes_and_stops_at_line_breaks() {
        let text = "他说：“要抓紧落实。”随后散会。\n  第二段第一句；第二段第二句。";
        assert_eq!(sentence(text, "抓紧"), "他说：“要抓紧落实。”");
        assert_eq!(sentence(text, "”随"), "他说：“要抓紧落实。”");
        assert_eq!(sentence(text, "散会"), "随后散会。");
        // 段首的缩进空白不算进去，分号也断句。
        assert_eq!(sentence(text, "第一句"), "第二段第一句；");
        assert_eq!(sentence(text, "第二句"), "第二段第二句。");
    }

    #[test]
    fn taking_a_part_keeps_the_rest_tidy() {
        let text = "甲句。乙句。丙句。";
        let start = text.find("乙句。").unwrap();
        assert_eq!(
            take_part(text, start..start + "乙句。".len()),
            "甲句。丙句。"
        );

        let text = "第一段。\n\n第二段。\n\n第三段。";
        let start = text.find("第二段。\n").unwrap();
        assert_eq!(
            take_part(text, start..start + "第二段。\n".len()),
            "第一段。\n\n第三段。"
        );
        let start = text.find("第三段。").unwrap();
        assert_eq!(take_part(text, start..text.len()), "第一段。\n\n第二段。");
        assert_eq!(take_part(text, 0..text.len()), "");
    }

    fn item(key: u64, text: &str, section: &str, label: &str) -> Candidate {
        Candidate {
            key,
            record: CandidateRecord {
                text: text.into(),
                section: section.into(),
                section_label: label.into(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn groups_follow_the_current_document_order() {
        let items = vec![
            item(1, "甲", "下一步打算", "三、下一步打算"),
            item(2, "乙", "已删小节", "二、已删小节"),
            item(3, "丙", "主要做法", "一、主要做法"),
            item(4, "丁", "", ""),
            item(5, "戊", "下一步打算", "三、下一步打算"),
        ];
        // 正文里「已删小节」没了，「下一步打算」重新编成了「二、」。
        let tops = vec![
            ("主要做法".to_string(), "一、主要做法".to_string()),
            ("下一步打算".to_string(), "二、下一步打算".to_string()),
        ];
        let groups = group_candidates(&items, &tops);
        let summary = groups
            .iter()
            .map(|group| (group.label.as_str(), group.missing, group.members.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            vec![
                ("开头", false, vec![3]),
                ("一、主要做法", false, vec![2]),
                ("二、下一步打算", false, vec![0, 4]),
                ("二、已删小节", true, vec![1]),
            ]
        );
    }

    #[test]
    fn size_label_counts_tables_paragraphs_and_characters() {
        assert_eq!(size_label("成立工作专班，每月调度。"), "12 字");
        assert_eq!(size_label("第一段。\n\n第二段。"), "2 段 8 字");
        assert_eq!(
            size_label("| 序号 | 名称 |\n| --- | --- |\n| 1 | 甲 |\n| 2 | 乙 |"),
            "表格 3 行"
        );
    }
}
