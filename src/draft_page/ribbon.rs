//! 功能区（Ribbon）各分区：首页/插入/格式/审校/视图/输出。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::ribbon`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::{DraftAction, VersionScope, warn};
use crate::draft_page::{
    DraftPage, PreviewMode, RagKindFilter, TOOLBAR_CONTROL_HEIGHT, TableOp, body_stats,
    chinese_today, table_grid_picker, tidy_blank_lines, toggle_bullet, toolbar_separator,
};
use crate::export;
use crate::export::ColumnAlign;
use crate::models::{ExportSelection, RibbonTab, TemplateKind};
use crate::storage;
use crate::theme;
use eframe::egui;

impl DraftPage<'_> {
    /// 起草页功能区：第一行是分区卡与常驻入口，第二行是当前分区的按钮。
    ///
    /// 仿 Word 的分区卡，把过去挤在一条里的二十来个入口按用途分成六区，腾出的
    /// 位置留给插入表格、标题层级这类更细的操作。分区卡与折叠状态记在配置里，
    /// 换稿件不重置——切分区是「现在要干哪一类活」，换篇稿子通常还在干同一类。
    pub(crate) fn ribbon(&mut self, ui: &mut egui::Ui) {
        // 背景先占绘制位置，等上下两行排版结束、拿到精确矩形后再回填；这样单一
        // 闭合曲线位于所有按钮后面，不会遮挡文字或点击反馈。
        let ribbon_background = ui.painter().add(egui::Shape::Noop);
        let selected_tab_rect = self.ribbon_tabs(ui);
        if self.config.ribbon_collapsed {
            return;
        }
        ui.add_space(1.0);
        let tray = theme::ribbon_tray_layout().show(ui, |ui| {
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = TOOLBAR_CONTROL_HEIGHT;
                ui.spacing_mut().item_spacing.x = 4.0;
                // 窗口窄下来时横向滚动，而不是折行：折行会把编辑区一路挤矮，
                // 而功能区的高度应当始终是固定的两行。
                egui::ScrollArea::horizontal()
                    .id_salt("ribbon_items")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| match self.config.ribbon_tab {
                            RibbonTab::Home => self.ribbon_home(ui),
                            RibbonTab::Insert => self.ribbon_insert(ui),
                            RibbonTab::Format => self.ribbon_format(ui),
                            RibbonTab::Review => self.ribbon_review(ui),
                            RibbonTab::View => self.ribbon_view(ui),
                            RibbonTab::Output => self.ribbon_output(ui),
                        });
                    });
            });
        });
        if let Some(tab_rect) = selected_tab_rect {
            ui.painter().set(
                ribbon_background,
                theme::connected_ribbon_shape(tab_rect, tray.response.rect),
            );
        }
    }

    /// 分区卡那一行。左端常驻公文要素填报区的开关，右端常驻五个视图模式和
    /// 收起功能区的箭头——这三样都不属于任何一个分区，切分区卡时必须一直在。
    pub(crate) fn ribbon_tabs(&mut self, ui: &mut egui::Ui) -> Option<egui::Rect> {
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = 26.0;
            ui.spacing_mut().item_spacing.x = 2.0;
            let mut selected_rect = None;
            ui.horizontal(|ui| {
                let collapsed = self.doc.form_collapsed;
                let icon = if collapsed {
                    theme::Icon::PanelOpen
                } else {
                    theme::Icon::PanelClose
                };
                if theme::nav_button(ui, !collapsed, icon, "公文要素")
                    .on_hover_text(if collapsed {
                        "展开公文要素填报区"
                    } else {
                        "收起公文要素填报区"
                    })
                    .clicked()
                {
                    self.doc.form_collapsed = !collapsed;
                }
                toolbar_separator(ui);

                let current = self.config.ribbon_tab;
                let mut picked = None;
                let mut toggle_collapse = false;
                for tab in RibbonTab::ALL {
                    // 双击当前分区卡收起/展开功能区（与 Word 一致），在悬停提示里
                    // 写明，不然这个手势几乎不可能被发现。
                    let tip = if current == tab {
                        format!("{}（双击收起/展开功能区）", tab.hint())
                    } else {
                        tab.hint().to_string()
                    };
                    let response = theme::ribbon_tab_button(ui, current == tab, tab.label())
                        .on_hover_text(tip);
                    if current == tab {
                        selected_rect = Some(response.rect);
                    }
                    // 双击当前分区卡收起第二行，与 Word 一致。
                    if response.double_clicked() && current == tab {
                        toggle_collapse = true;
                    } else if response.clicked() {
                        picked = Some(tab);
                    }
                }
                if let Some(tab) = picked
                    && tab != current
                {
                    self.config.ribbon_tab = tab;
                    self.persist_ribbon();
                }
                if toggle_collapse {
                    self.config.ribbon_collapsed = !self.config.ribbon_collapsed;
                    self.persist_ribbon();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let collapsed = self.config.ribbon_collapsed;
                    if theme::icon_button(
                        ui,
                        if collapsed {
                            theme::Icon::ChevronDown
                        } else {
                            theme::Icon::ChevronUp
                        },
                        if collapsed {
                            "展开功能区"
                        } else {
                            "收起功能区"
                        },
                    )
                    .on_hover_text("也可以双击当前分区卡收起或展开")
                    .clicked()
                    {
                        self.config.ribbon_collapsed = !collapsed;
                        self.persist_ribbon();
                    }
                    toolbar_separator(ui);
                    // 视图入口仿 Zed 收到最右侧，只保留稳定的图标锚点。
                    for (mode, icon, label, tip) in [
                        (
                            PreviewMode::VersionDiff,
                            theme::Icon::GitCommit,
                            "版本对照",
                            "版本对照：最新提交版本与当前修订逐字比较",
                        ),
                        (
                            PreviewMode::Split,
                            theme::Icon::Compare,
                            "Markdown 对照",
                            "Markdown 对照：左边编辑源码，右边同步显示打印分页",
                        ),
                        (
                            PreviewMode::Rendered,
                            theme::Icon::Eye,
                            "公文预览",
                            "公文预览：按导出后的字体与行距排版",
                        ),
                        (
                            PreviewMode::Source,
                            theme::Icon::PencilLine,
                            "Markdown",
                            "Markdown：带语法高亮的源码，导出以此为准",
                        ),
                    ] {
                        if theme::view_icon_button(ui, self.doc.preview_mode == mode, icon, label)
                            .on_hover_text(tip)
                            .clicked()
                        {
                            self.doc.preview_mode = mode;
                        }
                    }
                });
            });
            selected_rect
        })
        .inner
    }

    /// 分区卡与折叠状态立刻落盘，与设置页里切主题的处理一致。
    pub(crate) fn persist_ribbon(&mut self) {
        let _ = storage::save(self.config);
    }

    /// 开始：日常最常用的那几件事，绝大多数时候不必切分区卡。
    pub(crate) fn ribbon_home(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        let saved = self.doc.manuscript_id.is_some();
        // 只读稿件（已发布、已归档）不能改内容，但照常预览、导出、看版本。
        let editable = !self.doc.read_only();
        let save_shortcut = theme::primary_shortcut("S");

        // 一、入库与版本
        if ui
            .add_enabled(editable, theme::icon_text_button(theme::Icon::Save, "保存"))
            .on_hover_text(if saved {
                format!("更新稿件库中已打开的那条记录（{save_shortcut}）")
            } else {
                format!("保存为稿件库中的一条新草稿记录（{save_shortcut}）")
            })
            .clicked()
        {
            self.actions.push(DraftAction::SaveToLibrary);
        }
        if ui
            .add_enabled(
                saved && editable,
                theme::icon_text_button(theme::Icon::GitCommit, "提交版本"),
            )
            .on_hover_text(if saved {
                "把当前内容固化为一个新版本（需相对上一版本有变更）"
            } else {
                "先“保存”，再提交版本"
            })
            .clicked()
            && let Some(id) = self.doc.manuscript_id
        {
            self.actions
                .push(DraftAction::OpenVersionCommit(VersionScope::Manuscript(id)));
        }
        self.version_switch_picker(ui);
        toolbar_separator(ui);

        // 二、稿件本身的编辑动作
        if ui
            .add(theme::icon_text_button(
                theme::Icon::SearchClear,
                "查找替换",
            ))
            .on_hover_text(format!(
                "在审校稿里查找并替换（{}）",
                theme::primary_shortcut("F")
            ))
            .clicked()
        {
            self.doc.markdown_find.open = true;
            self.doc.markdown_find.focus_query = true;
        }
        if ui
            .add_enabled(
                has_draft,
                theme::icon_text_button(theme::Icon::Copy, "复制全文"),
            )
            .clicked()
        {
            self.doc.generated_markdown =
                export::normalize_ordered_list_punctuation(&self.doc.generated_markdown);
            ui.ctx().copy_text(self.doc.generated_markdown.clone());
            *self.status = "审校稿已复制到剪贴板。".into();
        }
        // 清空是不可逆操作：功能区保留明确文字，点击后再由模态框二次确认。
        if ui
            .add_enabled(
                has_draft && editable,
                theme::warning_icon_button(theme::Icon::Trash, "清空审校稿"),
            )
            .on_hover_text("清空审校稿全文，不可恢复")
            .clicked()
        {
            self.doc.clear_review_confirm = true;
        }
        toolbar_separator(ui);

        // 三、AI：有稿件是优化，没稿件是从零起草
        if theme::primary_icon_button_enabled(
            ui,
            !self.doc.busy && editable,
            theme::Icon::Sparkles,
            if has_draft { "AI 优化" } else { "AI 起草" },
        )
        .on_hover_text(if has_draft {
            "选一条提示词改写当前稿件，也可临时写一条；输出格式标准内置生效，不会破坏导出结构"
        } else {
            "审校稿为空：在面板里写明要起草什么，结合左侧公文要素从零生成"
        })
        .clicked()
        {
            self.actions.push(DraftAction::OpenAiPromptPicker);
        }
        // RAG 开关：仅起草（审校稿为空）有意义。勾选后起草时自动检索知识库
        // 相似稿件片段注入提示词作参考。
        //
        // 全局开关关着时置灰而不是照常可勾——否则勾了也什么都不会发生，
        // 界面上还没有任何提示，只会让人以为功能坏了。
        if editable && !has_draft {
            let rag_enabled = self.config.rag.enabled;
            ui.add_enabled_ui(rag_enabled, |ui| {
                ui.checkbox(&mut self.doc.use_knowledge_rag, "参考知识库")
                    .on_hover_text(if rag_enabled {
                        "起草时自动检索知识库中的相似稿件片段，注入提示词作写作风格参考"
                    } else {
                        "知识库检索增强尚未启用：请到「设置 → 知识库」勾选启用，并配置 embedding 模型"
                    });
            });
            if rag_enabled && self.doc.use_knowledge_rag {
                egui::ComboBox::from_id_salt("rag_kind_filter")
                    .selected_text(self.doc.rag_kind_filter.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.doc.rag_kind_filter,
                            RagKindFilter::Follow,
                            RagKindFilter::Follow.label(),
                        );
                        ui.selectable_value(
                            &mut self.doc.rag_kind_filter,
                            RagKindFilter::All,
                            RagKindFilter::All.label(),
                        );
                        for kind in TemplateKind::ALL {
                            ui.selectable_value(
                                &mut self.doc.rag_kind_filter,
                                RagKindFilter::Only(kind),
                                kind.label(),
                            );
                        }
                    });
            }
        }
        toolbar_separator(ui);

        // 四、校验
        if ui
            .add_enabled(
                has_draft,
                theme::icon_text_button(theme::Icon::Refresh, "重新校验"),
            )
            .clicked()
        {
            self.revalidate();
            if !self.doc.warnings.is_empty() {
                self.open_result_drawer();
            }
            *self.status = "已重新执行规则校验。".into();
        }
        self.warning_badge(ui);
    }

    /// 校验提示的计数徽章：有提示才出现，点一下开抽屉。放在“重新校验”旁边，
    /// 免得校验完还要自己去找结果在哪。
    pub(crate) fn warning_badge(&mut self, ui: &mut egui::Ui) {
        let count = self.doc.warnings.len();
        if count == 0 {
            return;
        }
        if ui
            .add(
                egui::Button::image_and_text(
                    theme::Icon::TriangleAlert.image().tint(warn()),
                    egui::RichText::new(format!("{count} 条提示")).color(warn()),
                )
                .image_tint_follows_text_color(false)
                .fill(theme::warn_soft())
                .stroke(egui::Stroke::NONE)
                .corner_radius(egui::CornerRadius::same(7)),
            )
            .on_hover_text("打开审校提示")
            .clicked()
        {
            self.open_result_drawer();
        }
    }

    /// 插入：往光标处放东西。功能区扩容之后最主要的受益者。
    pub(crate) fn ribbon_insert(&mut self, ui: &mut egui::Ui) {
        let editable = !self.doc.read_only();
        let in_table = self.table_at_cursor(ui.ctx()).is_some();

        // 一、表格
        let mut size = self.doc.table_size;
        let mut insert_table: Option<(usize, usize)> = None;
        ui.add_enabled_ui(editable, |ui| {
            egui::containers::menu::MenuButton::from_button(theme::icon_text_button(
                theme::Icon::Table,
                "表格",
            ))
            .ui(ui, |ui| {
                ui.set_min_width(206.0);
                if let Some(picked) = table_grid_picker(ui) {
                    insert_table = Some(picked);
                    ui.close();
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut size.0).range(1..=60).speed(0.15));
                    ui.label("行 ×");
                    ui.add(egui::DragValue::new(&mut size.1).range(2..=20).speed(0.15));
                    ui.label("列");
                    if ui.button("插入").clicked() {
                        insert_table = Some(size);
                        ui.close();
                    }
                });
                ui.weak("首行是表头。导出时列宽按内容自动排版。");
            });
        });
        self.doc.table_size = size;
        if let Some((rows, columns)) = insert_table {
            self.insert_table(ui.ctx(), rows, columns);
        }

        // 二、表格的行列增删与列对齐：光标在表格里才亮
        let mut op: Option<TableOp> = None;
        ui.add_enabled_ui(editable && in_table, |ui| {
            egui::containers::menu::MenuButton::from_button(theme::icon_text_button(
                theme::Icon::Rows,
                "行",
            ))
            .ui(ui, |ui| {
                for (label, action) in [
                    ("在上方插入行", TableOp::InsertRowAbove),
                    ("在下方插入行", TableOp::InsertRowBelow),
                    ("删除本行", TableOp::DeleteRow),
                ] {
                    if ui.button(label).clicked() {
                        op = Some(action);
                        ui.close();
                    }
                }
            });
            egui::containers::menu::MenuButton::from_button(theme::icon_text_button(
                theme::Icon::Columns,
                "列",
            ))
            .ui(ui, |ui| {
                for (label, action) in [
                    ("在左侧插入列", TableOp::InsertColumnLeft),
                    ("在右侧插入列", TableOp::InsertColumnRight),
                    ("删除本列", TableOp::DeleteColumn),
                ] {
                    if ui.button(label).clicked() {
                        op = Some(action);
                        ui.close();
                    }
                }
            });
            egui::containers::menu::MenuButton::from_button(theme::icon_text_button(
                theme::Icon::AlignCenter,
                "列对齐",
            ))
            .ui(ui, |ui| {
                ui.weak("改的是光标所在那一列");
                for (align, icon) in [
                    (ColumnAlign::Auto, theme::Icon::WandSparkles),
                    (ColumnAlign::Left, theme::Icon::AlignLeft),
                    (ColumnAlign::Center, theme::Icon::AlignCenter),
                    (ColumnAlign::Right, theme::Icon::AlignRight),
                ] {
                    if ui.add(theme::menu_item(icon, align.label())).clicked() {
                        op = Some(TableOp::Align(align));
                        ui.close();
                    }
                }
            });
        });
        if let Some(op) = op {
            self.apply_table_op(ui.ctx(), op);
        }
        toolbar_separator(ui);

        // 三、图片与现成文档
        ui.add_enabled_ui(editable, |ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::Paperclip, "图片"))
                .on_hover_text(
                    "选择图片（PNG / JPG / WebP / BMP / GIF）或 PDF 文件，复制入库后\
                     按 markdown 图片语法插到光标处，预览按页面宽度排版",
                )
                .clicked()
            {
                self.insert_images(ui.ctx());
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::FileUp, "导入文档"))
                .on_hover_text(
                    "从已有的 Word / Excel / PPT / ODF / RTF / EPUB / CSV 文件提取内容，\
                     转成 Markdown 插到光标处",
                )
                .clicked()
            {
                self.import_document(ui.ctx());
            }
        });
        toolbar_separator(ui);

        // 四、公文构件：区段标记与附件标识，导出器按它们切分正文与附件
        let mut marker: Option<(&'static str, &'static str)> = None;
        ui.add_enabled_ui(editable, |ui| {
            if ui
                .add(theme::icon_text_button(
                    theme::Icon::BookmarkCheck,
                    "正文标记",
                ))
                .on_hover_text("插入“<!-- [正文] -->”，声明正式标题之后的正文区段起点")
                .clicked()
            {
                marker = Some(("<!-- [正文] -->", "正文标记"));
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::Paperclip, "附件标记"))
                .on_hover_text("插入“<!-- [附件] -->”，把其后内容切换为附件区段")
                .clicked()
            {
                marker = Some(("<!-- [附件] -->", "附件标记"));
            }
        });
        if let Some((text, label)) = marker {
            self.insert_section_marker(ui.ctx(), text, label);
        }
        toolbar_separator(ui);

        // 五、标准词库：单位、人员、联系方式直接插到光标处
        ui.add_enabled_ui(editable, |ui| self.vocabulary_menus(ui));
        toolbar_separator(ui);

        // 六、公文里反复要写的符号与日期
        let mut snippet: Option<(String, usize, &'static str)> = None;
        ui.add_enabled_ui(editable, |ui| {
            if ui
                .add(theme::icon_text_button(theme::Icon::Calendar, "中文日期"))
                .on_hover_text("插入今天的中文数字日期，即成文日期的写法")
                .clicked()
            {
                snippet = Some((chinese_today(), 0, "中文日期"));
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::Quote, "引号"))
                .on_hover_text("插入一对中文双引号，光标落在中间")
                .clicked()
            {
                snippet = Some(("“”".to_string(), "”".len(), "中文引号"));
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::Braces, "待核实"))
                .on_hover_text("插入【待核实】占位，校验会把它挑出来提醒补齐")
                .clicked()
            {
                snippet = Some(("【待核实】".to_string(), 0, "待核实占位"));
            }
        });
        if let Some((text, back, label)) = snippet {
            self.insert_inline(ui.ctx(), &text, back, label);
        }
    }

    /// 标准词库的三个下拉：单位、人员、联系方式。选中就把规范名称插到光标处，
    /// 免得为了抄一个单位全称在词库页和起草页之间来回切。
    pub(crate) fn vocabulary_menus(&mut self, ui: &mut egui::Ui) {
        let units = self.unit_pool(false);
        let contacts = self.contacts();
        let mut snippet: Option<(String, &'static str)> = None;

        egui::containers::menu::MenuButton::from_button(theme::icon_text_button(
            theme::Icon::Building,
            "单位",
        ))
        .ui(ui, |ui| {
            ui.set_min_width(260.0);
            if units.is_empty() {
                ui.weak("标准词库里还没有单位。");
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for option in &units {
                        if ui
                            .add(theme::menu_text_item(option.full.as_str()))
                            .clicked()
                        {
                            snippet = Some((option.full.clone(), "单位名称"));
                            ui.close();
                        }
                    }
                });
        });
        egui::containers::menu::MenuButton::from_button(theme::icon_text_button(
            theme::Icon::UserPlus,
            "人员",
        ))
        .ui(ui, |ui| {
            ui.set_min_width(200.0);
            if contacts.is_empty() {
                ui.weak("标准词库里还没有人员。");
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (name, _) in &contacts {
                        if ui.add(theme::menu_text_item(name.as_str())).clicked() {
                            snippet = Some((name.clone(), "人员姓名"));
                            ui.close();
                        }
                    }
                });
        });
        egui::containers::menu::MenuButton::from_button(theme::icon_text_button(
            theme::Icon::Phone,
            "联系方式",
        ))
        .ui(ui, |ui| {
            ui.set_min_width(220.0);
            let with_phone = contacts
                .iter()
                .filter(|(_, phone)| !phone.is_empty())
                .collect::<Vec<_>>();
            if with_phone.is_empty() {
                ui.weak("词库里的人员都还没填电话。");
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (name, phone) in with_phone {
                        if ui
                            .add(theme::menu_text_item(format!("{name} {phone}")))
                            .clicked()
                        {
                            snippet = Some((phone.clone(), "联系电话"));
                            ui.close();
                        }
                    }
                });
        });
        if let Some((text, label)) = snippet {
            self.insert_inline(ui.ctx(), &text, 0, label);
        }
    }

    /// 格式：改已有文字的 Markdown 标记。这里只放导出器认识的语法——
    /// 放个斜体按钮插出来的 `*文字*`，预览和 Word 里就是原样带星号。
    pub(crate) fn ribbon_format(&mut self, ui: &mut egui::Ui) {
        let editable = !self.doc.read_only();
        let current = self.heading_level_at_cursor(ui.ctx());
        let mut heading: Option<(u8, &'static str)> = None;
        let mut bold = false;
        let mut bullet = false;
        let mut inline_ordered = false;
        let mut ordered = false;
        let mut tidy = false;
        let mut quotes = false;

        // 一、标题层级。按钮上写的是公文层级而不是 h2/h3：用的人脑子里想的是
        // 「这是一级标题」，编号由导出器统一生成。
        ui.add_enabled_ui(editable, |ui| {
            if ui
                .add(
                    theme::icon_text_button(theme::Icon::Heading, "标题")
                        .selected(current == Some(1)),
                )
                .on_hover_text("公文标题（#）：整篇只应有一个")
                .clicked()
            {
                heading = Some((1, "标题"));
            }
            for (label, level, tip) in [
                ("一、", 2u8, "一级标题（##）：导出时自动编号为「一、」"),
                ("（一）", 3, "二级标题（###）：自动编号为「（一）」"),
                ("1.", 4, "三级标题（####）：自动编号为「1.」"),
                ("（1）", 5, "四级标题（#####）：自动编号为「（1）」"),
            ] {
                if ui
                    .add(
                        egui::Button::new(label)
                            .selected(current == Some(level))
                            .min_size(egui::vec2(40.0, TOOLBAR_CONTROL_HEIGHT)),
                    )
                    .on_hover_text(tip)
                    .clicked()
                {
                    heading = Some((level, label));
                }
            }
            if ui
                .add(theme::icon_text_button(
                    theme::Icon::RemoveFormatting,
                    "降为正文",
                ))
                .on_hover_text("去掉行首的 # 标记，变回正文段落")
                .clicked()
            {
                heading = Some((0, "正文"));
            }
            toolbar_separator(ui);

            // 二、字符与段落
            if ui
                .add(theme::icon_text_button(theme::Icon::Bold, "加粗"))
                .on_hover_text(format!(
                    "给选中的文字加 `**`（{}）；已加粗的再点一次去掉",
                    theme::primary_shortcut("B")
                ))
                .clicked()
            {
                bold = true;
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::List, "项目符号"))
                .on_hover_text("当前行加上 `- `；已经是列表项的再点一次去掉")
                .clicked()
            {
                bullet = true;
            }
            if ui
                .add(theme::icon_text_button(
                    theme::Icon::ListOrdered,
                    "段内编号",
                ))
                .on_hover_text("选中列表行并紧接上方正文；排版为①②③圈号")
                .clicked()
            {
                inline_ordered = true;
            }
            if ui
                .add(theme::icon_text_button(
                    theme::Icon::ListOrdered,
                    "有序列表",
                ))
                .on_hover_text("选中列表行并在前面保留空行；排版为1.2.3.")
                .clicked()
            {
                ordered = true;
            }
            toolbar_separator(ui);

            // 三、清理
            if ui
                .add(theme::icon_text_button(theme::Icon::Quote, "规范引号"))
                .on_hover_text("把全文的直引号、方向错乱的引号统一成配对的中文引号")
                .clicked()
            {
                quotes = true;
            }
            if ui
                .add(theme::icon_text_button(theme::Icon::Eraser, "清理空行"))
                .on_hover_text("连续空行压成一行，去掉行尾空格")
                .clicked()
            {
                tidy = true;
            }
        });

        if let Some((level, label)) = heading {
            self.apply_heading(ui.ctx(), level, label);
        }
        if bold {
            self.toggle_bold(ui.ctx());
        }
        if bullet {
            self.apply_line_edit(ui.ctx(), toggle_bullet, "已切换项目符号。");
        }
        if inline_ordered {
            self.apply_ordered_list(ui.ctx(), true);
        }
        if ordered {
            self.apply_ordered_list(ui.ctx(), false);
        }
        if quotes {
            self.doc.generated_markdown =
                export::normalize_chinese_quotes(&self.doc.generated_markdown);
            *self.status = "已把全文引号规范为中文引号。".into();
        }
        if tidy {
            self.doc.generated_markdown = tidy_blank_lines(&self.doc.generated_markdown);
            *self.status = "已清理多余空行与行尾空格。".into();
        }
    }

    /// 审校：校验、版本比对、查找，外加一个字数。
    pub(crate) fn ribbon_review(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        let saved = self.doc.manuscript_id.is_some();

        if ui
            .add_enabled(
                has_draft,
                theme::icon_text_button(theme::Icon::Refresh, "重新校验"),
            )
            .on_hover_text("按当前公文要素与规则重新检查一遍审校稿")
            .clicked()
        {
            self.revalidate();
            if !self.doc.warnings.is_empty() {
                self.open_result_drawer();
            }
            *self.status = "已重新执行规则校验。".into();
        }
        let drawer_open = self.doc.result_drawer_open;
        if ui
            .add(
                theme::icon_text_button(theme::Icon::TriangleAlert, "校验结果")
                    .selected(drawer_open),
            )
            .on_hover_text("开关右侧的审校提示抽屉")
            .clicked()
        {
            self.doc.result_drawer_open = !drawer_open;
        }
        self.warning_badge(ui);
        toolbar_separator(ui);

        if ui
            .add(
                theme::icon_text_button(theme::Icon::Compare, "版本对照")
                    .selected(self.doc.preview_mode == PreviewMode::VersionDiff),
            )
            .on_hover_text("最新提交版本与当前修订逐字比较")
            .clicked()
        {
            self.doc.preview_mode = PreviewMode::VersionDiff;
        }
        let versions_open = self.doc.versions_open;
        if ui
            .add_enabled(
                saved,
                theme::icon_text_button(theme::Icon::History, "版本历史").selected(versions_open),
            )
            .on_hover_text(if saved {
                "开关右侧的版本历史抽屉"
            } else {
                "这篇还没保存到稿件库"
            })
            .clicked()
        {
            self.doc.versions_open = !versions_open;
        }
        toolbar_separator(ui);

        if ui
            .add(theme::icon_text_button(
                theme::Icon::SearchClear,
                "查找替换",
            ))
            .on_hover_text(format!(
                "在审校稿里查找并替换（{}）",
                theme::primary_shortcut("F")
            ))
            .clicked()
        {
            self.doc.markdown_find.open = true;
            self.doc.markdown_find.focus_query = true;
        }
        toolbar_separator(ui);

        // 字数按导出后的正文算：Markdown 标记、表格竖线都不计。
        let (characters, paragraphs) = body_stats(&self.doc.generated_markdown);
        ui.add(
            egui::Button::image_and_text(
                theme::Icon::Hash.image(),
                egui::RichText::new(format!("{characters} 字 · {paragraphs} 段"))
                    .color(theme::text_muted()),
            )
            .image_tint_follows_text_color(true)
            .frame(false),
        )
        .on_hover_text("正文字数：不含 Markdown 标记、表格竖线与图片引用");
    }

    /// 视图：显示方式、缩放，以及各个面板的开关。
    pub(crate) fn ribbon_view(&mut self, ui: &mut egui::Ui) {
        for (mode, icon, label, tip) in [
            (
                PreviewMode::Source,
                theme::Icon::PencilLine,
                "Markdown",
                "带语法高亮的源码，导出以此为准",
            ),
            (
                PreviewMode::Hybrid,
                theme::Icon::Book,
                "实时排版",
                "当前行显示 Markdown 标记，离开后按公文格式渲染",
            ),
            (
                PreviewMode::Rendered,
                theme::Icon::Eye,
                "公文预览",
                "按导出后的字体与行距排版",
            ),
            (
                PreviewMode::Split,
                theme::Icon::Compare,
                "对照",
                "左边改 Markdown，右边看公文版式",
            ),
            (
                PreviewMode::VersionDiff,
                theme::Icon::GitCommit,
                "版本对照",
                "最新提交版本与当前修订逐字比较",
            ),
        ] {
            if ui
                .add(theme::icon_text_button(icon, label).selected(self.doc.preview_mode == mode))
                .on_hover_text(tip)
                .clicked()
            {
                self.doc.preview_mode = mode;
            }
        }
        toolbar_separator(ui);

        // 缩放只作用于版式预览；源码与版本对照按界面字号显示。
        let zoomable = matches!(
            self.doc.preview_mode,
            PreviewMode::Rendered | PreviewMode::Split
        );
        ui.add_enabled_ui(zoomable, |ui| self.zoom_controls(ui));
        toolbar_separator(ui);

        let form_collapsed = self.doc.form_collapsed;
        if ui
            .add(
                theme::icon_text_button(theme::Icon::PanelOpen, "公文要素")
                    .selected(!form_collapsed),
            )
            .on_hover_text("开关左侧的公文要素填报区")
            .clicked()
        {
            self.doc.form_collapsed = !form_collapsed;
        }
        let drawer_open = self.doc.result_drawer_open;
        if ui
            .add(
                theme::icon_text_button(theme::Icon::TriangleAlert, "校验结果")
                    .selected(drawer_open),
            )
            .on_hover_text("开关右侧的审校提示抽屉")
            .clicked()
        {
            self.doc.result_drawer_open = !drawer_open;
        }
        let versions_open = self.doc.versions_open;
        if ui
            .add_enabled(
                self.doc.manuscript_id.is_some(),
                theme::icon_text_button(theme::Icon::History, "版本历史").selected(versions_open),
            )
            .on_hover_text("开关右侧的版本历史抽屉")
            .clicked()
        {
            self.doc.versions_open = !versions_open;
        }
        let line_numbers = self.config.show_editor_line_numbers;
        if ui
            .add(theme::icon_text_button(theme::Icon::ListOrdered, "行号").selected(line_numbers))
            .on_hover_text("在源码与实时排版编辑器左侧显示行号")
            .clicked()
        {
            self.config.show_editor_line_numbers = !line_numbers;
            self.persist_ribbon();
        }
    }

    /// 输出：导出与打开成品。格式在设置页统一管理，这里另给三个只出一种
    /// 格式的快捷入口。
    pub(crate) fn ribbon_output(&mut self, ui: &mut egui::Ui) {
        let has_draft = !self.doc.generated_markdown.trim().is_empty();
        let ready = !self.doc.busy && has_draft;
        if ui
            .add_enabled(
                ready,
                theme::secondary_icon_button(theme::Icon::FileDown, "导出"),
            )
            .on_hover_text("按设置里勾选的格式导出当前审校稿，可反复导出")
            .clicked()
        {
            self.start_export_current();
        }
        let overwrite = self.config.export.overwrite;
        let mut only: Option<(ExportSelection, &'static str)> = None;
        ui.add_enabled_ui(ready, |ui| {
            for (icon, label, selection, tip) in [
                (
                    theme::Icon::FileTypeDoc,
                    "仅 Word",
                    ExportSelection {
                        markdown: false,
                        docx: true,
                        tex: false,
                        overwrite,
                    },
                    "这一次只出 docx，不改设置里勾好的常用格式",
                ),
                (
                    theme::Icon::Tex,
                    "TeX 与 PDF",
                    ExportSelection {
                        markdown: false,
                        docx: false,
                        tex: true,
                        overwrite,
                    },
                    "这一次只出 tex，并用本机 Tectonic/XeLaTeX 编译成 PDF",
                ),
                (
                    theme::Icon::PencilLine,
                    "仅 Markdown",
                    ExportSelection {
                        markdown: true,
                        docx: false,
                        tex: false,
                        overwrite,
                    },
                    "这一次只出 md 原文，连同稿中引用的图片一起落盘",
                ),
            ] {
                if ui
                    .add(theme::icon_text_button(icon, label))
                    .on_hover_text(tip)
                    .clicked()
                {
                    only = Some((selection, label));
                }
            }
        });
        if let Some((selection, label)) = only {
            self.start_export_with(selection);
            *self.status = format!("正在按「{label}」导出…");
        }
        toolbar_separator(ui);

        if ui
            .add(theme::icon_text_button(theme::Icon::Folder, "输出目录"))
            .on_hover_text(self.config.output_dir.clone())
            .clicked()
        {
            self.open_output_dir();
        }
        self.export_open_buttons(ui);
        toolbar_separator(ui);

        if ui
            .add(theme::icon_text_button(theme::Icon::Settings, "导出设置"))
            .on_hover_text("到设置页勾选常用导出格式、输出目录与是否覆盖同名文件")
            .clicked()
        {
            self.actions.push(DraftAction::OpenSettings);
        }
    }
}
