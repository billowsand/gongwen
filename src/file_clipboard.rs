//! 把文件本身放进系统剪贴板，效果等同在文件管理器里“复制”该文件：
//! 之后可在访达 / 资源管理器 / 聊天窗口里直接粘贴出一份副本。
//!
//! egui 的剪贴板只认文本，这里按平台各走原生通道：
//! - macOS：`NSPasteboard` 写入文件 `NSURL`；
//! - Windows：`CF_HDROP`（`DROPFILES` + 宽字符路径），附带“首选放置效果 = 复制”；
//! - Linux：借 `wl-copy`（Wayland）或 `xclip`（X11）写 `text/uri-list`。

use std::path::Path;

/// 把一个文件复制到系统剪贴板。
pub(crate) fn copy_file(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("文件不存在：{}", path.display()));
    }
    let path = path
        .canonicalize()
        .map_err(|error| format!("无法解析路径：{error}"))?;
    platform::copy_file(&path)
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};
    use std::path::Path;

    pub(super) fn copy_file(path: &Path) -> Result<(), String> {
        let path = path.to_str().ok_or("路径含无法识别的字符")?;
        // SAFETY: 参数是调用期间存活的 `NSString`；返回的 `NSURL` 由 `Retained` 持有。
        let url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(path)) };
        let object: objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>> =
            ProtocolObject::from_retained(url);
        let objects = NSArray::from_vec(vec![object]);
        // SAFETY: 取系统通用剪贴板没有前置条件；返回值由 `Retained` 持有，
        // 下面的清空与写入都作用在这个存活的对象上。只在 UI 主线程调用。
        let pasteboard = unsafe { NSPasteboard::generalPasteboard() };
        // SAFETY: 同上，`pasteboard` 有效。
        unsafe { pasteboard.clearContents() };
        // SAFETY: `objects` 里的元素都实现 `NSPasteboardWriting`，写入期间由
        // `Retained` 持有。
        if unsafe { pasteboard.writeObjects(&objects) } {
            Ok(())
        } else {
            Err("系统剪贴板拒绝写入".into())
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL};
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock,
    };

    /// 标准剪贴板格式 `CF_HDROP`。
    const CF_HDROP: u32 = 15;
    /// `DROPEFFECT_COPY`：告诉资源管理器粘贴时复制而不是移动。
    const DROPEFFECT_COPY: u32 = 1;

    /// Shell 的 `DROPFILES` 头的字节数：五个 4 字节字段
    /// （`pFiles`、`pt.x`、`pt.y`、`fNC`、`fWide`），后面紧跟以双 NUL 结尾的
    /// 宽字符路径列表。
    const DROPFILES_LEN: u32 = 20;

    /// 按小端逐字段写出 `DROPFILES` 头：不必把 `repr(C)` 结构按字节重新解释，
    /// 也就不需要 `unsafe`。
    fn drop_files_header() -> [u8; DROPFILES_LEN as usize] {
        let fields: [u32; 5] = [
            DROPFILES_LEN, // pFiles：路径列表紧跟在头后面
            0,             // pt.x
            0,             // pt.y
            0,             // fNC
            1,             // fWide：路径是宽字符
        ];
        let mut header = [0u8; DROPFILES_LEN as usize];
        let (chunks, _) = header.as_chunks_mut::<4>();
        for (chunk, field) in chunks.iter_mut().zip(fields) {
            *chunk = field.to_le_bytes();
        }
        header
    }

    /// 打开着的系统剪贴板。`Drop` 时关闭，任何早退路径都不会让剪贴板一直被占着。
    struct OpenedClipboard;

    impl OpenedClipboard {
        fn open() -> Result<Self, String> {
            // SAFETY: 打开剪贴板没有指针参数；空的窗口句柄表示关联到当前任务。
            if unsafe { OpenClipboard(std::ptr::null_mut()) } == 0 {
                return Err("剪贴板正被其他程序占用，请稍后再试".into());
            }
            Ok(Self)
        }

        fn empty(&self) -> Result<(), String> {
            // SAFETY: 剪贴板已由本守卫打开。
            if unsafe { EmptyClipboard() } == 0 {
                return Err("无法清空剪贴板".into());
            }
            Ok(())
        }
    }

    impl Drop for OpenedClipboard {
        fn drop(&mut self) {
            // SAFETY: 剪贴板由 `open` 打开，这里恰好关闭一次。
            unsafe {
                CloseClipboard();
            }
        }
    }

    /// 剪贴板内存句柄的守卫。
    ///
    /// `GlobalAlloc` 出来的内存必须恰好释放一次：交给系统（`SetClipboardData`
    /// 成功）之后所有权归系统，没交出去就由守卫在 `Drop` 里释放。任何中途
    /// 返回的错误路径都不会漏掉这块内存。
    struct GlobalHandle(HGLOBAL);

    impl GlobalHandle {
        /// 分配一块 `len` 字节、内容清零的可移动全局内存。
        fn new(len: usize) -> Result<Self, String> {
            // SAFETY: `GlobalAlloc` 只按长度分配，没有指针参数；返回的句柄
            // 立刻交给本守卫持有。
            let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, len) };
            if handle.is_null() {
                return Err("分配剪贴板内存失败".into());
            }
            Ok(Self(handle))
        }

        /// 锁定内存，返回可写指针。调用方写完必须调 [`GlobalHandle::unlock`]。
        fn lock(&self) -> Result<*mut u8, String> {
            // SAFETY: 句柄由 `GlobalAlloc` 分配、尚未释放；对 `GMEM_MOVEABLE`
            // 内存 `GlobalLock` 返回可写指针，失败时返回空指针。
            let target = unsafe { GlobalLock(self.0) };
            if target.is_null() {
                return Err("锁定剪贴板内存失败".into());
            }
            Ok(target.cast::<u8>())
        }

        /// 解锁内存。解锁失败不影响数据已经写入的事实，只当作提示忽略。
        fn unlock(&self) {
            // SAFETY: 句柄有效且此刻处于锁定状态，解锁是配对操作。
            unsafe {
                GlobalUnlock(self.0);
            }
        }

        /// 把内存交给剪贴板。成功后所有权归系统（守卫不再释放它），失败时
        /// 守卫照旧释放，不漏内存。
        fn give_to_clipboard(self, format: u32) -> Result<(), String> {
            let raw = self.0;
            // SAFETY: 句柄有效、内存已解锁；`SetClipboardData` 失败时返回空指针，
            // 此时所有权仍在本函数手里，由守卫释放。
            if unsafe { SetClipboardData(format, raw.cast()) }.is_null() {
                return Err("写入剪贴板失败".into());
            }
            // 系统已接管这块内存，守卫不再释放。
            std::mem::forget(self);
            Ok(())
        }
    }

    impl Drop for GlobalHandle {
        fn drop(&mut self) {
            // SAFETY: 句柄来自 `GlobalAlloc`；走到 `Drop` 说明它没有经
            // `give_to_clipboard` 交给系统（那里会 `forget` 守卫），这里释放一次。
            unsafe {
                GlobalFree(self.0);
            }
        }
    }

    pub(super) fn copy_file(path: &Path) -> Result<(), String> {
        // canonicalize 在 Windows 上会得到 `\\?\` 前缀，资源管理器不认，去掉它。
        let text = path.to_string_lossy();
        let text = match text.strip_prefix(r"\\?\UNC\") {
            Some(rest) => format!(r"\\{rest}"),
            None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_string(),
        };
        let mut wide: Vec<u16> = std::ffi::OsStr::new(&text).encode_wide().collect();
        wide.extend([0, 0]);

        let header = drop_files_header();
        let mut bytes = Vec::with_capacity(header.len() + wide.len().saturating_mul(2));
        bytes.extend_from_slice(&header);
        // 宽字符按小端写出：剪贴板格式规定用小端，不依赖本机字节序。
        for unit in &wide {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let clipboard = OpenedClipboard::open()?;
        clipboard.empty()?;
        set_data(&clipboard, CF_HDROP, &bytes)?;
        let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
        // SAFETY: `name` 是以 NUL 结尾的宽字符串，调用期间存活。
        let effect_format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
        if effect_format != 0 {
            // 放置效果只是提示，写不进去也不影响粘贴。
            let _ = set_data(&clipboard, effect_format, &DROPEFFECT_COPY.to_le_bytes());
        }
        drop(clipboard);
        Ok(())
    }

    /// 分配可移动全局内存、拷入数据并交给剪贴板。
    ///
    /// 这是**安全**函数：分配、写入、释放都收在内部，外部拿不到裸指针，
    /// 也就不必把 `unsafe` 的义务传下去；「剪贴板须已打开」由参数
    /// `&OpenedClipboard` 在类型上保证。
    fn set_data(_clipboard: &OpenedClipboard, format: u32, data: &[u8]) -> Result<(), String> {
        let handle = GlobalHandle::new(data.len())?;
        {
            let target = handle.lock()?;
            // SAFETY: `target` 来自 `GlobalLock`，指向刚分配的、`data.len()` 字节的
            // 可写区域；`data` 与它不重叠（前者是堆上独立分配的全局内存）。
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), target, data.len());
            }
            handle.unlock();
        }
        handle.give_to_clipboard(format)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    use std::io::Write as _;
    use std::path::Path;
    use std::process::{Command, Stdio};

    pub(super) fn copy_file(path: &Path) -> Result<(), String> {
        let uri = format!("{}\r\n", super::file_uri(path));
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let candidates: &[(&str, &[&str])] = if wayland {
            &[
                ("wl-copy", &["--type", "text/uri-list"]),
                ("xclip", &["-selection", "clipboard", "-t", "text/uri-list"]),
            ]
        } else {
            &[
                ("xclip", &["-selection", "clipboard", "-t", "text/uri-list"]),
                ("wl-copy", &["--type", "text/uri-list"]),
            ]
        };
        for (program, args) in candidates {
            let Ok(mut child) = Command::new(program)
                .args(*args)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                continue;
            };
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(uri.as_bytes())
                    .map_err(|error| format!("写入 {program} 失败：{error}"))?;
            }
            // 两个工具读完输入都会转入后台守着剪贴板，前台进程随即退出。
            return match child.wait() {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => Err(format!("{program} 退出码 {status}")),
                Err(error) => Err(format!("{program} 运行失败：{error}")),
            };
        }
        Err("未找到 wl-copy 或 xclip，无法把文件放进剪贴板（可安装 wl-clipboard 或 xclip）".into())
    }
}

/// `file://` URI：路径按字节做百分号编码，只保留不需转义的字符与 `/`。
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut uri = String::from("file://");
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_escapes_chinese_and_spaces() {
        assert_eq!(
            file_uri(Path::new("/tmp/研 报 a.pdf")),
            "file:///tmp/%E7%A0%94%20%E6%8A%A5%20a.pdf"
        );
    }

    #[test]
    fn missing_file_is_rejected() {
        assert!(copy_file(Path::new("/definitely/not/here.pdf")).is_err());
    }
}
