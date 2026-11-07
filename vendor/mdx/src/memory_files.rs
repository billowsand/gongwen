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

// 输出捕获与输入作用域独立：嵌套和 panic 都恢复先前状态，业务内容不经过临时文件。
thread_local! {
    static OUTPUT: RefCell<Option<HashMap<PathBuf, Vec<u8>>>> = const { RefCell::new(None) };
}

pub fn capture_outputs<T>(convert: impl FnOnce() -> T) -> (T, HashMap<PathBuf, Vec<u8>>) {
    struct Restore(Option<HashMap<PathBuf, Vec<u8>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OUTPUT.with(|out| *out.borrow_mut() = self.0.take());
        }
    }
    let restore = Restore(OUTPUT.with(|out| out.replace(Some(HashMap::new()))));
    let result = convert();
    let files = OUTPUT.with(|out| out.borrow_mut().take().unwrap_or_default());
    drop(restore);
    (result, files)
}

pub fn output_is_captured() -> bool {
    OUTPUT.with(|out| out.borrow().is_some())
}

pub fn write_output(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    OUTPUT.with(|out| {
        if let Some(files) = out.borrow_mut().as_mut() {
            files.insert(path.to_owned(), bytes.to_vec());
            Ok(())
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, bytes)
        }
    })
}

pub fn read_output(path: &Path) -> std::io::Result<Vec<u8>> {
    OUTPUT.with(|out| match out.borrow().as_ref() {
        Some(files) => files
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "内存输出不存在")),
        None => std::fs::read(path),
    })
}
