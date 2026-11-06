//! 同步转换的只读内存输入。作用域内不存在磁盘回落；输出成品仍由调用方指定。
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

type PdfRenderer = fn(&[u8], u32) -> anyhow::Result<Vec<u8>>;

#[derive(Clone)]
pub struct MemoryFiles {
    files: Arc<HashMap<PathBuf, Arc<Vec<u8>>>>,
    pdf_renderer: PdfRenderer,
}

thread_local! {
    static INPUT: RefCell<Option<MemoryFiles>> = const { RefCell::new(None) };
}

impl MemoryFiles {
    pub fn new(files: HashMap<PathBuf, Arc<Vec<u8>>>, pdf_renderer: PdfRenderer) -> Self {
        Self {
            files: Arc::new(files),
            pdf_renderer,
        }
    }

    /// 仅用于当前线程同步执行的 mdx 转换；嵌套调用与 panic 均恢复先前作用域。
    pub fn run<T>(&self, convert: impl FnOnce() -> T) -> T {
        struct Restore(Option<MemoryFiles>);
        impl Drop for Restore {
            fn drop(&mut self) {
                INPUT.with(|input| *input.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(INPUT.with(|input| input.replace(Some(self.clone()))));
        convert()
    }
}

pub(crate) fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    INPUT.with(|input| match input.borrow().as_ref() {
        Some(input) => input
            .files
            .get(&PathBuf::from(path.to_string_lossy().replace('\\', "/")))
            .map(|bytes| bytes.as_ref().clone())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "内存输入中没有该资源")
            }),
        None => std::fs::read(path),
    })
}

pub(crate) fn read_to_string(path: &Path) -> std::io::Result<String> {
    String::from_utf8(read(path)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub(crate) fn is_file(path: &Path) -> bool {
    INPUT.with(|input| match input.borrow().as_ref() {
        Some(input) => input
            .files
            .contains_key(&PathBuf::from(path.to_string_lossy().replace('\\', "/"))),
        None => path.is_file(),
    })
}

pub(crate) fn active() -> bool {
    INPUT.with(|input| input.borrow().is_some())
}

pub(crate) fn rasterize_pdf(path: &Path, page: u32) -> Option<anyhow::Result<Vec<u8>>> {
    // 先复制句柄、释放 RefCell 借用，再调用宿主，允许宿主嵌套运行转换。
    let input = INPUT.with(|input| input.borrow().clone())?;
    Some(
        input
            .files
            .get(&PathBuf::from(path.to_string_lossy().replace('\\', "/")))
            .ok_or_else(|| anyhow::anyhow!("内存输入中没有 PDF 资源"))
            .and_then(|bytes| (input.pdf_renderer)(bytes, page)),
    )
}
