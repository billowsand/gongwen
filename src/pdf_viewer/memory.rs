//! PDF 附件的内存来源与显式明文释放。
use super::*;

impl PdfSession {
    pub(crate) fn from_memory(
        key: PdfKey,
        identity: PathBuf,
        title: String,
        bytes: Vec<u8>,
        results: Sender<WorkerResult>,
    ) -> Self {
        let mut session = Self::new(key, identity, Some(title), results);
        session.memory = Some(Arc::new(bytes));
        session
    }

    pub(crate) fn is_memory(&self) -> bool {
        self.memory.is_some()
    }

    pub(super) fn print_action(&self) -> PdfAction {
        match &self.memory {
            Some(bytes) => PdfAction::ReleaseMemory {
                bytes: Arc::clone(bytes),
                request: Box::new(self.print_request()),
                print: true,
            },
            None => PdfAction::Print(self.print_request()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{drain, minimal_pdf};
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn memory_attachment_renders_without_a_file_and_releases_only_by_explicit_action() {
        let dir = tempfile::tempdir().unwrap();
        let identity = dir.path().join("never-created.pdf");
        let (tx, rx) = mpsc::channel();
        let ctx = egui::Context::default();
        let bytes = minimal_pdf();
        let mut session =
            PdfSession::from_memory(7, identity.clone(), "附件".into(), bytes.clone(), tx);
        session.ensure_worker(&ctx);
        assert!(drain(&rx, |message| {
            let opened = matches!(message, PdfMessage::Opened { .. });
            session.apply_message(&ctx, message);
            opened
        }));
        assert_eq!(session.page_count(), 1);
        assert!(!identity.exists());
        assert!(session.tab_hover().contains("内存预览"));
        session.request_print_when_ready();
        let Some(PdfAction::ReleaseMemory {
            bytes: released,
            request,
            print,
        }) = session.take_print_when_ready()
        else {
            panic!("内存 PDF 不能绕过明文释放确认直接打印");
        };
        assert!(print);
        assert_eq!(released.as_ref(), &bytes);
        assert_eq!(request.pdf_key, Some(7));
        assert!(session.take_print_when_ready().is_none());
        assert!(!identity.exists());
    }

    #[test]
    fn memory_pdf_figure_is_embedded_in_word_without_poppler_or_input_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("not-created");
        let input = root.join("report.md");
        let pdf = minimal_pdf();
        let rendered = crate::pdf_image::png(&pdf, 1).unwrap();
        assert!(rendered.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(crate::pdf_image::png(&pdf, 0).is_err());
        assert!(crate::pdf_image::png(&pdf, 2).is_err());
        let files = mdx::memory_files::MemoryFiles::new(
            std::collections::HashMap::from([
                (
                    input.clone(),
                    Arc::new(b"# Report\n\n## Body\n\n![PDF](images/scan.pdf#page=1)\n".to_vec()),
                ),
                (root.join("images/scan.pdf"), Arc::new(pdf)),
            ]),
            crate::pdf_image::png,
        );
        let output = dir.path().join("report.docx");
        files
            .run(|| mdx::convert_research_docx_with_body_normalizer(&input, &output, str::to_owned))
            .unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
        let name = zip
            .file_names()
            .find(|name| name.starts_with("word/media/") && !name.ends_with('/'))
            .unwrap()
            .to_string();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut zip.by_name(&name).unwrap(), &mut bytes).unwrap();
        let embedded = image::load_from_memory(&bytes)
            .expect("Word 必须嵌入可解码的栅格图，不能把 PDF 变成缺图占位");
        assert!(embedded.width() > 100 && embedded.height() > 100);
        assert!(!root.exists());
    }
}
