//! 花脸稿：把两版正文合成一份带增删标记的 Markdown，交给现有导出器出 PDF / Word。
//!
//! 花脸稿是送签用的纸：**删掉的字留在原位画波浪线，新增的字就地套方框**，领导
//! 一眼就能看出这一稿动了哪儿。它和屏幕上的版本对照是两回事——对照是左右分栏，
//! 各看各的；花脸稿只有一张纸，增删必须交织在同一段里。
//!
//! 实现上不另起一套排版：正文里插 `export::text` 的私用区哨兵，再走**原来的**
//! 导出链路。这样花脸稿的字体、行距、版心、红头、版记与定稿完全一致，也不必为
//! 它维护第二套版式。
//!
//! **标题**（含主标题与各级小标题）刻意不做行内标记：它在导出器里走的是
//! `tex_escape` 而不是正文那条 `body_text_to_tex`，哨兵进去会变成缺字符印在
//! 纸上。标题改动改走末尾的「花脸稿说明」。
//!
//! **表格逐格标记**：整行插标记会把竖线结构拆散，所以按单元格比对，标记只落在
//! 格子内容里，竖线和对齐分隔行原样保留。列数变了就没法逐格对号，那种情况只出
//! 新版并另行说明。
//!
//! 还有一点要清楚：花脸稿比定稿长——删掉的字仍占版面，页数对不上是必然的。

use crate::diff::{ChangeKind, DiffBlock, InlineSpan, SpanKind, body_diff, merged_spans};
use crate::export::{self, mark_added, mark_deleted};
use crate::models::{DraftInput, FontConfig};
use crate::texcompile;
use crate::units::UnitDisplay;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 一份生成好的花脸稿。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedlineDoc {
    /// 带哨兵的 Markdown，直接交给 `export` 的导出器。
    pub markdown: String,
    /// 没法在正文里画出来的改动（标题、表格行），逐条说明。
    pub structural_changes: Vec<String>,
    /// 正文里真正画了标记的段落数。
    pub marked_paragraphs: usize,
}

impl RedlineDoc {
    /// 有没有任何改动。两版完全一致时花脸稿没有意义，调用方据此提示用户。
    pub fn is_empty(&self) -> bool {
        self.structural_changes.is_empty() && self.marked_paragraphs == 0
    }
}

/// 生成花脸稿。`old` 是旧版正文，`new` 是新版正文，方向与版本对照一致。
pub fn build(old: &str, new: &str) -> RedlineDoc {
    let diff = body_diff(old, new);
    let mut lines: Vec<String> = Vec::new();
    let mut structural_changes = Vec::new();
    let mut marked_paragraphs = 0usize;

    for block in &diff.blocks {
        match block {
            DiffBlock::Unchanged(context) => {
                for item in context {
                    lines.push(item.text.clone());
                }
            }
            DiffBlock::Changed(change) => {
                let before = join_spans(&change.before_spans);
                let after = join_spans(&change.after_spans);
                match change.kind {
                    ChangeKind::Insert => {
                        push_whole(&mut lines, &mut structural_changes, &after, true);
                        if !is_structural(&after) {
                            marked_paragraphs += 1;
                        }
                    }
                    ChangeKind::Delete => {
                        push_whole(&mut lines, &mut structural_changes, &before, false);
                        if !is_structural(&before) {
                            marked_paragraphs += 1;
                        }
                    }
                    ChangeKind::Replace => {
                        if is_table_row(&after) && is_table_row(&before) {
                            // 表格逐格比对：整行插标记会把竖线结构拆散，逐格插就
                            // 安全，而且改掉的时限、数字正是最该让人看见的地方。
                            match merge_table_row(&before, &after) {
                                Some(row) => {
                                    lines.push(row);
                                    marked_paragraphs += 1;
                                }
                                None => {
                                    // 列数变了，逐格对不上号，只出新版并说明。
                                    lines.push(after.clone());
                                    structural_changes.push(format!(
                                        "表格行列数有变动：「{}」改为「{}」",
                                        trimmed(&before),
                                        trimmed(&after)
                                    ));
                                }
                            }
                        } else if is_structural(&after) || is_structural(&before) {
                            // 标题只出新版，改动另行说明。
                            lines.push(after.clone());
                            structural_changes.push(format!(
                                "{}：「{}」改为「{}」",
                                kind_label(&after),
                                trimmed(&before),
                                trimmed(&after)
                            ));
                        } else {
                            let (prefix, before_body) = split_prefix(&before);
                            let (_, after_body) = split_prefix(&after);
                            let merged = render_marked(&merged_spans(before_body, after_body));
                            lines.push(format!("{prefix}{merged}"));
                            marked_paragraphs += 1;
                        }
                    }
                }
            }
        }
    }

    RedlineDoc {
        markdown: join_lines(&lines),
        structural_changes,
        marked_paragraphs,
    }
}

/// 把逐块产出的行拼回文档。
///
/// 不能一律用空行拼：表格的各行必须紧挨着，中间夹一个空行，GFM 表格就散成
/// 几段普通文字了。规则很简单——相邻两行都是表格行就用单换行，其余用空行。
fn join_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            let previous_is_row = is_table_row(&lines[index - 1]);
            out.push_str(if previous_is_row && is_table_row(line) {
                "\n"
            } else {
                "\n\n"
            });
        }
        out.push_str(line);
    }
    out
}

/// 整段新增或整段删除。标题与表格行不画标记，只记进清单。
fn push_whole(lines: &mut Vec<String>, changes: &mut Vec<String>, text: &str, added: bool) {
    let action = if added { "新增" } else { "删除" };
    if is_structural(text) {
        if added {
            // 新增的标题照排，读者需要它作为上下文。
            lines.push(text.to_string());
        }
        // 删除的标题不能照排——排出来看着像还在。只记一条说明。
        changes.push(format!(
            "{}{}：「{}」",
            action,
            kind_label(text),
            trimmed(text)
        ));
        return;
    }
    let (prefix, body) = split_prefix(text);
    let marked = if added {
        mark_added(body)
    } else {
        mark_deleted(body)
    };
    lines.push(format!("{prefix}{marked}"));
}

/// 把合流片段渲染成带哨兵的文本。
fn render_marked(spans: &[InlineSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        match span.kind {
            SpanKind::Same => out.push_str(&span.text),
            SpanKind::Removed => out.push_str(&mark_deleted(&span.text)),
            SpanKind::Added => out.push_str(&mark_added(&span.text)),
        }
    }
    out
}

fn join_spans(spans: &[InlineSpan]) -> String {
    spans.iter().map(|span| span.text.as_str()).collect()
}

/// 标题行与表格行不能整行加标记：标题在导出器里走 `tex_escape`，表格行的竖线
/// 是结构。表格另有逐格处理，见 [`merge_table_row`]。
fn is_structural(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('#') || trimmed.starts_with('|')
}

fn is_table_row(text: &str) -> bool {
    text.trim_start().starts_with('|')
}

/// GFM 的对齐分隔行（`| --- | :-: |`）。它是结构，不是内容，一个字都不能动。
fn is_separator_row(text: &str) -> bool {
    let body = text.trim().trim_start_matches('|').trim_end_matches('|');
    !body.trim().is_empty()
        && body.split('|').all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':'))
        })
}

/// 表格行逐格比对。列数不同返回 `None`，交给调用方降级处理。
///
/// 标记只落在单元格内容里，竖线和分隔行原样保留，产出的仍是合法 GFM 表格。
fn merge_table_row(before: &str, after: &str) -> Option<String> {
    if is_separator_row(after) || is_separator_row(before) {
        return Some(after.to_string());
    }
    let old_cells = split_cells(before);
    let new_cells = split_cells(after);
    if old_cells.len() != new_cells.len() {
        return None;
    }
    let merged = old_cells
        .iter()
        .zip(new_cells.iter())
        .map(|(old, new)| {
            if old == new {
                (*new).to_string()
            } else {
                // 单元格内容左右的空格属于版式，摘出来再拼回去，免得标记把
                // 对齐用的空格也包进去。
                let trimmed_old = old.trim();
                let trimmed_new = new.trim();
                let marked = render_marked(&merged_spans(trimmed_old, trimmed_new));
                format!(" {marked} ")
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    Some(format!("|{merged}|"))
}

/// 按竖线切出单元格内容，丢掉首尾那对边框竖线。
fn split_cells(row: &str) -> Vec<&str> {
    let body = row.trim();
    let body = body.strip_prefix('|').unwrap_or(body);
    let body = body.strip_suffix('|').unwrap_or(body);
    body.split('|').collect()
}

fn kind_label(text: &str) -> &'static str {
    if text.trim_start().starts_with('|') {
        "表格行"
    } else {
        "标题"
    }
}

fn trimmed(text: &str) -> String {
    let (_, body) = split_prefix(text);
    body.trim().to_string()
}

/// 切出行首的 Markdown 前缀（标题号、项目符号、有序号），标记只加在其后的正文上。
///
/// 前缀留在标记外面，产出的仍是合法 Markdown——否则 `#` 前面多个哨兵，这一行
/// 就不再被当成标题了。
fn split_prefix(text: &str) -> (&str, &str) {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    // 行首空白
    while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
        index += 1;
    }
    let after_space = index;
    // 标题号
    while index < bytes.len() && bytes[index] == b'#' {
        index += 1;
    }
    if index > after_space {
        while index < bytes.len() && bytes[index] == b' ' {
            index += 1;
        }
        return text.split_at(index);
    }
    // 项目符号
    if index < bytes.len() && matches!(bytes[index], b'-' | b'*' | b'+') {
        let marker = index;
        index += 1;
        if index < bytes.len() && bytes[index] == b' ' {
            while index < bytes.len() && bytes[index] == b' ' {
                index += 1;
            }
            return text.split_at(index);
        }
        index = marker;
    }
    // 有序号 `1. `
    let digits_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index > digits_start && index < bytes.len() && bytes[index] == b'.' {
        index += 1;
        if index < bytes.len() && bytes[index] == b' ' {
            while index < bytes.len() && bytes[index] == b' ' {
                index += 1;
            }
            return text.split_at(index);
        }
    }
    text.split_at(after_space)
}

/// 要导出哪些格式。至少选一个，调用方保证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedlineFormats {
    pub pdf: bool,
    pub docx: bool,
}

/// 花脸稿正文末尾附的说明段前缀。标题、表格这类没法在正文里画的改动落在这里，
/// 领导拿到纸也能知道除了画出来的地方还动了什么。
const NOTE_PREFIX: &str = "花脸稿说明：";

/// 生成花脸稿文件。返回写出的文件路径。
///
/// 复用定稿那条导出链路：同一个 `.cls`、同一套字体和版心，所以花脸稿看起来就是
/// 这份公文本身，只是多了增删标记。唯一的不同是文件名带 `-花脸稿`，且落在自己的
/// 子目录里，不会和定稿混在一起被误发。
pub fn export_files(
    output_dir: &Path,
    input: &DraftInput,
    doc: &RedlineDoc,
    formats: RedlineFormats,
    display: &UnitDisplay,
    fonts: &FontConfig,
) -> Result<Vec<PathBuf>> {
    let markdown = doc.markdown_with_notes();
    // 标题取自正文 H1，哨兵已在生成时避开标题，这里再兜一层底：文件名里绝不能
    // 出现私用区字符。
    let title = export::strip_redline(&export::extract_title(&markdown, &input.title_hint));
    let stem = format!("{}-花脸稿", export::safe_filename(&title));
    let dir = output_dir.join(&stem);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("无法创建花脸稿目录：{}", dir.display()))?;

    let mut files = Vec::new();
    if formats.docx {
        let path = dir.join(format!("{stem}.docx"));
        export::write_docx(&path, input, &markdown, display)?;
        files.push(path);
    }
    if formats.pdf {
        let tex = dir.join(format!("{stem}.tex"));
        export::write_tex(&tex, input, &markdown, display, fonts)?;
        files.push(tex.clone());
        let outcome = texcompile::compile_pdf_with_proof(&tex, fonts)
            .with_context(|| "花脸稿 PDF 编译失败".to_string())?;
        if let Some(pdf) = outcome.pdf {
            files.push(pdf);
        }
    }
    Ok(files)
}

impl RedlineDoc {
    /// 正文 + 末尾的改动说明段。
    fn markdown_with_notes(&self) -> String {
        if self.structural_changes.is_empty() {
            return self.markdown.clone();
        }
        let notes = self
            .structural_changes
            .iter()
            .map(|change| format!("{NOTE_PREFIX}{change}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        format!("{}\n\n{notes}", self.markdown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::strip_redline;

    /// 把哨兵换成好读的记号，断言起来直观：删除 ~…~、新增 [
    /// …]。
    fn readable(text: &str) -> String {
        use crate::export::{
            REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN,
        };
        text.chars()
            .map(|ch| match ch {
                REDLINE_DEL_OPEN | REDLINE_DEL_CLOSE => "~".to_string(),
                REDLINE_ADD_OPEN => "[".to_string(),
                REDLINE_ADD_CLOSE => "]".to_string(),
                other => other.to_string(),
            })
            .collect()
    }

    #[test]
    fn an_unchanged_document_produces_no_marks() {
        let text = "第一段。\n\n第二段。";
        let doc = build(text, text);
        assert!(doc.is_empty());
        assert!(!doc.markdown.contains('\u{E000}'));
        assert_eq!(doc.markdown, text);
    }

    #[test]
    fn a_rewritten_paragraph_interleaves_both_versions() {
        let doc = build(
            "同意你单位关于报送情况的请示。",
            "同意你单位关于开展检查的请示。",
        );
        assert_eq!(
            readable(&doc.markdown),
            "同意你单位关于~报送情况~[开展检查]的请示。"
        );
        assert_eq!(doc.marked_paragraphs, 1);
        assert!(doc.structural_changes.is_empty());
    }

    #[test]
    fn stripping_the_marks_gives_back_clean_text() {
        // 花脸稿去掉标记后应当是"旧稿 + 新稿"混排，但至少不能残留哨兵字符：
        // 它们一旦漏进标题或文件名，纸上就是缺字符。
        let doc = build("原来的表述。", "修改后的表述。");
        let clean = strip_redline(&doc.markdown);
        assert!(!clean.contains('\u{E000}'));
        assert!(!clean.contains('\u{E003}'));
    }

    #[test]
    fn a_whole_new_paragraph_is_boxed() {
        let doc = build("第一段。", "第一段。\n\n新增的第二段。");
        assert_eq!(readable(&doc.markdown), "第一段。\n\n[新增的第二段。]");
    }

    #[test]
    fn a_deleted_paragraph_stays_on_the_page_with_a_wave() {
        let doc = build("第一段。\n\n多余的第二段。", "第一段。");
        assert_eq!(readable(&doc.markdown), "第一段。\n\n~多余的第二段。~");
    }

    #[test]
    fn markdown_prefixes_stay_outside_the_marks() {
        // 标记跑到 `- ` 前面，这一行就不再是列表项了。
        let doc = build("- 原来的条目", "- 改过的条目");
        assert!(doc.markdown.starts_with("- "), "前缀必须留在标记外面");
        assert_eq!(readable(&doc.markdown), "- ~原来~[改过]的条目");
    }

    #[test]
    fn heading_changes_go_to_the_change_list_not_inline() {
        // 标题在导出器里走 tex_escape，哨兵进去会印成缺字符。
        let doc = build("## 报送内容\n\n正文。", "## 报送要求\n\n正文。");
        assert!(
            !doc.markdown.contains('\u{E000}') && !doc.markdown.contains('\u{E002}'),
            "标题行不该带标记：{}",
            doc.markdown
        );
        assert!(doc.markdown.contains("## 报送要求"), "应当排出新标题");
        assert_eq!(doc.structural_changes.len(), 1);
        assert!(doc.structural_changes[0].contains("报送内容"));
        assert!(doc.structural_changes[0].contains("报送要求"));
    }

    #[test]
    fn table_cells_are_marked_in_place_and_the_grid_survives() {
        let doc = build(
            "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |",
            "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 9月 |",
        );
        let marked = readable(&doc.markdown);
        // 改掉的那一格就地标记（逐字，比整格更细）；没改的格子原样。
        assert!(marked.contains("~8~[9]月"), "单元格应就地标记：{marked}");
        assert!(marked.contains("| 备案 |"), "未改的格子不该动：{marked}");
        // 表格结构必须还在：竖线、分隔行一个不少。
        assert!(marked.contains("| --- | --- |"), "分隔行不能被动：{marked}");
        // 表格三行必须紧挨着——中间夹空行 GFM 表就散成普通文字了。
        assert_eq!(marked.lines().count(), 3, "不该有空行插进表里：{marked}");
        assert!(marked.lines().all(|line| line.trim().starts_with('|')));
        assert!(
            !marked.contains("~|~") && !marked.contains("[|]"),
            "竖线不能进标记"
        );
    }

    #[test]
    fn an_untouched_table_comes_through_byte_for_byte() {
        // 踩过的坑：所有块一律用空行拼接，表格各行之间就被塞进空行，
        // GFM 表当场散成几段普通文字——连没改过的表也一样。
        let table = "正文一段。\n\n| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |\n\n正文二段。";
        let doc = build(table, table);
        assert!(doc.is_empty());
        assert_eq!(doc.markdown, table, "未改动的表格必须原样过来");
    }

    #[test]
    fn a_table_row_with_a_different_column_count_falls_back_to_the_new_version() {
        // 列数变了逐格对不上号，硬标只会错位，宁可只出新版另附说明。
        let doc = build(
            "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |",
            "| 事项 | 时限 | 责任人 |\n| --- | --- | --- |\n| 备案 | 8月 | 张三 |",
        );
        assert!(!doc.markdown.contains('\u{E000}'));
        assert!(
            doc.structural_changes
                .iter()
                .any(|change| change.contains("列数"))
        );
    }

    #[test]
    fn a_deleted_heading_is_not_printed_as_if_it_remained() {
        let doc = build("## 将被删掉\n\n正文。", "正文。");
        assert!(!doc.markdown.contains("将被删掉"), "删掉的标题不能照排");
        assert!(
            doc.structural_changes
                .iter()
                .any(|change| change.contains("删除标题"))
        );
    }
}

#[cfg(test)]
mod tex_tests {
    use super::*;
    use crate::export::write_tex;
    use crate::models::TemplateProfile;

    /// 端到端：花脸稿 Markdown 走定稿那条导出链路，产出的 TeX 里必须是
    /// `\GwDel` / `\GwAdd`，而不是把哨兵字符原样漏进去——那在纸上就是缺字符。
    #[test]
    fn redline_markdown_becomes_gw_macros_in_tex() {
        let doc = build(
            "# 关于报送情况的函\n\n同意你单位关于报送情况的请示。",
            "# 关于报送情况的函\n\n同意你单位关于开展检查的请示。",
        );
        let mut input = DraftInput {
            kind: crate::models::TemplateKind::PlainDocument,
            ..Default::default()
        };
        input.profile = TemplateProfile::for_kind(input.kind);

        let dir = tempfile::tempdir().expect("临时目录");
        let tex = dir.path().join("花脸稿.tex");
        write_tex(
            &tex,
            &input,
            &doc.markdown_with_notes(),
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .expect("写 TeX");
        let content = std::fs::read_to_string(&tex).expect("读 TeX");

        assert!(content.contains("\\GwDel{"), "缺少删除标记：{content}");
        assert!(content.contains("\\GwAdd{"), "缺少新增标记");
        assert!(
            !content.contains('\u{E000}') && !content.contains('\u{E002}'),
            "哨兵字符漏进了 TeX"
        );
    }

    /// 哨兵绝不能出现在 TeX 里——它是私用区码位，字体里没有字形，印出来是豆腐块。
    ///
    /// 这条把哨兵**硬塞**进标题、各级小标题、表格、列表、附件标题等所有位置，
    /// 包括那些故意不做标记的路径，验证兜底过滤（`tex_escape` / `plain_text` /
    /// 表格列宽）确实拦得住。将来谁新写一条渲染路径忘了处理，这条会先炸。
    #[test]
    fn no_sentinel_can_reach_the_tex_through_any_path() {
        use crate::export::{
            REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN,
        };
        let d0 = REDLINE_DEL_OPEN;
        let d1 = REDLINE_DEL_CLOSE;
        let a0 = REDLINE_ADD_OPEN;
        let a1 = REDLINE_ADD_CLOSE;
        let markdown = format!(
            "# 标题{d0}删{d1}里也塞\n\n## 二级{a0}增{a1}标题\n\n正文{d0}删{d1}与{a0}增{a1}。\n\n             - 列表{d0}删{d1}项\n\n1. 有序{a0}增{a1}项\n\n             | 表头{d0}删{d1} | 姓名 |\n| --- | --- |\n| 单元{a0}增{a1}格 | 李四 |\n\n             <!-- [附件] -->\n\n# 附件{d0}删{d1}标题\n\n附件正文{a0}增{a1}。"
        );

        for kind in [
            crate::models::TemplateKind::PlainDocument,
            crate::models::TemplateKind::OfficialLetter,
            crate::models::TemplateKind::WhitePaper,
            crate::models::TemplateKind::RedHeadApproval,
        ] {
            let input = DraftInput {
                kind,
                profile: TemplateProfile::for_kind(kind),
                ..Default::default()
            };
            let dir = tempfile::tempdir().expect("临时目录");
            let tex = dir.path().join("t.tex");
            write_tex(
                &tex,
                &input,
                &markdown,
                &UnitDisplay::new(&[]),
                &FontConfig::default(),
            )
            .expect("写 TeX");
            let content = std::fs::read_to_string(&tex).expect("读 TeX");
            for (name, ch) in [
                ("删除起", d0),
                ("删除止", d1),
                ("新增起", a0),
                ("新增止", a1),
            ] {
                assert!(!content.contains(ch), "{kind:?} 的 TeX 里漏出了{name}哨兵");
            }
        }
    }

    /// Word 侧同理：哨兵不能进 OOXML。docx 是 zip，直接读整个字节流找码位的
    /// UTF-8 编码就够——XML 里存的就是它。
    #[test]
    fn no_sentinel_can_reach_the_docx() {
        use crate::export::{REDLINE_ADD_OPEN, REDLINE_DEL_OPEN};
        let markdown = format!(
            "# 标题{d}删{c}\n\n正文{a}增{b}与{d}删{c}。\n\n| 表头{d}删{c} | 值 |\n| --- | --- |\n| 格{a}增{b} | 1 |",
            d = REDLINE_DEL_OPEN,
            c = crate::export::REDLINE_DEL_CLOSE,
            a = REDLINE_ADD_OPEN,
            b = crate::export::REDLINE_ADD_CLOSE,
        );
        let input = DraftInput {
            kind: crate::models::TemplateKind::PlainDocument,
            profile: TemplateProfile::for_kind(crate::models::TemplateKind::PlainDocument),
            ..Default::default()
        };
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("t.docx");
        export::write_docx(&path, &input, &markdown, &UnitDisplay::new(&[])).expect("写 docx");

        let bytes = std::fs::read(&path).expect("读 docx");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("解包");
        for index in 0..zip.len() {
            use std::io::Read;
            let mut entry = zip.by_index(index).expect("条目");
            let name = entry.name().to_string();
            let mut buffer = String::new();
            if entry.read_to_string(&mut buffer).is_err() {
                continue; // 二进制条目（图片等）跳过
            }
            for ch in ['\u{E000}', '\u{E001}', '\u{E002}', '\u{E003}'] {
                assert!(!buffer.contains(ch), "{name} 里漏出了哨兵");
            }
        }
    }

    /// 守住波浪线的两个坑，都是实打实踩过的：
    ///
    /// 1. `\CJKunderwave` 默认 `symbol` 是 `\sixly\char58`（lasy6 的波浪字形）。
    ///    lasy6 不在离线 bundle 里，而 `\char58` 在中文字体里是**冒号**——整行
    ///    删除会排成一串冒号。必须显式传 `symbol`。
    /// 2. 波浪单元的降段要先抬到升段终点，否则两段接不上，平铺出来是散撇。
    ///
    /// 类文件是编译进二进制的，这里直接查它的文本。
    #[test]
    fn the_class_keeps_the_wave_wired_correctly() {
        let class = include_str!("../gonghan-gwa.cls");
        assert!(
            class.contains("symbol=\\GwWaveUnit"),
            "\\GwDel 必须显式指定 symbol，否则会退回 lasy6 字形并排成一串冒号"
        );
        assert!(
            class.contains("\\raisebox{\\GwWaveRise}"),
            "波浪单元的降段必须抬升，否则两段接不上"
        );
        // 深度要用 em：ex 是西文 x 高，按它算线会落到字的下缘变成下划线。
        assert!(
            class.contains("\\newcommand{\\GwWaveDepth}{-0.5em}"),
            "波浪深度应为 -0.5em（穿过字身中部）"
        );
    }

    /// 反向保证：没有花脸稿标记的普通稿件，产出的 TeX 里不该出现这两个宏。
    /// 这条守着"改动不影响存量导出"。
    #[test]
    fn an_ordinary_document_gains_no_redline_macros() {
        let mut input = DraftInput {
            kind: crate::models::TemplateKind::PlainDocument,
            ..Default::default()
        };
        input.profile = TemplateProfile::for_kind(input.kind);

        let dir = tempfile::tempdir().expect("临时目录");
        let tex = dir.path().join("定稿.tex");
        write_tex(
            &tex,
            &input,
            "# 普通公文\n\n这是一段普通正文，没有任何增删标记。",
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .expect("写 TeX");
        let content = std::fs::read_to_string(&tex).expect("读 TeX");
        assert!(!content.contains("\\GwDel"));
        assert!(!content.contains("\\GwAdd"));
    }
}
