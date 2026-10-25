//! 稿件库的通用对话框：删除确认、归档与对话框尺寸/提示。
//!
//! 由 src/app/manuscript_ui.rs 拆分而来：本文件是模块 `app::manuscript_ui::dialogs`，与其它子模块共享
//! `app::manuscript_ui` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::app::manuscript_ui::{ArchivePending, BatchDeleteConfirm, ManuscriptAction};
use crate::app::{short_date, summarize, warn};
use crate::manuscript::send_package::{PinChoice, PinPreview, RevisionRef, SendPackageReferrer};
use crate::modal;
use crate::modal::Dismiss;
use crate::models::ManuscriptStatus;
use crate::theme;
use eframe::egui;

/// 单篇删除的二次确认气泡，贴在触发它的按钮下方弹出。
///
/// 以前确认条放在列表上方：离删除按钮太远，而且一插进来整个列表就往下挤一截。
/// 气泡是独立图层，不占版面，点外面或按 Esc 即收起（把 `open` 置为 false）。
/// 返回 true 表示用户点了「确认删除」。
pub(crate) fn delete_confirm_popup(
    anchor: &egui::Response,
    open: &mut bool,
    refs: &[SendPackageReferrer],
) -> bool {
    let mut confirmed = false;
    // 被已归档呈批件用作送批材料的稿件删不掉（归档材料不能缺件），确认键直接不给。
    let archived_owner = refs
        .iter()
        .find(|owner| owner.status == ManuscriptStatus::Archived);
    egui::Popup::from_response(anchor)
        .id(anchor.id.with("delete_confirm"))
        .open_bool(open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .align_alternatives(&[egui::RectAlign::TOP_END])
        .show(|ui| {
            if let Some(owner) = archived_owner {
                ui.colored_label(
                    warn(),
                    format!(
                        "该稿件是已归档呈批件《{}》的送批材料，不能删除。",
                        owner.title
                    ),
                );
            } else {
                ui.colored_label(warn(), "删除后不可恢复，确认删除这篇稿件吗？");
                if !refs.is_empty() {
                    let titles = refs
                        .iter()
                        .map(|owner| format!("《{}》", owner.title))
                        .collect::<Vec<_>>()
                        .join("、");
                    ui.weak(format!("它是{titles}的送批材料，删除后会从中移除。"));
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("取消").clicked() {
                    ui.close();
                }
                if archived_owner.is_none()
                    && ui
                        .add(theme::warning_icon_button(theme::Icon::Trash, "确认删除"))
                        .clicked()
                {
                    confirmed = true;
                    ui.close();
                }
            });
        });
    confirmed
}

/// 归档确认里的送批材料钉版：列出每件将钉在哪一版；送出之后有件改过时让人选
/// 钉送出时的版本还是当前最新版本。返回 true 表示有件钉不住、不能归档。
pub(crate) fn archive_pin_ui(
    ui: &mut egui::Ui,
    preview: &PinPreview,
    choice: &mut PinChoice,
) -> bool {
    ui.add_space(4.0);
    ui.strong(format!(
        "送批材料（{} 件）归档后固定在以下版本，以后导出都用这一套：",
        preview.entries.len() - 1
    ));
    if preview.needs_choice() {
        ui.weak(format!(
            "最近一次导出（{}）之后，有件又提交了新版本：",
            preview
                .exported_at
                .as_deref()
                .map(short_date)
                .unwrap_or_default()
        ));
        ui.radio_value(
            choice,
            PinChoice::Exported,
            "钉在送出时的版本（与已送出的材料一致）",
        );
        ui.radio_value(choice, PinChoice::Latest, "钉在当前最新版本");
    }
    let version = |pin: Option<&RevisionRef>| match pin {
        Some(pin) => pin
            .visible_number
            .map_or_else(|| "导出时的版本".to_string(), |n| format!("v{n}")),
        None => "钉不住".into(),
    };
    for (index, entry) in preview.entries.iter().enumerate() {
        let title = if entry.title.is_empty() {
            "（本机未找到的稿件）"
        } else {
            entry.title.as_str()
        };
        let role = if index == 0 { "主件" } else { "随行件" };
        let pin = entry.pin(*choice);
        let mut line = format!("{role}《{}》：{}", summarize(title, 24), version(pin));
        if entry.changed_since_export() {
            line.push_str(&format!(
                "（送出时 {}，最新 {}）",
                version(entry.exported.as_ref()),
                version(entry.latest.as_ref())
            ));
        } else if entry.exported.is_none()
            && entry.latest.is_some()
            && preview.exported_at.is_some()
        {
            line.push_str("（导出后才加入，用最新提交版）");
        }
        if pin.is_some() {
            ui.label(line);
        } else {
            ui.colored_label(
                theme::danger(),
                format!("{line}：既没导出过、也没有提交版本"),
            );
        }
    }
    !preview.unpinnable().is_empty()
}

/// 若 `pending` 正指向这一行，就在 `anchor` 下弹出删除确认；确认或收起后清掉 `pending`。
/// 批量删除确认框。点了删除返回 `inner == true`；删除范围是打开时冻结的 `confirm`。
pub(crate) fn batch_delete_dialog(
    ctx: &egui::Context,
    confirm: &BatchDeleteConfirm,
) -> modal::DialogResponse<bool> {
    modal::dialog(
        ctx,
        egui::Id::new("manuscript_batch_delete"),
        "批量删除稿件",
        400.0,
        Dismiss::EscOrBackdrop,
        |ui| {
            ui.label(format!(
                "确认删除所选的 {} 篇可删除稿件吗？",
                confirm.deletable.len()
            ));
            ui.colored_label(warn(), "此操作不可恢复。");
            if confirm.archived > 0 {
                ui.weak(format!(
                    "另有 {} 篇归档稿件受保护，将保留不动。",
                    confirm.archived
                ));
            }
            if confirm.packaged > 0 {
                ui.weak(format!(
                    "另有 {} 篇是已归档呈批件的送批材料，将保留不动。",
                    confirm.packaged
                ));
            }
            ui.add_space(10.0);
            let mut delete = false;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !confirm.deletable.is_empty(),
                        theme::warning_icon_button(
                            theme::Icon::Trash,
                            &format!("删除 {} 篇", confirm.deletable.len()),
                        ),
                    )
                    .clicked()
                {
                    delete = true;
                }
                if ui.button("取消").clicked() {
                    ui.close();
                }
            });
            delete
        },
    )
}

/// 大号模态（导入预览、合并）的宽度与正文可滚动高度：随窗口大小走，留出标题与按钮的位置。
pub(crate) fn large_dialog_size(ctx: &egui::Context) -> (f32, f32) {
    let screen = ctx.content_rect();
    (
        (screen.width() - 96.0).clamp(560.0, 980.0),
        (screen.height() - 240.0).clamp(220.0, 620.0),
    )
}

/// 模态里的错误提示：确认没能执行时说明原因。
pub(crate) fn dialog_notice(ui: &mut egui::Ui, notice: &Option<String>) {
    if let Some(notice) = notice {
        ui.colored_label(warn(), notice);
    }
}

/// 归档确认框的表单。点了「确认归档」返回 `true`。
///
/// 「选择扫描盖章 PDF…」不再当场开文件对话框（那会卡住主线程）：这里只把
/// `pick_pdfs` 置上，由上层拿到 `&mut GongwenApp` 后发起请求式对话框，挑完的
/// 路径再并回 pending。
pub(crate) fn archive_form(
    ui: &mut egui::Ui,
    pending: &mut ArchivePending,
    title: &str,
    pick_pdfs: &mut bool,
) -> bool {
    if !title.is_empty() {
        ui.strong(format!("《{}》", summarize(title, 30)));
    }
    ui.colored_label(
        warn(),
        "归档将冻结该稿件：标题、正文、时间等关键信息此后均不可修改。",
    );
    let mut blocked = false;
    if let Some(preview) = &pending.send_package {
        egui::ScrollArea::vertical()
            .id_salt("archive_pin_list")
            .max_height(260.0)
            .show(ui, |ui| {
                blocked = archive_pin_ui(ui, preview, &mut pending.pin_choice);
            });
    }
    ui.add_space(6.0);
    if ui
        .add(theme::icon_text_button(
            theme::Icon::Paperclip,
            "选择扫描盖章 PDF…",
        ))
        .on_hover_text("可多选；归档后仍可在详情页继续添加附件")
        .clicked()
    {
        // 对话框在后台线程开：这里只记下意图，由上层发起请求。
        *pick_pdfs = true;
    }
    if !pending.pdf_paths.is_empty() {
        ui.label(format!("已选 {} 个 PDF：", pending.pdf_paths.len()));
        let mut remove = None;
        for (index, path) in pending.pdf_paths.iter().enumerate() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("PDF")
                .to_string();
            ui.horizontal(|ui| {
                ui.label(format!("  {name}"));
                if theme::icon_button(ui, theme::Icon::X, "不附这个文件").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some(index) = remove {
            pending.pdf_paths.remove(index);
        }
    }
    ui.add_space(10.0);
    let mut archive = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !blocked,
                theme::warning_icon_button(theme::Icon::Archive, "确认归档"),
            )
            .on_disabled_hover_text("送批材料里有钉不住版本的件，先处理或移出清单")
            .clicked()
        {
            archive = true;
        }
        if ui.button("取消").clicked() {
            ui.close();
        }
    });
    archive
}

pub(crate) fn row_delete_confirm(
    anchor: &egui::Response,
    pending: &mut Option<i64>,
    refs: &[SendPackageReferrer],
    id: i64,
    action: &mut Option<ManuscriptAction>,
) {
    if *pending != Some(id) {
        return;
    }
    let mut open = true;
    if delete_confirm_popup(anchor, &mut open, refs) {
        *action = Some(ManuscriptAction::Delete(id));
        open = false;
    }
    if !open {
        *pending = None;
    }
}
