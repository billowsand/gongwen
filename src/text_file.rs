//! 读取用户交来的文本文件，自动识别编码。
//!
//! 机关里流转的 txt / csv / md 大量来自旧系统或 Windows 记事本，常见的是
//! GB18030 / GBK，偶尔是带 BOM 的 UTF-16。只认 UTF-8 的话，这些文件要么读不进来，
//! 要么让用户先去"另存为 UTF-8"——这一步对多数使用者就是门槛。
//!
//! 判定顺序：
//! 1. 有 BOM 就按 BOM（UTF-8 / UTF-16LE / UTF-16BE），并去掉 BOM；
//! 2. 整份是合法 UTF-8 就按 UTF-8——这是绝大多数情况，也最不会错；
//! 3. 否则交给 `chardetng` 猜，顶级域提示给 `cn`，GBK 与 Big5 拿不准时偏向 GBK。
//!
//! 只用于**用户选的外部文件**。程序自己写出的配置、TeX、埋点仍按 UTF-8 读，
//! 不走这里——那些文件读出乱码说明是真坏了，不该被"猜"掩盖过去。

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::Encoding;
use std::io;
use std::path::Path;

/// 读一个文本文件并转成 UTF-8 字符串。读文件失败时返回原始 I/O 错误；
/// 解码本身不会失败，个别无法映射的字节会变成 U+FFFD。
pub fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    std::fs::read(path).map(|bytes| decode(&bytes).0)
}

/// 把字节解码成 UTF-8，同时返回判定出的编码，供需要提示用户的地方使用。
pub fn decode(bytes: &[u8]) -> (String, &'static Encoding) {
    if let Some((encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return (text.into_owned(), encoding);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (text.to_owned(), encoding_rs::UTF_8);
    }
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let encoding = detector.guess(Some(b"cn"), Utf8Detection::Deny);
    let (text, _) = encoding.decode_without_bom_handling(bytes);
    (text.into_owned(), encoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "关于做好2026年度安全生产工作的通知\n各区县人民政府，市政府各部门：";

    #[test]
    fn plain_utf8_is_kept_as_is() {
        let (text, encoding) = decode(SAMPLE.as_bytes());
        assert_eq!(text, SAMPLE);
        assert_eq!(encoding, encoding_rs::UTF_8);
    }

    #[test]
    fn utf8_bom_is_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(SAMPLE.as_bytes());
        assert_eq!(decode(&bytes).0, SAMPLE);
    }

    #[test]
    fn gbk_text_is_detected() {
        let (bytes, _, _) = encoding_rs::GBK.encode(SAMPLE);
        let (text, encoding) = decode(&bytes);
        assert_eq!(text, SAMPLE);
        assert_eq!(encoding, encoding_rs::GBK);
    }

    #[test]
    fn utf16le_with_bom_is_decoded() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in SAMPLE.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode(&bytes).0, SAMPLE);
    }

    #[test]
    fn empty_input_is_empty_utf8() {
        let (text, encoding) = decode(&[]);
        assert!(text.is_empty());
        assert_eq!(encoding, encoding_rs::UTF_8);
    }

    #[test]
    fn read_to_string_decodes_gbk_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("来文.txt");
        let (bytes, _, _) = encoding_rs::GBK.encode(SAMPLE);
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(read_to_string(&path).unwrap(), SAMPLE);
    }
}
