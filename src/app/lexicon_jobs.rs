//! 公文词表的刷新、扫描任务与码表导出。
//!
//! 扫描要遍历整个稿件库并逐篇分词，放后台线程；确认、改读音、导出都在毫秒级，
//! 直接在主线程做。

use crate::app::{GongwenApp, LexiconPreview, WorkerResult};
use crate::lexicon::{self, TermState, export, scan};
use crate::storage;
use std::thread;

/// 词表后台任务的结果。
pub(crate) enum LexiconJob {
    ScanProgress {
        done: usize,
        total: usize,
        current_title: String,
    },
    ScanFinished(Result<scan::ScanSummary, String>),
}

impl GongwenApp {
    /// 重查概况与列表。`lexicon_dirty` 为真时由页面调用。
    pub(crate) fn refresh_lexicon(&mut self) {
        let Some(store) = self.lexicon_store.as_ref() else {
            return;
        };
        match store.stats() {
            Ok(stats) => self.lexicon_stats = stats,
            Err(error) => self.lexicon_error = Some(format!("读取词表概况失败：{error:#}")),
        }
        match store.list(&self.lexicon_filter) {
            Ok(terms) => self.lexicon_terms = terms,
            Err(error) => self.lexicon_error = Some(format!("读取词表失败：{error:#}")),
        }
        self.lexicon_dirty = false;
    }

    /// 导出预览。算一次要给每个词出码（几千次拼音查表），所以按口径缓存：
    /// 口径没变、词表也没动过就直接用上次的结果，不能每帧重算。
    pub(crate) fn lexicon_preview(&mut self) -> LexiconPreview {
        if let Some((options, preview)) = &self.lexicon_preview
            && *options == self.lexicon_export
        {
            return preview.clone();
        }
        let preview = self.compute_lexicon_preview();
        self.lexicon_preview = Some((self.lexicon_export.clone(), preview.clone()));
        preview
    }

    fn compute_lexicon_preview(&self) -> LexiconPreview {
        let Some(store) = self.lexicon_store.as_ref() else {
            return LexiconPreview::default();
        };
        let Ok(terms) = store.export_candidates() else {
            return LexiconPreview::default();
        };
        let built = export::build(&terms, &self.lexicon_export);
        LexiconPreview {
            written: built.written,
            truncated: built.truncated,
            conflict_codes: built.conflict_codes,
            conflict_terms: built.conflict_terms,
            saved_keys: built.saved_keys,
            failed: built
                .failed
                .iter()
                .map(|dropped| dropped.term.clone())
                .collect(),
            summary: built.describe(),
            origins: export::origin_breakdown(&terms, &self.lexicon_export)
                .into_iter()
                .map(|(origin, count)| format!("{} {}", origin.label(), count))
                .collect::<Vec<_>>()
                .join(" · "),
        }
    }

    pub(crate) fn set_lexicon_state(&mut self, id: i64, state: TermState) {
        let Some(store) = self.lexicon_store.as_mut() else {
            return;
        };
        if let Err(error) = store.set_state(id, state) {
            self.lexicon_error = Some(format!("更新词条状态失败：{error:#}"));
            return;
        }
        self.lexicon_dirty = true;
        self.lexicon_preview = None;
        // 接受/拒绝会改变 jieba 用户词典的内容，立刻换上新的，
        // 免得这一轮检索与标题断行还在用旧词典。
        self.reload_lexicon_user_dict();
    }

    /// 接受当前列表里所有还没接受的词。一条条点是二期工作台之前的权宜之计，
    /// 但没有它，第一次面对几千个候选词根本无从下手。
    pub(crate) fn accept_listed_lexicon_terms(&mut self) {
        let ids: Vec<i64> = self
            .lexicon_terms
            .iter()
            .filter(|term| term.state != TermState::Accepted)
            .map(|term| term.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let count = ids.len();
        let Some(store) = self.lexicon_store.as_mut() else {
            return;
        };
        match store.set_state_many(&ids, TermState::Accepted) {
            Ok(()) => {
                self.lexicon_dirty = true;
                self.lexicon_preview = None;
                self.status = format!("已接受 {count} 个词条。");
                self.reload_lexicon_user_dict();
            }
            Err(error) => self.lexicon_error = Some(format!("批量接受失败：{error:#}")),
        }
    }

    /// 清空整张词表，连同扫描记账。下次扫描从头再来。
    pub(crate) fn clear_lexicon(&mut self) {
        let Some(store) = self.lexicon_store.as_mut() else {
            return;
        };
        match store.clear_all() {
            Ok(()) => {
                self.lexicon_dirty = true;
                self.lexicon_preview = None;
                self.lexicon_scan_result = None;
                self.status = "公文词表已清空。".into();
                lexicon::segmenter::install_user_dict("");
            }
            Err(error) => self.lexicon_error = Some(format!("清空词表失败：{error:#}")),
        }
    }

    pub(crate) fn set_lexicon_reading(&mut self, id: i64, pinyin: &str, code_override: &str) {
        let Some(store) = self.lexicon_store.as_mut() else {
            return;
        };
        match store.set_reading(id, pinyin, code_override) {
            Ok(()) => {
                self.lexicon_dirty = true;
                self.lexicon_preview = None;
            }
            Err(error) => self.lexicon_error = Some(format!("保存读音失败：{error:#}")),
        }
    }

    pub(crate) fn add_lexicon_term(&mut self) {
        let term = self.lexicon_new_term.trim().to_string();
        let Some(store) = self.lexicon_store.as_mut() else {
            return;
        };
        match store.add_manual(&term) {
            Ok(()) => {
                self.lexicon_new_term.clear();
                self.lexicon_dirty = true;
                self.lexicon_preview = None;
                self.status = format!("「{term}」已加入公文词表。");
                self.reload_lexicon_user_dict();
            }
            Err(error) => self.lexicon_error = Some(format!("{error:#}")),
        }
    }

    /// 把已接受的词重新挂成 jieba 用户词典。
    fn reload_lexicon_user_dict(&mut self) {
        let Some(store) = self.lexicon_store.as_ref() else {
            return;
        };
        if let Ok(dict) = store.jieba_user_dict() {
            lexicon::segmenter::install_user_dict(&dict);
        }
    }

    /// 在后台线程扫一轮语料。`rescan_all` 为真时忽略哈希记账，全部重扫。
    pub(crate) fn start_lexicon_scan(&mut self, rescan_all: bool) {
        if self.lexicon_busy {
            self.status = "词表扫描正在进行中…".into();
            return;
        }
        let Ok(db_path) = storage::manuscript_db_path() else {
            self.lexicon_error = Some("词表路径不可用。".into());
            return;
        };
        self.lexicon_busy = true;
        self.lexicon_scan_result = None;
        self.lexicon_scan_progress = Some((0, 0, String::new()));
        let mut options = self.lexicon_scan_options.clone();
        options.rescan_all = rescan_all;
        let proofread = self.config.proofread.clone();
        let vocabulary = self.config.vocabulary.clone();
        let tx = self.sender.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = scan::run_scan_with(
                db_path,
                options,
                proofread,
                vocabulary,
                move |done, total, title| {
                    let _ = progress_tx.send(WorkerResult::Lexicon(LexiconJob::ScanProgress {
                        done,
                        total,
                        current_title: title,
                    }));
                },
            )
            .map_err(|error| format!("{error:#}"));
            let _ = tx.send(WorkerResult::Lexicon(LexiconJob::ScanFinished(result)));
        });
    }

    pub(crate) fn handle_lexicon_job(&mut self, job: LexiconJob) {
        match job {
            LexiconJob::ScanProgress {
                done,
                total,
                current_title,
            } => {
                self.lexicon_scan_progress = Some((done, total, current_title));
            }
            LexiconJob::ScanFinished(result) => {
                self.lexicon_busy = false;
                self.lexicon_scan_progress = None;
                match result {
                    Ok(summary) => {
                        self.lexicon_scan_result = Some(summary.describe());
                        self.status = format!("词表扫描完成：{}。", summary.describe());
                        if !summary.failed.is_empty() {
                            self.lexicon_error = Some(
                                summary
                                    .failed
                                    .iter()
                                    .map(|(title, error)| format!("{title}：{error}"))
                                    .collect::<Vec<_>>()
                                    .join("；"),
                            );
                        }
                    }
                    Err(error) => {
                        self.lexicon_error = Some(format!("词表扫描失败：{error}"));
                        self.status = "词表扫描失败。".into();
                    }
                }
                self.lexicon_dirty = true;
                self.lexicon_preview = None;
                // 扫描把标准词库的专名也并了进来，词典跟着换一次。
                self.reload_lexicon_user_dict();
            }
        }
    }

    /// 导出小鹤用户码表。有重码时一并写出重码报告，文件名与码表同目录同前缀。
    pub(crate) fn export_flypy_table(&mut self) {
        let Some(store) = self.lexicon_store.as_ref() else {
            return;
        };
        let terms = match store.export_candidates() {
            Ok(terms) => terms,
            Err(error) => {
                self.lexicon_error = Some(format!("读取词表失败：{error:#}"));
                return;
            }
        };
        let built = export::build(&terms, &self.lexicon_export);
        if built.written == 0 {
            self.lexicon_error = Some("当前口径下没有可导出的词。".into());
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("小鹤用户码表", &["txt"])
            .set_file_name(export::suggested_file_name())
            .save_file()
        else {
            return;
        };
        if let Err(error) = std::fs::write(&path, built.table.as_bytes()) {
            self.lexicon_error = Some(format!("写入码表失败：{error}"));
            return;
        }
        let mut message = format!("码表已导出到 {}：{}", path.display(), built.describe());
        if !built.conflicts.is_empty() {
            let report = path
                .parent()
                .map(|dir| dir.join(export::suggested_conflict_file_name()))
                .unwrap_or_else(
                    || std::path::PathBuf::from(export::suggested_conflict_file_name()),
                );
            match std::fs::write(&report, built.conflicts.as_bytes()) {
                Ok(()) => message.push_str(&format!("；重码报告见 {}", report.display())),
                Err(error) => message.push_str(&format!("；重码报告写入失败：{error}")),
            }
        }
        message.push_str("。在小鹤输入法的码表管理里导入「主码-用户码表」即可生效。");
        self.lexicon_export_result = Some(message.clone());
        self.status = message;
    }
}
