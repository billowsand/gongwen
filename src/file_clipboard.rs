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
        // SAFETY: 只在 UI 主线程调用；NSURL 与数组在写入期间由 Retained 持有。
        unsafe {
            let url = NSURL::fileURLWithPath(&NSString::from_str(path));
            let object: objc2::rc::Retained<ProtocolObject<dyn NSPasteboardWriting>> =
                ProtocolObject::from_retained(url);
            let objects = NSArray::from_vec(vec![object]);
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            if pasteboard.writeObjects(&objects) {
                Ok(())
            } else {
                Err("系统剪贴板拒绝写入".into())
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use windows_sys::Win32::Foundation::GlobalFree;
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

    /// Shell 的 `DROPFILES` 头，后面紧跟以双 NUL 结尾的宽字符路径列表。
    #[repr(C)]
    struct DropFiles {
        p_files: u32,
        pt_x: i32,
        pt_y: i32,
        f_nc: i32,
        f_wide: i32,
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

        let header = std::mem::size_of::<DropFiles>();
        let mut bytes = Vec::with_capacity(header + wide.len() * 2);
        let drop_files = DropFiles {
            p_files: header as u32,
            pt_x: 0,
            pt_y: 0,
            f_nc: 0,
            f_wide: 1,
        };
        // SAFETY: DropFiles 是 repr(C) 的纯整数结构，按字节读取没有未定义行为。
        bytes.extend_from_slice(unsafe {
            std::slice::from_raw_parts((&raw const drop_files).cast::<u8>(), header)
        });
        for unit in &wide {
            bytes.extend_from_slice(&unit.to_ne_bytes());
        }

        // SAFETY: 标准的 Win32 剪贴板调用序列；SetClipboardData 成功后内存归系统所有，
        // 失败时由本函数释放。
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                return Err("剪贴板正被其他程序占用，请稍后再试".into());
            }
            let result = (|| {
                if EmptyClipboard() == 0 {
                    return Err("无法清空剪贴板".to_string());
                }
                set_data(CF_HDROP, &bytes)?;
                let name: Vec<u16> = "Preferred DropEffect\0".encode_utf16().collect();
                let effect_format = RegisterClipboardFormatW(name.as_ptr());
                if effect_format != 0 {
                    // 放置效果只是提示，写不进去也不影响粘贴。
                    let _ = set_data(effect_format, &DROPEFFECT_COPY.to_ne_bytes());
                }
                Ok(())
            })();
            CloseClipboard();
            result
        }
    }

    /// 分配可移动全局内存、拷入数据并交给剪贴板。调用前剪贴板须已打开。
    unsafe fn set_data(format: u32, data: &[u8]) -> Result<(), String> {
        unsafe {
            let handle = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, data.len());
            if handle.is_null() {
                return Err("分配剪贴板内存失败".into());
            }
            let target = GlobalLock(handle);
            if target.is_null() {
                GlobalFree(handle);
                return Err("锁定剪贴板内存失败".into());
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), target.cast::<u8>(), data.len());
            GlobalUnlock(handle);
            if SetClipboardData(format, handle).is_null() {
                GlobalFree(handle);
                return Err("写入剪贴板失败".into());
            }
            Ok(())
        }
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
