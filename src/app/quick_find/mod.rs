//! 全局快捷查找：只打开已有对象或页面，不直接修改正文与行文要素。

mod index;
mod ui;
mod view;

pub(super) use ui::search_id;

use super::{GongwenApp, ManuscriptFilter, NavPage};
use crate::draft_page::DocKey;
use crate::models::ManuscriptStatus;
use eframe::egui;
use index::Entry;
use nucleo_matcher::{Config, Matcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Document(DocKey),
    Manuscript(i64),
    Vocabulary(u64),
    Page(NavPage),
    NewDocument,
}

pub(super) struct QuickFind {
    entries: Vec<Entry>,
    query: String,
    ranked_query: String,
    results: Vec<usize>,
    selected: usize,
    matcher: Matcher,
    focus_search: bool,
    pub(super) previous_focus: Option<egui::Id>,
    composing: bool,
    warning: Option<String>,
}

impl QuickFind {
    fn new(entries: Vec<Entry>, warning: Option<String>) -> Self {
        Self {
            entries,
            query: String::new(),
            ranked_query: String::new(),
            results: Vec::new(),
            selected: 0,
            matcher: Matcher::new(Config::DEFAULT),
            focus_search: true,
            previous_focus: None,
            composing: false,
            warning,
        }
    }

    fn refresh_results(&mut self) {
        if self.query == self.ranked_query {
            return;
        }
        self.results = if self.query.trim().is_empty() {
            Vec::new()
        } else {
            index::rank(&self.entries, &self.query, &mut self.matcher)
        };
        self.results.truncate(ui::MAX_RESULTS);
        self.ranked_query.clone_from(&self.query);
        self.selected = 0;
    }

    fn selected_target(&self) -> Option<Target> {
        let index = *self.results.get(self.selected)?;
        Some(self.entries.get(index)?.target)
    }
}

impl GongwenApp {
    pub(crate) fn open_quick_find(&mut self) {
        let mut entries = Vec::new();
        // 打开的工作稿排在前面，查找的是当前标题，不是上一次落库的旧标题。
        for doc in &self.docs {
            let number = if doc.draft.kind.has_document_number()
                && !doc.draft.profile.document_number.trim().is_empty()
            {
                let year = doc.draft.document_year();
                if year.is_empty() {
                    format!(
                        "{}{}号",
                        doc.draft.profile.department_code.trim(),
                        doc.draft.profile.document_number.trim()
                    )
                } else {
                    format!(
                        "{}〔{}〕{}号",
                        doc.draft.profile.department_code.trim(),
                        year,
                        doc.draft.profile.document_number.trim()
                    )
                }
            } else {
                doc.draft.kind.label().to_string()
            };
            entries.push(
                Entry::new(
                    doc.title(),
                    format!("{} · {}", number, doc.record_status.label()),
                    &format!(
                        "{} {} {}",
                        doc.draft.profile.department_code,
                        doc.draft.document_year(),
                        doc.draft.profile.document_number
                    ),
                    Target::Document(doc.key),
                )
                .with_badge(
                    if matches!(
                        doc.record_status,
                        ManuscriptStatus::Published | ManuscriptStatus::Archived
                    ) {
                        "只读"
                    } else {
                        "已打开"
                    },
                ),
            );
        }
        let warning = match self.manuscript_store.as_ref() {
            Some(store) => match store.search_rows() {
                Ok(rows) => {
                    for row in rows {
                        if self
                            .docs
                            .iter()
                            .any(|doc| doc.manuscript_id == Some(row.id))
                        {
                            continue;
                        }
                        let number = if row.kind.has_document_number() {
                            row.number_label()
                        } else {
                            row.kind.label().to_string()
                        };
                        let title = if row.title.trim().is_empty() {
                            "未命名公文".to_string()
                        } else {
                            row.title
                        };
                        let entry = Entry::new(
                            title,
                            format!("{} · {} · {}", number, row.status.label(), row.doc_date),
                            &format!("{} {}", number, row.doc_date),
                            Target::Manuscript(row.id),
                        );
                        entries.push(
                            if matches!(
                                row.status,
                                ManuscriptStatus::Published | ManuscriptStatus::Archived
                            ) {
                                entry.with_badge("只读")
                            } else {
                                entry
                            },
                        );
                    }
                    None
                }
                Err(error) => Some(format!(
                    "稿件列表读取失败：{error:#}。仍可查找已打开稿件、词库和页面。"
                )),
            },
            None => Some("稿件库不可用，仍可查找已打开稿件、词库和页面。".to_string()),
        };
        for entry in &self.config.vocabulary {
            entries.push(
                Entry::new(
                    entry.canonical.clone(),
                    format!("{} · 标准词库", entry.category.label()),
                    &format!(
                        "{} {} {}",
                        entry.external_name,
                        entry.aliases.join(" "),
                        entry.code
                    ),
                    Target::Vocabulary(entry.id),
                )
                .with_icon(match entry.category {
                    crate::models::VocabularyCategory::Unit => crate::theme::Icon::Building,
                    crate::models::VocabularyCategory::Person => crate::theme::Icon::UserPlus,
                }),
            );
        }
        for page in [
            NavPage::Manuscript,
            NavPage::Vocabulary,
            NavPage::Proofread,
            NavPage::Lexicon,
            NavPage::ImeTable,
            NavPage::Knowledge,
            NavPage::AiPrompts,
            NavPage::Settings,
            NavPage::Help,
        ] {
            entries.push(Entry::new(
                page.label().into(),
                "常用页面".into(),
                "",
                Target::Page(page),
            ));
        }
        entries.push(Entry::new(
            "新建空白文档".into(),
            "常用操作".into(),
            "新建公文",
            Target::NewDocument,
        ));
        self.quick_find = Some(QuickFind::new(entries, warning));
    }

    pub(crate) fn quick_find_window(&mut self, ctx: &egui::Context) {
        let Some(state) = &mut self.quick_find else {
            return;
        };
        if state.focus_search {
            state.previous_focus = ctx.memory(|memory| memory.focused());
        }
        let response = ui::modal(ctx).show(ctx, |ui| state.ui(ui));
        let (target, close) = response.inner;
        let close = close || response.should_close();
        if target.is_some() || close {
            let previous_focus = state.previous_focus;
            self.quick_find = None;
            ctx.memory_mut(|memory| {
                memory.surrender_focus(ui::search_id());
                if target.is_none()
                    && let Some(id) = previous_focus
                {
                    memory.request_focus(id);
                }
            });
        }
        if let Some(target) = target {
            self.open_quick_find_target(target);
        }
    }

    fn open_quick_find_target(&mut self, target: Target) {
        match target {
            Target::Document(key) => {
                if let Some(index) = self.docs.iter().position(|doc| doc.key == key) {
                    if self.docs[index].record_status == ManuscriptStatus::Archived
                        && let Some(id) = self.docs[index].manuscript_id
                    {
                        self.open_page(NavPage::Manuscript);
                        self.refresh_detail(id);
                    } else {
                        self.activate_doc(index);
                    }
                }
            }
            Target::Manuscript(id) => {
                // 打开时重新核对状态，查找列表不能绕过归档限制。
                let result = self
                    .manuscript_store
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("稿件库不可用"))
                    .and_then(|store| store.get(id));
                match result {
                    Ok(Some(record)) if record.status == ManuscriptStatus::Archived => {
                        self.manuscript_filter = ManuscriptFilter::default();
                        self.manuscript_dirty = true;
                        self.open_page(NavPage::Manuscript);
                        self.refresh_detail(id);
                    }
                    Ok(Some(_)) => self.open_in_editor(id),
                    Ok(None) => self.status = "稿件不存在或已被删除，请重新查找。".into(),
                    Err(error) => self.status = format!("打开稿件失败：{error:#}"),
                }
            }
            Target::Vocabulary(id) => {
                self.vocabulary_filter.clear();
                self.vocabulary_selected = Some(id);
                self.open_page(NavPage::Vocabulary);
            }
            Target::Page(page) => self.open_page(page),
            Target::NewDocument => self.new_blank_manuscript(),
        }
    }
}
