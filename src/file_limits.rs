//! 读用户交来的文件时的体积上限。
//!
//! 桌面程序里「整个文件读进内存」是最常见的耗尽点：用户挑一个几 GB 的文件
//! 或一个恶意构造的压缩包，程序就会被内存占用拖垮，界面直接没反应。这里给出
//! 各类文件的统一上限，各处按用途取用；超限的文件在读之前就被拒绝，不进内存。
//!
//! 上限只是「够用」的值：正常公文、报告、扫描件都远小于它们。

use std::io::{self, Read as _};
use std::path::Path;

/// 导入文档（Word / PDF / 图片等）的体积上限。
pub const MAX_DOCUMENT_BYTES: u64 = 256 * 1024 * 1024;
/// 打印用 PDF 的体积上限。
pub const MAX_PDF_BYTES: u64 = 200 * 1024 * 1024;
/// 字体文件的体积上限。
pub const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
/// 纯文本文件（txt / md / csv 等）的体积上限。
pub const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;
/// 单张图片的体积上限：插图入库与同步包导入用同一个值，插得进去的图
/// 就一定能随同步包带回来。
pub const MAX_IMAGE_BYTES: u64 = 100 * 1024 * 1024;
/// 稿件包（`.gwx`）整包的体积上限：导入前要复制一份到暂存目录，挡住误选的
/// 超大文件把磁盘写满。包内各条目另有各自的上限（见 `manuscript_io`）。
pub const MAX_PACKAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 读一个文件，超过上限就报错，不进内存。
///
/// 先看已打开句柄的大小再读，读的时候最多只读 `max + 1` 字节：文件在读的
/// 过程中被换掉或变大（大文件替换、网络盘抖动、管道与设备文件报不出大小）
/// 时以实际读到的字节数为准，内存占用也不会超过上限。
pub fn read_bytes(path: &Path, max: u64, label: &str) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let declared = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    if declared > max {
        return Err(too_large(label, max));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(declared).unwrap_or(0));
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(too_large(label, max));
    }
    Ok(bytes)
}

/// 超限的错误：统一措辞，方便界面直接显示。
pub fn too_large(label: &str, max: u64) -> io::Error {
    io::Error::other(format!("{label}超过 {} MB 上限", max / 1024 / 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_bytes_rejects_files_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, b"12345").unwrap();
        assert_eq!(read_bytes(&path, 5, "测试").unwrap(), b"12345");
        let error = read_bytes(&path, 4, "测试").unwrap_err();
        assert!(error.to_string().contains("测试超过"), "{error}");
    }
}
