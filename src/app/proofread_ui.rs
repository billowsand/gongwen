//! 校对词表管理页：改、停用、自建、试规则。
//!
//! 由 src/app.rs 拆分而来：本文件是模块 `app::proofread_ui`，与其它子模块共享
//! `app` 根模块的私有可见性。
//!
//! 内置的 141 条是编译进二进制的**种子**，发行版里用户碰不到那个文件，所以
//! 这一页是唯一的管理入口。三条语义与 AI 提示词页保持一致：内置条目可改、
//! 可停用、可恢复默认，但删不掉；自建条目可以删。
//!
//! 编辑区里刻意不让用户裸写正则——命中条件走下拉，「后接/前接」这类前后文
//! 判断由程序做。真需要正则时也有入口，但会当场试编译并红字报错，而不是等到
//! 校对时才发现规则没生效。旁边的试验区是配套的：粘一段话进去立刻看命中在
//! 哪儿，否则用户写完规则心里没底，就不会写第二条。

use crate::app::GongwenApp;
use crate::models::ProofreadRule;
use crate::proofread::{self, Entry, Level};
use crate::theme;
use eframe::egui;

/// 命中条件的五种形态。界面上是下拉框，避免用户为了"后接"去学正则。
const CONDITION_KINDS: [(&str, &str); 6] = [
    ("总是", "出现即命中"),
    ("后接", "紧随其后是这些词之一才命中"),
    ("不后接", "紧随其后不是这些词才命中"),
    ("前接", "紧邻其前是这些词之一才命中"),
    ("不前接", "紧邻其前不是这些词才命中"),
    ("正则", "整句型问题才用，如「通过…使…」缺主语"),
];

const LEVELS: [(&str, &str); 3] = [
    ("必错", "确定的错误，可一键替换"),
    ("疑似", "需人判断，只提示不自动改"),
    ("提示", "弱提示，可整组关闭"),
];

/// 词表页的界面状态。不进配置文件——都是当次会话的筛选和编辑暂存。
#[derive(Default)]
pub(crate) struct ProofreadPageState {
    pub(crate) filter: String,
    /// 分组筛选；空串表示全部。
    pub(crate) group: String,
    /// 级别筛选；`None` 表示全部。
    pub(crate) level: Option<Level>,
    /// 当前选中的条目编号。
    pub(crate) selected: Option<String>,
    /// 试验区的文本，跨条目保留——试完一条接着试下一条不用重贴。
    pub(crate) sample: String,
}

impl GongwenApp {
    pub(crate) fn proofread_ui(&mut self, ui: &mut egui::Ui) {
        // 每帧重算生效词表。141 条的合并是纯内存操作，比维护缓存失效简单可靠。
        let lexicon = proofread::Lexicon::resolved(&self.config.proofread);

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading("校对词表");
                let enabled = lexicon.entries.iter().filter(|e| e.enabled).count();
                let custom = self.config.proofread.custom.len();
                let changed = self.config.proofread.overrides.len();
                ui.weak(format!(
                    "共 {} 条，启用 {enabled} 条 · 自建 {custom} 条 · 已改动内置 {changed} 条 · 仅保存在本机",
                    lexicon.entries.len()
                ));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_icon_button(ui, theme::Icon::Save, "保存更改").clicked() {
                    self.config.proofread.prune();
                    self.persist();
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::FilePlus, "新建词条"))
                    .on_hover_text("新增一条本单位自己的校对规则")
                    .clicked()
                {
                    let id = self.config.proofread.next_custom_id();
                    self.config.proofread.custom.push(ProofreadRule {
                        id: id.clone(),
                        level: "疑似".into(),
                        condition: "总是".into(),
                        group: "自定义".into(),
                        enabled: true,
                        ..Default::default()
                    });
                    self.proofread_page.selected = Some(id);
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::FileUp, "导入 Excel"))
                    .on_hover_text("按条目编号增量合并；与内置一致的行不会留下改动")
                    .clicked()
                {
                    self.import_proofread_xlsx();
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::FileDown, "导出 Excel"))
                    .on_hover_text("导出当前生效的全部词条，改完再导回来")
                    .clicked()
                {
                    self.export_proofread_xlsx();
                }
            });
        });

        if !lexicon.load_warnings.is_empty() {
            ui.add_space(4.0);
            for warning in &lexicon.load_warnings {
                ui.colored_label(theme::danger(), format!("⚠ {warning}"));
            }
        }

        ui.add_space(8.0);
        self.proofread_filters_ui(ui, &lexicon);
        ui.add_space(8.0);

        egui::ScrollArea::vertical()
            .id_salt("proofread_page")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let available = ui.available_width();
                let list_width = (available * 0.46).clamp(260.0, 520.0);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(list_width, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| self.proofread_list_ui(ui, &lexicon),
                    );
                    ui.add_space(12.0);
                    ui.vertical(|ui| self.proofread_editor_ui(ui, &lexicon));
                });
            });
    }

    fn export_proofread_xlsx(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel", &["xlsx"])
            .set_file_name("公文助手校对词表.xlsx")
            .save_file()
        else {
            return;
        };
        match crate::proofread_xlsx::to_xlsx(&self.config.proofread, &path) {
            Ok(()) => self.status = format!("校对词表已导出到 {}。", path.display()),
            Err(error) => self.status = format!("导出校对词表失败：{error:#}"),
        }
    }

    fn import_proofread_xlsx(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Excel 词表", &["xlsx"])
            .pick_file()
        else {
            return;
        };
        match crate::proofread_xlsx::parse(&path) {
            Ok((rules, warnings)) => {
                let report = crate::proofread_xlsx::merge(&mut self.config.proofread, rules);
                self.persist();
                // 坏行数量要说出来。只报"导入成功"，用户不会知道有几条被丢了。
                self.status = if warnings.is_empty() {
                    format!("校对词表已导入：{}。", report.summary())
                } else {
                    format!(
                        "校对词表已导入：{}；{} 行有问题被跳过（{}）。",
                        report.summary(),
                        warnings.len(),
                        warnings.join("；")
                    )
                };
            }
            Err(error) => self.status = format!("导入校对词表失败：{error:#}"),
        }
    }

    /// 搜索 + 分组 + 级别三个筛选，外加整组启停。
    fn proofread_filters_ui(&mut self, ui: &mut egui::Ui, lexicon: &proofread::Lexicon) {
        let mut groups: Vec<String> = lexicon
            .entries
            .iter()
            .map(|entry| entry.group.clone())
            .collect();
        groups.sort();
        groups.dedup();

        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.proofread_page.filter)
                    .hint_text("搜索错误写法、建议写法或说明")
                    .desired_width(240.0),
            );

            egui::ComboBox::from_id_salt("proofread_group")
                .selected_text(if self.proofread_page.group.is_empty() {
                    "全部分组".to_string()
                } else {
                    self.proofread_page.group.clone()
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.proofread_page.group, String::new(), "全部分组");
                    for group in &groups {
                        ui.selectable_value(
                            &mut self.proofread_page.group,
                            group.clone(),
                            group.as_str(),
                        );
                    }
                });

            egui::ComboBox::from_id_salt("proofread_level")
                .selected_text(
                    self.proofread_page
                        .level
                        .map_or("全部级别", |level| level.label()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.proofread_page.level, None, "全部级别");
                    for level in [Level::MustFix, Level::Suspect, Level::Hint] {
                        ui.selectable_value(
                            &mut self.proofread_page.level,
                            Some(level),
                            level.label(),
                        );
                    }
                });

            // 整组启停：套话虚词这类整片开关的需求，逐条点太折磨人。
            if !self.proofread_page.group.is_empty() {
                let group = self.proofread_page.group.clone();
                let ids: Vec<String> = lexicon
                    .entries
                    .iter()
                    .filter(|entry| entry.group == group)
                    .map(|entry| entry.id.clone())
                    .collect();
                if ui
                    .add(theme::icon_text_button(
                        theme::Icon::SquareCheck,
                        "整组启用",
                    ))
                    .clicked()
                {
                    self.set_proofread_enabled(&ids, true);
                }
                if ui
                    .add(theme::icon_text_button(theme::Icon::Square, "整组停用"))
                    .clicked()
                {
                    self.set_proofread_enabled(&ids, false);
                }
            }
        });
    }

    /// 批量改启用状态。内置条目写覆盖层，自建条目直接改本体。
    fn set_proofread_enabled(&mut self, ids: &[String], enabled: bool) {
        for id in ids {
            if let Some(rule) = self
                .config
                .proofread
                .custom
                .iter_mut()
                .find(|rule| &rule.id == id)
            {
                rule.enabled = enabled;
                continue;
            }
            // 与种子默认值一致时不留覆盖记录，配置文件才不会越攒越大。
            let default_enabled = proofread::builtin_entry(id).is_some_and(|entry| entry.enabled);
            if enabled == default_enabled {
                let patch = self.config.proofread.override_mut(id);
                patch.enabled = None;
            } else {
                self.config.proofread.override_mut(id).enabled = Some(enabled);
            }
        }
        self.config.proofread.prune();
    }

    fn proofread_list_ui(&mut self, ui: &mut egui::Ui, lexicon: &proofread::Lexicon) {
        let filter = self.proofread_page.filter.trim().to_lowercase();
        let group = self.proofread_page.group.clone();
        let level = self.proofread_page.level;

        let mut toggle: Option<(String, bool)> = None;
        let mut select: Option<String> = None;
        let mut shown = 0usize;

        for entry in &lexicon.entries {
            if !group.is_empty() && entry.group != group {
                continue;
            }
            if level.is_some_and(|level| entry.level != level) {
                continue;
            }
            if !filter.is_empty() {
                let haystack = format!(
                    "{} {} {} {}",
                    entry.id, entry.wrong, entry.suggestion, entry.note
                )
                .to_lowercase();
                if !haystack.contains(&filter) {
                    continue;
                }
            }
            shown += 1;

            let selected = self.proofread_page.selected.as_deref() == Some(entry.id.as_str());
            let frame = if selected {
                theme::card().fill(theme::accent_soft())
            } else {
                theme::card()
            };
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let mut enabled = entry.enabled;
                    if ui.checkbox(&mut enabled, "").changed() {
                        toggle = Some((entry.id.clone(), enabled));
                    }
                    ui.label(
                        egui::RichText::new(if entry.wrong.is_empty() {
                            "（未填写错误写法）"
                        } else {
                            entry.wrong.as_str()
                        })
                        .strong()
                        .color(if entry.enabled {
                            theme::text()
                        } else {
                            theme::text_muted()
                        }),
                    );
                    theme::chip(
                        ui,
                        entry.level.label(),
                        level_color(entry.level),
                        theme::surface_sunk(),
                    );
                    if !entry.is_builtin() {
                        theme::chip(ui, "自建", theme::success(), theme::surface_sunk());
                    } else if self.config.proofread.override_for(&entry.id).is_some() {
                        theme::chip(ui, "已改", theme::info(), theme::surface_sunk());
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(&entry.id);
                    });
                });
                ui.horizontal(|ui| {
                    ui.weak(format!("→ {}", entry.suggestion));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(&entry.group);
                    });
                });
                if ui
                    .add(theme::icon_text_button(theme::Icon::Edit, "编辑"))
                    .clicked()
                {
                    select = Some(entry.id.clone());
                }
            });
            ui.add_space(4.0);
        }

        if shown == 0 {
            ui.weak("没有符合筛选条件的词条。");
        }
        if let Some((id, enabled)) = toggle {
            self.set_proofread_enabled(&[id], enabled);
        }
        if let Some(id) = select {
            self.proofread_page.selected = Some(id);
        }
    }

    fn proofread_editor_ui(&mut self, ui: &mut egui::Ui, lexicon: &proofread::Lexicon) {
        let Some(id) = self.proofread_page.selected.clone() else {
            ui.weak("在左侧选一条词条进行编辑，或点右上角「新建词条」。");
            return;
        };
        let Some(entry) = lexicon.entries.iter().find(|entry| entry.id == id) else {
            self.proofread_page.selected = None;
            return;
        };
        let builtin = entry.is_builtin();

        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.heading(&id);
                if builtin {
                    theme::chip(ui, "内置", theme::info(), theme::surface_sunk());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if builtin {
                        let changed = self.config.proofread.override_for(&id).is_some();
                        if ui
                            .add_enabled(
                                changed,
                                theme::icon_text_button(theme::Icon::RotateCcw, "恢复默认"),
                            )
                            .on_hover_text("丢弃对这条内置词条的全部改动")
                            .clicked()
                        {
                            self.config.proofread.clear_override(&id);
                        }
                    } else if theme::danger_icon_button(ui, theme::Icon::Trash, "删除本条")
                        .clicked()
                    {
                        self.config.proofread.custom.retain(|rule| rule.id != id);
                        self.proofread_page.selected = None;
                    }
                });
            });
            ui.separator();

            if builtin {
                self.proofread_builtin_fields_ui(ui, entry);
            } else {
                self.proofread_custom_fields_ui(ui, &id);
            }
        });

        ui.add_space(8.0);
        self.proofread_sample_ui(ui, entry);
    }

    /// 内置条目：错误写法与分组是种子的一部分，不给改——改了就等于换了一条
    /// 规则，那该新建而不是改内置。可改的是级别、建议写法、命中条件和说明。
    fn proofread_builtin_fields_ui(&mut self, ui: &mut egui::Ui, entry: &Entry) {
        let id = entry.id.clone();
        ui.horizontal(|ui| {
            ui.label("错误写法");
            ui.weak(&entry.wrong);
            ui.label("·");
            ui.label("分组");
            ui.weak(&entry.group);
        });

        let default = proofread::builtin_entry(&id);

        // 级别
        ui.horizontal(|ui| {
            ui.label("级别");
            let mut level = entry.level;
            egui::ComboBox::from_id_salt(("proofread_level_edit", &id))
                .selected_text(level.label())
                .show_ui(ui, |ui| {
                    for (label, hint) in LEVELS {
                        let value = match label {
                            "必错" => Level::MustFix,
                            "疑似" => Level::Suspect,
                            _ => Level::Hint,
                        };
                        ui.selectable_value(&mut level, value, label)
                            .on_hover_text(hint);
                    }
                });
            if level != entry.level {
                let same_as_default = default.is_some_and(|entry| entry.level == level);
                let patch = self.config.proofread.override_mut(&id);
                patch.level = (!same_as_default).then(|| level.label().to_string());
                self.config.proofread.prune();
            }
        });

        // 建议写法
        let mut suggestion = entry.suggestion.clone();
        ui.horizontal(|ui| {
            ui.label("建议写法");
            if ui
                .add(egui::TextEdit::singleline(&mut suggestion).desired_width(280.0))
                .changed()
            {
                let same = default.is_some_and(|entry| entry.suggestion == suggestion);
                let patch = self.config.proofread.override_mut(&id);
                patch.suggestion = (!same).then(|| suggestion.clone());
                self.config.proofread.prune();
            }
        });

        // 命中条件。`entry` 已经是合并过覆盖层的结果，它的原文就是当前生效值。
        let current = entry.condition_text.clone();
        if let Some(next) = condition_editor_ui(ui, &id, &current) {
            let same = proofread::builtin_condition_text(&id).is_some_and(|text| text == next);
            let patch = self.config.proofread.override_mut(&id);
            patch.condition = (!same).then_some(next);
            self.config.proofread.prune();
        }

        // 说明
        let mut note = entry.note.clone();
        ui.horizontal(|ui| {
            ui.label("说明");
            if ui
                .add(egui::TextEdit::singleline(&mut note).desired_width(320.0))
                .changed()
            {
                let same = default.is_some_and(|entry| entry.note == note);
                let patch = self.config.proofread.override_mut(&id);
                patch.note = (!same).then(|| note.clone());
                self.config.proofread.prune();
            }
        });

        if !entry.is_replaceable() {
            ui.weak("这条不提供一键替换：只有「必错 + 非正则 + 建议写法是纯词」才会替换正文。");
        }
    }

    /// 自建条目：所有字段都能改，直接写本体。
    fn proofread_custom_fields_ui(&mut self, ui: &mut egui::Ui, id: &str) {
        let Some(index) = self
            .config
            .proofread
            .custom
            .iter()
            .position(|rule| rule.id == id)
        else {
            return;
        };

        // 命中条件要单独取出来编辑，避免同时可变借用 rule 和 self。
        let current = self.config.proofread.custom[index].condition.clone();
        let rule = &mut self.config.proofread.custom[index];
        ui.horizontal(|ui| {
            ui.label("错误写法");
            ui.add(
                egui::TextEdit::singleline(&mut rule.wrong)
                    .hint_text("要挑出来的写法")
                    .desired_width(200.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("建议写法");
            ui.add(egui::TextEdit::singleline(&mut rule.suggestion).desired_width(280.0));
        });
        ui.horizontal(|ui| {
            ui.label("级别");
            egui::ComboBox::from_id_salt(("proofread_custom_level", id))
                .selected_text(rule.level.clone())
                .show_ui(ui, |ui| {
                    for (label, hint) in LEVELS {
                        ui.selectable_value(&mut rule.level, label.to_string(), label)
                            .on_hover_text(hint);
                    }
                });
            ui.label("分组");
            ui.add(egui::TextEdit::singleline(&mut rule.group).desired_width(120.0));
        });
        ui.horizontal(|ui| {
            ui.label("说明");
            ui.add(egui::TextEdit::singleline(&mut rule.note).desired_width(320.0));
        });

        if let Some(next) = condition_editor_ui(ui, id, &current) {
            self.config.proofread.custom[index].condition = next;
        }
    }

    /// 规则试验区：粘一段话，立刻看这条规则命中在哪儿。
    ///
    /// 没有这个，用户写完规则不知道对不对，也就不会写第二条。默认样本从词库里
    /// 借不到，就用条目自身的错误写法拼一句，至少能验证"能不能命中"。
    fn proofread_sample_ui(&mut self, ui: &mut egui::Ui, entry: &Entry) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("试一试").strong());
                ui.weak("粘一段正文，看这条规则会不会命中、命中在哪里");
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.proofread_page.sample)
                    .hint_text("在这里粘贴或输入一段公文正文")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );

            let sample = self.proofread_page.sample.clone();
            if sample.trim().is_empty() {
                ui.weak("——");
                return;
            }
            // 只用当前这一条规则去扫，结果才对得上号。
            let single = proofread::Lexicon {
                entries: vec![Entry {
                    enabled: true,
                    ..entry.clone()
                }],
                load_warnings: Vec::new(),
            };
            let notes = single.check(&sample);
            if notes.is_empty() {
                ui.colored_label(theme::text_muted(), "这段文字没有命中本条规则。");
                return;
            }
            ui.colored_label(theme::success(), format!("命中 {} 处：", notes.len()));
            for note in notes {
                ui.horizontal_wrapped(|ui| {
                    ui.weak(format!("第 {} 字节：", note.span.start));
                    ui.colored_label(theme::danger(), &sample[note.span.clone()]);
                    if let Some(replacement) = &note.replacement {
                        ui.weak(format!("→ 可替换为 {replacement}"));
                    }
                });
            }
        });
    }
}

/// 命中条件编辑器：下拉选形态 + 文本框填参数。返回 `Some` 表示这一帧被改过。
///
/// 正则当场试编译，错了立刻红字——不能等到校对时才发现规则从来没生效过。
fn condition_editor_ui(ui: &mut egui::Ui, id: &str, current: &str) -> Option<String> {
    let (kind, rest) = split_condition(current);
    let mut kind = kind.to_string();
    let mut rest = rest.to_string();
    let mut changed = false;

    ui.horizontal(|ui| {
        ui.label("命中条件");
        egui::ComboBox::from_id_salt(("proofread_condition_kind", id))
            .selected_text(&kind)
            .show_ui(ui, |ui| {
                for (label, hint) in CONDITION_KINDS {
                    if ui
                        .selectable_label(kind == label, label)
                        .on_hover_text(hint)
                        .clicked()
                    {
                        kind = label.to_string();
                        changed = true;
                    }
                }
            });
        if kind != "总是" {
            let hint = if kind == "正则" {
                "Rust 正则，不支持环视 (?=…)"
            } else {
                "多个词用 | 分隔"
            };
            if ui
                .add(
                    egui::TextEdit::singleline(&mut rest)
                        .hint_text(hint)
                        .desired_width(300.0),
                )
                .changed()
            {
                changed = true;
            }
        }
    });

    let composed = if kind == "总是" {
        "总是".to_string()
    } else {
        format!("{kind}:{rest}")
    };

    // 试编译：合法就绿字回显最终写法，不合法就红字说明。
    match crate::proofread::Condition::parse(&composed) {
        Ok(_) => {
            ui.weak(format!("当前条件：{composed}"));
        }
        Err(error) => {
            ui.colored_label(theme::danger(), format!("⚠ {error}"));
            // 不合法就不往外报改动，免得把坏规则写进配置。
            return None;
        }
    }

    changed.then_some(composed)
}

/// 把 `后接:目前|今天` 拆成 (`后接`, `目前|今天`)。
fn split_condition(value: &str) -> (&str, &str) {
    let value = value.trim();
    if value == "总是" || value.is_empty() {
        return ("总是", "");
    }
    match value.split_once(':').or_else(|| value.split_once('：')) {
        Some((head, rest)) => (head.trim(), rest),
        None => ("总是", ""),
    }
}

fn level_color(level: Level) -> egui::Color32 {
    match level {
        Level::MustFix => theme::danger(),
        Level::Suspect => theme::warn(),
        Level::Hint => theme::text_muted(),
    }
}
