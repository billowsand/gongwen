//! Typst 排版引擎：全部 PDF（公文与研究报告）在本进程里排版，不起子进程、不落
//! 中间文件。整体分工：
//!
//! - 版式全部写在随二进制编译进来的模板里：公文 `assets/typst/gongwen.typ`，研究
//!   报告 `assets/typst/research.typ`；
//! - 模板只认一份 JSON（`export::typst` 生成）：编号、标题断行、表格列宽、
//!   压缩比例、要素显示值都由 Rust 算好，模板只负责排；
//! - 这里实现 Typst 的 `World`：主文件是模板，`/doc.json` 是数据，其余相对
//!   路径（插图）按导出目录解析，且不许越出该目录；字体来自随包 `runtime/fonts`
//!   与设置里选的本机字体文件。
//!
//! 字体解析一次后进程内缓存（按文件路径），反复导出不再重读上百兆字体。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Context, Result, bail};
use typst::diag::{FileError, FileResult, Severity, SourceDiagnostic};
use typst::foundations::{Bytes, Datetime, Dict, Label, Selector, Smart, Value};
use typst::introspection::{Introspector, MetadataElem};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World, WorldExt};
use typst_layout::PagedDocument;

use crate::models::{FontConfig, FontRole};

/// 公文模板。改版式只改这一个文件。
pub const TEMPLATE: &str = include_str!("../assets/typst/gongwen.typ");
/// 研究报告模板（对应 mdx 的 `md2tex.cls` + `template.tex`）。
pub const RESEARCH_TEMPLATE: &str = include_str!("../assets/typst/research.typ");

const DATA_PATH: &str = "/doc.json";

/// 排哪一种文档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Template {
    #[default]
    Official,
    Research,
}

impl Template {
    fn main_path(self) -> &'static str {
        match self {
            Self::Official => "/gongwen.typ",
            Self::Research => "/research.typ",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Self::Official => TEMPLATE,
            Self::Research => RESEARCH_TEMPLATE,
        }
    }
}

/// 一次排版的输入。
pub struct TypstJob<'a> {
    /// `export::typst` 生成的文档数据。
    pub data: String,
    /// 插图等相对路径的解析基准（导出目录）。
    pub base_dir: &'a Path,
    pub template: Template,
    /// 只在内存里的文件（虚拟绝对路径 → 内容），如研究报告的公式 SVG。
    pub files: HashMap<String, Vec<u8>>,
}

impl<'a> TypstJob<'a> {
    /// 公文：模板数据 + 插图目录。
    pub fn official(data: String, base_dir: &'a Path) -> Self {
        Self {
            data,
            base_dir,
            template: Template::Official,
            files: HashMap::new(),
        }
    }
}

/// 一次排版的产物。
#[derive(Debug, Default)]
pub struct TypstOutcome {
    pub pdf: Vec<u8>,
    /// 孤行探针报告，格式与 `.gwaproof` 相同（见 `orphan_probe`），没开探针时为 `None`。
    pub proof: Option<String>,
    /// Typst 的警告（排版不收敛之类），不阻断导出，交给调用方决定是否提示。
    pub warnings: Vec<String>,
}

/// 字体文件解析结果的进程级缓存：路径 → 文件里的全部字面（TTC 会有多个）。
static FONT_CACHE: LazyLock<Mutex<HashMap<PathBuf, Arc<Vec<Font>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn load_font_file(path: &Path) -> Result<Arc<Vec<Font>>> {
    let mut cache = FONT_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(fonts) = cache.get(path) {
        return Ok(fonts.clone());
    }
    let bytes =
        std::fs::read(path).with_context(|| format!("无法读取字体文件：{}", path.display()))?;
    let fonts: Vec<Font> = Font::iter(Bytes::new(bytes)).collect();
    if fonts.is_empty() {
        bail!("字体文件无法解析：{}", path.display());
    }
    let fonts = Arc::new(fonts);
    cache.insert(path.to_path_buf(), fonts.clone());
    Ok(fonts)
}

/// 模板里各字体角色实际使用的家族名（按 Typst 从字体文件里读到的名字）。
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct FontFamilies {
    pub title: String,
    pub heading1: String,
    pub heading2: String,
    pub body: String,
    pub page_number: String,
    /// 选了「专用粗体字体」时的家族名；`None` 表示用描边伪粗。
    pub bold: Option<String>,
    /// 页码数字的拉丁子集（宋体字面），排在页码字体之前接管 ASCII；页码另选了
    /// 本机字体时不挂（`None`）。正文、楷体的国标西文字面在合成字体里，不用子集。
    pub page_number_latin: Option<String>,
    /// 缺字兜底链，按顺序查找（用户选的在前，内置宋体垫底）。
    pub fallback: Vec<String>,
}

/// 本次排版要装进 World 的全部字体，以及各角色对应的家族名。
pub struct FontSet {
    fonts: Vec<Font>,
    pub families: FontFamilies,
}

/// 随包字体目录。发布包在可执行文件旁的 `runtime/fonts`，开发构建另查仓库根目录。
fn bundled_font_dir() -> Result<PathBuf> {
    crate::portable_runtime::find_font_dir().context("找不到内置字体目录 runtime/fonts")
}

/// 随包字体文件的家族名（按 Typst 从文件里读到的名字）。研究报告的字体全部钉死在
/// 随包文件上，不受设置页的本机字体影响。
pub fn bundled_family(file: &str) -> Result<String> {
    let fonts = load_font_file(&bundled_font_dir()?.join(file))?;
    Ok(fonts[0].info().family.clone())
}

/// 装载随包字体与设置里生效的本机字体，并定出各角色的家族名。
pub fn font_set(fonts: &FontConfig) -> Result<FontSet> {
    let dir = bundled_font_dir()?;
    let mut all: Vec<Font> = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut family_of = |path: &Path, all: &mut Vec<Font>| -> Result<String> {
        let loaded = load_font_file(path)?;
        if !seen.iter().any(|p| p == path) {
            seen.push(path.to_path_buf());
            all.extend(loaded.iter().cloned());
        }
        Ok(loaded[0].info().family.clone())
    };
    // 先装全部随包字体：兜底链与页码里的破折号都可能落到它们上。
    let mut bundled = std::collections::BTreeMap::new();
    for name in crate::portable_runtime::font_files() {
        let path = dir.join(name);
        if path.is_file() {
            let family = family_of(&path, &mut all)?;
            bundled.insert(name.to_string(), family);
        }
    }
    let mut role = |role: FontRole, all: &mut Vec<Font>| -> Result<String> {
        if let Some(choice) = fonts.active(role) {
            return family_of(Path::new(choice.path.trim()), all);
        }
        let file = role.bundled_file();
        bundled
            .get(file)
            .cloned()
            .with_context(|| format!("内置字体缺失：{file}"))
    };
    let title = role(FontRole::Title, &mut all)?;
    let heading1 = role(FontRole::Heading1, &mut all)?;
    let heading2 = role(FontRole::Heading2, &mut all)?;
    let body = role(FontRole::Body, &mut all)?;
    let page_number = role(FontRole::PageNumber, &mut all)?;
    let bold = if fonts.uses_dedicated_bold_font() {
        Some(role(FontRole::Bold, &mut all)?)
    } else {
        None
    };
    let mut fallback = Vec::new();
    if let Some(choice) = fonts.active(FontRole::Fallback) {
        fallback.push(family_of(Path::new(choice.path.trim()), &mut all)?);
    }
    let bundled_fallback = bundled
        .get(FontRole::Fallback.bundled_file())
        .cloned()
        .context("内置兜底字体缺失")?;
    if !fallback.contains(&bundled_fallback) {
        fallback.push(bundled_fallback);
    }
    // 页码的拉丁子集只在页码用内置字体时挂上：用户另选了本机字体，数字就用那支
    // 字体自己的字面，不再叠国标字面。
    let page_number_latin = if fonts.active(FontRole::PageNumber).is_some() {
        None
    } else {
        let file = crate::portable_runtime::SIMSUN_LATIN_SUBSET_FILE;
        Some(
            bundled
                .get(file)
                .cloned()
                .with_context(|| format!("拉丁子集字体缺失：{file}"))?,
        )
    };
    Ok(FontSet {
        fonts: all,
        families: FontFamilies {
            title,
            heading1,
            heading2,
            body,
            page_number,
            bold,
            page_number_latin,
            fallback,
        },
    })
}

struct GongwenWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: FileId,
    main_source: Source,
    data_id: FileId,
    data: Bytes,
    base_dir: PathBuf,
    template: Template,
    files: HashMap<FileId, Bytes>,
}

fn file_id(path: &str) -> FileId {
    let vpath = VirtualPath::new(path).expect("内置虚拟路径必须合法");
    RootedPath::new(VirtualRoot::Project, vpath).intern()
}

impl GongwenWorld {
    #[cfg(test)]
    fn new(set: &FontSet, data: String, base_dir: &Path) -> Self {
        Self::with_template(set, data, base_dir, Template::Official, &HashMap::new())
    }

    fn with_template(
        set: &FontSet,
        data: String,
        base_dir: &Path,
        template: Template,
        files: &HashMap<String, Vec<u8>>,
    ) -> Self {
        let main = file_id(template.main_path());
        let data_id = file_id(DATA_PATH);
        Self {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(FontBook::from_fonts(&set.fonts)),
            fonts: set.fonts.clone(),
            main,
            main_source: Source::new(main, template.source().to_string()),
            data_id,
            data: Bytes::new(data.into_bytes()),
            base_dir: base_dir.to_path_buf(),
            template,
            files: files
                .iter()
                .map(|(path, bytes)| (file_id(path), Bytes::new(bytes.clone())))
                .collect(),
        }
    }

    /// 相对导出目录解析插图路径；拒绝 `..` 与绝对路径（与 `images::resolve` 同口径）。
    fn resolve(&self, id: FileId) -> FileResult<PathBuf> {
        let rel = id.get().vpath().get_without_slash();
        let rel_path = Path::new(rel);
        if rel_path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(FileError::AccessDenied);
        }
        Ok(self.base_dir.join(rel_path))
    }
}

impl World for GongwenWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main {
            return Ok(self.main_source.clone());
        }
        let path = self.resolve(id)?;
        let text = std::fs::read_to_string(&path).map_err(|e| FileError::from_io(e, &path))?;
        Ok(Source::new(id, text))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.data_id {
            return Ok(self.data.clone());
        }
        if id == self.main {
            return Ok(Bytes::new(self.template.source().as_bytes().to_vec()));
        }
        if let Some(bytes) = self.files.get(&id) {
            return Ok(bytes.clone());
        }
        let path = self.resolve(id)?;
        std::fs::read(&path)
            .map(Bytes::new)
            .map_err(|e| FileError::from_io(e, &path))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<typst::foundations::Duration>) -> Option<Datetime> {
        None
    }
}

/// 把诊断翻成一行人话：消息 + 出错位置（模板行号）+ 提示。
fn describe(world: &GongwenWorld, diag: &SourceDiagnostic) -> String {
    let mut text = diag.message.to_string();
    if let Some(id) = diag.span.id()
        && let Some(range) = world.range(diag.span)
        && let Ok(source) = world.source(id)
    {
        let line = source
            .lines()
            .byte_to_line(range.start)
            .map_or(0, |l| l + 1);
        text.push_str(&format!(
            "（{} 第 {line} 行）",
            id.get().vpath().get_with_slash()
        ));
    }
    for hint in &diag.hints {
        text.push_str(&format!("；提示：{}", hint.v));
    }
    text
}

/// 排版并导出 PDF。字体由调用方先用 [`font_set`] 装好：模板数据里要写各角色的
/// 家族名，两边必须是同一份。
pub fn compile(job: &TypstJob, set: &FontSet) -> Result<TypstOutcome> {
    let world = GongwenWorld::with_template(
        set,
        job.data.clone(),
        job.base_dir,
        job.template,
        &job.files,
    );
    let (document, warnings) = layout(&world)?;
    let proof = proof_report(&document);
    let options = typst_pdf::PdfOptions {
        ident: Smart::Auto,
        creator: Smart::Custom(Some("公文助手".to_string())),
        ..Default::default()
    };
    let pdf = typst_pdf::pdf(&document, &options).map_err(|errors| {
        let message = errors
            .iter()
            .map(|d| describe(&world, d))
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::anyhow!("Typst 导出 PDF 失败：{message}")
    })?;
    let pdf = rotate_landscape_pages(pdf)?;
    Ok(TypstOutcome {
        pdf,
        proof,
        warnings,
    })
}

/// 使用导出时的完整排版结果数物理页；不生成 PDF，也不写输出文件。
pub(crate) fn page_count(job: &TypstJob, set: &FontSet) -> Result<usize> {
    let world = GongwenWorld::with_template(
        set,
        job.data.clone(),
        job.base_dir,
        job.template,
        &job.files,
    );
    Ok(layout(&world)?.0.pages().len())
}

fn layout(world: &GongwenWorld) -> Result<(PagedDocument, Vec<String>)> {
    let warned = typst::compile::<PagedDocument>(world);
    let warnings: Vec<String> = warned
        .warnings
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| describe(world, d))
        .collect();
    let document = match warned.output {
        Ok(document) => document,
        Err(errors) => {
            let message = errors
                .iter()
                .map(|d| describe(world, d))
                .collect::<Vec<_>>()
                .join("\n");
            bail!("Typst 排版失败：{message}");
        }
    };
    Ok((document, warnings))
}

/// 横向附件页改成与 TeX（pdflscape）同一种页面结构：纸张仍是竖向 A4、版面内容逆时针
/// 转 90°，再标 `/Rotate 90` 让阅读器横着显示。Typst 直接输出横向纸张，屏幕上看一样，
/// 但双面打印时打印机按纸张方向自动旋转，朝哪边转因机型而异，装订边可能落错；与 TeX
/// 产物结构一致才能保证打出来一样。没有横页时原样返回，不重写 PDF。
fn rotate_landscape_pages(pdf: Vec<u8>) -> Result<Vec<u8>> {
    use lopdf::{Dictionary, Document, Object, Stream};

    let mut doc = Document::load_mem(&pdf).context("无法读取 Typst 输出的 PDF")?;
    let landscape: Vec<(lopdf::ObjectId, f32, f32)> = doc
        .get_pages()
        .values()
        .filter_map(|&id| {
            let page = doc.get_dictionary(id).ok()?;
            let rect = page.get(b"MediaBox").ok()?.as_array().ok()?;
            let num = |i: usize| rect.get(i).and_then(|v| v.as_float().ok());
            let (w, h) = (num(2)? - num(0)?, num(3)? - num(1)?);
            (w > h).then_some((id, w, h))
        })
        .collect();
    if landscape.is_empty() {
        return Ok(pdf);
    }
    for (id, w, h) in landscape {
        let contents = doc.get_page_contents(id);
        // 横向内容 (x, y) → 竖向纸张 (h − y, x)：逆时针转 90° 再右移一个横页高度。
        let prefix = doc.add_object(Stream::new(
            Dictionary::new(),
            format!("q 0 1 -1 0 {h} 0 cm\n").into_bytes(),
        ));
        let suffix = doc.add_object(Stream::new(Dictionary::new(), b"\nQ".to_vec()));
        let mut parts = vec![Object::Reference(prefix)];
        parts.extend(contents.into_iter().map(Object::Reference));
        parts.push(Object::Reference(suffix));
        let page = doc.get_dictionary_mut(id).context("PDF 页面对象损坏")?;
        page.set("Contents", Object::Array(parts));
        let portrait = vec![0.into(), 0.into(), Object::Real(h), Object::Real(w)];
        page.set("MediaBox", Object::Array(portrait.clone()));
        if page.has(b"CropBox") {
            page.set("CropBox", Object::Array(portrait));
        }
        page.set("Rotate", Object::Integer(90));
    }
    let mut out = Vec::with_capacity(pdf.len());
    doc.save_to(&mut out).context("无法写回横页调整后的 PDF")?;
    Ok(out)
}

/// 1pt（PostScript 点）合多少 TeX sp：探针报告沿用 `.gwaproof` 的 sp 单位。
const SP_PER_PT: f64 = 65536.0 * 72.27 / 72.0;

fn dict_f64(dict: &Dict, key: &str) -> Option<f64> {
    match dict.get(key).ok()? {
        Value::Float(v) => Some(*v),
        Value::Int(v) => Some(*v as f64),
        _ => None,
    }
}

/// 收集模板写下的段首（`gwa-head`）、段尾（`gwa-tail`）标记，拼成与 TeX
/// `\GwaTail` 同格式的探针报告，交给 `orphan_probe` 解析。
///
/// 标记的值由模板在 `context` 里取：行号、页码、所在位置（pt，页面坐标）、
/// 行距与版心参数。行数按段首、段尾两个位置推算：同页时是基线差除以行距加一，
/// 跨页时两页各算一段相加。
fn proof_report(document: &PagedDocument) -> Option<String> {
    let introspector = document.introspector();
    let collect = |name: &str| -> Vec<Dict> {
        introspector
            .query(&Selector::Label(
                Label::new(typst::utils::PicoStr::intern(name)).expect("标签名合法"),
            ))
            .iter()
            .filter_map(|content| content.to_packed::<MetadataElem>())
            .filter_map(|meta| match &meta.value {
                Value::Dict(dict) => Some(dict.clone()),
                _ => None,
            })
            .collect()
    };
    let heads = collect("gwa-head");
    let tails = collect("gwa-tail");
    if tails.is_empty() {
        return None;
    }
    let mut out = String::new();
    for tail in &tails {
        let (Some(line), Some(page), Some(x), Some(y), Some(char_w), Some(hsize), Some(left)) = (
            dict_f64(tail, "line"),
            dict_f64(tail, "page"),
            dict_f64(tail, "x"),
            dict_f64(tail, "y"),
            dict_f64(tail, "char"),
            dict_f64(tail, "hsize"),
            dict_f64(tail, "left"),
        ) else {
            continue;
        };
        let pitch = dict_f64(tail, "pitch").unwrap_or(28.87);
        let lines = heads
            .iter()
            .find(|head| dict_f64(head, "line") == Some(line))
            .and_then(|head| {
                let head_page = dict_f64(head, "page")?;
                let head_y = dict_f64(head, "y")?;
                if head_page == page {
                    Some(((y - head_y) / pitch).round() as i64 + 1)
                } else {
                    // 跨页：上页从段首到版心底，下页从版心顶到段尾。
                    let bottom = dict_f64(head, "bottom")?;
                    let top = dict_f64(tail, "top")?;
                    Some(
                        ((bottom - head_y) / pitch).floor() as i64
                            + ((y - top) / pitch).round() as i64
                            + 1,
                    )
                }
            })
            .unwrap_or(1)
            .max(1);
        let sp = |pt: f64| (pt * SP_PER_PT).round() as i64;
        out.push_str(&format!(
            "lines {line} {lines}\ntail {line} {page} {} {} {} {} {}\n",
            sp(x),
            sp(char_w),
            sp(hsize),
            sp(left),
            sp(left),
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_paths_escaping_the_export_dir() {
        let set = FontSet {
            fonts: Vec::new(),
            families: FontFamilies {
                title: String::new(),
                heading1: String::new(),
                heading2: String::new(),
                body: String::new(),
                page_number: String::new(),
                bold: None,
                page_number_latin: None,
                fallback: Vec::new(),
            },
        };
        let world = GongwenWorld::new(&set, "{}".into(), Path::new("."));
        // Typst 自己的虚拟路径就不收 `..`；resolve 那道检查是再兜一层底。
        assert!(VirtualPath::new("/../secret.png").is_err());
        let inside = file_id("/images/a.png");
        assert_eq!(
            world.resolve(inside).unwrap(),
            Path::new(".").join("images/a.png")
        );
    }
}

/// 测试用：排版后按出现顺序列出每段文字与它实际落到的字体（家族名、字重）。
/// Typst 逐字回退时一段文字按字体切开，查字体配方就看这个。
#[cfg(test)]
pub(crate) fn text_fonts_for_test(job: &TypstJob, set: &FontSet) -> Result<Vec<TextFont>> {
    fn walk(frame: &typst::layout::Frame, page: usize, out: &mut Vec<TextFont>) {
        for (_, item) in frame.items() {
            match item {
                typst::layout::FrameItem::Group(group) => walk(&group.frame, page, out),
                typst::layout::FrameItem::Text(text) => {
                    let info = text.font.font().info();
                    out.push(TextFont {
                        text: text.text.to_string(),
                        family: info.family.clone(),
                        weight: info.variant.weight.to_number(),
                        page,
                        missing_glyphs: text.glyphs.iter().filter(|glyph| glyph.id == 0).count(),
                    });
                }
                _ => {}
            }
        }
    }
    let world = GongwenWorld::with_template(
        set,
        job.data.clone(),
        job.base_dir,
        job.template,
        &job.files,
    );
    let (document, _) = layout(&world)?;
    let mut out = Vec::new();
    for (i, page) in document.pages().iter().enumerate() {
        walk(&page.frame, i + 1, &mut out);
    }
    Ok(out)
}

/// 见 [`text_fonts_for_test`]。
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct TextFont {
    pub text: String,
    pub family: String,
    pub weight: u16,
    pub page: usize,
    pub missing_glyphs: usize,
}

/// 测试用：用自定义源码代替模板排版（字体同随包），返回 PDF。
#[cfg(test)]
pub(crate) fn compile_source_for_test(source: &str) -> Result<Vec<u8>> {
    let set = font_set(&FontConfig::default())?;
    let mut world = GongwenWorld::new(&set, "{}".into(), Path::new("."));
    world.main_source = Source::new(world.main, source.to_string());
    let document = typst::compile::<PagedDocument>(&world)
        .output
        .map_err(|e| {
            anyhow::anyhow!(
                "{:?}",
                e.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
            )
        })?;
    typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default()).map_err(|_| anyhow::anyhow!("pdf"))
}

#[cfg(test)]
mod glyph_probe {
    /// 与 tmp/typst-calib 的 TeX 标定对照：标点被硬挤时各压多少。
    #[test]
    #[ignore]
    fn calibrate_punct_shrink() {
        let mut src = String::from("#set page(width: 300mm, height: auto, margin: 5mm)
#set text(font: \"GW FangSong\", size: 16pt, lang: \"zh\", region: \"cn\", overhang: false, cjk-latin-spacing: none)
#set par(justify: true)
");
        for p in "，。、；：！？）」』》（「『《“”".chars() {
            let body: String = (0..8).map(|_| format!("字{p}字")).collect();
            let n = body.chars().count();
            src.push_str(&format!(
                "#block(width: {}em, par(justify: true, [{body}#linebreak(justify: true)]))
",
                n - 4
            ));
        }
        let pdf = super::compile_source_for_test(&src).unwrap();
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tmp/typst-calib/typst.pdf"),
            pdf,
        )
        .unwrap();
    }

    #[test]
    #[ignore]
    fn print_punct_bounds() {
        let dir = crate::portable_runtime::find_font_dir().unwrap();
        for file in ["GWFangSong.ttf", "GWKai.ttf", "FZHei.ttf"] {
            let bytes = std::fs::read(dir.join(file)).unwrap();
            let face = ttf_parser::Face::parse(&bytes, 0).unwrap();
            let upem = face.units_per_em() as f32;
            for ch in "，。、；：！？）」』》（「『《“”".chars() {
                let id = face.glyph_index(ch).unwrap();
                let adv = face.glyph_hor_advance(id).unwrap() as f32 / upem;
                let b = face.glyph_bounding_box(id).unwrap();
                eprintln!(
                    "{file} {ch} 宽 {adv:.3} 墨迹 {:.3}..{:.3} 左空 {:.3} 右空 {:.3}",
                    b.x_min as f32 / upem,
                    b.x_max as f32 / upem,
                    b.x_min as f32 / upem,
                    adv - b.x_max as f32 / upem
                );
            }
        }
    }
}
