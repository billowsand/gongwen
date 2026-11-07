//! 加密工作区向普通文件释放内容前的单次风险确认。
//! 这里只排队，不在确认前创建目录、打开保存框或启动导出线程。

use super::GongwenApp;
use crate::modal::{self, Dismiss};
use eframe::egui;

pub(crate) const CLIPBOARD_WARNING: &str = "复制或剪切会把明文交给系统剪贴板；系统历史记录、跨设备同步或第三方剪贴板工具可能保存副本，不受应用锁保护。锁定或清空当前剪贴板不能保证删除历史副本。";
pub(crate) const PLAINTEXT_WARNING: &str = "当前工作区已启用加密。此次导出的 PDF、Word、源码文件及随附的《份号清单.txt》（含份号与主送/抄送单位）将以普通文件保存，不再受应用密码和本地加密保护。文件可能包含正文、图片、附件及人员等敏感信息；复制、同步、发送或由其他软件打开时，可能导致信息泄露。请确认保存位置和接收范围。";
pub(crate) const PDF_ARCHIVE_WARNING: &str = "当前工作区已启用加密。此次导出的 7z 压缩包由独立密码保护，包内文件名也已加密，但解压后的 PDF 是普通文件，不受应用密码和本地加密保护。PDF 可能包含正文、图片、附件及人员等敏感信息；解压、复制、同步或发送后，可能导致信息泄露。压缩包自身的文件名和大小仍可见，请妥善保管导出密码并确认接收范围。";
pub(crate) const PRINT_WARNING: &str = "当前工作区已启用加密。打印会把明文 PDF 交给系统打印队列和打印机：队列可能自行保存副本，打印机缓存、纸件与后续处置都可能留下敏感信息，这些都不受应用密码和本地加密保护。请确认接收范围与纸件/队列记录的处置方式。";
pub(crate) const PACKAGE_ARCHIVE_WARNING: &str = "当前工作区已启用加密。此次导出的是稿件包（7z 格式，文件内容与包内文件名均由独立密码保护），但解压后的 Markdown 正文、历史版本、图片、PDF 附件与随行件全部是普通文件，不再受应用密码和本地加密保护。稿件包还可能包含人员、单位、文号等敏感信息；复制、同步、发送或由其他软件打开后，可能导致信息泄露。压缩包自身的文件名和大小仍可见，请妥善保管导出密码并确认接收范围。";
pub(crate) const PII_EXPORT_WARNING: &str = "当前工作区已启用加密。此次导出的表格或词表是普通文件，可能包含人员姓名、职务、联系电话、个人简介、单位名称等敏感信息，以及从稿件派生出的正文用语；复制、同步、发送或由其他软件打开后，可能导致信息泄露。请确认导出范围与接收对象。";
pub(crate) const BACKUP_WARNING: &str = "全量备份的内容和包内文件名均由独立备份密码保护，内部数据还保留应用锁加密。恢复需要备份密码和备份当时的应用锁密码；程序不保存备份密码，遗失后无法恢复。外层文件名和总大小仍可见，请使用中性名称并妥善保管。";

pub(crate) type OutputAction = Box<dyn FnOnce(&mut GongwenApp)>;

pub(super) struct PendingOutput {
    operation: String,
    warning: &'static str,
    slot: crate::storage::DataSlot,
    action: OutputAction,
}

impl GongwenApp {
    pub(crate) fn copy_file_confirmed(&mut self, path: &std::path::Path) {
        self.status = match crate::file_clipboard::copy_file(path) {
            Ok(()) => "文件已复制到系统剪贴板。".into(),
            Err(error) => format!("复制文件失败：{error}"),
        };
    }

    /// 剪切必须在编辑器删除文字前确认；取消确认不能丢失源文字。
    pub(crate) fn guard_clipboard_input(&mut self, ctx: &egui::Context) {
        if let Some(focus) = self.clipboard_cut_replay.take() {
            if let Some(id) = focus {
                ctx.memory_mut(|memory| memory.request_focus(id));
            }
            ctx.input_mut(|input| input.events.push(egui::Event::Cut));
            self.clipboard_output_approved = true;
            return;
        }
        if crate::vault::key_handle().is_none() {
            return;
        }
        let mut cut = false;
        ctx.input_mut(|input| {
            input.events.retain(|event| {
                if matches!(event, egui::Event::Cut) {
                    cut = true;
                    false
                } else {
                    true
                }
            })
        });
        if cut {
            let focus = ctx.memory(|memory| memory.focused());
            self.request_output_with_warning(
                "剪切内容到系统剪贴板",
                CLIPBOARD_WARNING,
                move |app| {
                    app.clipboard_cut_replay = Some(focus);
                },
            );
        }
    }

    /// 捕获所有 egui 文本/图片复制（包括 Ctrl+C），确认前不交给系统。
    pub(crate) fn guard_clipboard_output(&mut self, ctx: &egui::Context) {
        if crate::vault::key_handle().is_none() {
            return;
        }
        let mut copied = Vec::new();
        ctx.output_mut(|output| {
            output.commands.retain(|command| {
                if matches!(
                    command,
                    egui::OutputCommand::CopyText(_) | egui::OutputCommand::CopyImage(_)
                ) {
                    copied.push(command.clone());
                    false
                } else {
                    true
                }
            })
        });
        if copied.is_empty() {
            self.clipboard_output_approved = false;
            return;
        }
        if std::mem::take(&mut self.clipboard_output_approved) {
            ctx.output_mut(|output| output.commands.extend(copied));
            return;
        }
        let ctx = ctx.clone();
        self.request_output_with_warning(
            "复制内容到系统剪贴板",
            CLIPBOARD_WARNING,
            move |app| {
                app.clipboard_output_approved = true;
                ctx.output_mut(|output| output.commands.extend(copied));
            },
        );
    }

    pub(crate) fn request_plaintext_output(
        &mut self,
        operation: impl Into<String>,
        action: impl FnOnce(&mut Self) + 'static,
    ) {
        self.request_sensitive_output(operation.into(), PLAINTEXT_WARNING, Box::new(action));
    }

    pub(crate) fn request_pdf_archive_output(
        &mut self,
        operation: impl Into<String>,
        action: impl FnOnce(&mut Self) + 'static,
    ) {
        self.request_sensitive_output(operation.into(), PDF_ARCHIVE_WARNING, Box::new(action));
    }

    /// 打印前的风险确认：加密工作区允许打印，但必须明确「队列/纸件不在保护范围内」。
    pub(crate) fn request_print_output(
        &mut self,
        operation: impl Into<String>,
        action: impl FnOnce(&mut Self) + 'static,
    ) {
        self.request_sensitive_output(operation.into(), PRINT_WARNING, Box::new(action));
    }

    /// 自定义文案的输出确认：风格档案、词库/PII 表格、全量备份这类
    /// 「不是 PDF/Word 正文但仍把用户数据写成普通文件」的出口用它。
    pub(crate) fn request_output_with_warning(
        &mut self,
        operation: impl Into<String>,
        warning: &'static str,
        action: impl FnOnce(&mut Self) + 'static,
    ) {
        self.request_sensitive_output(operation.into(), warning, Box::new(action));
    }

    fn request_sensitive_output(
        &mut self,
        operation: String,
        warning: &'static str,
        action: OutputAction,
    ) {
        // fail-closed：只有确认「没启用加密」才直接执行。加密工作区里失去密钥
        // 句柄时（锁定中 / 异常降级）绝不能静默放行导出，宁可取消。
        if !self.vault_locked && crate::vault::key_handle().is_none() {
            action(self);
            return;
        }
        if crate::vault::key_handle().is_none() {
            self.status = "当前未解锁，已取消本次导出。".into();
            return;
        }
        // 不覆盖另一项待确认操作，避免用户确认的对象与执行的对象不一致。
        if self.pending_output.is_some() {
            self.status = "请先处理当前的导出风险确认。".into();
            return;
        }
        self.pending_output = Some(PendingOutput {
            operation,
            warning,
            slot: crate::storage::current_slot(),
            action,
        });
    }

    pub(super) fn output_risk_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_output.as_ref() else {
            return;
        };
        // 锁定会丢弃整个工作区；此处还防范确认期间状态被其它入口改变。
        if crate::vault::key_handle().is_none() || pending.slot != crate::storage::current_slot() {
            self.pending_output = None;
            self.status = "加密状态已变化，本次导出已取消。".into();
            return;
        }
        let clipboard = pending.warning == CLIPBOARD_WARNING;
        let mut confirmed = false;
        let mut cancelled = false;
        let response = modal::dialog(
            ctx,
            egui::Id::new("plaintext_output_risk"),
            if clipboard {
                "剪贴板内容可能泄露"
            } else {
                "导出文件可能泄露信息"
            },
            480.0,
            Dismiss::EscOnly,
            |ui| {
                ui.label(&pending.operation);
                ui.add_space(8.0);
                ui.label(egui::RichText::new(pending.warning).color(crate::theme::danger()));
                ui.add_space(8.0);
                ui.weak(if clipboard { "本次确认仅用于此次操作。取消不向剪贴板释放内容；锁定应用不能撤回外部副本。" } else { "本次确认仅用于此次操作。取消不会生成文件。文件保存后，锁定应用也不会加密或撤回这些文件。" });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    cancelled = ui.button("取消").clicked();
                    confirmed = ui.button("确认风险并继续").clicked();
                });
            },
        );
        if cancelled || response.dismissed {
            self.pending_output = None;
            self.status = if clipboard {
                "已取消剪贴板操作。"
            } else {
                "已取消导出，未生成文件。"
            }
            .into();
        } else if confirmed && let Some(pending) = self.pending_output.take() {
            (pending.action)(self);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft_page::ExportKind;

    #[test]
    fn clipboard_copy_waits_for_confirmation_and_cut_waits_before_modifying_the_editor() {
        let mut h = Harness::new(true);
        h.ctx.copy_text("保密剪贴板内容".into());
        h.app.guard_clipboard_output(&h.ctx);
        assert!(h.ctx.output(|output| output.commands.is_empty()));
        assert!(h.app.pending_output.is_some());
        let pending = h.app.pending_output.take().unwrap();
        (pending.action)(&mut h.app);
        h.app.guard_clipboard_output(&h.ctx);
        assert!(h.ctx.output(|output| output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "保密剪贴板内容"))));
        h.ctx.output_mut(|output| output.commands.clear());
        h.ctx.input_mut(|input| input.events.push(egui::Event::Cut));
        h.app.guard_clipboard_input(&h.ctx);
        assert!(
            !h.ctx
                .input(|input| input.events.contains(&egui::Event::Cut))
        );
        assert!(h.app.pending_output.is_some());
        h.app.pending_output = None; // 取消：不重放剪切，源文字不会被编辑器删除。
        assert!(h.app.clipboard_cut_replay.is_none());
    }

    #[test]
    fn memory_attachment_preview_needs_no_output_and_print_asks_once() {
        let mut h = Harness::new(true);
        let store = h.app.manuscript_store.as_mut().unwrap();
        let id = store
            .create(&crate::manuscript::NewManuscript::default(), None)
            .unwrap();
        store
            .add_pdf(id, "附件.pdf", b"PDF bytes stay in memory")
            .unwrap();
        let record = store.get(id).unwrap().unwrap();
        let pdf_id = record.pdfs[0].id;
        h.app.manuscript_detail = Some(record);
        h.app.open_pdf_attachment(pdf_id);
        assert!(h.app.pending_output.is_none());
        assert!(h.app.pdfs[0].is_memory());
        assert!(!h.dir.path().join("output").exists());
        h.app.open_pdf_attachment(pdf_id);
        assert_eq!(h.app.pdfs.len(), 1, "同一附件重复打开只切换标签");
        let key = h.app.pdfs[0].key;
        h.app.apply_pdf_actions(
            vec![crate::pdf_viewer::PdfAction::ReleaseMemory {
                bytes: std::sync::Arc::new(b"PDF bytes stay in memory".to_vec()),
                request: Box::new(crate::print::PrintRequest {
                    path: std::path::PathBuf::new(),
                    title: "附件.pdf".into(),
                    page_count: 1,
                    current_page: 0,
                    hints: Default::default(),
                    pdf_key: Some(key),
                }),
                print: true,
            }],
            &h.ctx,
        );
        // 加密工作区允许打印，但先弹一次「队列/纸件不在保护范围内」的风险确认；
        // 确认前既不开保存框也不打开打印对话框。
        let pending = h.app.pending_output.as_ref().expect("打印前应确认风险");
        assert_eq!(pending.warning, PRINT_WARNING);
        assert!(h.app.print_dialog.is_none());
        assert!(h.app.pending_file_dialogs.is_empty());
        assert!(!h.dir.path().join("output").exists());
        h.click("取消");
        assert!(h.app.pending_output.is_none());
        assert!(h.app.print_dialog.is_none());
        assert!(h.app.pending_file_dialogs.is_empty());
        assert!(!h.dir.path().join("output").exists());
    }

    struct Harness {
        app: GongwenApp,
        ctx: egui::Context,
        dir: tempfile::TempDir,
    }

    impl Harness {
        fn new(encrypted: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
            let mut app = super::super::management_samples::fixture();
            app.config.output_dir = dir.path().join("output").to_string_lossy().into_owned();
            let ctx = egui::Context::default();
            if encrypted {
                crate::vault::set_key_handle([31; 32], true);
            }
            Self { app, ctx, dir }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            crate::test_support::run_ui(
                &self.ctx,
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 700.0),
                    )),
                    events,
                    ..Default::default()
                },
                |_| self.app.output_risk_window(&self.ctx),
            )
        }

        fn click(&mut self, label: &str) {
            self.frame(Vec::new());
            let output = self.frame(Vec::new());
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => {
                        Some(text.pos + text.galley.size() * 0.5)
                    }
                    _ => None,
                })
                .expect("风险框应显示按钮");
            self.frame(vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                self.frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }]);
            }
        }

        fn key(&mut self, key: egui::Key) {
            self.frame(vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }]);
        }

        fn queue_file(&mut self, file: &str) -> std::path::PathBuf {
            let path = self.dir.path().join(file);
            let output = path.clone();
            self.app.request_plaintext_output("导出测试稿件", move |_| {
                std::fs::write(output, b"sensitive document").unwrap();
            });
            path
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            crate::vault::clear_key_handle();
            crate::storage::set_test_config_dir(None);
        }
    }

    #[test]
    fn encrypted_output_writes_only_after_explicit_confirmation_once() {
        let mut h = Harness::new(true);
        let path = h.queue_file("document.pdf");
        assert!(!path.exists());
        assert!(h.app.pending_file_dialogs.is_empty());
        h.frame(Vec::new());
        h.key(egui::Key::Enter);
        assert!(!path.exists(), "Enter 不能默认放行");
        h.click("确认风险并继续");
        assert_eq!(std::fs::read(&path).unwrap(), b"sensitive document");
        assert!(h.app.pending_output.is_none());
        std::fs::remove_file(&path).unwrap();
        h.frame(Vec::new());
        assert!(!path.exists(), "回调只能执行一次");
        h.queue_file("second.pdf");
        assert!(h.app.pending_output.is_some(), "第二次导出仍需确认");
    }

    #[test]
    fn cancel_and_escape_discard_output_without_creating_files() {
        let mut h = Harness::new(true);
        let path = h.queue_file("cancel.pdf");
        h.click("取消");
        assert!(h.app.pending_output.is_none());
        assert!(!path.exists());
        let path = h.queue_file("escape.docx");
        h.frame(Vec::new());
        h.frame(Vec::new());
        h.key(egui::Key::Escape);
        assert!(h.app.pending_output.is_none());
        assert!(!path.exists());
    }

    /// 加密工作区但当前没有密钥句柄：必须拒绝导出（fail-closed），不能静默放行。
    #[test]
    fn encrypted_workspace_without_key_refuses_output() {
        let mut h = Harness::new(false);
        h.app.vault_locked = true;
        crate::vault::clear_key_handle();
        let path = h.dir.path().join("blocked.pdf");
        let output = path.clone();
        h.app.request_plaintext_output("导出测试", move |_| {
            std::fs::write(output, b"sensitive").unwrap();
        });
        assert!(h.app.pending_output.is_none());
        assert!(!path.exists(), "未解锁时不得写出明文");
        assert!(h.app.status.contains("未解锁"));
    }

    /// 加密工作区允许打印：先确认风险，确认后照常打开打印对话框（不硬拦）。
    #[test]
    fn encrypted_print_warns_and_then_opens_the_dialog() {
        let mut h = Harness::new(true);
        let request = crate::print::PrintRequest {
            path: h.dir.path().join("尚未导出.pdf"),
            title: "关于测试的请示".into(),
            page_count: 1,
            current_page: 0,
            hints: Default::default(),
            pdf_key: None,
        };
        h.app.open_print_dialog(request, &h.ctx);
        let pending = h.app.pending_output.as_ref().expect("打印前应确认风险");
        assert_eq!(pending.warning, PRINT_WARNING);
        assert!(pending.operation.contains("关于测试的请示"));
        assert!(h.app.print_dialog.is_none());
        h.click("确认风险并继续");
        assert!(h.app.pending_output.is_none());
        assert!(
            h.app.print_dialog.is_some(),
            "确认风险后应照常打开打印对话框"
        );
    }

    #[test]
    fn ordinary_output_does_not_require_confirmation() {
        let mut h = Harness::new(false);
        let path = h.queue_file("plain.pdf");
        assert!(path.exists());
        assert!(h.app.pending_output.is_none());
    }

    #[test]
    fn pending_operation_cannot_be_replaced_and_loss_of_key_cancels_it() {
        let mut h = Harness::new(true);
        let first = h.queue_file("first.pdf");
        let second = h.queue_file("second.pdf");
        h.click("确认风险并继续");
        assert!(first.exists());
        assert!(!second.exists());
        let path = h.queue_file("locked.pdf");
        crate::vault::clear_key_handle();
        h.frame(Vec::new());
        assert!(h.app.pending_output.is_none());
        assert!(!path.exists());
    }

    #[test]
    fn quick_export_and_print_wait_without_attaching_followup_to_other_jobs() {
        let mut h = Harness::new(true);
        h.app.docs[0].generated_markdown = "# 标题\n\n正文。".into();
        for kind in ExportKind::ALL {
            h.app.draft_page().start_export_and_open(kind);
            assert!(!h.app.docs[0].busy);
            assert!(h.app.docs[0].open_after_export.is_none());
            h.app.apply_draft_actions(&h.ctx);
            assert!(h.app.pending_output.is_some());
            h.click("取消");
        }
        // 「打印…」先走一次导出确认（含打印意图）；取消后不导出、不打印，
        // 打开/打印意图也不能提前挂到别的任务上。
        h.app.draft_page().start_export_for_print();
        h.app.apply_draft_actions(&h.ctx);
        assert!(h.app.pending_output.is_some());
        assert!(!h.app.docs[0].print_after_export);
        assert!(!h.app.docs[0].busy);
        h.click("取消");
        assert!(h.app.pending_output.is_none());
        assert!(!h.app.docs[0].print_after_export);
        assert!(!h.dir.path().join("output").exists());
    }

    #[test]
    fn pdf_archive_explains_the_independent_password_and_plaintext_after_extraction() {
        let mut h = Harness::new(true);
        h.app.perform_export_selected_manuscript_pdfs(
            crate::manuscript_io::PdfExportOptions {
                stamped: true,
                compiled: true,
            },
            vec![1, 2],
            "independent-zip-password",
            false,
        );
        let pending = h.app.pending_output.as_ref().unwrap();
        assert_eq!(pending.warning, PDF_ARCHIVE_WARNING);
        assert!(pending.operation.contains("2 篇"));
        assert!(!h.app.manuscript_pdf_export_busy);
        assert!(h.app.pending_file_dialogs.is_empty());
        h.click("取消");
        assert!(h.app.pending_file_dialogs.is_empty());
    }

    #[test]
    #[ignore = "生成输出风险确认框样张"]
    fn output_risk_sample() {
        let mut h = Harness::new(true);
        h.queue_file("sample.pdf");
        crate::theme::configure_fonts(&h.ctx, &crate::models::FontConfig::default());
        crate::theme::configure_style(&h.ctx);
        let size = egui::vec2(800.0, 700.0);
        let mut canvas = crate::ui_snapshot::Canvas::default();
        for frame in 0..12 {
            let mut output = h.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    time: Some(frame as f64 * 0.1),
                    ..Default::default()
                },
                |_| h.app.output_risk_window(&h.ctx),
            );
            canvas.absorb(&mut output.textures_delta);
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tmp");
            std::fs::create_dir_all(&root).unwrap();
            canvas.render(
                &h.ctx,
                output,
                size,
                crate::theme::canvas(),
                &root.join("output-risk.png"),
            );
        }
    }
}
