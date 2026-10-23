//! 剪贴板里的图片：只从 data: 与本机 file: 取字节，网络图片一律不下载。
//!
//! Word 常把图片做成内联的 data: 地址，网页给 http(s) 地址，WPS 有时给 file:
//! 路径。能拿到字节的交给 anydoc 的 HTML 入口（vendor/anydoc 补丁三），再由
//! embedded::render 按字节认格式、去重、写进图片资源库；拿不到的留一行
//! 「图片未导入」注释，让用户手动补图。
//!
//! 一次粘贴最多取 MAX_PASTE_IMAGES 张，超出的计未导入；整批字节数另有上限：
//! 五十张 100 MB 的图挨个读也能把内存吃光，总量上限是方案外的加固。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use anydoc::html::PastedImage;

use crate::doc_import::embedded::{ImageReport, MISSING_DATA_REASON, marker_text};
use crate::file_limits::{MAX_IMAGE_BYTES, read_bytes};

/// 一次粘贴最多取多少张图。超出的按「一次粘贴的图片超过 50 张」记未导入。
pub(crate) const MAX_PASTE_IMAGES: usize = 50;

/// 一次粘贴取的图片字节总量上限（方案外加固，见模块说明）。
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// 解析剪贴板里的图片地址，并记下每处取不到字节的原因。
///
/// 同一个地址只读一次；「同一张图只入库一次」由 embedded::render 按字节去重，
/// 这里只保证不重复读文件、不重复计数。
pub(crate) struct ClipboardImages {
    state: RefCell<State>,
}

struct State {
    /// 地址 → 解析结果；同一张图出现两次时第二次直接复用。
    cache: HashMap<String, Result<PastedImage, String>>,
    /// 按出现顺序记下取不到字节的原因，渲染后换进正文注释与状态栏。
    failures: Vec<String>,
    /// 已经尝试过的图片处数（含失败），用来卡 MAX_PASTE_IMAGES。
    taken: usize,
    /// 已经读到的字节总数。
    bytes: u64,
    /// 单张上限，默认 MAX_IMAGE_BYTES；测试里换成小数字。
    max_image_bytes: u64,
    /// 整批上限，默认 MAX_TOTAL_BYTES。
    max_total_bytes: u64,
}

impl Default for ClipboardImages {
    fn default() -> Self {
        Self::with_limits(MAX_IMAGE_BYTES, MAX_TOTAL_BYTES)
    }
}

impl ClipboardImages {
    fn with_limits(max_image_bytes: u64, max_total_bytes: u64) -> Self {
        Self {
            state: RefCell::new(State {
                cache: HashMap::new(),
                failures: Vec::new(),
                taken: 0,
                bytes: 0,
                max_image_bytes,
                max_total_bytes,
            }),
        }
    }

    /// 取一处的图片字节；取不到时记一条原因并返回 None。
    ///
    /// anydoc 每遇到一个 img 调一次。同一地址的第二处命中缓存，但失败原因仍按处
    /// 再记一条：正文里每张未导入的图都对应一条注释、状态栏里的一张。
    pub(crate) fn resolve(&self, src: &str) -> Option<PastedImage> {
        let mut state = self.state.borrow_mut();
        let cached = state.cache.get(src).cloned();
        let result = match cached {
            Some(result) => result,
            None => {
                let result = state.load(src);
                state.cache.insert(src.to_string(), result.clone());
                result
            }
        };
        if let Err(reason) = &result {
            state.failures.push(reason.clone());
        }
        result.ok()
    }

    /// 渲染后把通用原因（渲染器只知道「没有图片数据」）换成本地更具体的原因。
    ///
    /// 正文里的「图片未导入」注释与 ImageReport 一起换，状态栏才说得具体。正文里
    /// 的注释是渲染后按行写的，顺序与图片出现顺序一致；少数被挪到表格或标题后面
    /// 的图可能让两种原因互换位置，只影响措辞，不影响入库。
    pub(crate) fn explain(&self, markdown: String, report: &mut ImageReport) -> String {
        let failures = self.state.borrow().failures.clone();
        let mut reasons = failures.iter();
        for reason in &mut report.skipped {
            if reason == MISSING_DATA_REASON
                && let Some(local) = reasons.next()
            {
                *reason = local.clone();
            }
        }
        let generic = format!("<!-- [图片未导入：{MISSING_DATA_REASON}] -->");
        if !markdown.contains(&generic) {
            return markdown;
        }
        let mut reasons = failures.iter();
        markdown
            .split(char::from(10))
            .map(|line| {
                if line.trim() == generic
                    && let Some(local) = reasons.next()
                {
                    return format!("<!-- [图片未导入：{}] -->", marker_text(local));
                }
                line.to_string()
            })
            .collect::<Vec<_>>()
            .join(&String::from(char::from(10)))
    }
}

impl State {
    fn load(&mut self, src: &str) -> Result<PastedImage, String> {
        if self.taken >= MAX_PASTE_IMAGES {
            return Err(format!("一次粘贴的图片超过 {MAX_PASTE_IMAGES} 张"));
        }
        self.taken += 1;
        let image = read_image(src, self.max_image_bytes)?;
        let size = image.bytes.len() as u64;
        if self.bytes.saturating_add(size) > self.max_total_bytes {
            return Err(format!(
                "一批图片的总量超过 {} MB",
                self.max_total_bytes / (1024 * 1024)
            ));
        }
        self.bytes += size;
        Ok(image)
    }
}

/// 按地址读出图片字节：data: 只解码、file: 只读本机文件，别的一概不碰。
/// 剪贴板 HTML 里有没有**能入库**的图（`data:` 或本机 `file:` 地址）。
///
/// 起草页只粘一行时据此决定接不接管：网络图反正不下载，旁边一个跟踪像素就让「复制
/// 一个词」变成块级粘贴、外加一行「图片未导入」注释，得不偿失。
pub(crate) fn has_importable_image(html: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)<img\b[^>]*?\bsrc\s*=\s*["']?\s*(?:data|file):"#)
            .expect("valid image pattern")
    })
    .is_match(html)
}

fn read_image(src: &str, max_bytes: u64) -> Result<PastedImage, String> {
    if src.starts_with("data:") {
        return read_data_url(src, max_bytes);
    }
    let url = url::Url::parse(src).map_err(|_| "图片路径无法识别".to_string())?;
    if url.scheme() == "file" {
        read_file_url(&url, max_bytes)
    } else {
        // http(s) 与其它 scheme 都不联网取图，留注释让用户手动补。
        Err("网络图片不下载".to_string())
    }
}

/// 内联 data: 图片。剪贴板 HTML 本身已卡 8 MiB，这里的体积检查是防解析器
/// 意外吃到超大字符串：编码长度至少是解码长度的四分之三，先按编码长度挡一道。
fn read_data_url(src: &str, max_bytes: u64) -> Result<PastedImage, String> {
    if src.len() as u64 > max_bytes {
        return Err(too_large(max_bytes));
    }
    let url = data_url::DataUrl::process(src).map_err(|_| "图片数据损坏".to_string())?;
    let media_type = url.mime_type().to_string();
    if !media_type.starts_with("image/") {
        return Err("不是图片格式".to_string());
    }
    let (bytes, _fragment) = url
        .decode_to_vec()
        .map_err(|_| "图片数据损坏".to_string())?;
    if bytes.len() as u64 > max_bytes {
        return Err(too_large(max_bytes));
    }
    Ok(PastedImage { media_type, bytes })
}

/// 本机文件。Word / WPS 给的是 file: 地址，中文用户名与空格都百分号编码过，
/// 交给 url 的 to_file_path 解；带主机名的一律当网络共享拒掉（Windows 会把
/// file://server/share 转成 UNC 路径，读之前就得挡住）。
fn read_file_url(url: &url::Url, max_bytes: u64) -> Result<PastedImage, String> {
    let host = url.host_str().unwrap_or_default();
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return Err("网络共享路径的图片".to_string());
    }
    let path = url
        .to_file_path()
        .map_err(|_| "图片路径无法识别".to_string())?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| "图片文件读不到".to_string())?;
    if !metadata.file_type().is_file() {
        return Err("图片路径不是文件".to_string());
    }
    if metadata.len() > max_bytes {
        return Err(too_large(max_bytes));
    }
    let bytes = read_bytes(&path, max_bytes, "图片").map_err(|_| "图片文件读不到".to_string())?;
    Ok(PastedImage {
        media_type: media_type_for(&path).to_string(),
        bytes,
    })
}

/// 体积原因的文案：正式上限按 MB 说，测试里的小上限按字节说。
fn too_large(max_bytes: u64) -> String {
    if max_bytes >= 1024 * 1024 {
        format!("图片超过 {} MB", max_bytes / (1024 * 1024))
    } else {
        format!("图片超过 {max_bytes} 字节")
    }
}

/// 按扩展名给 file: 图片一个 media_type；embedded 靠它认出 SVG / TIFF 这类
/// 二进制嗅探认不出的格式。认不出就留空，交给按字节嗅探。
fn media_type_for(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("bmp") => "image/bmp",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("tif" | "tiff") => "image/tiff",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// PNG 文件头，够 embedded 认出是 PNG。
    const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    /// 八字节签名的 base64 就是 iVBORw0KGgo=。
    const TINY_PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    /// 每个用例一个临时目录，用完不删（系统临时目录自己会清）。
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gongwen-paste-images-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    /// 测试用的最小 base64 编码，免得为几十张图引额外依赖。
    fn base64(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let value = (b0 << 16) | (b1 << 8) | b2;
            out.push(TABLE[(value >> 18) as usize & 63] as char);
            out.push(TABLE[(value >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                TABLE[(value >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                TABLE[value as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    impl ClipboardImages {
        fn failures(&self) -> Vec<String> {
            self.state.borrow().failures.clone()
        }
    }

    #[test]
    fn data_url_png_is_decoded() {
        let images = ClipboardImages::default();
        let image = images.resolve(TINY_PNG).expect("data: 图应能解析");
        assert_eq!(image.media_type, "image/png");
        assert_eq!(image.bytes, PNG_SIGNATURE);
        assert!(images.failures().is_empty());
    }

    #[test]
    fn data_url_of_another_type_is_not_an_image() {
        let images = ClipboardImages::default();
        assert!(images.resolve("data:text/plain;base64,aGk=").is_none());
        assert_eq!(images.failures(), vec!["不是图片格式".to_string()]);
    }

    #[test]
    fn broken_data_url_is_rejected() {
        let images = ClipboardImages::default();
        assert!(
            images
                .resolve("data:image/png;base64,这不是base64")
                .is_none()
        );
        assert_eq!(images.failures(), vec!["图片数据损坏".to_string()]);
    }

    #[test]
    fn network_images_are_not_downloaded() {
        let images = ClipboardImages::default();
        assert!(images.resolve("https://example.com/a.png").is_none());
        assert!(images.resolve("http://example.com/a.png").is_none());
        assert!(images.resolve("ftp://example.com/a.png").is_none());
        assert_eq!(images.failures().len(), 3);
        assert!(
            images
                .failures()
                .iter()
                .all(|reason| reason == "网络图片不下载")
        );
    }

    #[test]
    fn file_url_decodes_percent_encoded_chinese_names() {
        let dir = temp_dir("chinese");
        let path = dir.join("通知 附件.png");
        std::fs::write(&path, PNG_SIGNATURE).expect("写测试图片");
        let src = url::Url::from_file_path(&path)
            .expect("转 file URL")
            .to_string();
        assert!(src.contains('%'), "中文与空格应当被百分号编码：{src}");
        let images = ClipboardImages::default();
        let image = images.resolve(&src).expect("本机文件应能读");
        assert_eq!(image.bytes, PNG_SIGNATURE);
        assert_eq!(image.media_type, "image/png");
    }

    #[test]
    fn file_url_with_a_host_is_a_network_share() {
        let images = ClipboardImages::default();
        assert!(images.resolve("file://server/share/a.png").is_none());
        assert_eq!(images.failures(), vec!["网络共享路径的图片".to_string()]);
    }

    #[test]
    fn localhost_in_a_file_url_is_still_local() {
        let dir = temp_dir("localhost");
        let path = dir.join("a.png");
        std::fs::write(&path, PNG_SIGNATURE).expect("写测试图片");
        let local = url::Url::from_file_path(&path).expect("转 file URL");
        let src = format!("file://localhost{}", local.path());
        let images = ClipboardImages::default();
        assert!(images.resolve(&src).is_some(), "localhost 就是本机：{src}");
    }

    #[test]
    fn directory_and_missing_files_are_reported() {
        let dir = temp_dir("bad-paths");
        let dir_src = url::Url::from_directory_path(&dir)
            .expect("转目录 URL")
            .to_string();
        let missing = url::Url::from_file_path(dir.join("没有这个文件.png"))
            .expect("转 file URL")
            .to_string();
        let images = ClipboardImages::default();
        assert!(images.resolve(&dir_src).is_none());
        assert!(images.resolve(&missing).is_none());
        assert_eq!(
            images.failures(),
            vec!["图片路径不是文件".to_string(), "图片文件读不到".to_string()]
        );
    }

    #[test]
    fn oversized_files_are_rejected() {
        let dir = temp_dir("oversized");
        let path = dir.join("big.png");
        std::fs::write(&path, [b'x'; 64]).expect("写测试图片");
        let src = url::Url::from_file_path(&path)
            .expect("转 file URL")
            .to_string();
        let images = ClipboardImages::with_limits(16, 1024);
        assert!(images.resolve(&src).is_none());
        assert_eq!(images.failures(), vec!["图片超过 16 字节".to_string()]);
    }

    #[test]
    fn the_same_address_is_read_once() {
        let images = ClipboardImages::default();
        assert!(images.resolve(TINY_PNG).is_some());
        assert!(images.resolve(TINY_PNG).is_some());
        assert!(images.failures().is_empty(), "命中缓存不该记失败");
    }

    #[test]
    fn at_most_fifty_images_per_paste() {
        let images = ClipboardImages::default();
        for index in 0..MAX_PASTE_IMAGES {
            let mut bytes = PNG_SIGNATURE.to_vec();
            bytes.push(index as u8);
            let src = format!("data:image/png;base64,{}", base64(&bytes));
            assert!(images.resolve(&src).is_some(), "第 {index} 张应能解析");
        }
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.push(MAX_PASTE_IMAGES as u8);
        let extra = format!("data:image/png;base64,{}", base64(&bytes));
        assert!(images.resolve(&extra).is_none());
        assert_eq!(
            images.failures(),
            vec![format!("一次粘贴的图片超过 {MAX_PASTE_IMAGES} 张")]
        );
    }

    #[test]
    fn generic_reasons_are_replaced_in_order() {
        let images = ClipboardImages::default();
        assert!(images.resolve("https://example.com/a.png").is_none());
        assert!(images.resolve("data:text/plain;base64,aGk=").is_none());
        let generic = format!("<!-- [图片未导入：{MISSING_DATA_REASON}] -->");
        let lines = [
            "文字",
            "",
            generic.as_str(),
            "",
            "更多",
            "",
            generic.as_str(),
            "",
        ];
        let markdown = lines.join(&String::from(char::from(10)));
        let mut report = ImageReport {
            imported: 1,
            skipped: vec![
                MISSING_DATA_REASON.to_string(),
                MISSING_DATA_REASON.to_string(),
            ],
        };
        let out = images.explain(markdown, &mut report);
        assert!(
            out.contains("<!-- [图片未导入：网络图片不下载] -->"),
            "{out}"
        );
        assert!(out.contains("<!-- [图片未导入：不是图片格式] -->"), "{out}");
        assert_eq!(
            report.skipped,
            vec!["网络图片不下载".to_string(), "不是图片格式".to_string()]
        );
    }

    /// 只有 `data:` 与 `file:` 的图算「能入库」；网络图、没有 src 的不算。
    #[test]
    fn importable_images_are_data_or_file_urls() {
        assert!(has_importable_image(
            "<p>见下图<IMG width=10 SRC='data:image/png;base64,AAAA'></p>"
        ));
        assert!(has_importable_image(
            r#"<img src="file:///C:/Temp/msohtmlclip1/01/clip_image001.png">"#
        ));
        assert!(!has_importable_image(
            r#"<p>通知<img src="https://stat.example.com/p.gif" width=1 height=1></p>"#
        ));
        assert!(!has_importable_image("<p>通知</p>"));
    }
}
