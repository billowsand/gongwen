//! 通用 UI 部件与辅助函数（表单行、下拉、截断、文件打开等）。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::widgets`，与其它子模块共享
//! `app` 根模块的私有可见性（`GongwenApp` 结构体与根模块常量仍在 app.rs 中）。

use crate::app::{CONTENT_WIDTH, FORM_CONTROL_HEIGHT, LABEL_WIDTH, MANUAL_BACK_WIDTH, warn};
use crate::export;
use crate::manuscript;
use crate::models::{
    DraftInput, ExportSelection, FontConfig, ManuscriptStatus, ReviewNote, SecurityLevel,
    VocabularyCategory, VocabularyEntry, join_units, split_units,
};
use crate::orphan_probe;
use crate::system_fonts;
use crate::texcompile;
use crate::theme;
use crate::units::UnitDisplay;
use eframe::egui;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(crate) fn row_label(ui: &mut egui::Ui, label: &str) {
    form_row_label(ui, label);
}

/// 说明直接附在字段名称上，悬停文字即可查看，不额外占用图标宽度。
pub(crate) fn row_label_with_info(ui: &mut egui::Ui, label: &str, tip: impl Into<String>) {
    form_row_label(ui, label).on_hover_text(tip.into());
}

/// 章节标题带图标；说明挂在标题文字本身，图标只作视觉锚点不占额外行。
pub(crate) fn section_heading_with_info(
    ui: &mut egui::Ui,
    icon: theme::Icon,
    heading: &str,
    tip: impl Into<String>,
) {
    ui.horizontal(|ui| {
        ui.add(
            icon.image()
                .tint(theme::text_soft())
                .fit_to_exact_size(egui::vec2(16.0, 16.0)),
        );
        ui.heading(heading).on_hover_text(tip.into());
    });
}

pub(crate) fn form_row_label(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let height = if label.contains('\n') {
        FORM_CONTROL_HEIGHT + 14.0
    } else {
        FORM_CONTROL_HEIGHT
    };
    ui.add_sized(
        [LABEL_WIDTH, height],
        egui::Label::new(label).wrap_mode(egui::TextWrapMode::Wrap),
    )
}

/// 表格单元格默认不换行，长说明必须给定宽度并显式开启换行，
/// 否则会把整个可调整面板顶宽。
pub(crate) fn wrapped_hint(ui: &mut egui::Ui, text: impl Into<String>, width: f32) {
    ui.add_sized(
        [sane_width(width), 18.0],
        egui::Label::new(egui::RichText::new(text.into()).weak())
            .wrap_mode(egui::TextWrapMode::Wrap),
    );
}

/// `available_width()` 在部分容器里会是无穷大，直接交给 `add_sized` 会让布局失效。
pub(crate) fn sane_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(80.0, 2000.0)
    } else {
        *CONTENT_WIDTH.end()
    }
}

pub(crate) fn present_or_dash(value: &str) -> &str {
    if value.trim().is_empty() {
        "—"
    } else {
        value
    }
}

pub(crate) fn joined_metadata(values: &[&str]) -> String {
    let joined = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    if joined.is_empty() {
        "—".into()
    } else {
        joined
    }
}

pub(crate) fn metadata_grid_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).color(theme::text_muted()));
    ui.add(egui::Label::new(present_or_dash(value)).wrap_mode(egui::TextWrapMode::Wrap));
    ui.end_row();
}

/// 稿件状态的徽标颜色：新建（仅历史遗留）灰、草稿蓝、发布绿、归档橙。
pub(crate) fn status_color(status: ManuscriptStatus) -> egui::Color32 {
    match status {
        ManuscriptStatus::New => theme::text_muted(),
        ManuscriptStatus::Draft => theme::accent(),
        ManuscriptStatus::Published => theme::success(),
        ManuscriptStatus::Archived => theme::info(),
    }
}

/// 稿件列表密级颜色：由蓝到红逐级增强；未标密使用弱化文字。
pub(crate) fn security_level_color(level: SecurityLevel) -> egui::Color32 {
    match level {
        SecurityLevel::Unmarked => theme::text_muted(),
        SecurityLevel::Internal => theme::info(),
        SecurityLevel::Secret => theme::warn(),
        SecurityLevel::Confidential => theme::accent_active(),
        SecurityLevel::TopSecret => theme::danger(),
    }
}

pub(crate) fn security_level_list_label(level: SecurityLevel) -> &'static str {
    match level {
        SecurityLevel::Unmarked => "—",
        _ => level.marking(),
    }
}

/// 时间显示：中文成文日期原样返回；RFC3339/ISO 日期只取前 10 位（YYYY-MM-DD）。
pub(crate) fn short_date(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "—".to_string();
    }
    if value.contains('年') {
        return value.to_string();
    }
    value.chars().take(10).collect()
}

/// 截断字符串并追加省略号，避免把表格/预览行撑得过宽。
/// 中部省略。公文标题两头都有信息量——开头是事由、结尾是文种，
/// 掐掉尾巴会让"…的函"和"…的通知"看起来一模一样。
pub(crate) fn truncate_middle(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head = max.div_ceil(2) - 1;
    let tail = max - head - 1;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - tail..]);
    out
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// 多行文本的单行摘要：先把换行和连续空白压成单个空格，再截断。
/// 提示词指令通常带手写换行，直接 `truncate` 会把列表撑成几行高。
pub(crate) fn summarize(text: &str, max: usize) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&flattened, max)
}

/// 版本名默认值：时间戳精确到分。
pub(crate) fn default_version_name() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()
}

/// 给默认版本名去重：同名时追加 `-2`、`-3`……
pub(crate) fn unique_version_name(existing: &[String], base: &str) -> String {
    if !existing.iter().any(|name| name == base) {
        return base.to_string();
    }
    let mut index = 2;
    loop {
        let candidate = format!("{base}-{index}");
        if !existing.iter().any(|name| name == &candidate) {
            return candidate;
        }
        index += 1;
    }
}

/// 版本下拉里的一行悬停详情。
pub(crate) fn version_hover(row: &manuscript::VersionRow) -> String {
    let mut out = format!("v{} · {}\n", row.version_number, row.name);
    if !row.comment.is_empty() {
        out.push_str(&format!("注释：{}\n", row.comment));
    }
    out.push_str(&format!(
        "该版标题：{}\n文号：{}\n成文日期：{}\n提交于 {}",
        row.title,
        if row.doc_number.is_empty() {
            "—"
        } else {
            &row.doc_number
        },
        if row.doc_date.is_empty() {
            "—"
        } else {
            &row.doc_date
        },
        short_date(&row.created_at),
    ));
    out
}

/// 版本下拉的选中文案。
pub(crate) fn version_label(rows: &[manuscript::VersionRow], number: i64) -> String {
    rows.iter()
        .find(|row| row.version_number == number)
        .map_or_else(
            || format!("v{number}"),
            |row| {
                format!(
                    "v{} · {}{}",
                    row.version_number,
                    row.name,
                    if row.is_latest { " · 最新" } else { "" }
                )
            },
        )
}

pub(crate) fn vocabulary_matches(entry: &VocabularyEntry, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    format!(
        "{} {} {} {} {} {} {} {} {} {} {} {}",
        entry.code,
        entry.canonical,
        entry.external_name,
        entry.abbr,
        entry.department_code,
        entry.parent,
        entry.unit,
        entry.aliases.join(" "),
        entry.note,
        entry.phone,
        entry.position,
        if entry.seal_on_behalf { "代章" } else { "" }
    )
    .to_lowercase()
    .contains(filter)
}

/// 新建词条时给一个便于立即编辑的占位名称；正式关联使用单位编码，同名是允许的。
pub(crate) fn unique_name(
    vocab: &[VocabularyEntry],
    category: VocabularyCategory,
    base: &str,
) -> String {
    let taken = |candidate: &str| {
        vocab
            .iter()
            .any(|entry| entry.category == category && entry.canonical.trim() == candidate)
    };
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|suffix| format!("{base}{suffix}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_string())
}

/// 标准词库树在界面上的缩进深度。单位以编码关联上级，人员以所属单位编码挂靠，
/// 因此名称是否重复不会影响任意层级的显示。
pub(crate) fn vocabulary_depths(vocab: &[VocabularyEntry]) -> Vec<usize> {
    let mut depth_by_unit: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    vocab
        .iter()
        .map(|entry| match entry.category {
            VocabularyCategory::Unit => {
                let parent = entry.parent.trim();
                let depth = if parent.is_empty() {
                    0
                } else {
                    depth_by_unit
                        .get(parent)
                        .map(|depth| depth + 1)
                        .unwrap_or(0)
                };
                depth_by_unit.insert(entry.code.trim(), depth);
                depth
            }
            VocabularyCategory::Person => depth_by_unit
                .get(entry.unit.trim())
                .map(|depth| depth + 1)
                .unwrap_or(0),
        })
        .collect()
}

/// 按当前可见高度换算编辑框行数；必须在进入 `ScrollArea` 之前调用。
pub(crate) fn visible_rows(ui: &egui::Ui) -> usize {
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace).max(1.0);
    let height = ui.available_height();
    if !height.is_finite() {
        return 24;
    }
    (((height - 12.0) / row_height).floor() as i64).clamp(8, 200) as usize
}

pub(crate) fn field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &str) -> bool {
    ui.horizontal(|ui| {
        row_label(ui, label);
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(f32::INFINITY),
        )
        .changed()
    })
    .inner
}

/// 起草页下拉框中的一个候选项。
/// 单位按层级显示：本列表中没有上级的显示全称，有上级的缩进一级、只显示本级名称，
/// 与成文时“同一上级不重复、跨上级用逗号”的写法对应。
/// 人员、机关代字等没有层级的字段用 `plain_options` 生成，层级一律为 0。
#[derive(Clone)]
pub(crate) struct SelectOption {
    /// 写入配置的值，即词库中的规范名称。
    pub(crate) value: String,
    /// 下拉列表中显示的文字。
    pub(crate) label: String,
    /// 展开后的完整名称，用于收起后的选中项和已选标签。
    pub(crate) full: String,
    /// 上级单位的规范名称，空串表示顶层；只有单位候选项会用到。
    pub(crate) parent: String,
    /// 缩进层级，只相对本列表中可见的上级计算。
    pub(crate) depth: usize,
}

/// 没有层级的候选项：标签就是取值本身。
pub(crate) fn plain_options(values: &[String]) -> Vec<SelectOption> {
    values
        .iter()
        .map(|value| SelectOption {
            value: value.clone(),
            label: value.clone(),
            full: value.clone(),
            parent: String::new(),
            depth: 0,
        })
        .collect()
}

/// 按层级摆放候选项：`allowed` 为 `Some` 时只保留其中的取值。
/// 缩进只相对“列表里还在的上级”计算——找得到上级的缩进一级、只显示本级名称，
/// 找不到的排在顶层、显示完整名称，这样过滤之后也不会出现没有上级却缩进的孤儿项。
pub(crate) fn layout_options(
    pool: &[SelectOption],
    allowed: Option<&[String]>,
) -> Vec<SelectOption> {
    let mut placed: Vec<SelectOption> = Vec::new();
    for option in pool.iter().filter(|option| {
        allowed.is_none_or(|allowed| allowed.iter().any(|item| item.trim() == option.value))
    }) {
        let mut ancestor = option.parent.clone();
        let mut parent_depth = None;
        while !ancestor.is_empty() {
            if let Some(found) = placed.iter().find(|kept| kept.value == ancestor) {
                parent_depth = Some(found.depth);
                break;
            }
            let Some(next) = pool.iter().find(|candidate| candidate.value == ancestor) else {
                break;
            };
            if next.parent == ancestor {
                break;
            }
            ancestor = next.parent.clone();
        }
        let (depth, label) = match parent_depth {
            Some(depth) => (depth + 1, option.label.clone()),
            None => (0, option.full.clone()),
        };
        placed.push(SelectOption {
            label,
            depth,
            ..option.clone()
        });
    }
    placed
}

/// 下拉列表中的显示文字。缩进写进文字本身，而不是靠嵌套布局，
/// 这样下拉框里整行仍然可以点击。用全角空格保证中文字体下的缩进宽度一致。
pub(crate) fn indented_label(option: &SelectOption) -> String {
    format!("{}{}", "　".repeat(option.depth), option.label)
}

/// 从标准词库中单选。允许手工输入时，右侧的“手填/选择”按钮在下拉与文本框之间切换。
#[allow(clippy::too_many_arguments)] // Shared form helper; call sites keep these options explicit.
pub(crate) fn single_select(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    options: &[SelectOption],
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    width: f32,
    hint: &str,
) -> bool {
    let mut changed = false;
    // 词库里没有候选项时只能手写，否则字段将无法填写。
    let manual = manual_fields.contains(id) || options.is_empty();
    // 收起状态显示完整名称，让下级单位也能一眼看出挂在哪个机关下面。
    let selected_text = if value.trim().is_empty() {
        "（未选择）".to_string()
    } else {
        options
            .iter()
            .find(|option| option.value == *value)
            .map(|option| option.full.clone())
            .unwrap_or_else(|| value.clone())
    };
    ui.horizontal(|ui| {
        if manual {
            if options.is_empty() && !allow_free_text {
                ui.colored_label(warn(), "标准词库中暂无该类词条，请先到“标准词库”页维护");
                return;
            }
            // 手填态才借走图标按钮那一格，其余时候整行宽度都归输入控件。
            let back = !options.is_empty();
            let text_width = if back {
                (width - MANUAL_BACK_WIDTH - 6.0).max(90.0)
            } else {
                width
            };
            changed = ui
                .add(
                    egui::TextEdit::singleline(value)
                        .hint_text(hint)
                        .desired_width(text_width),
                )
                .changed();
            if back {
                back_to_select_button(ui, id, manual_fields);
            }
        } else {
            egui::ComboBox::from_id_salt(id)
                .selected_text(selected_text)
                .width(width)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(value.trim().is_empty(), "（未选择）")
                        .clicked()
                    {
                        value.clear();
                        changed = true;
                    }
                    for option in options {
                        let response =
                            ui.selectable_label(*value == option.value, indented_label(option));
                        let response = if option.full == option.label {
                            response
                        } else {
                            response.on_hover_text(&option.full)
                        };
                        if response.clicked() {
                            *value = option.value.clone();
                            changed = true;
                        }
                    }
                    if allow_free_text {
                        manual_entry_item(ui, id, manual_fields);
                    }
                });
        }
    });
    changed
}

/// 联合发文承办单位 ↔ 联系人成对录入：每行一个承办单位 + 一个联系人（自动带出
/// 绑定电话），数量天然一致、一一对应，从源头避免“承办单位 3 个、联系人只有 2 个”
/// 这类错配；联系人按该行承办单位过滤（规格 §2.3“人随事走”）。返回本帧是否有改动。
#[allow(clippy::too_many_arguments)]
pub(crate) fn joint_responsible_editor(
    ui: &mut egui::Ui,
    profile: &mut crate::models::TemplateProfile,
    unit_options: &[SelectOption],
    vocabulary: &[VocabularyEntry],
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    width: f32,
    max_rows: Option<usize>,
) -> bool {
    use crate::models::{JointContact, JointResponsibleEntry, sync_joint_responsible};
    let display = crate::units::UnitDisplay::new(vocabulary);
    // 旧稿件可能只有承办单位字符串、没有联系人条目：补齐成对条目，避免既有单位
    // 丢失。行数以较大者为准——旧数据承办单位与联系人按索引配对，数量不必相等。
    {
        let units = split_units(&profile.joint_responsible_units);
        if units.len() > profile.joint_contacts.len() {
            profile
                .joint_contacts
                .extend(
                    units
                        .iter()
                        .skip(profile.joint_contacts.len())
                        .map(|unit| JointContact {
                            unit: unit.clone(),
                            name: String::new(),
                            phone: String::new(),
                        }),
                );
        }
    }
    // 在副本上编辑，避免渲染中与 profile 反复借用冲突；结束后整体写回并同步两个字段。
    let mut entries = profile.joint_contacts.clone();
    let mut changed = false;
    let mut remove_index = None;
    // 该编辑器位于 Grid 的横向单元格中：必须用 vertical 包住，每行 ui.horizontal
    // 才会向下堆叠；否则多个承办单位会在同一行向右排开。
    ui.vertical(|ui| {
        let row_count = entries.len();
        // 去掉常驻手填列后，这一行只剩「承办单位 + 联系人 + 删除」三样，
        // 省下的宽度全部还给两个下拉。
        let unit_width = ((width - 40.0) * 0.52).max(120.0);
        let contact_width = (width - unit_width - 40.0).max(96.0);
        for (index, entry) in entries.iter_mut().enumerate() {
            let unit_key = format!("joint_resp_unit_{index}");
            let unit_manual = manual_fields.contains(&unit_key) || unit_options.is_empty();
            let people = display.responsible_people_of(&entry.unit);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // 承办单位：下拉选择，或（允许手填时）切换为文本框。
                if unit_manual {
                    if unit_options.is_empty() && !allow_free_text {
                        ui.colored_label(warn(), "标准词库中暂无单位，请先到“标准词库”页维护");
                    } else {
                        let back = !unit_options.is_empty();
                        let text_width = if back {
                            (unit_width - MANUAL_BACK_WIDTH - 6.0).max(80.0)
                        } else {
                            unit_width
                        };
                        changed |= ui
                            .add(
                                egui::TextEdit::singleline(&mut entry.unit)
                                    .hint_text("承办单位")
                                    .desired_width(text_width),
                            )
                            .changed();
                        if back {
                            back_to_select_button(ui, &unit_key, manual_fields);
                        }
                    }
                } else {
                    let selected_text = if entry.unit.trim().is_empty() {
                        "（未选择）".to_string()
                    } else {
                        unit_options
                            .iter()
                            .find(|option| option.value == entry.unit)
                            .map(|option| option.full.clone())
                            .unwrap_or_else(|| entry.unit.clone())
                    };
                    egui::ComboBox::from_id_salt(&unit_key)
                        .selected_text(selected_text)
                        .width(unit_width)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(entry.unit.trim().is_empty(), "（未选择）")
                                .clicked()
                            {
                                entry.unit.clear();
                                changed = true;
                            }
                            for option in unit_options {
                                let response = ui.selectable_label(
                                    entry.unit == option.value,
                                    indented_label(option),
                                );
                                let response = if option.full == option.label {
                                    response
                                } else {
                                    response.on_hover_text(&option.full)
                                };
                                if response.clicked() {
                                    entry.unit = option.value.clone();
                                    changed = true;
                                }
                            }
                            if allow_free_text {
                                manual_entry_item(ui, &unit_key, manual_fields);
                            }
                        });
                }
                // 联系人：按该行承办单位过滤，选中后自动带出绑定电话。
                let contact_selected = if entry.name.trim().is_empty() {
                    "（未选择）".to_string()
                } else {
                    entry.name.clone()
                };
                egui::ComboBox::from_id_salt(format!("joint_resp_contact_{index}"))
                    .selected_text(contact_selected)
                    .width(contact_width)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(entry.name.trim().is_empty(), "（未选择）")
                            .clicked()
                        {
                            entry.name.clear();
                            entry.phone.clear();
                            changed = true;
                        }
                        if people.is_empty() && !entry.unit.trim().is_empty() {
                            ui.weak("该单位下暂无可选联系人");
                        }
                        for (candidate, phone) in &people {
                            if ui
                                .selectable_label(entry.name == *candidate, candidate)
                                .clicked()
                            {
                                entry.name = candidate.clone();
                                entry.phone = phone.clone();
                                changed = true;
                            }
                        }
                    });
                if row_count > 1
                    && theme::danger_icon_button(ui, theme::Icon::Trash, "删除该行").clicked()
                {
                    remove_index = Some(index);
                }
            });
            if remove_index.is_some() {
                break;
            }
        }
        if let Some(index) = remove_index {
            entries.remove(index);
            changed = true;
        }
        // 红头呈批件的承办区在首页底部，条目数有硬上限；到顶后停用按钮并说明原因。
        let full = max_rows.is_some_and(|max| entries.len() >= max);
        let response = ui.add_enabled(!full, egui::Button::new("＋ 添加一行"));
        if full && let Some(max) = max_rows {
            response.on_hover_text(format!("最多 {max} 条承办信息，首页承办区放不下更多"));
        } else if response.clicked() {
            entries.push(JointContact::default());
            changed = true;
        }
    });
    if changed {
        sync_joint_responsible(
            profile,
            &entries
                .iter()
                .map(|contact| JointResponsibleEntry {
                    unit: contact.unit.clone(),
                    name: contact.name.clone(),
                    phone: contact.phone.clone(),
                })
                .collect::<Vec<_>>(),
        );
    }
    changed
}

/// 主送、抄送等多值字段：勾选多个规范名称，界面上以标签形式回显，配置中以顿号分隔存储。
/// `excluded` 中的候选项已被另一个字段占用（主送与抄送互斥），只展示不可勾选。
#[allow(clippy::too_many_arguments)]
pub(crate) fn multi_select(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut String,
    options: &[SelectOption],
    excluded: &[String],
    excluded_reason: &str,
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    width: f32,
) -> bool {
    let mut changed = false;
    let manual = manual_fields.contains(id) || options.is_empty();
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            if manual {
                if options.is_empty() && !allow_free_text {
                    ui.colored_label(warn(), "标准词库中暂无该类词条，请先到“标准词库”页维护");
                    return;
                }
                let back = !options.is_empty();
                let text_width = if back {
                    (width - MANUAL_BACK_WIDTH - 6.0).max(90.0)
                } else {
                    width
                };
                changed = ui
                    .add(
                        egui::TextEdit::singleline(value)
                            .hint_text("多个单位用顿号“、”分隔")
                            .desired_width(text_width),
                    )
                    .changed();
                if back {
                    back_to_select_button(ui, id, manual_fields);
                }
            } else {
                let mut selected = split_units(value);
                egui::ComboBox::from_id_salt(id)
                    // 收起态直接报第一个已选项加计数：“已选 3 个”本身没有信息量，
                    // 而下面的标签行在折叠或滚动出屏时看不见。
                    .selected_text(match selected.len() {
                        0 => "（未选择）".to_string(),
                        1 => options
                            .iter()
                            .find(|option| option.value == selected[0])
                            .map(|option| option.full.clone())
                            .unwrap_or_else(|| selected[0].clone()),
                        n => {
                            let first = options
                                .iter()
                                .find(|option| option.value == selected[0])
                                .map(|option| option.label.clone())
                                .unwrap_or_else(|| selected[0].clone());
                            format!("{first} 等 {n} 个")
                        }
                    })
                    .width(width)
                    // 多选需要连续勾选，点一次就收起会很难用。
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show_ui(ui, |ui| {
                        for option in options {
                            let label = indented_label(option);
                            let mut checked = selected.contains(&option.value);
                            if !checked && excluded.contains(&option.value) {
                                // 互斥规则是硬约束，原因写在条目右边，不藏在悬停里。
                                ui.horizontal(|ui| {
                                    ui.add_enabled(false, egui::Checkbox::new(&mut false, label));
                                    ui.weak(egui::RichText::new(excluded_reason).size(11.0));
                                });
                                continue;
                            }
                            let response = ui.checkbox(&mut checked, label);
                            if option.full != option.label {
                                response.clone().on_hover_text(&option.full);
                            }
                            if response.changed() {
                                if checked {
                                    selected.push(option.value.clone());
                                } else {
                                    selected.retain(|item| *item != option.value);
                                }
                                changed = true;
                            }
                        }
                        if allow_free_text {
                            manual_entry_item(ui, id, manual_fields);
                        }
                    });
                // 无论是刚勾选还是配置里的旧值，一律按词库顺序回写：
                // 导出内容取决于词库排序，而不是勾选的先后顺序。
                let ordered = sort_by_vocabulary(selected, options);
                if ordered != split_units(value) {
                    *value = join_units(&ordered);
                    changed = true;
                }
            }
        });

        if !manual {
            let selected = split_units(value);
            if !selected.is_empty() {
                let mut remove = None;
                ui.set_max_width(width);
                ui.horizontal_wrapped(|ui| {
                    for (index, item) in selected.iter().enumerate() {
                        let known = options.iter().find(|option| option.value == *item);
                        // 已选标签显示完整名称，免得只看到“办公室”分不清是哪家的。
                        let display = known.map(|option| option.full.as_str()).unwrap_or(item);
                        let known = known.is_some();
                        let button = theme::removable_tag_button(display, !known);
                        let response = ui.add(button);
                        let response = if known {
                            response.on_hover_text("点击移除")
                        } else {
                            response.on_hover_text("该名称不在标准词库中，点击移除")
                        }
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if response.clicked() {
                            remove = Some(index);
                        }
                    }
                });
                if let Some(index) = remove {
                    let mut selected = selected;
                    selected.remove(index);
                    *value = join_units(&sort_by_vocabulary(selected, options));
                    changed = true;
                }
            }
        }
    });
    changed
}

/// 按标准词库中的先后顺序（即单位层级顺序）排列已选项；
/// 词库里没有的旧值保持原有相对顺序排在最后。
pub(crate) fn sort_by_vocabulary(mut units: Vec<String>, options: &[SelectOption]) -> Vec<String> {
    units.sort_by_key(|item| {
        options
            .iter()
            .position(|option| option.value == *item)
            .unwrap_or(usize::MAX)
    });
    units
}

/// 联系人与电话是一组：从词库选人时自动带出绑定电话，电话框只读回显。
pub(crate) fn contact_pair(
    ui: &mut egui::Ui,
    person: &mut String,
    phone: &mut String,
    contacts: &[(String, String)],
    manual_fields: &mut BTreeSet<String>,
    allow_free_text: bool,
    width: f32,
) {
    let names = plain_options(
        &contacts
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>(),
    );
    let manual = manual_fields.contains("contact_person") || names.is_empty();
    ui.vertical(|ui| {
        if single_select(
            ui,
            "contact_person",
            person,
            &names,
            manual_fields,
            allow_free_text,
            width,
            "姓名标准写法",
        ) && !manual
        {
            *phone = contacts
                .iter()
                .find(|(name, _)| name == person)
                .map(|(_, bound)| bound.clone())
                .unwrap_or_default();
        }
        ui.horizontal(|ui| {
            ui.label("电话");
            if manual {
                ui.add(
                    egui::TextEdit::singleline(phone)
                        .hint_text("座机或手机")
                        .desired_width((width - 40.0).max(110.0)),
                );
            } else if phone.trim().is_empty() {
                ui.colored_label(
                    warn(),
                    if person.trim().is_empty() {
                        "选择联系人后自动带出".to_string()
                    } else {
                        format!("“{}”在词库中未维护电话", person.trim())
                    },
                );
            } else {
                ui.label(phone.as_str())
                    .on_hover_text("电话与联系人在标准词库中绑定，改电话请到“标准词库”页");
            }
        });
    });
}

/// 下拉列表末尾的“临时手填”入口。手填不推荐、也很少用，却曾经在每一行占着
/// 72px 的常驻按钮——把它收进候选项列表末尾：用户翻开下拉、确认里面没有想要的
/// 词条，才会看见这一项，位置正好在“我找不到”的那一刻。
pub(crate) fn manual_entry_item(ui: &mut egui::Ui, id: &str, manual_fields: &mut BTreeSet<String>) {
    ui.separator();
    if ui
        .add(theme::menu_item(
            theme::Icon::PencilLine,
            "词库里没有？临时手填…",
        ))
        .on_hover_text("临时手工输入（不推荐，容易写错名称；正式用法请到“标准词库”页维护）")
        .clicked()
    {
        manual_fields.insert(id.to_string());
    }
}

/// 手填态下切回下拉选择的窄按钮：只占一个图标的位置。
pub(crate) fn back_to_select_button(
    ui: &mut egui::Ui,
    id: &str,
    manual_fields: &mut BTreeSet<String>,
) {
    if theme::icon_button(ui, theme::Icon::Book, "改回从标准词库选择").clicked() {
        manual_fields.remove(id);
    }
}

/// 用系统默认程序打开文件或文件夹。
pub(crate) fn open_in_os(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // explorer 成功时也可能返回非 0 退出码，只看能否启动。
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// 在文件管理器中定位并选中文件。
pub(crate) fn reveal_in_os(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // explorer 的 /select 参数不接受常规转义，必须整体作为原始命令行传入。
        std::process::Command::new("explorer")
            .raw_arg(format!("/select,\"{}\"", path.display()))
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = path.parent().unwrap_or(path);
        open_in_os(target)
    }
}

/// 本机是否具备打印能力。不具备时界面把「打印」按钮置灰，并引导改用
/// 「系统打开」后自行打印，而不是让人点了没反应。
///
/// Unix（含 macOS 与麒麟等国产系统）统一看 CUPS 的 `lp` 在不在 PATH 里：
/// macOS 自带，Linux 由 `cups-client` 提供，deb 包已把它列入依赖，但用户
/// 自行编译或用便携包时仍可能缺。Windows 走 shell 的 print 谓词，是否真能
/// 打印取决于系统注册的 PDF 处理程序，事前探测不出来，一律按可用处理，
/// 失败时由错误信息说明。
pub(crate) fn printing_available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(unix)]
    {
        lp_program().is_some()
    }
}

/// 在 PATH 里找 `lp`。找不到就说明没装 CUPS 客户端。
#[cfg(unix)]
fn lp_program() -> Option<PathBuf> {
    lp_in_path(&std::env::var_os("PATH")?)
}

/// 在给定的 PATH 串里找 `lp`。与环境变量解耦，便于单测。
#[cfg(unix)]
fn lp_in_path(path: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .map(|dir| dir.join("lp"))
        .find(|candidate| candidate.is_file())
}

/// 拼 Windows 上触发 print 谓词的 PowerShell 命令。
///
/// 单独拿出来是为了能测转义：路径里出现单引号（用户完全可能把稿件放在名字带
/// 撇号的目录下）时，不转义会让命令截断甚至改变语义。PowerShell 单引号字符串
/// 里的单引号要写成两个。非 Windows 上也编译，只为跑测试。
#[cfg_attr(not(windows), allow(dead_code))]
fn print_verb_script(path: &Path) -> String {
    format!(
        "Start-Process -FilePath '{}' -Verb Print",
        path.display().to_string().replace('\'', "''")
    )
}

/// 把 PDF 送到系统默认打印机。
///
/// 三个平台的路子不一样：Unix 直接交给 CUPS 的 `lp`——它原生就吃 PDF，成败
/// 看退出码，`lp` 自己的报错（没有默认打印机之类）比我们编的话准确，所以原样
/// 转出去。Windows 没有命令行打印，只能借 shell 的 print 谓词，实际由注册的
/// PDF 处理程序执行；用 PowerShell 触发而不引入 windows-sys，与本文件其余
/// 几个系统调用保持一致，并用 CREATE_NO_WINDOW 压掉一闪而过的控制台窗口。
///
/// 注意份数：这里固定送一份。需要多份时不能靠打印机的份数设置——那样每份的
/// 份号会完全一样，必须由导出阶段生成含多个份号的 PDF。
pub(crate) fn print_in_os(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// CREATE_NO_WINDOW：不给子进程分配控制台，避免黑框闪一下。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = print_verb_script(path);
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command"])
            .arg(&script)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("无法启动打印：{error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        Err(if detail.is_empty() {
            "系统未能执行打印，请确认已安装可打印 PDF 的程序并设置了默认打印机".to_string()
        } else {
            detail.to_string()
        })
    }
    #[cfg(unix)]
    {
        let program = lp_program()
            .ok_or_else(|| "未找到 lp 命令，本机可能没有安装 CUPS 打印服务".to_string())?;
        // `--` 之后即使文件名以短横线开头也不会被当成选项。
        let output = std::process::Command::new(program)
            .arg("--")
            .arg(path)
            .output()
            .map_err(|error| format!("无法启动打印：{error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        Err(if detail.is_empty() {
            "打印命令执行失败，请检查是否已设置默认打印机".to_string()
        } else {
            detail.to_string()
        })
    }
}

/// 一次导出的结果。孤行提示单独拿出来：它是编译实测的产物，只对当时那份正文
/// 有效，正文一改就该作废，所以不能和其他审校提示混在一起长期留着。
#[derive(Debug, Clone, Default)]
pub(crate) struct ExportOutcome {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) warnings: Vec<String>,
    pub(crate) proof_warnings: Vec<ReviewNote>,
    /// 这次是否真的排完版量到了数据。为空的 `proof_warnings` 有两种含义——
    /// “量过，没有孤行”和“压根没编译”，前者该压住粗估提示，后者不该。
    pub(crate) proof_measured: bool,
    /// 导出本身成功但内置 Tectonic 编译 PDF 失败时填这个；走审校抽屉顶部的红色框，
    /// 不与"缺少一级标题"这类样式提示混在一起。
    pub(crate) compile_error: Option<String>,
}

/// 导出全部勾选格式；生成 TeX 后自动检测本机编译器，有可用引擎时把 PDF 一并加入结果。
/// 编译失败不阻断导出，PDF 缺失会单独记到 `compile_error` 上走红色提示框，
/// 已写好的 md/docx/tex 照常保留。
pub(crate) fn export_and_compile(
    output_dir: &Path,
    input: &DraftInput,
    markdown: &str,
    selection: &ExportSelection,
    vocabulary: &[VocabularyEntry],
    fonts: &FontConfig,
    mut progress: impl FnMut(&str),
) -> anyhow::Result<ExportOutcome> {
    progress("正在生成导出文件…");
    let display = UnitDisplay::new(vocabulary);
    // 字体文件在写 TeX 之前就要落实：TeX 里写死了按哪个文件加载，等到编译时
    // 才发现文件不在就只能报错，而这里还来得及退回内置字体。
    let (fonts, warnings) = system_fonts::resolve(fonts);
    let mut proof_warnings: Vec<ReviewNote> = Vec::new();
    let mut proof_measured = false;
    let mut files = export::export_all(output_dir, input, markdown, selection, &display, &fonts)?;
    let mut compile_error: Option<String> = None;
    if let Some(tex) = files
        .iter()
        .find(|file| file.extension().is_some_and(|ext| ext == "tex"))
    {
        progress("正在使用内置 Tectonic 离线编译 PDF…");
        match texcompile::compile_pdf_with_proof(tex, &fonts) {
            Ok(outcome) => {
                if let Some(pdf) = outcome.pdf {
                    files.push(pdf);
                    progress("PDF 编译完成，正在整理导出结果…");
                }
                // 孤行只有排完版才知道，这里读的是类文件写出的实测坐标。
                if let Some(report) = outcome.proof {
                    proof_measured = true;
                    proof_warnings.extend(orphan_probe::find_orphans(&report).iter().map(
                        |metric| {
                            let message = orphan_probe::format_warning(metric, markdown);
                            // 带上这一段的字节范围，审校面板里点一下就能选中它。
                            match export::block_span_for_line(markdown, metric.source_line) {
                                Some(span) => ReviewNote::located(message, span),
                                None => ReviewNote::from(message),
                            }
                        },
                    ));
                }
            }
            Err(error) => compile_error = Some(format!("{error:#}")),
        }
    }
    Ok(ExportOutcome {
        files,
        warnings,
        proof_warnings,
        proof_measured,
        compile_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_script_doubles_single_quotes_in_the_path() {
        // 目录名带撇号是完全可能的；不转义会截断 PowerShell 的单引号字符串。
        let script = print_verb_script(Path::new("/tmp/张三's/公文.pdf"));
        assert_eq!(
            script,
            "Start-Process -FilePath '/tmp/张三''s/公文.pdf' -Verb Print"
        );
    }

    #[test]
    fn print_script_keeps_an_ordinary_path_verbatim() {
        let script = print_verb_script(Path::new("/tmp/公文.pdf"));
        assert_eq!(
            script,
            "Start-Process -FilePath '/tmp/公文.pdf' -Verb Print"
        );
    }

    #[cfg(unix)]
    #[test]
    fn lp_is_found_only_when_it_exists_on_the_path() {
        use std::os::unix::fs::PermissionsExt;

        let empty = tempfile::tempdir().expect("临时目录");
        let with_lp = tempfile::tempdir().expect("临时目录");
        let lp = with_lp.path().join("lp");
        std::fs::write(&lp, "#!/bin/sh\n").expect("写入假 lp");
        std::fs::set_permissions(&lp, std::fs::Permissions::from_mode(0o755)).expect("置可执行");

        // 只有空目录：找不到。
        let path = std::env::join_paths([empty.path()]).expect("拼 PATH");
        assert!(lp_in_path(&path).is_none());

        // 后一个目录里有 lp：找得到，且返回的就是它。
        let path = std::env::join_paths([empty.path(), with_lp.path()]).expect("拼 PATH");
        assert_eq!(lp_in_path(&path), Some(lp));
    }
}
