//! 起草页外壳：create_ui 主入口、只读横幅与清空审校确认。
//!
//! 由 src/draft_page.rs 拆分而来：本文件是模块 `draft_page::page`，与其它子模块共享
//! `draft_page` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::theme;
use crate::app::{DraftAction, FORM_CONTENT_MIN_WIDTH, FORM_PANEL_DEFAULT_WIDTH, FORM_PANEL_MAX_WIDTH, FORM_PANEL_MIN_WIDTH, accent, warn};
use crate::models::{ManuscriptStatus};
use eframe::egui;
use crate::draft_page::{DraftPage, DraftDiffState, MarkdownFindState};

impl DraftPage<'_> {
    pub(crate) fn create_ui(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("draft_toolbar")
            .frame(theme::panel(theme::surface(), 10))
            .show(ui, |ui| self.ribbon(ui));
        // 两个右侧抽屉的展开/收起交给 egui 自己的 `show_collapsible`：它把面板整体
        // 滑出/滑入窗口边缘（内部按完整宽度排版，再平移出界），而不是把宽度压到
        // 几个像素——后者会让抽屉里的 TextEdit 与 ScrollArea 在动画那十几帧里按
        // 负的可用宽度重排，滚动位置被打乱。同时保住手动拖拽调宽：动画结束后
        // 面板恢复 resizable，宽度仍在 240~460 之间持久化。
        // `is_expanded` 取局部副本再写回：闭包里要 `&mut self` 画抽屉内容，
        // 没法同时借出 `self.doc` 的字段。
        const DRAWER_DEFAULT_WIDTH: f32 = 300.0;
        // 抽屉内容函数只返回"是否点了关闭"：`show_collapsible` 的 `is_expanded`
        // 是进入时读取的目标值，闭包内直接写 `self.doc.*_open` 会被下面这句用
        // 局部副本覆盖回去，关闭按钮就失效了。所以关闭请求拿到这里落地。
        let mut versions_open = self.doc.versions_open;
        let mut close_versions = false;
        egui::Panel::right("draft_versions")
            .default_size(DRAWER_DEFAULT_WIDTH)
            .size_range(240.0..=460.0)
            .frame(theme::panel(theme::canvas(), 12))
            .show_collapsible(ui, &mut versions_open, |ui| {
                close_versions = self.versions_drawer(ui);
            });
        if close_versions {
            versions_open = false;
        }
        self.doc.versions_open = versions_open;
        // 新 ID：旧 ID 上持久化的是"底部抽屉"的外框矩形，沿用会让右侧抽屉按那条横条的
        // 位置排版，内容整体左移到中央区底下。换个 ID 直接丢掉那份旧状态。
        let mut result_open = self.doc.result_drawer_open;
        let mut close_result = false;
        egui::Panel::right("review_result_drawer_right_v1")
            .default_size(DRAWER_DEFAULT_WIDTH)
            .size_range(240.0..=460.0)
            .frame(theme::panel(theme::canvas(), 12))
            .show_collapsible(ui, &mut result_open, |ui| {
                close_result = self.result_drawer_ui(ui);
            });
        if close_result {
            result_open = false;
        }
        self.doc.result_drawer_open = result_open;
        if !self.doc.form_collapsed {
            // 新 ID 用于丢弃旧版本持久化的超宽面板尺寸，让紧凑默认值立即生效。
            egui::Panel::left("create_form_compact_v3")
                .default_size(FORM_PANEL_DEFAULT_WIDTH)
                .size_range(FORM_PANEL_MIN_WIDTH..=FORM_PANEL_MAX_WIDTH)
                .frame(theme::panel(theme::canvas(), 12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("公文要素");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if theme::icon_button(ui, theme::Icon::PanelClose, "收起公文要素填报区")
                                .on_hover_text("收起左侧填报区，扩大审校稿空间")
                                .clicked()
                            {
                                self.doc.form_collapsed = true;
                            }
                        });
                    });
                    ui.separator();
                    // 必须在进入纵向 ScrollArea 前取得有限宽度；滚动内容内部的
                    // available_width 首帧可能为无穷大，会反向把侧栏撑到最大值。
                    let form_content_width =
                        (ui.available_width() - 18.0).max(FORM_CONTENT_MIN_WIDTH);
                    let editable = !self.doc.read_only();
                    // 文种、套版和缩略导航图是"去哪儿填"，不跟着"填什么"一起滚。
                    ui.add_enabled_ui(editable, |ui| {
                        self.form_header_ui(ui, form_content_width)
                    });
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("form_scroll")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.add_enabled_ui(editable, |ui| self.form_ui(ui, form_content_width));
                        });
                });
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                self.read_only_banner(ui);
                if let Some(loaded) = &self.doc.loaded_version
                    && self.doc.manuscript_id == Some(loaded.manuscript_id)
                {
                    ui.group(|ui| {
                        ui.colored_label(
                            accent(),
                            format!(
                                "当前编辑内容来自版本 v{}《{}》，提交版本将追加为新版本。",
                                loaded.version_number, loaded.name
                            ),
                        );
                    });
                    ui.add_space(6.0);
                }
                self.preview_ui(ui);
            });
        self.clear_review_confirm_modal(ui.ctx());
    }

    /// 清空审校稿的确认框使用真正的 Modal：遮罩会阻止点击穿透到编辑器或功能区，
    /// Esc、点遮罩和“取消”都只关闭确认，不改稿件。
    pub(crate) fn clear_review_confirm_modal(&mut self, ctx: &egui::Context) {
        if !self.doc.clear_review_confirm {
            return;
        }

        let mut confirm = false;
        let response = egui::Modal::new(egui::Id::new(("clear_review_confirm", self.doc.key)))
            .frame(theme::card())
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.heading("清空审校稿？");
                ui.add_space(4.0);
                ui.label("正文、审校提示、查找状态和本次导出结果都会被清除。");
                ui.colored_label(
                    theme::warn(),
                    "此操作不可恢复，但不会删除稿件库中的已保存版本。",
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::warning_icon_button(theme::Icon::Trash, "确认清空"))
                        .clicked()
                    {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        ui.close();
                    }
                });
            });

        if confirm {
            self.clear_review_output();
            self.doc.clear_review_confirm = false;
            *self.status = "已清空审校稿；稿件库中的已保存版本未受影响。".into();
        } else if response.should_close() {
            self.doc.clear_review_confirm = false;
        }
    }

    /// 清空与审校稿内容绑定的全部瞬时状态，避免正文没了但旧提示、旧导出链接或
    /// “来自版本 vN”的横幅仍留在界面上。
    pub(crate) fn clear_review_output(&mut self) {
        self.doc.generated_markdown.clear();
        self.doc.warnings.clear();
        self.doc.proof_warnings.clear();
        self.doc.proof_markdown.clear();
        self.doc.output_files.clear();
        self.doc.export_error = None;
        self.doc.loaded_version = None;
        self.doc.draft_diff = DraftDiffState::default();
        self.doc.result_drawer_open = false;
        self.doc.preview_anchor = None;
        self.doc.pending_source_jump = None;
        self.doc.pending_source_selection = None;
        self.doc.pending_render_jump = false;
        self.doc.markdown_find = MarkdownFindState::default();
    }

    /// 只读稿件的顶部横幅。发布件可以就地退回草稿继续改，归档件只作说明。
    pub(crate) fn read_only_banner(&mut self, ui: &mut egui::Ui) {
        if !self.doc.read_only() {
            return;
        }
        let published = self.doc.record_status == ManuscriptStatus::Published;
        ui.horizontal(|ui| {
            ui.colored_label(
                warn(),
                if published {
                    "这篇已发布，当前为只读。可预览、导出、看版本，但不能改。"
                } else {
                    "这篇已归档，归档稿不可修改。可预览、导出、查看版本历史。"
                },
            );
            if published
                && let Some(id) = self.doc.manuscript_id
                && ui
                    .add(theme::icon_text_button(theme::Icon::Undo, "退回草稿并编辑"))
                    .clicked()
            {
                self.actions.push(DraftAction::RevertToDraft(id));
            }
        });
        ui.add_space(6.0);
    }
}
