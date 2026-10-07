//! 会话与稿件库对齐、会话列表（新会话 / 历史会话 / 改名 / 删除）。挂在 `DraftPage` 上。
//!
//! 存取的格式与规则在 `session.rs`；这里管什么时候读、什么时候写，以及切会话时稿件上挂着的
//! 提案怎么办：提案跟着它那一轮存在会话里，切走时从稿件上卸下，切回来正文没动过就重新装上。

use super::session::SavedProposal;
use crate::ai_panel::TurnState;
use crate::app::GongwenApp;
use crate::draft_page::DraftPage;
use crate::theme;
use eframe::egui;

/// RFC3339 只留「月-日 时:分」，卡片上用。
fn short_time(rfc3339: &str) -> String {
    let Some((_, rest)) = rfc3339.split_once('T') else {
        return rfc3339.chars().take(16).collect();
    };
    rest.chars().take(16).collect()
}

impl DraftPage<'_> {
    /// 每帧调用：新打开的稿件读回当前会话，新稿第一次入库时把内存里的会话落盘，有改动就写。
    pub(crate) fn sync_ai_session(&mut self) {
        let Some(id) = self.doc.manuscript_id else {
            return;
        };
        if self.doc.ai_panel.session.loaded_for != Some(id) {
            // 库里这篇还没有会话、侧栏里却有（新稿第一次入库，或另存成了新稿）：把它带过去。
            let has_saved = self
                .store
                .as_deref_mut()
                .and_then(|store| store.list_ai_sessions(id).ok())
                .is_some_and(|list| !list.is_empty());
            if !has_saved && !self.doc.ai_panel.turns.is_empty() {
                self.doc.ai_panel.adopt_session(id);
            } else {
                self.load_current_session(id);
            }
        }
        self.refresh_resumable();
        self.save_ai_session(false);
    }

    /// 按库里最新的检查点，标出哪些轮次可以「接着跑」（内核加固第 4 期）。
    ///
    /// 只看已经结束的轮次（`Interrupted` / `Stopped` / `Failed`）；还在跑或已经出结果的不管。
    /// 每帧都查代价太大（一次 SELECT × 轮数），所以只在状态集合变化时重查。
    pub(crate) fn refresh_resumable(&mut self) {
        let session_id = self.doc.ai_panel.session.id.clone();
        let targets: Vec<(u64, bool)> = self
            .doc
            .ai_panel
            .turns
            .iter()
            .filter(|turn| {
                matches!(
                    turn.state,
                    TurnState::Interrupted | TurnState::Stopped | TurnState::Failed(_)
                )
            })
            .map(|turn| (turn.id, turn.resumable.is_some()))
            .collect();
        if targets.is_empty() {
            return;
        }
        let Some(store) = self.store.as_deref() else {
            // 没连稿件库：没有检查点，卡片保持现在的样子。
            return;
        };
        for (turn_id, _) in targets {
            let stored = store
                .latest_run_checkpoint(&session_id, turn_id as i64)
                .ok()
                .flatten();
            let summary = stored.map(|stored| {
                let label = if stored.label.trim().is_empty() {
                    format!(
                        "第 {} 步",
                        stored.checkpoint.at.first().copied().unwrap_or(0) + 1
                    )
                } else {
                    stored.label.clone()
                };
                format!("{label} · {}", short_time(&stored.created_at))
            });
            if let Some(turn) = self.doc.ai_panel.turn_mut(turn_id)
                && turn.resumable != summary
            {
                turn.resumable = summary;
            }
        }
    }

    /// 写当前会话。`force` 不等一秒间隔（切会话前用）。
    pub(crate) fn save_ai_session(&mut self, force: bool) {
        let doc = &mut *self.doc;
        let pending = doc.ai_proposal.as_ref();
        let make = || SavedProposal::of(pending.expect("只在有提案时调用"));
        let error = doc.ai_panel.save_session(
            doc.manuscript_id,
            self.store.as_deref_mut(),
            pending
                .is_some()
                .then_some(&make as &dyn Fn() -> SavedProposal),
            force,
        );
        if let Some(error) = error {
            *self.status = error;
        }
    }

    /// 读回这篇稿件的当前会话（没有就从空白开始）。
    fn load_current_session(&mut self, id: i64) {
        let Some(store) = self.store.as_deref_mut() else {
            return;
        };
        let records = match store.list_ai_sessions(id) {
            Ok(records) => records,
            Err(error) => {
                *self.status = format!("读取 AI 会话失败：{error:#}");
                self.doc.ai_panel.session.loaded_for = Some(id);
                return;
            }
        };
        let Some(record) = records
            .iter()
            .find(|record| record.is_current)
            .or_else(|| records.first())
            .cloned()
        else {
            self.doc.ai_panel.start_new_session();
            self.doc.ai_panel.session.loaded_for = Some(id);
            return;
        };
        self.switch_to(id, &record);
        if record.panel_open {
            self.doc.ai_panel.open = true;
        }
    }

    /// 把库里的某个会话装进侧栏；它的提案正文没动过就重新装上。
    fn switch_to(&mut self, id: i64, record: &crate::manuscript::ai_sessions::AiSessionRecord) {
        let Some(store) = self.store.as_deref_mut() else {
            return;
        };
        let rows = match store.load_ai_session_turns(&record.id) {
            Ok(rows) => rows,
            Err(error) => {
                *self.status = format!("读取 AI 会话失败：{error:#}");
                return;
            }
        };
        let document = self.doc.generated_markdown.clone();
        let (proposal, problems) = self.doc.ai_panel.load_session(id, record, rows, &document);
        if let Some(saved) = proposal {
            // 存的就是定稿检查过的提案正文，原样装回；再检查一遍会重复补标题占位这类东西。
            let draft = crate::models::GeneratedDraft {
                markdown: saved.markdown,
                title: String::new(),
                warnings: if saved.truncated {
                    vec![super::TRUNCATED_NOTE.to_string().into()]
                } else {
                    Vec::new()
                },
                proof_warnings: Vec::new(),
                proof_measured: false,
                files: Vec::new(),
            };
            GongwenApp::install_ai_proposal(
                self.doc,
                saved.before,
                draft,
                saved.label,
                &self.config.vocabulary,
            );
        }
        if !problems.is_empty() {
            *self.status = format!("AI 会话有 {} 轮读不出来，已跳过。", problems.len());
        }
    }

    /// 能不能换会话：跑着任务时不行（同一时刻只有当前会话能跑）。
    fn can_switch_session(&mut self) -> bool {
        if self.doc.ai_panel.running() || self.doc.busy {
            *self.status = "AI 还在跑，停下或等它做完再换会话。".into();
            return false;
        }
        true
    }

    /// 「新会话」：当前的写下去，换一个空白会话（追问不带旧会话）。
    pub(crate) fn new_ai_session(&mut self) {
        if !self.can_switch_session() {
            return;
        }
        self.save_ai_session(true);
        self.doc.ai_proposal = None;
        self.doc.ai_panel.start_new_session();
        *self.status = "已开新会话。".into();
    }

    /// 切到历史里的某个会话。
    pub(crate) fn open_ai_session(&mut self, session_id: &str) {
        let Some(id) = self.doc.manuscript_id else {
            return;
        };
        if session_id == self.doc.ai_panel.session.id || !self.can_switch_session() {
            return;
        }
        self.save_ai_session(true);
        let record = self
            .store
            .as_deref_mut()
            .and_then(|store| store.list_ai_sessions(id).ok())
            .and_then(|list| list.into_iter().find(|r| r.id == session_id));
        if let Some(record) = record {
            self.doc.ai_proposal = None;
            self.switch_to(id, &record);
            // 切过来的会话成为当前：下一次写入时会把它标上。
            self.doc.ai_panel.session.force_meta_write();
        }
    }

    /// 打开历史会话列表前：先把当前会话写下去，再重读列表。
    pub(crate) fn refresh_ai_sessions(&mut self) {
        let Some(id) = self.doc.manuscript_id else {
            return;
        };
        self.save_ai_session(true);
        if let Some(store) = self.store.as_deref_mut() {
            self.doc.ai_panel.session.list = store.list_ai_sessions(id).unwrap_or_default();
        }
    }

    /// 历史会话列表：按时间列出，点开切过去，可改名、可删。
    pub(crate) fn ai_sessions_menu(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.doc.manuscript_id else {
            ui.weak("这篇稿件还没存进稿件库，会话只在这次打开期间保留。");
            return;
        };
        let current = self.doc.ai_panel.session.id.clone();
        let list = self.doc.ai_panel.session.list.clone();
        if list.is_empty() {
            ui.weak("还没有保存的会话。");
            return;
        }
        let mut open = None;
        let mut delete = None;
        let mut rename: Option<(String, String)> = None;
        let editing_id = ui.id().with("renaming");
        let editing: Option<(String, String)> = ui.memory(|m| m.data.get_temp(editing_id));
        theme::popup_scroll(360.0).show(ui, |ui| {
            for record in &list {
                ui.horizontal(|ui| {
                    if let Some((edit_id, mut text)) =
                        editing.clone().filter(|(e, _)| *e == record.id)
                    {
                        let response =
                            ui.add(egui::TextEdit::singleline(&mut text).desired_width(200.0));
                        let done =
                            response.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if done {
                            rename = Some((edit_id.clone(), text.clone()));
                            ui.memory_mut(|m| m.data.remove::<(String, String)>(editing_id));
                        } else {
                            ui.memory_mut(|m| m.data.insert_temp(editing_id, (edit_id, text)));
                            response.request_focus();
                        }
                        return;
                    }
                    let label = format!(
                        "{}  {} 轮 · {}",
                        if record.title.is_empty() {
                            "（未命名）"
                        } else {
                            &record.title
                        },
                        record.turns,
                        record
                            .updated_at
                            .chars()
                            .take(16)
                            .collect::<String>()
                            .replace('T', " ")
                    );
                    if ui
                        .add(theme::menu_selectable_item(record.id == current, &label))
                        .clicked()
                    {
                        open = Some(record.id.clone());
                    }
                    if theme::icon_button(ui, theme::Icon::Edit, "改名").clicked() {
                        ui.memory_mut(|m| {
                            m.data
                                .insert_temp(editing_id, (record.id.clone(), record.title.clone()))
                        });
                    }
                    if theme::icon_button(ui, theme::Icon::Trash, "删除这个会话").clicked() {
                        delete = Some(record.id.clone());
                    }
                });
            }
        });
        if let Some((session_id, title)) = rename {
            let title = title.trim().to_string();
            if session_id == current {
                self.doc.ai_panel.session.title = title.clone();
            }
            if let Some(store) = self.store.as_deref_mut() {
                let _ = store.rename_ai_session(&session_id, &title);
                self.doc.ai_panel.session.list = store.list_ai_sessions(id).unwrap_or_default();
            }
        }
        if let Some(session_id) = delete {
            if session_id == current {
                if self.can_switch_session() {
                    self.doc.ai_proposal = None;
                    self.doc.ai_panel.start_new_session();
                    if let Some(store) = self.store.as_deref_mut() {
                        let _ = store.delete_ai_session(&session_id);
                    }
                }
            } else if let Some(store) = self.store.as_deref_mut() {
                let _ = store.delete_ai_session(&session_id);
            }
            if let Some(store) = self.store.as_deref_mut() {
                self.doc.ai_panel.session.list = store.list_ai_sessions(id).unwrap_or_default();
            }
        }
        if let Some(session_id) = open {
            self.open_ai_session(&session_id);
            ui.close();
        }
    }
}
