//! 加密工作区向普通文件释放内容前的单次风险确认。
//! 这里只排队，不在确认前创建目录、打开保存框或启动导出线程。

use super::GongwenApp;
use crate::modal::{self, Dismiss};
use eframe::egui;

pub(crate) const CLIPBOARD_WARNING: &str = "此操作会把内容交给系统剪贴板；系统历史记录、跨设备同步或第三方剪贴板工具可能保存副本，不受应用锁保护。锁定或清空当前剪贴板不能保证删除历史副本。";
pub(crate) const PLAINTEXT_WARNING: &str = "此次导出的文件将以明文保存，可能包含正文、图片、附件、人员和单位等敏感信息，不受应用锁或导出密码保护。复制、同步、发送或由其他软件打开后，可能留下无法撤回的副本。请确认保存位置、接收范围及传输方式。";
pub(crate) const DOCUMENT_WARNING: &str = "已选择不加密导出。此次 Word、PDF、源码包及随行单据将以普通文件保存，正文、图片、人员、单位和份号等敏感信息不受密码保护。复制、发送或同步后可能导致信息泄露，锁定应用不能撤回文件。请确认保存位置、接收范围与传输方式。";
pub(crate) const PDF_ARCHIVE_WARNING: &str = "当前工作区已启用加密。此次导出的 7z 压缩包由独立密码保护，包内文件名也已加密，但解压后的 PDF 是普通文件，不受应用密码和本地加密保护。PDF 可能包含正文、图片、附件及人员等敏感信息；解压、复制、同步或发送后，可能导致信息泄露。压缩包自身的文件名和大小仍可见，请妥善保管导出密码并确认接收范围。";
pub(crate) const PRINT_WARNING: &str = "当前工作区已启用加密。打印会把明文 PDF 交给系统打印队列和打印机：队列可能自行保存副本，打印机缓存、纸件与后续处置都可能留下敏感信息，这些都不受应用密码和本地加密保护。请确认接收范围与纸件/队列记录的处置方式。";
pub(crate) const EXTERNAL_OPEN_WARNING: &str = "用系统程序打开会把文档交给外部软件处理；阅读器、Office/WPS 可能生成最近记录、缩略图、恢复文件或交换文件等明文副本，不受应用锁或导出密码保护。应用无法撤回这些副本，请确认当前环境与文档接收范围。";
pub(crate) const PACKAGE_ARCHIVE_WARNING: &str = "当前工作区已启用加密。此次导出的是稿件包（7z 格式，文件内容与包内文件名均由独立密码保护），但解压后的 Markdown 正文、历史版本、图片、PDF 附件与随行件全部是普通文件，不再受应用密码和本地加密保护。稿件包还可能包含人员、单位、文号等敏感信息；复制、同步、发送或由其他软件打开后，可能导致信息泄露。压缩包自身的文件名和大小仍可见，请妥善保管导出密码并确认接收范围。";
pub(crate) const PII_EXPORT_WARNING: &str = "当前工作区已启用加密。此次导出的表格或词表是普通文件，可能包含人员姓名、职务、联系电话、个人简介、单位名称等敏感信息，以及从稿件派生出的正文用语；复制、同步、发送或由其他软件打开后，可能导致信息泄露。请确认导出范围与接收对象。";
pub(crate) const BACKUP_WARNING: &str = "全量备份的内容和包内文件名均由独立备份密码保护，内部数据还保留应用锁加密。恢复需要备份密码和备份当时的应用锁密码；程序不保存备份密码，遗失后无法恢复。外层文件名和总大小仍可见，请使用中性名称并妥善保管。";

pub(crate) type OutputAction = Box<dyn FnOnce(&mut GongwenApp)>;

pub(super) struct PendingOutput {
    operation: String,
    warning: &'static str,
    slot: crate::storage::DataSlot,
    requires_key: bool,
    action: OutputAction,
}

impl GongwenApp {
    #[cfg(test)]
    pub(crate) fn request_plaintext_output(
        &mut self,
        operation: impl Into<String>,
        action: impl FnOnce(&mut Self) + 'static,
    ) {
        self.request_sensitive_output(operation.into(), PLAINTEXT_WARNING, Box::new(action));
    }

    pub(crate) fn request_document_output(
        &mut self,
        operation: impl Into<String>,
        action: impl FnOnce(&mut Self) + 'static,
    ) {
        self.request_sensitive_output(operation.into(), DOCUMENT_WARNING, Box::new(action));
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
        let document = warning == DOCUMENT_WARNING;
        let has_key = crate::vault::key_handle().is_some();
        if self.vault_locked && !has_key {
            self.status = "当前未解锁，已取消本次导出。".into();
            return;
        }
        // 稿件包、PDF 集合和全量备份始终由各自独立密码加密，无需明文风险提示。
        if warning == PDF_ARCHIVE_WARNING
            || warning == PACKAGE_ARCHIVE_WARNING
            || warning == BACKUP_WARNING
        {
            action(self);
            return;
        }
        if document {
            match crate::export::protection::Protection::current() {
                Ok(Some(_)) => {
                    action(self);
                    return;
                }
                Ok(None) => {} // 明文导出，包括未启用应用锁的工作区，每次确认。
                Err(error) => {
                    self.status = format!("无法导出：{error:#}");
                    return;
                }
            }
        } else if !has_key && warning != CLIPBOARD_WARNING {
            action(self);
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
            requires_key: has_key,
            action,
        });
    }

    pub(super) fn output_risk_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_output.as_ref() else {
            return;
        };
        // 锁定会丢弃整个工作区；此处还防范确认期间状态被其它入口改变。
        if pending.requires_key != crate::vault::key_handle().is_some()
            || pending.slot != crate::storage::current_slot()
        {
            self.pending_output = None;
            self.status = "加密状态已变化，本次导出已取消。".into();
            return;
        }
        let clipboard = pending.warning == CLIPBOARD_WARNING;
        let mut confirmed = false;
        let mut cancelled = false;
        let response = modal::dialog(
            ctx,
            egui::Id::unique("plaintext_output_risk"),
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
                ui.weak(if clipboard { "本次确认仅用于此次操作。取消不向剪贴板释放内容；锁定应用不能撤回外部副本。" } else { "本次确认仅用于此次操作。取消不会生成文件。文件保存后，锁定应用不会撤回这些文件；已加密成果仍由独立导出密码保护。" });
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
    fn approved_clipboard_payload_is_the_only_output_sent_to_the_platform() {
        for encrypted in [false, true] {
            let mut h = Harness::new(encrypted);
            super::super::clipboard::configure(&h.ctx);
            h.app
                .request_clipboard_export(egui::OutputCommand::CopyText("外发选区".into()));
            let pending = h
                .app
                .pending_output
                .take()
                .expect("普通与加密工作区都须确认");
            (pending.action)(&mut h.app);
            let output = crate::test_support::run_ui(&h.ctx, egui::RawInput::default(), |_| {
                h.app.handle_clipboard_input(&h.ctx);
                h.ctx.copy_text("仅应用内使用".into());
                h.app.publish_approved_copy(&h.ctx);
            });
            assert!(
                matches!(output.platform_output.commands.as_slice(), [egui::OutputCommand::CopyText(text)] if text == "外发选区")
            );
        }
    }

    #[test]
    fn memory_attachment_preview_needs_no_output_and_print_opens_dialog() {
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
        // 内存附件直接打印：打开对话框本身不释放内容，实际点击打印时才确认。
        assert!(h.app.pending_output.is_none());
        assert!(h.app.print_dialog.is_some());
        assert!(h.app.pending_file_dialogs.is_empty());
        assert!(!h.dir.path().join("output").exists());
        h.app.print_dialog = None;
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
                |_| {
                    self.app.print_dialog_window(&self.ctx);
                    self.app.output_risk_window(&self.ctx);
                },
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

    #[test]
    fn external_document_open_waits_for_confirmation() {
        let mut h = Harness::new(true);
        let path = h.dir.path().join("加密文档.pdf");
        h.app.apply_pdf_actions(
            vec![crate::pdf_viewer::PdfAction::OpenExternal(path)],
            &h.ctx,
        );
        let pending = h.app.pending_output.as_ref().expect("外部打开前应确认风险");
        assert_eq!(pending.warning, EXTERNAL_OPEN_WARNING);
        h.click("取消");
        assert!(h.app.pending_output.is_none());
    }

    /// 打开打印对话框不释放内容；每次实际提交打印任务都要确认。
    #[test]
    fn encrypted_print_is_confirmed_on_every_submission() {
        let mut h = Harness::new(true);
        let request = crate::print::PrintRequest {
            path: h.dir.path().join("尚未导出.pdf"),
            title: "关于测试的请示".into(),
            page_count: 1,
            current_page: 0,
            hints: Default::default(),
            pdf_key: None,
        };
        h.app.print_dialog = Some(crate::print::PrintDialog::new(
            1,
            request,
            "",
            Default::default(),
        ));
        h.app.apply_printers_listed(
            1,
            Ok(vec![crate::print::PrinterInfo {
                name: "测试打印机".into(),
                is_default: true,
                state: crate::print::PrinterState::Unknown,
            }]),
        );
        assert!(h.app.pending_output.is_none());

        h.key(egui::Key::Enter);
        let pending = h.app.pending_output.as_ref().expect("实际打印前应确认风险");
        assert_eq!(pending.warning, PRINT_WARNING);
        assert!(pending.operation.contains("关于测试的请示"));

        // 模拟取消：没有启动打印，也没有卡在「打印中」；再次点击仍会确认。
        h.app.pending_output = None;
        h.key(egui::Key::Enter);
        assert!(h.app.pending_output.is_some());
        h.app.pending_output = None;
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
        crate::vault::write_path(
            &h.dir.path().join(crate::export::protection::PASSWORD_FILE),
            "config",
            b"Jade!River7Cloud",
        )
        .unwrap();
        let mut policy = crate::export::protection::ExportPolicy::load().unwrap();
        policy.enabled = false;
        policy.save(&crate::vault::key_handle().unwrap()).unwrap();
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
    fn encrypted_documents_skip_confirmation_but_plaintext_requires_it_every_time() {
        let mut h = Harness::new(true);
        crate::vault::write_path(
            &h.dir.path().join(crate::export::protection::PASSWORD_FILE),
            "config",
            b"Jade!River7Cloud",
        )
        .unwrap();
        h.app
            .request_document_output("加密导出", |app| app.status = "已执行加密导出".into());
        assert!(h.app.pending_output.is_none());
        assert_eq!(h.app.status, "已执行加密导出");
        let mut policy = crate::export::protection::ExportPolicy::load().unwrap();
        policy.enabled = false;
        policy.save(&crate::vault::key_handle().unwrap()).unwrap();
        for _ in 0..2 {
            h.app.request_document_output("明文导出", |app| {
                app.status = "已执行明文导出".into()
            });
            let pending = h.app.pending_output.as_ref().unwrap();
            assert_eq!(pending.warning, DOCUMENT_WARNING);
            h.click("取消");
            assert_ne!(h.app.status, "已执行明文导出");
        }
        h.app
            .request_document_output("明文导出", |app| app.status = "已执行明文导出".into());
        h.click("确认风险并继续");
        assert_eq!(h.app.status, "已执行明文导出");
    }

    #[test]
    fn ordinary_workspace_document_export_also_warns_before_writing() {
        let mut h = Harness::new(false);
        let path = h.dir.path().join("plain.md");
        let target = path.clone();
        h.app.request_document_output("明文导出", move |_| {
            std::fs::write(target, b"body").unwrap();
        });
        assert!(!path.exists());
        h.click("确认风险并继续");
        assert_eq!(std::fs::read(path).unwrap(), b"body");
    }

    #[test]
    fn encrypted_archives_skip_the_plaintext_risk_dialog() {
        let mut h = Harness::new(true);
        for warning in [PDF_ARCHIVE_WARNING, PACKAGE_ARCHIVE_WARNING, BACKUP_WARNING] {
            h.app
                .request_output_with_warning("加密包导出", warning, |app| {
                    app.status = "已继续导出加密包".into()
                });
            assert!(h.app.pending_output.is_none());
            assert_eq!(h.app.status, "已继续导出加密包");
        }
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
