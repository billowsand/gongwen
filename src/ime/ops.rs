//! 个人层的修改操作。基础表只读，只能整体导入更新；日常的加词、改词、屏蔽、调序
//! 都写进个人层（`tables.json`），基础表升级不会丢。每次修改前留一条修改记录。
use super::{
    data, history,
    session::Ime,
    table::{self, Batch, Entry, PERSONAL, Personal},
};

impl Ime {
    /// 写入个人层并立即生效；写之前把旧文件存进修改记录。
    pub(super) fn save_personal(&mut self, personal: Personal, label: &str) -> anyhow::Result<()> {
        self.persist_personal(personal, label, true)
    }

    fn persist_personal(
        &mut self,
        personal: Personal,
        label: &str,
        remember: bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.storage_ok,
            "个人词表文件读取失败，不能覆盖；请先恢复原文件"
        );
        if personal == self.table.personal {
            return Ok(());
        }
        let snapshot = remember
            .then(|| serde_json::to_vec(&self.table.personal))
            .transpose()?;
        let dir = data::directory()?;
        // 修改记录写不进去不挡正事，只提示一声。
        if let Err(error) = history::record(&dir, label) {
            self.notice = Some(format!("修改记录保存失败：{error}"));
        }
        let bytes = serde_json::to_vec_pretty(&personal)?;
        anyhow::ensure!(
            bytes.len() as u64 <= crate::file_limits::MAX_TEXT_BYTES,
            "个人词表超过 64 MB 上限"
        );
        data::save_bytes(&dir.join("tables.json"), &bytes)?;
        if let Some(snapshot) = snapshot {
            self.manager.undo.push(snapshot, label.into());
        }
        let rules_changed = personal.rules != self.table.personal.rules;
        self.table.personal = personal;
        if rules_changed {
            self.rebuild_encoder();
        }
        self.manager.dirty = true;
        self.table.rebuild();
        self.reload_candidates();
        Ok(())
    }

    /// 撤销最近一次成功保存；失败时保留撤销项，允许修复磁盘后再试。
    pub(super) fn undo_personal(&mut self) -> anyhow::Result<()> {
        let Some((snapshot, label)) = self.manager.undo.last() else {
            return Ok(());
        };
        let personal = serde_json::from_slice::<Personal>(snapshot)?;
        let label = label.clone();
        validate(&personal)?;
        self.persist_personal(personal, &format!("撤销：{label}"), false)?;
        self.manager.undo.pop();
        self.manager.message = format!("已撤销：{label}");
        Ok(())
    }

    /// 移除的是可输入的候选，而不只是个人来源：其他来源仍有的词需要屏蔽。
    pub(super) fn remove_entries(&mut self, entries: &[Entry]) -> anyhow::Result<()> {
        let selected: std::collections::HashSet<(&str, &str)> = entries
            .iter()
            .map(|e| (e.code.as_str(), e.text.as_str()))
            .collect();
        let mut personal = self.table.personal.clone();
        personal
            .entries
            .retain(|e| !selected.contains(&(e.code.as_str(), e.text.as_str())));
        let mut hidden: std::collections::HashSet<(String, String)> = personal
            .hidden
            .iter()
            .map(|e| (e.code.clone(), e.text.clone()))
            .collect();
        for entry in entries {
            let foreign = self
                .table
                .all(&entry.code)
                .iter()
                .find(|c| c.text == entry.text)
                .is_some_and(|c| c.sources.iter().any(|s| s != PERSONAL));
            if foreign && hidden.insert((entry.code.clone(), entry.text.clone())) {
                personal.hidden.push(entry.clone());
            }
            if let Some(order) = personal.order.get_mut(&entry.code) {
                order.retain(|text| text != &entry.text);
            }
        }
        self.save_personal(personal, &format!("移除 {} 条词条", selected.len()))
    }

    /// 对同一批改码只保存一次。先验证整批，避免部分修改后才发现坏编码。
    pub(super) fn edited_personal(&self, changes: &[(Entry, Entry)]) -> anyhow::Result<Personal> {
        anyhow::ensure!(
            changes
                .iter()
                .all(|(_, new)| table::valid_entry(new, false)),
            "编码为 1–4 个小写字母，文字不能为空或含换行、制表符"
        );
        let changes: Vec<_> = changes.iter().filter(|(old, new)| old != new).collect();
        let mut next = self.table.personal.clone();
        // 先移除所有旧个人词，再一次性加入新词，交换编码时也不会互相删掉。
        next.entries
            .retain(|e| !changes.iter().any(|(old, _)| e == old));
        for (old, new) in &changes {
            let foreign = self
                .table
                .all(&old.code)
                .iter()
                .find(|c| c.text == old.text)
                .is_some_and(|c| c.sources.iter().any(|s| s != PERSONAL));
            if foreign && !next.hidden.contains(old) {
                next.hidden.push(old.clone());
            }
            let known = self
                .table
                .all(&new.code)
                .iter()
                .any(|c| c.text == new.text && c.sources.iter().any(|s| s != PERSONAL));
            if !known && !next.entries.contains(new) {
                next.entries.push(new.clone());
            }
            if old.code == new.code
                && let Some(order) = next.order.get_mut(&old.code)
            {
                for word in order.iter_mut().filter(|w| **w == old.text) {
                    *word = new.text.clone();
                }
            }
        }
        next.hidden
            .retain(|e| !changes.iter().any(|(_, new)| e == new));
        Ok(next)
    }

    /// 加一条个人词条；别的来源已有同码同词就只取消屏蔽。`first` 时排到该码首位。
    pub(super) fn add_entry(&mut self, entry: &Entry, first: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            table::valid_entry(entry, false),
            "编码为 1–4 个小写字母，文字不能为空或含换行、制表符"
        );
        let mut personal = self.table.personal.clone();
        personal.hidden.retain(|e| e != entry);
        let known = self
            .table
            .all(&entry.code)
            .iter()
            .any(|c| c.text == entry.text);
        if !known && !personal.entries.contains(entry) {
            personal.entries.push(entry.clone());
        }
        if first {
            let mut order: Vec<String> = self
                .table
                .all(&entry.code)
                .iter()
                .map(|c| c.text.clone())
                .filter(|t| t != &entry.text)
                .collect();
            order.insert(0, entry.text.clone());
            personal.order.insert(entry.code.clone(), order);
        }
        self.save_personal(
            personal,
            &format!("加词「{}」（{}）", entry.text, entry.code),
        )
    }

    /// 改一条词条：个人词条直接替换；其他来源的屏蔽旧的、另加一条个人词条。
    pub(super) fn edit_entry(&mut self, old: &Entry, new: &Entry) -> anyhow::Result<()> {
        anyhow::ensure!(
            table::valid_entry(new, false),
            "编码为 1–4 个小写字母，文字不能为空或含换行、制表符"
        );
        if old == new {
            return Ok(());
        }
        let mut personal = self.table.personal.clone();
        let from_elsewhere = self
            .table
            .all(&old.code)
            .iter()
            .find(|c| c.text == old.text)
            .is_some_and(|c| c.sources.iter().any(|s| s != PERSONAL));
        personal.entries.retain(|e| e != old);
        if from_elsewhere && !personal.hidden.contains(old) {
            personal.hidden.push(old.clone());
        }
        personal.hidden.retain(|e| e != new);
        let known = self.table.all(&new.code).iter().any(|c| c.text == new.text);
        if !known && !personal.entries.contains(new) {
            personal.entries.push(new.clone());
        }
        // 同码改字时，显式排序里的位置跟着新词走。
        if old.code == new.code
            && let Some(words) = personal.order.get_mut(&old.code)
        {
            for word in words.iter_mut().filter(|w| **w == old.text) {
                *word = new.text.clone();
            }
        }
        self.save_personal(
            personal,
            &format!(
                "改「{}」（{}）为「{}」（{}）",
                old.text, old.code, new.text, new.code
            ),
        )
    }

    /// 屏蔽或恢复一个候选：只写个人层，来源表不动。
    pub(super) fn set_hidden(&mut self, entry: &Entry, hidden: bool) -> anyhow::Result<()> {
        let mut personal = self.table.personal.clone();
        if hidden {
            if !personal.hidden.contains(entry) {
                personal.hidden.push(entry.clone());
            }
        } else {
            personal.hidden.retain(|e| e != entry);
        }
        let verb = if hidden { "屏蔽" } else { "恢复" };
        self.save_personal(
            personal,
            &format!("{verb}「{}」（{}）", entry.text, entry.code),
        )
    }

    pub(super) fn block(&mut self, code: &str, text: &str) -> anyhow::Result<()> {
        self.set_hidden(
            &Entry {
                code: code.into(),
                text: text.into(),
            },
            true,
        )
    }

    /// 删个人词条。别的来源也有这条时它照样能打出来。
    pub(super) fn remove_personal(&mut self, entry: &Entry) -> anyhow::Result<()> {
        let mut personal = self.table.personal.clone();
        personal.entries.retain(|e| e != entry);
        self.save_personal(
            personal,
            &format!("删个人词条「{}」（{}）", entry.text, entry.code),
        )
    }

    /// 调整候选顺序：`delta` 为 0 置顶，正负数下移、上移。
    pub(super) fn move_word(&mut self, code: &str, text: &str, delta: isize) -> anyhow::Result<()> {
        let mut order: Vec<String> = self
            .table
            .all(code)
            .iter()
            .map(|c| c.text.clone())
            .collect();
        let Some(index) = order.iter().position(|w| w == text) else {
            return Ok(());
        };
        let target = if delta == 0 {
            0
        } else {
            (index as isize + delta).clamp(0, order.len().saturating_sub(1) as isize) as usize
        };
        let word = order.remove(index);
        order.insert(target, word);
        let mut personal = self.table.personal.clone();
        personal.order.insert(code.into(), order);
        self.save_personal(personal, &format!("调整 {code} 的候选顺序"))
    }

    pub(super) fn reset_order(&mut self, code: &str) -> anyhow::Result<()> {
        let mut personal = self.table.personal.clone();
        if personal.order.remove(code).is_none() {
            return Ok(());
        }
        self.save_personal(personal, &format!("恢复 {code} 的默认顺序"))
    }

    pub(super) fn add_batch(&mut self, name: String, entries: Vec<Entry>) -> anyhow::Result<()> {
        let mut personal = self.table.personal.clone();
        let label = format!("导入「{name}」{} 条", entries.len());
        personal.batches.push(Batch {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            enabled: true,
            entries,
        });
        self.save_personal(personal, &label)
    }

    /// 启停或撤销一批导入表。`enabled` 为 None 表示撤销整批。
    pub(super) fn update_batch(&mut self, id: &str, enabled: Option<bool>) -> anyhow::Result<()> {
        let mut personal = self.table.personal.clone();
        let Some(batch) = personal.batches.iter_mut().find(|b| b.id == id) else {
            return Ok(());
        };
        let label = match enabled {
            Some(true) => format!("启用「{}」", batch.name),
            Some(false) => format!("停用「{}」", batch.name),
            None => format!("撤销导入「{}」", batch.name),
        };
        match enabled {
            Some(enabled) => batch.enabled = enabled,
            None => personal.batches.retain(|b| b.id != id),
        }
        self.save_personal(personal, &label)
    }

    /// 清掉多余的个人词条与失效的屏蔽、排序。返回清理了几项。
    pub(super) fn cleanup_overlay(&mut self) -> anyhow::Result<usize> {
        let report = self.table.overlay_report();
        if report.is_empty() {
            return Ok(0);
        }
        let count = report.redundant.len() + report.stale_hidden.len() + report.stale_order.len();
        let personal = self.table.cleaned(&report);
        self.save_personal(personal, "清理多余与失效记录")?;
        Ok(count)
    }

    /// 回到某条修改记录之前的状态。回退本身也留一条记录，可以再撤回来。
    pub(super) fn restore_history(&mut self, id: &str) -> anyhow::Result<()> {
        let personal = history::load(&data::directory()?, id)?;
        validate(&personal)?;
        self.save_personal(personal, "回退修改")
    }

    /// 改构词规则；None 恢复按基础表推算。
    pub(super) fn set_rules(&mut self, rules: Option<[String; 3]>) -> anyhow::Result<()> {
        if let Some(texts) = &rules {
            anyhow::ensure!(
                super::encoder::Rules::parse(texts).is_some(),
                "规则写法不对：每两个字母一组，大写选字、小写选码，如 AaAbBaBb"
            );
        }
        let mut personal = self.table.personal.clone();
        personal.rules = rules;
        self.save_personal(personal, "修改构词规则")
    }
}

/// 外来的个人层（备份、修改记录）落地前的检查。
pub(super) fn validate(personal: &Personal) -> anyhow::Result<()> {
    anyhow::ensure!(
        personal
            .entries
            .iter()
            .chain(personal.hidden.iter())
            .all(|e| table::valid_entry(e, false))
            && personal
                .batches
                .iter()
                .all(|b| b.entries.iter().all(|e| table::valid_entry(e, true))),
        "个人词表中有无效词条"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(code: &str, text: &str) -> Entry {
        Entry {
            code: code.into(),
            text: text.into(),
        }
    }

    fn ime() -> (Ime, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
        let mut ime = Ime::bare(super::super::ImeSettings::default());
        ime.table.base = table::parse("abcd,1=基础\nabcd,2=次选\nefgh,1=词表", false).entries;
        ime.table.rebuild();
        (ime, dir)
    }

    #[test]
    fn bulk_remove_and_undo_restores_all_sources_in_one_operation() {
        let (mut ime, _dir) = ime();
        ime.table.personal.entries = vec![entry("abcd", "基础"), entry("ijkl", "个人")];
        ime.table.rebuild();
        let before = ime.table.personal.clone();
        ime.remove_entries(&[entry("abcd", "基础"), entry("ijkl", "个人")])
            .unwrap();
        assert!(!ime.table.lookup("abcd").iter().any(|c| c.text == "基础"));
        assert!(ime.table.lookup("ijkl").is_empty());
        assert_eq!(ime.table.base.len(), 3);
        assert!(ime.manager.undo.last().is_some());
        ime.undo_personal().unwrap();
        assert_eq!(ime.table.personal, before);
        assert!(ime.manager.undo.last().is_none());
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn first_add_and_import_can_each_be_undone_without_redo_loops() {
        let (mut ime, _dir) = ime();
        ime.add_entry(&entry("ijkl", "新词"), false).unwrap();
        ime.add_batch("导入".into(), vec![entry("mnop", "导入词")])
            .unwrap();
        ime.undo_personal().unwrap();
        assert!(ime.table.personal.batches.is_empty());
        assert_eq!(ime.table.personal.entries, [entry("ijkl", "新词")]);
        ime.undo_personal().unwrap();
        assert!(ime.table.personal.entries.is_empty());
        assert!(ime.manager.undo.last().is_none());
        let saved: Personal = serde_json::from_slice(
            &std::fs::read(data::directory().unwrap().join("tables.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved, ime.table.personal);
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn failed_save_keeps_memory_and_failed_undo_keeps_snapshot() {
        let (mut ime, _dir) = ime();
        let path = data::directory().unwrap().join("tables.json");
        std::fs::create_dir(&path).unwrap();
        assert!(ime.add_entry(&entry("ijkl", "新词"), false).is_err());
        assert!(ime.table.personal.entries.is_empty());
        assert!(ime.manager.undo.last().is_none());
        std::fs::remove_dir(&path).unwrap();
        ime.add_entry(&entry("ijkl", "新词"), false).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(ime.undo_personal().is_err());
        assert_eq!(ime.table.personal.entries, [entry("ijkl", "新词")]);
        assert!(ime.manager.undo.last().is_some());
        std::fs::remove_dir(&path).unwrap();
        ime.undo_personal().unwrap();
        assert!(ime.table.personal.entries.is_empty());
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn batch_code_swap_preserves_both_words_and_rejects_invalid_batch() {
        let (mut ime, _dir) = ime();
        let left = entry("ijkl", "同词");
        let right = entry("mnop", "同词");
        ime.table.personal.entries = vec![left.clone(), right.clone()];
        ime.table.rebuild();
        let changes = [(left.clone(), right.clone()), (right.clone(), left.clone())];
        let next = ime.edited_personal(&changes).unwrap();
        assert!(next.entries.contains(&left));
        assert!(next.entries.contains(&right));
        assert_eq!(next.entries.len(), 2);
        assert!(
            ime.edited_personal(&[(left, entry("123", "坏码"))])
                .is_err()
        );
        assert_eq!(ime.table.personal.entries.len(), 2);
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn edits_stay_in_personal_layer_and_history_restores_them() {
        let (mut ime, _dir) = ime();
        // 改基础词：基础表不动，屏蔽原词、另加个人词条。
        ime.edit_entry(&entry("abcd", "基础"), &entry("abcd", "改过"))
            .unwrap();
        assert_eq!(ime.table.base.len(), 3);
        assert_eq!(ime.table.personal.hidden, [entry("abcd", "基础")]);
        let texts: Vec<String> = ime
            .table
            .lookup("abcd")
            .into_iter()
            .map(|c| c.text)
            .collect();
        assert_eq!(texts, ["次选", "改过"]);
        // 改个人词条：直接替换，不再多一条屏蔽。
        ime.edit_entry(&entry("abcd", "改过"), &entry("abce", "再改"))
            .unwrap();
        assert_eq!(ime.table.personal.entries, [entry("abce", "再改")]);
        assert_eq!(ime.table.personal.hidden.len(), 1);
        ime.move_word("abcd", "次选", 1).unwrap();
        ime.set_hidden(&entry("efgh", "词表"), true).unwrap();
        // 修改记录：最新的是屏蔽，回到它之前就是取消屏蔽。
        let dir = data::directory().unwrap();
        let records = history::list(&dir);
        assert_eq!(records.len(), 3, "第一次写入前没有旧文件，不记");
        assert!(records[0].label.contains("屏蔽「词表」"));
        ime.restore_history(&records[0].id).unwrap();
        assert!(!ime.table.hidden("efgh", "词表"));
        assert_eq!(history::list(&dir)[0].label, "回退修改");
        crate::storage::set_test_config_dir(None);
    }

    #[test]
    fn add_entry_cleanup_and_rules() {
        let (mut ime, _dir) = ime();
        ime.add_entry(&entry("abcd", "新词"), true).unwrap();
        assert_eq!(ime.table.lookup("abcd")[0].text, "新词");
        // 已在基础表的词只取消屏蔽，不另存个人词条。
        ime.set_hidden(&entry("efgh", "词表"), true).unwrap();
        ime.add_entry(&entry("efgh", "词表"), false).unwrap();
        assert!(!ime.table.hidden("efgh", "词表"));
        assert_eq!(ime.table.personal.entries.len(), 1);
        // 个人词条被基础表收录后成了多余记录，清理掉。
        ime.table.base.push(entry("abcd", "新词"));
        ime.table.rebuild();
        assert_eq!(ime.cleanup_overlay().unwrap(), 1);
        assert!(ime.table.personal.entries.is_empty());
        assert!(
            ime.set_rules(Some(["Aa".into(), "AaBa".into(), "坏".into()]))
                .is_err()
        );
        ime.set_rules(Some(["AaBa".into(), "AaBaCa".into(), "AaBaCa".into()]))
            .unwrap();
        assert!(ime.encoder.overridden);
        crate::storage::set_test_config_dir(None);
    }
}
