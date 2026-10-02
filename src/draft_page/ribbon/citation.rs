//! 「研报」分区的「文献引用」下拉。
//!
//! 版式照交叉引用下拉：两个筛选组（已引 / 未引）带个数、记住上次停在哪组；每行粗体
//! 序号、作者年份、淡色题名、右侧淡色文献类型；悬停给出参考文献表里的整条著录。
//! 序号就是 PDF 印的号：取自预览第一遍（GB/T 7714 顺序编码制，按首次引用先后），
//! 著录取自 `export::bibliography`（与 Typst 同一个 hayagriva、同一份样式）。
//!
//! 单击一行插入并收起；勾选或 Ctrl+单击攒成一组，底部「插入」写成 `[@a; @b]`。
//! 光标在已有的 `[@…]` 里或紧挨在它后面时，选中的并进那一组。

use crate::draft_page::bib_marks::entry_card;
use crate::export::bibliography::{BibEntry, Library};
use crate::export::crossref::{self, CitedKey};
use crate::theme;
use eframe::egui;
use egui::AtomExt;

/// 下拉里选了什么。
pub(super) enum CitationPick {
    /// 插入这些键（按顺序）。
    Insert(Vec<String>),
    /// 还没有文献库：打开文件框导入 `.bib`。
    Import,
}

/// 两个筛选组。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Group {
    Cited,
    Uncited,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Self::Cited => "已引",
            Self::Uncited => "未引",
        }
    }
}

/// 下拉的临时状态：搜索词、勾选、键盘高亮行。收起再展开时清空，筛选组另记、不清。
#[derive(Clone, Default)]
struct MenuState {
    search: String,
    checked: Vec<String>,
    highlight: usize,
    /// 上一次画这个下拉是第几遍（`cumulative_pass_nr`），隔了一遍以上就是重新展开。
    last_pass: u64,
}

/// 下拉里的一行：一条文献，或正文引了、文献库里却没有的键。
struct Row<'a> {
    key: &'a str,
    number: Option<usize>,
    uses: usize,
    entry: Option<&'a BibEntry>,
}

impl Row<'_> {
    fn missing(&self) -> bool {
        self.entry.is_none()
    }
}

/// 画下拉内容，返回选中的动作。`cursor` 是编辑框光标（字节）。
pub(super) fn citation_menu(
    ui: &mut egui::Ui,
    markdown: &str,
    library: &Library,
    cited: &[CitedKey],
    cursor: Option<usize>,
) -> Option<CitationPick> {
    ui.set_min_width(440.0);
    ui.set_max_width(440.0);
    if library.entries.is_empty() {
        ui.weak("还没有文献库。导入 .bib 后，这里按作者、题名、年份列出全部文献。");
        ui.add_space(4.0);
        if ui
            .add(theme::icon_text_button(theme::Icon::FileUp, "导入 .bib…"))
            .clicked()
        {
            return Some(CitationPick::Import);
        }
        return None;
    }

    let state_id = egui::Id::new("research-citation-menu");
    let group_id = egui::Id::new("research-citation-group");
    let pass = ui.ctx().cumulative_pass_nr();
    let mut state: MenuState = ui.data(|data| data.get_temp(state_id)).unwrap_or_default();
    let fresh = pass > state.last_pass + 1 || state.last_pass == 0;
    if fresh {
        state = MenuState::default();
    }
    state.last_pass = pass;

    // 光标所在的那组引用：选中的并进去。
    let around = cursor.and_then(|cursor| crossref::citation_at(markdown, cursor));
    if let Some((_, keys)) = &around {
        let list = keys
            .iter()
            .map(|key| format!("@{key}"))
            .collect::<Vec<_>>()
            .join("; ");
        egui::Frame::new()
            .fill(theme::accent_soft())
            .corner_radius(egui::CornerRadius::same(5))
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    egui::RichText::new(format!("光标在 [{list}] 里：选中的文献并入这一组"))
                        .size(theme::font_sizes::SMALL)
                        .color(theme::accent()),
                );
            });
        ui.add_space(4.0);
    }

    let search = ui.add(theme::field(
        &mut state.search,
        "搜索键、作者、题名、年份",
        f32::INFINITY,
    ));
    if fresh {
        search.request_focus();
    }
    let query = state.search.trim().to_lowercase();

    // 已引按序号，未引按 .bib 里的先后。正文引了、库里没有的键也列在已引里——它照样
    // 占着一个号，后面的号是跟着它排的。
    let rows = |group: Group| -> Vec<Row<'_>> {
        match group {
            Group::Cited => cited
                .iter()
                .map(|cited| Row {
                    key: &cited.key,
                    number: Some(cited.number),
                    uses: cited.uses,
                    entry: library.get(&cited.key),
                })
                .filter(|row| match row.entry {
                    Some(entry) => entry.matches(&query),
                    None => query.is_empty() || row.key.to_lowercase().contains(&query),
                })
                .collect(),
            Group::Uncited => library
                .entries
                .iter()
                .filter(|entry| !cited.iter().any(|cited| cited.key == entry.key))
                .filter(|entry| entry.matches(&query))
                .map(|entry| Row {
                    key: &entry.key,
                    number: None,
                    uses: 0,
                    entry: Some(entry),
                })
                .collect(),
        }
    };
    let cited_rows = rows(Group::Cited);
    let uncited_rows = rows(Group::Uncited);
    let mut group =
        ui.data(|data| data.get_temp::<Group>(group_id))
            .unwrap_or(if cited.is_empty() {
                Group::Uncited
            } else {
                Group::Cited
            });
    ui.add_space(4.0);
    theme::segmented(ui, |ui| {
        for (candidate, count) in [
            (Group::Cited, cited_rows.len()),
            (Group::Uncited, uncited_rows.len()),
        ] {
            let label = format!("{} {count}", candidate.label());
            if ui.selectable_label(group == candidate, label).clicked() && group != candidate {
                group = candidate;
                state.highlight = 0;
            }
        }
    });
    ui.data_mut(|data| data.insert_temp(group_id, group));
    ui.add_space(4.0);

    let (shown, other) = match group {
        Group::Cited => (&cited_rows, &uncited_rows),
        Group::Uncited => (&uncited_rows, &cited_rows),
    };

    // 键盘：↑↓ 挪高亮行，Enter 插入（有勾选时插勾选的那组）。
    let mut moved = false;
    if search.has_focus() && !shown.is_empty() {
        ui.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                state.highlight = (state.highlight + 1).min(shown.len() - 1);
                moved = true;
            }
            if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                state.highlight = state.highlight.saturating_sub(1);
                moved = true;
            }
        });
    }
    state.highlight = state.highlight.min(shown.len().saturating_sub(1));
    let enter = (search.has_focus() || search.lost_focus())
        && ui.input(|input| input.key_pressed(egui::Key::Enter));

    let mut pick = None;
    if shown.is_empty() {
        ui.add_space(6.0);
        ui.weak(match (group, query.is_empty()) {
            (Group::Cited, true) => "正文还没有引用文献。到「未引」里挑一条插进来。".to_string(),
            (Group::Uncited, true) => "文献库里的文献都已引用。".to_string(),
            (_, false) if !other.is_empty() => format!(
                "这一组里没有匹配“{}”的文献；「{}」里有 {} 条。",
                state.search.trim(),
                match group {
                    Group::Cited => Group::Uncited.label(),
                    Group::Uncited => Group::Cited.label(),
                },
                other.len()
            ),
            (_, false) => format!("没有匹配“{}”的文献。", state.search.trim()),
        });
        ui.add_space(6.0);
    } else {
        let width = ui.available_width();
        theme::popup_scroll(320.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (index, row) in shown.iter().enumerate() {
                    let mut checked = state.checked.iter().any(|key| key == row.key);
                    let was = checked;
                    let response = ui
                        .horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.add(egui::Checkbox::without_text(&mut checked))
                                .on_hover_text("勾选几条，在底部一起插成一组");
                            let row_width = ui.available_width().min(width);
                            let text_height = ui.text_style_height(&egui::TextStyle::Button);
                            ui.add(citation_row(
                                row,
                                index == state.highlight,
                                row_width,
                                text_height,
                            ))
                        })
                        .inner;
                    let response = response.on_hover_ui(|ui| row_card(ui, row));
                    if moved && index == state.highlight {
                        response.scroll_to_me(None);
                    }
                    if response.clicked() {
                        if ui.input(|input| input.modifiers.command) {
                            checked = !checked;
                        } else {
                            pick = Some(CitationPick::Insert(vec![row.key.to_string()]));
                        }
                    }
                    if checked != was {
                        if checked {
                            state.checked.push(row.key.to_string());
                        } else {
                            state.checked.retain(|key| key != row.key);
                        }
                    }
                }
            });
    }
    if enter && pick.is_none() {
        pick = if state.checked.is_empty() {
            shown
                .get(state.highlight)
                .map(|row| CitationPick::Insert(vec![row.key.to_string()]))
        } else {
            Some(CitationPick::Insert(state.checked.clone()))
        };
    }

    if !state.checked.is_empty() {
        ui.separator();
        ui.horizontal(|ui| {
            let list = state
                .checked
                .iter()
                .map(|key| format!("@{key}"))
                .collect::<Vec<_>>()
                .join("; ");
            if ui
                .add(theme::icon_text_button(theme::Icon::Quote, "插入"))
                .on_hover_text("按勾选的先后插成一组；回车也行")
                .clicked()
            {
                pick = Some(CitationPick::Insert(state.checked.clone()));
            }
            if ui.add(egui::Link::new(small("清空"))).clicked() {
                state.checked.clear();
            }
            ui.add(
                egui::Label::new(
                    small(&format!("已选 {} 条 · [{list}]", state.checked.len()))
                        .color(theme::text_soft()),
                )
                .truncate(),
            );
        });
    }

    diagnostics(ui, markdown, library, cited, group);

    if pick.is_some() {
        state = MenuState::default();
    }
    ui.data_mut(|data| data.insert_temp(state_id, state));
    pick
}

/// 底部提示：缺键、`.bib` 有错、没有参考文献区段、未引的不进文献表。
fn diagnostics(
    ui: &mut egui::Ui,
    markdown: &str,
    library: &Library,
    cited: &[CitedKey],
    group: Group,
) {
    let missing: Vec<&str> = cited
        .iter()
        .filter(|cited| !library.contains(&cited.key))
        .map(|cited| cited.key.as_str())
        .collect();
    let mut notes: Vec<(String, bool)> = Vec::new();
    if !missing.is_empty() {
        notes.push((
            format!(
                "正文有 {} 个键不在文献库里，会印成 [?]，导出 PDF 会中止：{}",
                missing.len(),
                missing.join("、")
            ),
            true,
        ));
    }
    if let Some(problem) = &library.problem {
        notes.push((
            format!(
                "BibTeX 第 {} 行：{}。导出 PDF 会中止，改好后到文档要素重新读入。",
                problem.line, problem.message
            ),
            true,
        ));
    }
    if !cited.is_empty()
        && !markdown.lines().any(|line| {
            crate::export::parse_research_marker(line)
                == Some(crate::export::ResearchSection::References)
        })
    {
        notes.push((
            "稿中没有 <!-- [参考文献] -->，文献表会排在全文末尾。".to_string(),
            false,
        ));
    }
    if group == Group::Uncited {
        notes.push(("未引用的文献不进参考文献表。".to_string(), false));
    }
    if notes.is_empty() {
        return;
    }
    ui.separator();
    for (note, warning) in notes {
        let text = small(&note);
        ui.label(if warning {
            text.color(theme::warn())
        } else {
            text.color(theme::text_muted())
        });
    }
}

fn small(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(theme::font_sizes::SMALL)
}

/// 一行：粗体序号（未引是淡色“—”）、作者与年份、淡色题名（过长截断）、右侧淡色类型。
/// 正文引了、库里没有的键整行橙色。
fn citation_row(
    row: &Row<'_>,
    highlighted: bool,
    width: f32,
    text_height: f32,
) -> egui::Button<'static> {
    let mut atoms = egui::Atoms::default();
    let number = match row.number {
        Some(number) if row.missing() => egui::RichText::new(format!("[{number}]"))
            .strong()
            .color(theme::warn()),
        Some(number) => egui::RichText::new(format!("[{number}]")).strong(),
        None => egui::RichText::new("—").weak(),
    };
    atoms.push_right(
        number
            .atom_size(egui::vec2(30.0, text_height))
            .atom_align(egui::Align2::LEFT_CENTER),
    );
    let (lead, title, kind) = match row.entry {
        Some(entry) => {
            let lead = match (entry.author.is_empty(), entry.year.is_empty()) {
                (false, false) => format!("{} {}", entry.author, entry.year),
                (false, true) => entry.author.clone(),
                (true, false) => entry.year.clone(),
                // 解析不了的库只剩键可看。
                (true, true) => entry.key.clone(),
            };
            (
                egui::RichText::new(lead),
                egui::RichText::new(entry.title.clone()).weak(),
                egui::RichText::new(entry.kind).weak().small(),
            )
        }
        None => (
            egui::RichText::new(row.key.to_string()).color(theme::warn()),
            egui::RichText::new("文献库里没有这一条").color(theme::warn()),
            egui::RichText::new("缺失").color(theme::warn()).small(),
        ),
    };
    atoms.push_right(lead.atom_max_width(width * 0.42));
    atoms.push_right(title.atom_shrink(true));
    egui::Button::new(atoms)
        .right_text(kind)
        .truncate()
        .selected(highlighted)
        .frame_when_inactive(highlighted)
        .corner_radius(egui::CornerRadius::same(5))
        .min_size(egui::vec2(width, 26.0))
}

/// 悬停一行时的卡片：参考文献表里那一条、引用处数、源码写法。
fn row_card(ui: &mut egui::Ui, row: &Row<'_>) {
    ui.set_max_width(360.0);
    entry_card(ui, row.key, row.number, row.uses, row.entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::bibliography;
    use crate::models::FontConfig;

    const BIB: &str = "@article{wang2020, author={王明 and 李华}, title={数字政府建设的制度逻辑与实践路径研究综述}, journal={中国行政管理}, year={2020}}\n@book{smith2019, author={Smith, John}, title={Public Administration}, publisher={Springer}, year={2019}}\n@phdthesis{zhang2019, author={张华}, title={政务信息资源共享研究}, school={某大学}, year={2019}}\n";

    /// 在一帧里画出下拉，返回每段文字的（内容，左缘，右缘，纵坐标）。
    fn menu_texts(
        markdown: &str,
        bib: &str,
        cursor: Option<usize>,
    ) -> Vec<(String, f32, f32, f32)> {
        let ctx = egui::Context::default();
        theme::configure_fonts(&ctx, &FontConfig::default());
        let library = bibliography::parse(bib);
        let cited = crate::preview::research_citations(&ctx, markdown);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 800.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            let _ = citation_menu(ui, markdown, &library, &cited, cursor);
        });
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    text.pos.x,
                    text.pos.x + text.galley.size().x,
                    text.pos.y,
                )),
                _ => None,
            })
            .collect()
    }

    fn find<'a>(texts: &'a [(String, f32, f32, f32)], text: &str) -> &'a (String, f32, f32, f32) {
        texts
            .iter()
            .find(|(t, ..)| t == text)
            .unwrap_or_else(|| panic!("没画出「{text}」：{texts:?}"))
    }

    /// 已引一组按 PDF 序号排；库里没有的键也占一个号，标成缺失；长题名截断在类型之前。
    #[test]
    fn cited_rows_show_pdf_numbers_and_missing_keys_take_their_number() {
        let markdown =
            "<!-- [正文] -->\n\n## 背景\n\n先引[@zhang2019]，再引[@nope]与[@wang2020]。\n";
        let texts = menu_texts(markdown, BIB, None);
        for tab in ["已引 3", "未引 1"] {
            find(&texts, tab);
        }
        let first = find(&texts, "[1]").3;
        let second = find(&texts, "[2]").3;
        let third = find(&texts, "[3]").3;
        assert!(first < second && second < third, "{texts:?}");
        let same_row = |a: f32, b: f32| (a - b).abs() < 1.0;
        assert!(
            same_row(find(&texts, "张华 2019").3, first),
            "[1] 是张华：{texts:?}"
        );
        assert!(
            same_row(find(&texts, "nope").3, second),
            "缺失的键占 [2]：{texts:?}"
        );
        assert!(same_row(find(&texts, "王明 等 2020").3, third), "{texts:?}");
        let title = texts
            .iter()
            .find(|(t, ..)| t.starts_with("数字政府"))
            .expect("题名");
        assert!(title.2 <= find(&texts, "期刊").1, "{texts:?}");
        assert!(
            texts
                .iter()
                .any(|(t, ..)| t.starts_with("正文有 1 个键不在文献库里")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|(t, ..)| t.contains("文献表会排在全文末尾")),
            "{texts:?}"
        );
    }

    #[test]
    fn cursor_inside_a_citation_announces_the_merge() {
        let markdown = "<!-- [正文] -->\n\n见[@wang2020]。\n";
        let cursor = markdown.find("2020]").unwrap();
        let texts = menu_texts(markdown, BIB, Some(cursor));
        assert!(
            texts
                .iter()
                .any(|(t, ..)| t.starts_with("光标在 [@wang2020] 里")),
            "{texts:?}"
        );
    }

    #[test]
    fn without_a_library_the_menu_offers_an_import() {
        let texts = menu_texts("正文", "", None);
        find(&texts, "导入 .bib…");
    }
}
