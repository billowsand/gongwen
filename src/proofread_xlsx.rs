//! 校对词表的 Excel 导入导出。
//!
//! 与标准词库的 xlsx 走同一套思路，但简单得多：只有一张表、八列，就是内置种子
//! 表 `proofread-lexicon.tsv` 的那八列。机关里改词表的人手上有 Excel，没有别的
//! 趁手工具，所以宁可多一个导出/导入的往返，也别让人去碰配置文件。
//!
//! 合并按**条目编号**走，不按错误写法：
//! - 编号命中内置种子 → 与种子逐字段比对，只有真不一样的才落成覆盖记录；
//! - 编号是别的（含 `USR-` 开头）→ 落进自建条目，同号覆盖、新号追加。
//!
//! 这样把导出的表原样导回去是个空操作，不会平白造出一堆"改动"。

use crate::models::{ProofreadConfig, ProofreadRule};
use crate::proofread;
use anyhow::{Context, Result};
use calamine::{Data, Reader, Xlsx};
use rust_xlsxwriter::{Format, FormatAlign, Workbook};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

const SHEET: &str = "校对词表";
const HEADERS: &[&str] = &[
    "条目编号",
    "错误写法",
    "建议写法",
    "级别",
    "命中条件",
    "分组",
    "说明",
    "启用",
];
const COLUMN_WIDTHS: &[f64] = &[12.0, 16.0, 24.0, 8.0, 34.0, 12.0, 44.0, 8.0];

/// 一次导入的结果，用来给用户一句交代。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// 改动了的内置条目数。
    pub overridden: usize,
    /// 恢复成默认（与种子一致，覆盖记录被清掉）的内置条目数。
    pub reverted: usize,
    /// 新增的自建条目数。
    pub added: usize,
    /// 更新的自建条目数。
    pub updated: usize,
    /// 逐行的问题：级别不认识、正则编译不过、编号为空等。
    pub warnings: Vec<String>,
}

impl ImportReport {
    pub fn summary(&self) -> String {
        format!(
            "内置改动 {} 条、恢复默认 {} 条、自建新增 {} 条、自建更新 {} 条",
            self.overridden, self.reverted, self.added, self.updated
        )
    }
}

/// 把当前生效的词表导出成 xlsx。
pub fn to_xlsx(config: &ProofreadConfig, path: &Path) -> Result<()> {
    let lexicon = proofread::Lexicon::resolved(config);
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet.set_name(SHEET)?;

    let header = Format::new()
        .set_bold()
        .set_align(FormatAlign::Center)
        .set_background_color(0xEFEFEF);
    for (index, title) in HEADERS.iter().enumerate() {
        sheet.write_string_with_format(0, index as u16, *title, &header)?;
        sheet.set_column_width(index as u16, COLUMN_WIDTHS[index])?;
    }
    // 表头冻结：141 行往下翻时还看得见列名。
    sheet.set_freeze_panes(1, 0)?;

    for (row, entry) in lexicon.entries.iter().enumerate() {
        let rule = entry.to_rule();
        let row = row as u32 + 1;
        sheet.write_string(row, 0, &rule.id)?;
        sheet.write_string(row, 1, &rule.wrong)?;
        sheet.write_string(row, 2, &rule.suggestion)?;
        sheet.write_string(row, 3, &rule.level)?;
        sheet.write_string(row, 4, &rule.condition)?;
        sheet.write_string(row, 5, &rule.group)?;
        sheet.write_string(row, 6, &rule.note)?;
        sheet.write_string(row, 7, if rule.enabled { "是" } else { "否" })?;
    }

    workbook
        .save(path)
        .with_context(|| format!("无法写入 {}", path.display()))?;
    Ok(())
}

/// 读 xlsx，解析成规则行。格式问题逐行记进 warnings，不中断整份导入。
pub fn parse(path: &Path) -> Result<(Vec<ProofreadRule>, Vec<String>)> {
    let file = File::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
    let mut workbook: Xlsx<BufReader<File>> =
        Xlsx::new(BufReader::new(file)).context("这不是一个可读的 xlsx 文件")?;
    let range = workbook
        .worksheet_range(SHEET)
        .or_else(|_| {
            // 用户可能改过表名，退而取第一张表。
            let first = workbook
                .sheet_names()
                .first()
                .cloned()
                .unwrap_or_else(|| SHEET.to_string());
            workbook.worksheet_range(&first)
        })
        .context("工作簿里没有可读的工作表")?;

    let mut rules = Vec::new();
    let mut warnings = Vec::new();
    for (index, row) in range.rows().enumerate() {
        if index == 0 {
            continue; // 表头
        }
        let cell = |column: usize| -> String {
            row.get(column)
                .map(|value| match value {
                    Data::Empty => String::new(),
                    other => other.to_string(),
                })
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let id = cell(0);
        if id.is_empty() && cell(1).is_empty() {
            continue; // 整行空，跳过
        }
        if id.is_empty() {
            warnings.push(format!("第 {} 行缺少条目编号，已跳过", index + 1));
            continue;
        }
        let enabled_cell = cell(7);
        let rule = ProofreadRule {
            id,
            wrong: cell(1),
            suggestion: cell(2),
            level: cell(3),
            condition: cell(4),
            group: cell(5),
            note: cell(6),
            enabled: matches!(enabled_cell.as_str(), "是" | "TRUE" | "true" | "1"),
        };
        // 当场试编译，坏行不进配置——否则要到校对时才发现规则从没生效。
        if let Err(error) = proofread::Entry::from_rule(&rule) {
            warnings.push(format!("第 {} 行（{}）{error}，已跳过", index + 1, rule.id));
            continue;
        }
        rules.push(rule);
    }
    Ok((rules, warnings))
}

/// 把导入的行合并进配置。
pub fn merge(config: &mut ProofreadConfig, rules: Vec<ProofreadRule>) -> ImportReport {
    let mut report = ImportReport::default();
    for rule in rules {
        match proofread::builtin_entry(&rule.id) {
            Some(seed) => {
                // 与种子逐字段比对，一样的字段不落覆盖——把导出的表原样导回来
                // 应当是空操作。
                let level = (seed.level.label() != rule.level).then(|| rule.level.clone());
                let suggestion =
                    (seed.suggestion != rule.suggestion).then(|| rule.suggestion.clone());
                let condition =
                    (seed.condition_text != rule.condition).then(|| rule.condition.clone());
                let note = (seed.note != rule.note).then(|| rule.note.clone());
                let enabled = (seed.enabled != rule.enabled).then_some(rule.enabled);

                let had_override = config.override_for(&rule.id).is_some();
                let patch = config.override_mut(&rule.id);
                patch.level = level;
                patch.suggestion = suggestion;
                patch.condition = condition;
                patch.note = note;
                patch.enabled = enabled;
                if patch.is_empty() {
                    if had_override {
                        report.reverted += 1;
                    }
                } else {
                    report.overridden += 1;
                }
            }
            None => match config.custom.iter_mut().find(|item| item.id == rule.id) {
                Some(existing) => {
                    if *existing != rule {
                        *existing = rule;
                        report.updated += 1;
                    }
                }
                None => {
                    config.custom.push(rule);
                    report.added += 1;
                }
            },
        }
    }
    config.prune();
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join(name);
        (dir, path)
    }

    #[test]
    fn exported_table_round_trips_into_no_changes() {
        // 最关键的一条：导出再导回来必须是空操作，否则用户只是想备份一下，
        // 就凭空多出 141 条“改动”。
        let mut config = ProofreadConfig::default();
        let (_dir, path) = temp("词表.xlsx");
        to_xlsx(&config, &path).expect("导出");

        let (rules, warnings) = parse(&path).expect("解析");
        assert!(warnings.is_empty(), "回读不应有告警：{warnings:?}");
        assert_eq!(rules.len(), proofread::builtin_lexicon().entries.len());

        let report = merge(&mut config, rules);
        assert_eq!(report, ImportReport::default(), "原样导回应当毫无改动");
        assert!(config.overrides.is_empty());
        assert!(config.custom.is_empty());
    }

    #[test]
    fn changing_a_builtin_row_lands_as_an_override() {
        let mut config = ProofreadConfig::default();
        let seed = proofread::builtin_entry("TYP-001").expect("内置条目");
        let mut rule = seed.to_rule();
        rule.enabled = false;
        rule.note = "本单位不查这条".into();

        let report = merge(&mut config, vec![rule]);
        assert_eq!(report.overridden, 1);
        let patch = config.override_for("TYP-001").expect("覆盖记录");
        assert_eq!(patch.enabled, Some(false));
        assert_eq!(patch.note.as_deref(), Some("本单位不查这条"));
        // 没改的字段不该留痕。
        assert!(patch.suggestion.is_none());
        assert!(patch.level.is_none());
    }

    #[test]
    fn restoring_a_row_to_seed_values_clears_the_override() {
        let mut config = ProofreadConfig::default();
        config.override_mut("TYP-001").enabled = Some(false);

        let seed = proofread::builtin_entry("TYP-001").expect("内置条目");
        let report = merge(&mut config, vec![seed.to_rule()]);
        assert_eq!(report.reverted, 1);
        assert!(config.override_for("TYP-001").is_none());
    }

    #[test]
    fn unknown_ids_become_custom_entries() {
        let mut config = ProofreadConfig::default();
        let rule = ProofreadRule {
            id: "USR-001".into(),
            wrong: "我办".into(),
            suggestion: "本办".into(),
            level: "疑似".into(),
            condition: "总是".into(),
            group: "自定义".into(),
            note: String::new(),
            enabled: true,
        };
        assert_eq!(merge(&mut config, vec![rule.clone()]).added, 1);
        // 同号再导一次是更新不是重复追加。
        let mut changed = rule;
        changed.suggestion = "我单位".into();
        let report = merge(&mut config, vec![changed]);
        assert_eq!(report.added, 0);
        assert_eq!(report.updated, 1);
        assert_eq!(config.custom.len(), 1);
    }

    /// 手工造一份 xlsx，用来模拟"用户在 Excel 里改坏了"的情形。
    /// 不能靠导出来造：坏规则压根进不了生效词表，也就导不出来。
    fn write_rows(path: &Path, rows: &[[&str; 8]]) {
        let mut workbook = Workbook::new();
        let sheet = workbook.add_worksheet();
        sheet.set_name(SHEET).expect("表名");
        for (index, title) in HEADERS.iter().enumerate() {
            sheet.write_string(0, index as u16, *title).expect("写表头");
        }
        for (row, cells) in rows.iter().enumerate() {
            for (column, value) in cells.iter().enumerate() {
                sheet
                    .write_string(row as u32 + 1, column as u16, *value)
                    .expect("写单元格");
            }
        }
        workbook.save(path).expect("保存");
    }

    #[test]
    fn broken_rows_are_skipped_with_a_warning() {
        let (_dir, path) = temp("坏词表.xlsx");
        write_rows(
            &path,
            &[
                // 环视在 Rust regex 里编译不过。
                [
                    "USR-009",
                    "x",
                    "y",
                    "必错",
                    "正则:截止(?=目前)",
                    "自定义",
                    "",
                    "是",
                ],
                // 级别写错。
                ["USR-010", "z", "w", "严重", "总是", "自定义", "", "是"],
                // 缺编号。
                ["", "q", "r", "必错", "总是", "自定义", "", "是"],
                // 这一行是好的，应当活下来。
                [
                    "USR-011",
                    "我办",
                    "本办",
                    "疑似",
                    "总是",
                    "自定义",
                    "",
                    "是",
                ],
            ],
        );

        let (rules, warnings) = parse(&path).expect("解析");
        assert_eq!(warnings.len(), 3, "三种坏行都应挑出来：{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("USR-009")));
        assert!(warnings.iter().any(|w| w.contains("USR-010")));
        assert!(warnings.iter().any(|w| w.contains("缺少条目编号")));
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "USR-011");
    }
}
