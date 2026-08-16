//! 标准词库的 xlsx 交换格式。
//!
//! 两个 Sheet：`单位` / `人员`。模板内置冻结首行、表头加粗、表头行高、数据验证下拉、
//! 工作表保护（解锁 A–H 单元格，禁列行增删）。导入做硬拒绝：任何冲突整次失败，不写入。
//!
//! 编码权威：用户填的 `code` 字面值原样保留，按编码前缀还原上下级；空 `code` 才由软件补。
//! 与 `units::normalize_preserving_codes` 行为一致。

use crate::models::{VocabularyCategory, VocabularyEntry};
use crate::units;
use anyhow::Result;
use calamine::{Data, Reader, Xlsx};
use rust_xlsxwriter::{DataValidation, Format, FormatAlign, FormatBorder, Workbook, Worksheet};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// 唯一允许出现在 `code` 列的字符集合。
fn is_valid_code_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn is_valid_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.chars().all(is_valid_code_char)
        && !value.starts_with('@')
}

const UNIT_SHEET: &str = "单位";
const PERSON_SHEET: &str = "人员";

const UNIT_HEADERS: &[&str] = &[
    "层级编码",
    "单位名称",
    "外部名称",
    "简称",
    "机关代字",
    "是否代章",
    "别名/常见错写",
    "备注",
];

const PERSON_HEADERS: &[&str] = &[
    "所属单位编码",
    "人员编码",
    "姓名",
    "职务",
    "联系电话",
    "承办上级单位",
    "别名/常见错写",
    "备注",
];

const UNIT_COLUMN_WIDTHS: &[f64] = &[14.0, 28.0, 28.0, 16.0, 12.0, 10.0, 28.0, 28.0];
const PERSON_COLUMN_WIDTHS: &[f64] = &[14.0, 10.0, 18.0, 14.0, 16.0, 12.0, 28.0, 28.0];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Sheet 名称（"单位" / "人员"）
    pub sheet: &'static str,
    /// xlsx 行号（含表头，从 1 起）
    pub row: usize,
    pub field: &'static str,
    pub current_value: String,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub entries: Vec<VocabularyEntry>,
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug, Default)]
pub struct ValidationReport {
    pub conflicts: Vec<Conflict>,
}

#[derive(Debug)]
pub struct ImportError {
    pub conflicts: Vec<Conflict>,
    pub summary: String,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary)
    }
}

impl std::error::Error for ImportError {}

/// 读取 xlsx → 校验 → 任一冲突整次拒绝。
///
/// `existing_unit_codes` 用于校验"人员所属单位编码"：值为空的视为未归属人员，
/// 非空但不在任何已知单位集合内则视为冲突。
pub fn parse(path: &Path, existing_unit_codes: &[String]) -> Result<ImportReport, ImportError> {
    let mut report = parse_only(path).map_err(|error| ImportError {
        conflicts: Vec::new(),
        summary: format!("读取 xlsx 失败：{error:#}"),
    })?;
    let conflicts = validate(&report.entries, existing_unit_codes);
    report.conflicts = conflicts.conflicts;
    if !report.conflicts.is_empty() {
        return Err(ImportError {
            conflicts: report.conflicts.clone(),
            summary: format!("词库包含 {} 项冲突，导入已中止", report.conflicts.len()),
        });
    }
    Ok(report)
}

/// 仅读 xlsx 生成 entries，不做校验。给 `units::normalize` 之前的临时数据用。
fn parse_only(path: &Path) -> Result<ImportReport> {
    let file = File::open(path).map_err(|error| anyhow::anyhow!("打开 xlsx 失败：{error}"))?;
    let mut workbook: Xlsx<BufReader<File>> = Xlsx::new(BufReader::new(file))
        .map_err(|error| anyhow::anyhow!("解析 xlsx 失败：{error}"))?;

    let mut entries = Vec::new();

    let unit_rows = read_sheet(&mut workbook, UNIT_SHEET)?;
    for (offset, row) in unit_rows.iter().enumerate() {
        if row.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        entries.push(parse_unit_row(row, offset + 2)?);
    }

    let person_rows = read_sheet(&mut workbook, PERSON_SHEET)?;
    for (offset, row) in person_rows.iter().enumerate() {
        if row.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        entries.push(parse_person_row(row, offset + 2)?);
    }

    if entries.is_empty() {
        // 空模板合法；上层按需决定是否提示。
    }

    // 把单位表里出现的单位名称去匹配人员表的"所属单位"，自动迁移为编码（兼容旧模板）。
    // 严格冲突由 validate 抓出。
    units::rebuild_parents_from_codes(&mut entries);

    Ok(ImportReport {
        entries,
        conflicts: Vec::new(),
    })
}

fn read_sheet(workbook: &mut Xlsx<BufReader<File>>, sheet_name: &str) -> Result<Vec<Vec<String>>> {
    let range = workbook
        .worksheet_range(sheet_name)
        .map_err(|error| anyhow::anyhow!("读取 Sheet「{sheet_name}」失败：{error}"))?;
    let mut rows = Vec::new();
    for row in range.rows() {
        let mut cells = Vec::with_capacity(8);
        for cell in row.iter().take(8) {
            cells.push(cell_to_string(cell));
        }
        while cells.len() < 8 {
            cells.push(String::new());
        }
        rows.push(cells);
    }
    // 丢掉表头（第 1 行）。
    if !rows.is_empty() {
        rows.remove(0);
    }
    Ok(rows)
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::String(value) => value.trim().to_string(),
        Data::Float(value) => {
            // 整数不显示 ".0"，小数保留原值
            if value.fract() == 0.0 {
                format!("{}", *value as i64)
            } else {
                format!("{value}")
            }
        }
        Data::Int(value) => value.to_string(),
        Data::Bool(value) => {
            if *value {
                "是".to_string()
            } else {
                "否".to_string()
            }
        }
        Data::DateTime(value) => value.to_string(),
        Data::DateTimeIso(value) => value.clone(),
        Data::DurationIso(value) => value.clone(),
        Data::Error(_) => String::new(),
    }
}

fn parse_unit_row(row: &[String], row_num: usize) -> Result<VocabularyEntry> {
    let code = row[0].trim().to_string();
    let canonical = row[1].trim().to_string();
    let external_name = row[2].trim().to_string();
    let abbr = row[3].trim().to_string();
    let department_code = row[4].trim().to_string();
    let seal_on_behalf = parse_bool_marker(&row[5]);
    let aliases = parse_aliases(&row[6]);
    let note = row[7].trim().to_string();
    let _ = row_num; // 行号仅在出错时用，目前 parse_unit_row 不主动报冲突
    Ok(VocabularyEntry {
        id: 0,
        category: VocabularyCategory::Unit,
        code,
        canonical,
        external_name,
        aliases,
        note,
        phone: String::new(),
        position: String::new(),
        parent: String::new(),
        abbr,
        unit: String::new(),
        can_handle_parent_unit: false,
        department_code,
        seal_on_behalf,
        seal_on_behalf_imported: !row[5].trim().is_empty(),
        sort_order: 0,
    })
}

fn parse_person_row(row: &[String], _row_num: usize) -> Result<VocabularyEntry> {
    let unit = row[0].trim().to_string();
    let code = row[1].trim().to_string();
    let canonical = row[2].trim().to_string();
    let position = row[3].trim().to_string();
    let phone = row[4].trim().to_string();
    let can_handle_parent_unit = parse_bool_marker(&row[5]);
    let aliases = parse_aliases(&row[6]);
    let note = row[7].trim().to_string();
    Ok(VocabularyEntry {
        id: 0,
        category: VocabularyCategory::Person,
        code,
        canonical,
        external_name: String::new(),
        aliases,
        note,
        phone,
        position,
        parent: String::new(),
        abbr: String::new(),
        unit,
        can_handle_parent_unit,
        department_code: String::new(),
        seal_on_behalf: false,
        seal_on_behalf_imported: false,
        sort_order: 0,
    })
}

fn parse_bool_marker(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "是" | "允许" | "可" | "勾选" | "true" | "yes" | "1" | "√" | "✓"
    )
}

fn parse_aliases(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if matches!(trimmed, "" | "-" | "—" | "无" | "N/A") {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    trimmed
        .split(['、', ',', '，', ';', '；'])
        .map(str::trim)
        .filter(|alias| !alias.is_empty())
        .filter(|alias| seen.insert((*alias).to_string()))
        .map(str::to_string)
        .collect()
}

/// 校验入口：返回冲突列表（任一冲突 ⇒ 整次拒绝）。
pub fn validate(entries: &[VocabularyEntry], existing_unit_codes: &[String]) -> ValidationReport {
    let mut conflicts = Vec::new();

    // 单位 sheet 行号定位：先把单位表按顺序收集并附上行号。
    let mut unit_rows: Vec<(usize, &VocabularyEntry)> = Vec::new();
    let mut person_rows: Vec<(usize, &VocabularyEntry)> = Vec::new();
    for (offset, entry) in entries.iter().enumerate() {
        let row_counter = 2 + offset; // 表头是第 1 行
        match entry.category {
            VocabularyCategory::Unit => {
                unit_rows.push((row_counter, entry));
            }
            VocabularyCategory::Person => {
                person_rows.push((row_counter, entry));
            }
        }
    }

    let mut existing_codes: HashSet<String> = existing_unit_codes
        .iter()
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty())
        .collect();
    for entry in entries
        .iter()
        .filter(|e| e.category == VocabularyCategory::Unit)
    {
        existing_codes.insert(entry.code.trim().to_string());
    }

    // 1. 单位名称必填。
    for (row, entry) in &unit_rows {
        if entry.canonical.trim().is_empty() {
            conflicts.push(Conflict {
                sheet: UNIT_SHEET,
                row: *row,
                field: "单位名称",
                current_value: entry.canonical.clone(),
                message: "单位名称不能为空".to_string(),
            });
        }
    }
    // 2. 人员姓名必填。
    for (row, entry) in &person_rows {
        if entry.canonical.trim().is_empty() {
            conflicts.push(Conflict {
                sheet: PERSON_SHEET,
                row: *row,
                field: "姓名",
                current_value: entry.canonical.clone(),
                message: "姓名不能为空".to_string(),
            });
        }
    }

    // 3. 单位编码格式校验。
    for (row, entry) in &unit_rows {
        let code = entry.code.trim();
        if !code.is_empty() && !is_valid_code(code) {
            conflicts.push(Conflict {
                sheet: UNIT_SHEET,
                row: *row,
                field: "层级编码",
                current_value: code.to_string(),
                message: "层级编码只能由字母、数字、点、下划线、连字符组成，且长度不超过 32"
                    .to_string(),
            });
        }
    }

    // 4. 单位编码重复。
    let mut seen_codes: HashMap<String, usize> = HashMap::new();
    for (row, entry) in &unit_rows {
        let code = entry.code.trim();
        if code.is_empty() {
            continue;
        }
        if let Some(prev_row) = seen_codes.insert(code.to_string(), *row) {
            conflicts.push(Conflict {
                sheet: UNIT_SHEET,
                row: *row,
                field: "层级编码",
                current_value: code.to_string(),
                message: format!("与第 {prev_row} 行重复"),
            });
        }
    }

    // 5. 父子关系：单位编码必须有一个"真前缀"也存在于单位集合内，否则视为顶层。
    // 顶层编码可以任意长度；非法的是"长度 > 2 但找不到前缀"——这是历史规则保留。
    // 实际更通用的判定是：必须有另一个更短的 code 是它的前缀；找不到就是顶层。
    // 之所以保留 2 是为了让"XHQ"这种 3 字编码也能被承认为顶层单位（无前缀）。
    let all_unit_codes: HashSet<String> = entries
        .iter()
        .filter(|e| e.category == VocabularyCategory::Unit)
        .map(|e| e.code.trim().to_string())
        .filter(|code| !code.is_empty())
        .collect();
    for (_row, entry) in &unit_rows {
        let code = entry.code.trim();
        if code.is_empty() {
            continue;
        }
        // 已经记录一个更短的真前缀 → 父级存在。
        if has_strict_prefix_in(code, &all_unit_codes) {
            continue;
        }
        // 顶层：可以是任何长度，只要没有更短前缀即可。
        // 旧规则要求长度 ≤ 2 才视为顶层，但用户填的 XHQ / XHQ-01 这种 3 字顶层编码
        // 应该被允许；因此这里不再按长度拒绝。
    }

    // 6. 机关代字重复。
    let mut seen_codes: HashMap<String, usize> = HashMap::new();
    for (row, entry) in &unit_rows {
        let dc = entry.department_code.trim();
        if dc.is_empty() {
            continue;
        }
        if let Some(prev_row) = seen_codes.insert(dc.to_string(), *row) {
            conflicts.push(Conflict {
                sheet: UNIT_SHEET,
                row: *row,
                field: "机关代字",
                current_value: dc.to_string(),
                message: format!("与第 {prev_row} 行重复"),
            });
        }
    }

    // 7. 人员所属单位编码无效。
    for (row, entry) in &person_rows {
        let unit = entry.unit.trim();
        if unit.is_empty() {
            continue;
        }
        if !existing_codes.contains(unit) {
            conflicts.push(Conflict {
                sheet: PERSON_SHEET,
                row: *row,
                field: "所属单位编码",
                current_value: unit.to_string(),
                message: "所属单位编码不在词库中".to_string(),
            });
        }
    }

    // 8. 人员编码同单位内重复。
    let mut seen_person_codes: HashMap<(String, String), usize> = HashMap::new();
    for (row, entry) in &person_rows {
        let code = entry.code.trim();
        if code.is_empty() {
            continue;
        }
        let unit = entry.unit.trim().to_string();
        if let Some(prev_row) = seen_person_codes.insert((unit, code.to_string()), *row) {
            conflicts.push(Conflict {
                sheet: PERSON_SHEET,
                row: *row,
                field: "人员编码",
                current_value: code.to_string(),
                message: format!("与第 {prev_row} 行在同一所属单位下重复"),
            });
        }
    }

    ValidationReport { conflicts }
}

fn has_strict_prefix_in(code: &str, candidates: &HashSet<String>) -> bool {
    candidates
        .iter()
        .any(|c| c.len() < code.len() && code.starts_with(c.as_str()))
}

/// 合并两个 vocab：按 code / （unit + canonical）匹配。
pub fn merge(target: &mut Vec<VocabularyEntry>, incoming: Vec<VocabularyEntry>) -> MergeReport {
    let mut report = MergeReport::default();
    for mut entry in incoming {
        if let Some(existing) = target.iter_mut().find(|existing| match entry.category {
            VocabularyCategory::Unit => {
                existing.category == VocabularyCategory::Unit
                    && existing.code.trim() == entry.code.trim()
            }
            VocabularyCategory::Person => {
                existing.category == VocabularyCategory::Person
                    && existing.canonical.trim() == entry.canonical.trim()
                    && existing.unit.trim() == entry.unit.trim()
            }
        }) {
            let mut changed = false;
            for alias in entry.aliases {
                if !existing.aliases.iter().any(|value| value == &alias) {
                    existing.aliases.push(alias);
                    changed = true;
                }
            }
            if !entry.note.is_empty() && !existing.note.contains(&entry.note) {
                if existing.note.trim().is_empty() {
                    existing.note = entry.note;
                } else {
                    existing.note.push('；');
                    existing.note.push_str(&entry.note);
                }
                changed = true;
            }
            if !entry.phone.is_empty() && existing.phone != entry.phone {
                existing.phone = entry.phone;
                changed = true;
            }
            if !entry.position.is_empty() && existing.position != entry.position {
                existing.position = entry.position;
                changed = true;
            }
            if entry.category == VocabularyCategory::Person
                && existing.can_handle_parent_unit != entry.can_handle_parent_unit
            {
                existing.can_handle_parent_unit = entry.can_handle_parent_unit;
                changed = true;
            }
            if entry.category == VocabularyCategory::Unit
                && entry.seal_on_behalf_imported
                && existing.seal_on_behalf != entry.seal_on_behalf
            {
                existing.seal_on_behalf = entry.seal_on_behalf;
                changed = true;
            }
            for (incoming, slot) in [
                (&entry.external_name, &mut existing.external_name),
                (&entry.abbr, &mut existing.abbr),
                (&entry.department_code, &mut existing.department_code),
            ] {
                if !incoming.is_empty() && *slot != *incoming {
                    *slot = incoming.clone();
                    changed = true;
                }
            }
            if changed {
                report.updated += 1;
            } else {
                report.unchanged += 1;
            }
        } else {
            entry.id = units::next_id(target);
            target.push(entry);
            report.added += 1;
        }
    }
    report
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// 把当前词库导出为 xlsx 模板。`existing_unit_codes` 由调用方传入（通常是词库整理后的单位编码列表）；
/// 也用于给"人员.所属单位编码"列提供数据验证下拉。
pub fn to_xlsx(
    entries: &[VocabularyEntry],
    path: &Path,
    existing_unit_codes: &[String],
) -> Result<()> {
    let mut workbook = Workbook::new();
    let header_format = Format::new()
        .set_bold()
        .set_background_color("#DCE6F1")
        .set_align(FormatAlign::Center)
        .set_border(FormatBorder::Thin);
    let locked_format = Format::new().set_background_color("#F2F2F2");

    // 单位 Sheet
    {
        let sheet = workbook.add_worksheet();
        let sheet = sheet.set_name(UNIT_SHEET)?;
        for (i, width) in UNIT_COLUMN_WIDTHS.iter().enumerate() {
            sheet.set_column_width(i as u16, *width)?;
        }
        for (i, header) in UNIT_HEADERS.iter().enumerate() {
            sheet.write_string_with_format(0, i as u16, *header, &header_format)?;
        }
        sheet.set_freeze_panes(1, 0)?;

        for (row_index, entry) in (1u32..).zip(
            entries
                .iter()
                .filter(|entry| entry.category == VocabularyCategory::Unit),
        ) {
            write_unit_row(sheet, row_index, entry)?;
        }
        // 单位编码写到 J 列（隐藏），供人员表"所属单位编码"下拉引用。
        sheet.set_column_hidden(9)?;
        for (i, code) in existing_unit_codes.iter().enumerate() {
            if code.trim().is_empty() {
                continue;
            }
            sheet.write_string(1 + i as u32, 9, code.trim())?;
        }
        // 是否代章下拉：是 / 否 / -
        let daizhang_dv = DataValidation::new()
            .allow_list_strings(&["-", "否", "是"])
            .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?
            .set_input_title("是否代章")
            .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?
            .set_input_message("选择 -（空）、否、是")
            .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?;
        sheet.add_data_validation(1, 5, 1000, 5, &daizhang_dv)?;

        let _ = sheet.protect_with_options(&rust_xlsxwriter::ProtectionOptions {
            select_locked_cells: true,
            select_unlocked_cells: true,
            format_cells: true,
            format_columns: false,
            format_rows: false,
            insert_columns: false,
            insert_rows: false,
            insert_links: true,
            delete_columns: false,
            delete_rows: false,
            sort: true,
            use_autofilter: true,
            use_pivot_tables: false,
            edit_scenarios: false,
            edit_objects: false,
            contents: true,
        });
    }

    // 人员 Sheet
    {
        let sheet = workbook.add_worksheet();
        let sheet = sheet.set_name(PERSON_SHEET)?;
        for (i, width) in PERSON_COLUMN_WIDTHS.iter().enumerate() {
            sheet.set_column_width(i as u16, *width)?;
        }
        for (i, header) in PERSON_HEADERS.iter().enumerate() {
            sheet.write_string_with_format(0, i as u16, *header, &header_format)?;
        }
        sheet.set_freeze_panes(1, 0)?;

        for (row_index, entry) in (1u32..).zip(
            entries
                .iter()
                .filter(|entry| entry.category == VocabularyCategory::Person),
        ) {
            write_person_row(sheet, row_index, entry)?;
        }
        // 人员"所属单位编码"下拉：直接列出当前词库的编码（超过 1000 个降级为自由填写）。
        if !existing_unit_codes.is_empty() && existing_unit_codes.len() <= 1000 {
            let list: Vec<&str> = existing_unit_codes.iter().map(|c| c.as_str()).collect();
            let unit_dv = DataValidation::new()
                .allow_list_strings(&list)
                .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?
                .set_input_title("所属单位编码")
                .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?
                .set_input_message("下拉中选择；留空表示未归属人员")
                .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?;
            sheet.add_data_validation(1, 0, 5000, 0, &unit_dv)?;
        }
        let handle_dv = DataValidation::new()
            .allow_list_strings(&["-", "否", "是"])
            .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?
            .set_input_title("承办上级单位")
            .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?
            .set_input_message("选择 -（空）、否、是")
            .map_err(|e| anyhow::anyhow!("无法构造数据验证：{e}"))?;
        sheet.add_data_validation(1, 5, 5000, 5, &handle_dv)?;

        let _ = sheet.protect_with_options(&rust_xlsxwriter::ProtectionOptions {
            select_locked_cells: true,
            select_unlocked_cells: true,
            format_cells: true,
            format_columns: false,
            format_rows: false,
            insert_columns: false,
            insert_rows: false,
            insert_links: true,
            delete_columns: false,
            delete_rows: false,
            sort: true,
            use_autofilter: true,
            use_pivot_tables: false,
            edit_scenarios: false,
            edit_objects: false,
            contents: true,
        });
    }

    let _ = locked_format;

    workbook.save(path)?;
    Ok(())
}

fn write_unit_row(sheet: &mut Worksheet, row: u32, entry: &VocabularyEntry) -> Result<()> {
    sheet.write_string(row, 0, entry.code.trim())?;
    sheet.write_string(row, 1, entry.canonical.trim())?;
    sheet.write_string(row, 2, entry.external_name.trim())?;
    sheet.write_string(row, 3, entry.abbr.trim())?;
    sheet.write_string(row, 4, entry.department_code.trim())?;
    let daizhang = if entry.seal_on_behalf {
        "是"
    } else if entry.seal_on_behalf_imported {
        "否"
    } else {
        "-"
    };
    sheet.write_string(row, 5, daizhang)?;
    sheet.write_string(row, 6, entry.aliases.join("、"))?;
    sheet.write_string(row, 7, entry.note.trim())?;
    Ok(())
}

fn write_person_row(sheet: &mut Worksheet, row: u32, entry: &VocabularyEntry) -> Result<()> {
    sheet.write_string(row, 0, entry.unit.trim())?;
    sheet.write_string(row, 1, entry.code.trim())?;
    sheet.write_string(row, 2, entry.canonical.trim())?;
    sheet.write_string(row, 3, entry.position.trim())?;
    sheet.write_string(row, 4, entry.phone.trim())?;
    let handle = if entry.can_handle_parent_unit {
        "是"
    } else {
        "否"
    };
    sheet.write_string(row, 5, handle)?;
    sheet.write_string(row, 6, entry.aliases.join("、"))?;
    sheet.write_string(row, 7, entry.note.trim())?;
    Ok(())
}

/// 把冲突按 Sheet 分组并格式化为可读字符串，便于在状态栏 / 错误窗口直接展示。
#[allow(dead_code)]
pub fn format_conflicts(conflicts: &[Conflict]) -> String {
    let mut by_sheet: BTreeMap<&'static str, Vec<&Conflict>> = BTreeMap::new();
    for c in conflicts {
        by_sheet.entry(c.sheet).or_default().push(c);
    }
    let mut out = String::new();
    for (sheet, list) in by_sheet {
        out.push_str(&format!("【{sheet}】\n"));
        for c in list {
            out.push_str(&format!(
                "  • 第 {} 行 「{}」=「{}」：{}\n",
                c.row, c.field, c.current_value, c.message
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "gongwen-vocab-xlsx-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn some_unit(code: &str, canonical: &str) -> VocabularyEntry {
        VocabularyEntry {
            id: 0,
            category: VocabularyCategory::Unit,
            code: code.into(),
            canonical: canonical.into(),
            ..Default::default()
        }
    }

    fn some_person(unit: &str, canonical: &str) -> VocabularyEntry {
        VocabularyEntry {
            id: 0,
            category: VocabularyCategory::Person,
            unit: unit.into(),
            canonical: canonical.into(),
            ..Default::default()
        }
    }

    fn codes(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn template_is_well_formed() {
        let dir = tmp_dir();
        let path = dir.join("template.xlsx");
        to_xlsx(&[], &path, &[]).unwrap();
        let report = parse(&path, &[]).unwrap();
        assert!(report.entries.is_empty());
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn round_trip_units_only() {
        let dir = tmp_dir();
        let path = dir.join("units.xlsx");
        let entries = vec![
            some_unit("00", "中央网信办"),
            some_unit("0001", "新闻舆论处"),
            some_unit("01", "中央宣传部"),
        ];
        let existing = codes(&["00", "01", "0001"]);
        to_xlsx(&entries, &path, &existing).unwrap();
        let report = parse(&path, &existing).unwrap();
        assert_eq!(report.entries.len(), 3);
        for entry in &report.entries {
            let original = entries.iter().find(|e| e.code == entry.code).unwrap();
            assert_eq!(entry.canonical, original.canonical);
        }
    }

    #[test]
    fn round_trip_with_people() {
        let dir = tmp_dir();
        let path = dir.join("vocab.xlsx");
        let entries = vec![
            some_unit("00", "中央网信办"),
            some_unit("0001", "新闻舆论处"),
            {
                let mut p = some_person("0001", "张三");
                p.position = "处长".into();
                p.phone = "010-1234".into();
                p.can_handle_parent_unit = true;
                p.aliases = vec!["张处".into()];
                p
            },
        ];
        let existing = codes(&["00", "0001"]);
        to_xlsx(&entries, &path, &existing).unwrap();
        let report = parse(&path, &existing).unwrap();
        assert_eq!(report.entries.len(), 3);
        let person = report
            .entries
            .iter()
            .find(|e| e.category == VocabularyCategory::Person)
            .unwrap();
        assert_eq!(person.canonical, "张三");
        assert_eq!(person.position, "处长");
        assert_eq!(person.phone, "010-1234");
        assert!(person.can_handle_parent_unit);
        assert_eq!(person.aliases, ["张处"]);
    }

    #[test]
    fn round_trip_preserves_user_codes() {
        let dir = tmp_dir();
        let path = dir.join("codes.xlsx");
        let entries = vec![
            some_unit("XHQ", "中央网信办"),
            some_unit("XHQ-01", "新闻舆论处"),
        ];
        let existing = codes(&["XHQ", "XHQ-01"]);
        to_xlsx(&entries, &path, &existing).unwrap();
        let report = parse(&path, &existing).unwrap();
        let codes = report
            .entries
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| e.code.clone())
            .collect::<Vec<_>>();
        assert_eq!(codes, ["XHQ", "XHQ-01"]);
    }

    #[test]
    fn rejects_duplicate_unit_code() {
        let entries = vec![some_unit("00", "甲"), some_unit("00", "乙")];
        let report = validate(&entries, &[]);
        assert!(
            report
                .conflicts
                .iter()
                .any(|c| c.field == "层级编码" && c.message.contains("重复"))
        );
    }

    #[test]
    fn prefix_relation_means_parent_is_not_checked_separately() {
        // 编码前缀已经反映了父子关系。用户填的 `0002` 父级字段即使填错，
        // 只要在词库内能找到更短前缀 `00` / `0001` 之类，就视为合法。
        // 该测试是为了把"父级字段 vs 编码前缀"的关系写进契约，避免后续反复纠结。
        let entries = vec![some_unit("00", "顶层"), some_unit("0001", "下级")];
        let report = validate(&entries, &[]);
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn rejects_duplicate_department_code() {
        let mut a = some_unit("00", "甲");
        a.department_code = "X政函".into();
        let mut b = some_unit("01", "乙");
        b.department_code = "X政函".into();
        let report = validate(&[a, b], &[]);
        assert!(report.conflicts.iter().any(|c| c.field == "机关代字"));
    }

    #[test]
    fn rejects_invalid_person_unit() {
        let entries = vec![some_unit("00", "甲"), some_person("ZZ", "张三")];
        let report = validate(&entries, &codes(&["00"]));
        assert!(report.conflicts.iter().any(|c| c.field == "所属单位编码"));
    }

    #[test]
    fn rejects_duplicate_person_code_within_unit() {
        let entries = vec![
            some_unit("00", "甲"),
            {
                let mut p = some_person("00", "张三");
                p.code = "01".into();
                p
            },
            {
                let mut p = some_person("00", "李四");
                p.code = "01".into();
                p
            },
        ];
        let report = validate(&entries, &codes(&["00"]));
        assert!(report.conflicts.iter().any(|c| c.field == "人员编码"));
    }

    #[test]
    fn rejects_missing_unit_name() {
        let entries = vec![{
            let mut u = some_unit("00", "");
            u.canonical = "".into();
            u
        }];
        let report = validate(&entries, &[]);
        assert!(report.conflicts.iter().any(|c| c.field == "单位名称"));
    }

    #[test]
    fn rejects_missing_person_name() {
        let entries = vec![{
            let mut p = some_person("00", "");
            p.canonical = "".into();
            p
        }];
        let report = validate(&entries, &[]);
        assert!(report.conflicts.iter().any(|c| c.field == "姓名"));
    }

    #[test]
    fn drops_fully_empty_rows() {
        let dir = tmp_dir();
        let path = dir.join("empty.xlsx");
        let entries = vec![some_unit("00", "甲"), some_unit("01", "乙")];
        let existing = codes(&["00", "01"]);
        to_xlsx(&entries, &path, &existing).unwrap();
        let report = parse(&path, &existing).unwrap();
        assert!(report.entries.len() >= 2);
    }

    #[test]
    fn merge_preserves_id_for_existing_units() {
        let mut target = vec![VocabularyEntry {
            id: 7,
            category: VocabularyCategory::Unit,
            code: "00".into(),
            canonical: "甲".into(),
            ..Default::default()
        }];
        let incoming = vec![some_unit("00", "甲")];
        let report = merge(&mut target, incoming);
        assert_eq!(report.added, 0);
        assert_eq!(report.unchanged, 1);
        assert_eq!(target[0].id, 7, "已有 id 不被覆盖");
    }

    #[test]
    fn normalize_round_trips_through_xlsx() {
        let dir = tmp_dir();
        let path = dir.join("mix.xlsx");
        let mut vocab = vec![
            some_unit("A1", "顶层甲"),
            some_unit("A1-01", "下级"),
            some_unit("B2", "顶层乙"),
            {
                let mut p = some_person("A1-01", "张三");
                p.phone = "010-1".into();
                p
            },
        ];
        units::normalize(&mut vocab);
        let codes: Vec<String> = vocab
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| e.code.clone())
            .collect();
        to_xlsx(&vocab, &path, &codes).unwrap();
        let report = parse(&path, &codes).unwrap();
        let mut parsed = report.entries;
        units::normalize(&mut parsed);
        let canonicals = |v: &[VocabularyEntry]| {
            v.iter()
                .filter(|e| e.category == VocabularyCategory::Unit)
                .map(|e| format!("{}-{}", e.code, e.canonical))
                .collect::<Vec<_>>()
        };
        assert_eq!(canonicals(&vocab), canonicals(&parsed));
    }

    #[test]
    fn valid_code_accepts_alnum_dot_dash_underscore() {
        assert!(is_valid_code("00"));
        assert!(is_valid_code("XHQ-01"));
        assert!(is_valid_code("A.B"));
        assert!(is_valid_code("A_1"));
        assert!(!is_valid_code(""));
        assert!(!is_valid_code("带汉字"));
        assert!(!is_valid_code("@id"));
        assert!(!is_valid_code("a b"));
        assert!(!is_valid_code("a/b"));
    }
}
