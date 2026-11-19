//! 销毁审批表的入库稿件快照与光盘编号。

use super::ManuscriptStore;
use crate::models::{DraftInput, TemplateKind};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, TransactionBehavior};
use std::collections::HashSet;

pub(crate) fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS disposal_disc_sequence (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            last_serial INTEGER NOT NULL CHECK(last_serial >= 0)
        );",
    )?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormKind {
    File,
    Disc,
}

impl FormKind {
    pub(crate) const ALL: [Self; 2] = [Self::File, Self::Disc];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::File => "文件销毁审批表",
            Self::Disc => "光盘销毁审批表",
        }
    }

    pub(crate) fn columns(self) -> [&'static str; 5] {
        match self {
            Self::File => ["序号", "文件号", "文件标题", "密级", "份数"],
            Self::Disc => ["序号", "编号", "文件名/主要内容", "密级", "份数"],
        }
    }
}

pub(crate) struct FormRow {
    pub(crate) number: String,
    pub(crate) title: String,
    pub(crate) security: String,
    pub(crate) copies: u32,
}

pub(crate) struct DisposalBatch {
    pub(crate) files: Vec<FormRow>,
    pub(crate) discs: Vec<FormRow>,
    pub(crate) last_disc_serial: i64,
}

impl DisposalBatch {
    pub(crate) fn rows(&self, kind: FormKind) -> &[FormRow] {
        match kind {
            FormKind::File => &self.files,
            FormKind::Disc => &self.discs,
        }
    }

    pub(crate) fn rows_mut(&mut self, kind: FormKind) -> &mut [FormRow] {
        match kind {
            FormKind::File => &mut self.files,
            FormKind::Disc => &mut self.discs,
        }
    }

    pub(crate) fn planned_disc_numbers(&self) -> Result<Vec<String>> {
        Ok(disc_number_plan(&self.discs, self.last_disc_serial)?.0)
    }
}

fn file_number(input: &DraftInput) -> String {
    if input.kind.is_research() {
        return input.research.file_number.trim().to_owned();
    }
    let (code, year, serial) = crate::export::element_display::number_display_parts(input);
    if serial.is_empty() || !(input.kind.has_document_number() || input.kind.has_minutes_number()) {
        return String::new();
    }
    // 兼容将整条文号存入序号字段的旧稿。
    let bare_serial = serial.strip_suffix('号').unwrap_or(&serial);
    if bare_serial != serial && !bare_serial.chars().all(|ch| ch.is_ascii_digit()) {
        return serial;
    }
    let code = if input.kind.has_minutes_number() {
        ""
    } else {
        &code
    };
    if year.is_empty() {
        format!("{code}{bare_serial}号")
    } else {
        format!("{code}〔{year}〕{bare_serial}号")
    }
}

fn last_disc_serial(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE((SELECT last_serial FROM disposal_disc_sequence WHERE id = 1), 0)",
        [],
        |row| row.get(0),
    )?)
}

fn disc_number_plan(rows: &[FormRow], last_serial: i64) -> Result<(Vec<String>, i64)> {
    let mut used = HashSet::new();
    let mut serial = last_serial;
    // 现有编号优先使用。已手工登记的 GCB 编号也计入水位，以免将来自动分配给另一张盘。
    for row in rows {
        let number = row.number.trim().to_ascii_uppercase();
        if number.is_empty() {
            continue;
        }
        let key = if let Some(suffix) = number.strip_prefix("GCB-")
            && !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && let Ok(existing) = suffix.parse::<i64>()
        {
            serial = serial.max(existing);
            format!("GCB-{existing:04}")
        } else {
            number
        };
        ensure!(used.insert(key), "光盘编号重复：{}", row.number.trim());
    }
    let mut numbers = Vec::with_capacity(rows.len());
    for row in rows {
        let number = row.number.trim();
        if number.is_empty() {
            serial = serial.checked_add(1).context("光盘自动编号已用尽")?;
            numbers.push(format!("GCB-{serial:04}"));
        } else {
            numbers.push(number.to_owned());
        }
    }
    Ok((numbers, serial))
}

impl ManuscriptStore {
    pub(crate) fn disposal_batch(&self, ids: &[i64]) -> Result<DisposalBatch> {
        ensure!(!ids.is_empty(), "请先勾选要导出的稿件");
        let tx = self.conn.unchecked_transaction()?;
        let mut files = Vec::with_capacity(ids.len());
        let mut discs = Vec::with_capacity(ids.len());
        for &id in ids {
            let record = self
                .read_record(id, false, false)?
                .with_context(|| format!("稿件 {id} 已不存在，请重新勾选"))?;
            let input = record.snapshot;
            let title = record.title;
            let security = input.security_marking().0.to_owned();
            files.push(FormRow {
                number: file_number(&input),
                title: title.clone(),
                security: security.clone(),
                copies: if input.kind == TemplateKind::OfficialLetter {
                    crate::export::copy_count(&input)
                } else {
                    1
                },
            });
            discs.push(FormRow {
                number: String::new(),
                title,
                security,
                copies: 1,
            });
        }
        let last_disc_serial = last_disc_serial(&tx)?;
        tx.commit()?;
        Ok(DisposalBatch {
            files,
            discs,
            last_disc_serial,
        })
    }

    pub(crate) fn reserve_disposal_disc_numbers(
        &mut self,
        batch: &mut DisposalBatch,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = last_disc_serial(&tx)?;
        let (numbers, serial) = disc_number_plan(&batch.discs, previous)?;
        if serial > previous {
            tx.execute(
                "INSERT INTO disposal_disc_sequence (id, last_serial) VALUES (1, ?1)
                 ON CONFLICT(id) DO UPDATE SET last_serial = excluded.last_serial",
                [serial],
            )?;
        }
        tx.commit()?;
        batch.last_disc_serial = serial;
        for (row, number) in batch.discs.iter_mut().zip(numbers) {
            row.number = number;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manuscript::NewManuscript;
    use std::path::Path;

    #[test]
    fn disposal_rows_follow_saved_fields_and_count_only_letters_by_distribution() {
        let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
        let mut letter = DraftInput {
            kind: TemplateKind::OfficialLetter,
            date: "2026年11月19日".into(),
            ..Default::default()
        };
        letter.profile.department_code = "某办函".into();
        letter.profile.document_number = "12".into();
        letter.profile.recipient = "甲单位、乙单位".into();
        letter.profile.copies_to = "丙单位".into();
        letter.profile.responsible_unit = "丁单位".into();
        letter.profile.security_level = "机密".into();
        letter.profile.security_period = "20年".into();
        let mut report = DraftInput {
            kind: TemplateKind::ResearchReport,
            ..letter.clone()
        };
        report.research.file_number = "YJ-2026-04".into();
        report.research.security = "内部".into();
        let mut minutes = DraftInput {
            kind: TemplateKind::MeetingMinutes,
            ..letter.clone()
        };
        minutes.profile.document_number = "3".into();
        let mut plain = DraftInput {
            kind: TemplateKind::PlainDocument,
            ..letter.clone()
        };
        plain.profile.security_level.clear();
        let mut ids = Vec::new();
        for (snapshot, title) in [
            (letter, "工作通知"),
            (report, "研究报告"),
            (minutes, "会议纪要"),
            (plain, "一般文件"),
        ] {
            ids.push(
                store
                    .create(
                        &NewManuscript {
                            snapshot,
                            content_markdown: format!("# {title}\n\n正文"),
                            ..Default::default()
                        },
                        None,
                    )
                    .unwrap(),
            );
        }
        let batch = store.disposal_batch(&ids).unwrap();
        assert_eq!(batch.files[0].title, "工作通知");
        assert_eq!(batch.files[0].number, "某办函〔2026〕12号");
        assert_eq!(batch.files[0].security, "机密");
        assert_eq!(batch.files[0].copies, 4);
        assert_eq!(batch.files[1].number, "YJ-2026-04");
        assert_eq!(batch.files[1].security, "内部");
        assert_eq!(batch.files[2].number, "〔2026〕3号");
        assert!(batch.files[3].number.is_empty() && batch.files[3].security.is_empty());
        assert!(batch.files[1..].iter().all(|row| row.copies == 1));
        assert_eq!(batch.files.len(), batch.discs.len());
        for (file, disc) in batch.files.iter().zip(&batch.discs) {
            assert_eq!(file.title, disc.title);
            assert_eq!(file.security, disc.security);
            assert_eq!(disc.copies, 1);
        }
    }

    #[test]
    fn disposal_number_reservations_survive_reopening_and_stale_previews() {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("drafts.db");
        let mut first = ManuscriptStore::open(&db).unwrap();
        let id = first
            .create(
                &NewManuscript {
                    content_markdown: "# 待销毁文件\n\n正文".into(),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let mut batch = first.disposal_batch(&[id]).unwrap();
        let mut stale = first.disposal_batch(&[id]).unwrap();
        assert_eq!(batch.planned_disc_numbers().unwrap(), ["GCB-0001"]);
        first.reserve_disposal_disc_numbers(&mut batch).unwrap();
        drop(first);

        let mut store = ManuscriptStore::open(&db).unwrap();
        store.reserve_disposal_disc_numbers(&mut stale).unwrap();
        assert_eq!(stale.discs[0].number, "GCB-0002");
        store.reserve_disposal_disc_numbers(&mut batch).unwrap();
        assert_eq!(batch.discs[0].number, "GCB-0001");
        batch.discs[0].number = "GCB-0008".into();
        store.reserve_disposal_disc_numbers(&mut batch).unwrap();
        assert_eq!(
            store
                .disposal_batch(&[id])
                .unwrap()
                .planned_disc_numbers()
                .unwrap(),
            ["GCB-0009"]
        );
    }

    #[test]
    fn disposal_rejects_repeated_disc_numbers_with_different_padding_or_case() {
        let disc = |number: &str| FormRow {
            number: number.to_owned(),
            title: String::new(),
            security: String::new(),
            copies: 1,
        };
        assert!(disc_number_plan(&[disc("GCB-0001"), disc(" gcb-01 ")], 0).is_err());
    }
}
