//! 「Typst 包」导出：模板、模板数据与全部资源打成 zip，解压后可用 typst CLI 编译。
//!
//! 包内不带字体。`doc.json` 的 `fonts` 换成固定的系统字体别名链：应用内的合成字体
//! 族名（`GW FangSong` 等）与拉丁子集只存在于本机，带出去没人认得。模板的字体角色
//! 因此同时接受字符串与数组——应用内仍传字符串，包内传别名数组，逐名回退。
//!
//! 出包只准备数据，不调用 `typst_engine::compile()`：本机缺字体不影响出包，编译由
//! 拿到包的人在装有常见中文字体的机器上完成。

use std::collections::HashSet;
use std::fs;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::export::research::ResearchSourceBundle;
use crate::models::{DraftInput, NumberingConfig};
use crate::typst_engine;
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;

/// 宋体：公文页码、研究报告正文与两者的兜底共用。
const SONG_CHAIN: [&str; 4] = ["FZShuSong-Z01", "方正书宋_GBK", "SimSun", "宋体"];
/// 仿宋：公文正文。
const BODY_CHAIN: [&str; 6] = [
    "FangSong",
    "仿宋",
    "仿宋_GB2312",
    "FangSong_GB2312",
    "FZFangSong-Z02",
    "方正仿宋_GBK",
];
/// 楷体：公文二级标题、研究报告楷体。
const KAI_CHAIN: [&str; 6] = [
    "KaiTi",
    "楷体",
    "楷体_GB2312",
    "KaiTi_GB2312",
    "FZKai-Z03",
    "方正楷体_GBK",
];
/// 黑体：公文一级标题、研究报告黑体。
const HEI_CHAIN: [&str; 4] = ["FZHei-B01", "方正黑体_GBK", "SimHei", "黑体"];
/// 小标宋：公文大标题、研究报告封面大标题。
const TITLE_CHAIN: [&str; 4] = [
    "FZXiaoBiaoSong-B05",
    "方正小标宋_GBK",
    "FZXiaoBiaoSong-B05S",
    "方正小标宋简体",
];
/// 研究报告的西文字体。
const LATIN_CHAIN: [&str; 2] = ["TeX Gyre Termes", "Times New Roman"];
/// 研究报告的等宽字体。
const MONO_CHAIN: [&str; 3] = ["JetBrains Mono", "Consolas", "Courier New"];

fn chain(names: &[&str]) -> Value {
    Value::Array(
        names
            .iter()
            .map(|name| Value::String((*name).to_string()))
            .collect(),
    )
}

/// 公文模板的便携字体表。角色名与 `typst_engine::FontFamilies` 的字段一一对应。
pub(crate) fn portable_official_fonts() -> Value {
    json!({
        "title": chain(&TITLE_CHAIN),
        "heading1": chain(&HEI_CHAIN),
        "heading2": chain(&KAI_CHAIN),
        "body": chain(&BODY_CHAIN),
        "page_number": chain(&SONG_CHAIN),
        "bold": null,
        "page_number_latin": null,
        "fallback": chain(&SONG_CHAIN),
    })
}

/// 研究报告模板的便携字体表。
pub(crate) fn portable_research_fonts() -> Value {
    json!({
        "song": chain(&SONG_CHAIN),
        "hei": chain(&HEI_CHAIN),
        "kai": chain(&KAI_CHAIN),
        "xbs": chain(&TITLE_CHAIN),
        "latin": chain(&LATIN_CHAIN),
        "mono": chain(&MONO_CHAIN),
        "fallback": chain(&SONG_CHAIN),
    })
}

/// 只准备数据时给模板的占位字体角色。值会在写包前被便携表整体覆盖。
pub(crate) fn placeholder_fonts() -> typst_engine::FontFamilies {
    typst_engine::FontFamilies {
        title: String::new(),
        heading1: String::new(),
        heading2: String::new(),
        body: String::new(),
        page_number: String::new(),
        bold: None,
        page_number_latin: None,
        fallback: Vec::new(),
    }
}

/// 把模板数据里的 `fonts` 换成便携表，其余字段原样保留。
pub(crate) fn rewrite_fonts(data: &str, fonts: Value) -> Result<String> {
    let mut value: Value = serde_json::from_str(data).context("无法解析 Typst 文档数据")?;
    value["fonts"] = fonts;
    serde_json::to_string(&value).context("无法序列化 Typst 文档数据")
}

/// 一包要写进 zip 的全部内容。
struct Prepared {
    template_name: &'static str,
    template: &'static str,
    data: String,
    resources: Vec<(String, Vec<u8>)>,
    warnings: Vec<String>,
}

/// 导出 Typst 包，插图按用户配置目录解析。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_package(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
) -> Result<Vec<String>> {
    write_package_with_base(
        path,
        input,
        markdown,
        display,
        numbering,
        elements,
        &crate::storage::config_dir()?,
    )
}

/// 同上，插图与参考文献按 `base_dir` 解析（测试用）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_package_with_base(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
    base_dir: &Path,
) -> Result<Vec<String>> {
    let mut prepared = if input.kind.is_research() {
        prepare_research(input, markdown, numbering, base_dir)?
    } else {
        prepare_official(input, markdown, display, numbering, elements, base_dir)?
    };
    prepared.resources.sort_by(|a, b| a.0.cmp(&b.0));
    prepared.resources.dedup_by(|a, b| a.0 == b.0);

    // 写完 ZIP 中央目录后再替换目标，写入失败时保留上一次可用的包。
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("无法创建 Typst 包临时文件：{}", path.display()))?;
    let mut zip = zip::ZipWriter::new(file.as_file_mut());
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    zip.start_file(prepared.template_name, options)?;
    zip.write_all(prepared.template.as_bytes())?;
    zip.start_file("doc.json", options)?;
    zip.write_all(prepared.data.as_bytes())?;
    let readme = readme(
        prepared.template_name,
        input.kind.is_research(),
        input.kind.is_research().then_some(&prepared.resources),
    );
    zip.start_file("README.txt", options)?;
    zip.write_all(readme.as_bytes())?;
    for (name, bytes) in &prepared.resources {
        zip.start_file(name, options)?;
        zip.write_all(bytes)?;
    }
    zip.finish()
        .with_context(|| format!("无法写完 Typst 包：{}", path.display()))?;
    file.persist(path)
        .with_context(|| format!("无法保存 Typst 包：{}", path.display()))?;
    Ok(prepared.warnings)
}

/// 公文：复用 PDF 导出的数据准备，只把字体表换掉。
#[allow(clippy::too_many_arguments)]
fn prepare_official(
    input: &DraftInput,
    markdown: &str,
    display: &UnitDisplay,
    numbering: &NumberingConfig,
    elements: &ElementMarks,
    base_dir: &Path,
) -> Result<Prepared> {
    let (rendered, data, missing) =
        super::prepare_package(input, markdown, display, numbering, elements, base_dir)?;
    let data = rewrite_fonts(&data, portable_official_fonts())?;
    let mut resources = Vec::new();
    let mut seen = HashSet::new();
    for src in crate::images::image_refs(&rendered) {
        let Ok(source) = crate::images::resolve_from_base(base_dir, &src) else {
            continue;
        };
        if !source.is_file() {
            continue;
        }
        let name = crate::images::normalize_ref(&src);
        if !seen.insert(name.clone()) {
            continue;
        }
        let bytes =
            fs::read(&source).with_context(|| format!("无法读取图片 {}", source.display()))?;
        resources.push((name, bytes));
    }
    let warnings = missing
        .into_iter()
        .map(|src| format!("插图文件不存在，已略过：{src}"))
        .collect();
    Ok(Prepared {
        template_name: "gongwen.typ",
        template: typst_engine::TEMPLATE,
        data,
        resources,
        warnings,
    })
}

/// 研究报告：临时目录里已经有插图与参考文献，整份收进包；公式 SVG 只在内存里。
fn prepare_research(
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
    base_dir: &Path,
) -> Result<Prepared> {
    let bundle = ResearchSourceBundle::create(
        input,
        markdown,
        numbering,
        crate::mermaid::Format::Pdf,
        base_dir,
    )?;
    let (data, files, warnings) = super::research::document_with_fonts(
        &bundle.markdown,
        input,
        numbering,
        markdown,
        portable_research_fonts(),
    )?;
    let mut resources = Vec::new();
    collect_directory(
        bundle.root(),
        bundle.root(),
        &bundle.markdown,
        &mut resources,
    )?;
    for (virtual_path, bytes) in files {
        resources.push((virtual_path.trim_start_matches('/').to_string(), bytes));
    }
    Ok(Prepared {
        template_name: "research.typ",
        template: typst_engine::RESEARCH_TEMPLATE,
        data,
        resources,
        warnings,
    })
}

/// 递归收临时目录里的文件，跳过交给 mdx 的源 Markdown（编译只需要模板与资源）。
fn collect_directory(
    root: &Path,
    dir: &Path,
    skip: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("无法读取研究报告资源目录 {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path == skip {
            continue;
        }
        if path.is_dir() {
            collect_directory(root, &path, skip, out)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("无法计算资源相对路径：{}", path.display()))?;
        let name = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = fs::read(&path).with_context(|| format!("无法读取资源 {}", path.display()))?;
        out.push((name, bytes));
    }
    Ok(())
}

/// 包内说明：编译命令、字体清单、与 App 内 PDF 的差异。
fn readme(template_name: &str, research: bool, resources: Option<&[(String, Vec<u8>)]>) -> String {
    let mut text = String::from(
        "公文助手 —— Typst 包\n\
         ====================\n\n\
         本包内含排版所需的模板、数据与资源，不含字体。解压后在本目录执行：\n\n",
    );
    text.push_str(&format!("    typst compile {template_name}\n\n"));
    text.push_str(
        "环境要求\n\
         --------\n\
         - Typst 0.15.1（本应用内置排版引擎的版本）。\n\
         - 系统需装有下列中文字体，每个角色装别名链中的任意一个即可：\n\n",
    );
    let roles: &[(&str, &str)] = if research {
        &[
            (
                "正文宋体与兜底",
                "FZShuSong-Z01 / 方正书宋_GBK / SimSun / 宋体",
            ),
            ("黑体", "FZHei-B01 / 方正黑体_GBK / SimHei / 黑体"),
            (
                "楷体",
                "KaiTi / 楷体 / 楷体_GB2312 / KaiTi_GB2312 / FZKai-Z03 / 方正楷体_GBK",
            ),
            (
                "小标宋",
                "FZXiaoBiaoSong-B05 / 方正小标宋_GBK / FZXiaoBiaoSong-B05S / 方正小标宋简体",
            ),
        ]
    } else {
        &[
            (
                "正文仿宋",
                "FangSong / 仿宋 / 仿宋_GB2312 / FangSong_GB2312 / FZFangSong-Z02 / 方正仿宋_GBK",
            ),
            (
                "二级标题楷体",
                "KaiTi / 楷体 / 楷体_GB2312 / KaiTi_GB2312 / FZKai-Z03 / 方正楷体_GBK",
            ),
            ("一级标题黑体", "FZHei-B01 / 方正黑体_GBK / SimHei / 黑体"),
            (
                "大标题小标宋",
                "FZXiaoBiaoSong-B05 / 方正小标宋_GBK / FZXiaoBiaoSong-B05S / 方正小标宋简体",
            ),
            ("页码宋体", "FZShuSong-Z01 / 方正书宋_GBK / SimSun / 宋体"),
        ]
    };
    for (role, names) in roles {
        text.push_str(&format!("  * {role}：{names}\n"));
    }
    if research {
        text.push_str("  * 西文：TeX Gyre Termes / Times New Roman\n");
        text.push_str("  * 等宽：JetBrains Mono / Consolas / Courier New\n");
    }
    text.push_str(
        "\n编译时若出现 “unknown font family” 警告，说明该别名在本机不存在；每个角色只要\n\
         命中别名链中的一个即可，未命中的别名可以忽略。\n",
    );
    text.push_str(
        "\n字体缺失时\n\
         ----------\n\
         若本机没有上述字体，可用 --font-path 指向任意含这些字体的目录，例如：\n\n\
             typst compile --font-path \"<字体目录>\" ",
    );
    text.push_str(template_name);
    text.push_str(
        "\n\n公文助手安装目录下的 runtime\\fonts 自带完整字体，但本包按设计不携带字体。\n\n",
    );
    text.push_str(
        "与 App 内 PDF 的差异\n\
         --------------------\n\
         - App 内 PDF 使用随包字体（合成仿宋、合成楷体等）；本包改用本机系统字体，\n\
           字形与字宽可能有细微差异。\n\
         - 官方 typst CLI 没有本项目 vendored typst-layout 的标点收缩补丁，断行、\n\
           分页可能与 App 内 PDF 略有不同。\n\
         - 换用其他 Typst 版本编译也可能出现版式差异；以 App 内 PDF 为准。\n",
    );
    if let Some(resources) = resources {
        let figures = resources
            .iter()
            .filter(|(name, _)| name.starts_with("images/") || name.starts_with("mermaid-cache/"))
            .count();
        let formulas = resources
            .iter()
            .filter(|(name, _)| name.starts_with("math/"))
            .count();
        text.push_str(&format!(
            "\n本包资源：插图 {figures} 个，公式 {formulas} 个。\n"
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;
    use crate::models::{DraftInput, NumberingConfig, TemplateKind};
    use crate::typst_engine::{Template, TypstJob};

    fn zip_entries(path: &Path) -> Vec<String> {
        let file = fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect()
    }

    fn read_entry(path: &Path, name: &str) -> Vec<u8> {
        let file = fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name(name).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        bytes
    }

    /// 解压到临时目录，模拟拿到包的人在同一目录里编译。
    fn extract(path: &Path, dir: &Path) {
        let file = fs::File::open(path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let target = dir.join(entry.name());
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            fs::write(target, bytes).unwrap();
        }
    }

    fn official_input() -> DraftInput {
        let mut input = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..Default::default()
        };
        let profile = &mut input.profile;
        profile.issuing_unit = "某某市民政局".into();
        profile.department_code = "某民函".into();
        profile.document_year = "2026".into();
        profile.document_number = "18".into();
        profile.recipient = "各区民政局".into();
        profile.responsible_unit = "养老服务处".into();
        profile.contact_person = "张三".into();
        profile.contact_phone = "010-12345678".into();
        input.date = "2026年9月30日".into();
        input
    }

    fn research_input() -> DraftInput {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "公共数据授权运营机制研究".into(),
            ..Default::default()
        };
        input.research.institution = "某某市数据研究中心".into();
        input.research.date = "2026年9月".into();
        input.research.bibliography_content =
            "@book{demo, title={示例文献}, author={张三}, year={2026}}\n".into();
        input
    }

    /// 公文包：条目齐全、字体换成别名数组，模板能接受数组并排出 PDF。
    #[test]
    fn official_package_carries_template_data_images_and_readme() {
        let base = tempfile::tempdir().unwrap();
        fs::create_dir_all(base.path().join("images")).unwrap();
        image::RgbImage::from_pixel(40, 20, image::Rgb([31, 78, 158]))
            .save(base.path().join("images/chart.png"))
            .unwrap();
        let markdown = "# 关于推进社区养老服务设施改造工作的通知\n\n\
                        <!-- [正文] -->\n\n\
                        ## 工作目标\n\n\
                        今年完成全部站点改造，**共排查站点312个**。\n\n\
                        | 区域 | 站点数 |\n| --- | ---: |\n| 东城区 | 120 |\n\n\
                        ![改造图](images/chart.png)\n";
        let path = base.path().join("pkg.zip");
        let warnings = write_package_with_base(
            &path,
            &official_input(),
            markdown,
            &UnitDisplay::new(&[]),
            &NumberingConfig::default(),
            &ElementMarks::default(),
            base.path(),
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");

        let entries = zip_entries(&path);
        for expected in ["gongwen.typ", "doc.json", "README.txt", "images/chart.png"] {
            assert!(
                entries.iter().any(|name| name == expected),
                "缺少 {expected}：{entries:?}"
            );
        }

        let data = String::from_utf8(read_entry(&path, "doc.json")).unwrap();
        let value: Value = serde_json::from_str(&data).unwrap();
        let fonts = &value["fonts"];
        assert!(fonts["body"].is_array(), "{fonts}");
        assert!(fonts["fallback"].is_array(), "{fonts}");
        assert!(fonts["bold"].is_null(), "{fonts}");
        assert!(fonts["page_number_latin"].is_null(), "{fonts}");
        let fonts_text = fonts.to_string();
        assert!(
            !fonts_text.contains("GW "),
            "便携表不能带合成字体名：{fonts_text}"
        );
        assert!(
            fonts_text.contains("仿宋") && fonts_text.contains("宋体"),
            "便携表应给出中英文族名：{fonts_text}"
        );

        // 空字体集：本机字体一律没有，仍然排得出来，说明模板接受数组取值。
        let extracted = tempfile::tempdir().unwrap();
        extract(&path, extracted.path());
        let outcome = typst_engine::compile(
            &TypstJob::official(data, extracted.path()),
            &typst_engine::empty_font_set_for_test(),
        )
        .unwrap();
        assert!(outcome.pdf.starts_with(b"%PDF"));
    }

    /// 研究报告包：公式 SVG、插图、文献与模板都在，字体表键齐全且无合成字体名。
    #[test]
    fn research_package_carries_formulas_images_and_bibliography() {
        let base = tempfile::tempdir().unwrap();
        fs::create_dir_all(base.path().join("images")).unwrap();
        image::RgbImage::from_pixel(40, 20, image::Rgb([220, 235, 255]))
            .save(base.path().join("images/arch.png"))
            .unwrap();
        let markdown = "# 公共数据授权运营机制研究\n\n\
                        <!-- [摘要] -->\n\n摘要正文。\n\n\
                        <!-- [目录] -->\n\n\
                        <!-- [正文] -->\n\n\
                        ## 研究背景\n\n正文[@demo]，公式 $x^2$。\n\n\
                        $$\\frac{1}{3}$$\n\n\
                        ![总体架构](images/arch.png)\n\n\
                        <!-- [参考文献] -->\n\n## 参考文献\n";
        let path = base.path().join("pkg.zip");
        let warnings = write_package_with_base(
            &path,
            &research_input(),
            markdown,
            &UnitDisplay::new(&[]),
            &NumberingConfig::default(),
            &ElementMarks::default(),
            base.path(),
        )
        .unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");

        let entries = zip_entries(&path);
        for expected in [
            "research.typ",
            "doc.json",
            "README.txt",
            "images/arch.png",
            "references.bib",
        ] {
            assert!(
                entries.iter().any(|name| name == expected),
                "缺少 {expected}：{entries:?}"
            );
        }
        assert!(
            entries
                .iter()
                .any(|name| name.starts_with("math/") && name.ends_with(".svg")),
            "公式 SVG 应进包：{entries:?}"
        );
        assert!(
            entries.iter().all(|name| name != "research.md"),
            "交给 mdx 的源 Markdown 不应进包：{entries:?}"
        );

        let data = String::from_utf8(read_entry(&path, "doc.json")).unwrap();
        let value: Value = serde_json::from_str(&data).unwrap();
        let fonts = &value["fonts"];
        for key in ["song", "hei", "kai", "xbs", "latin", "mono", "fallback"] {
            assert!(fonts[key].is_array(), "缺少字体角色 {key}：{fonts}");
        }
        let fonts_text = fonts.to_string();
        assert!(
            !fonts_text.contains("GW "),
            "便携表不能带合成字体名：{fonts_text}"
        );

        let extracted = tempfile::tempdir().unwrap();
        extract(&path, extracted.path());
        let outcome = typst_engine::compile(
            &TypstJob {
                data,
                base_dir: extracted.path(),
                template: Template::Research,
                files: Default::default(),
            },
            &typst_engine::empty_font_set_for_test(),
        )
        .unwrap();
        assert!(outcome.pdf.starts_with(b"%PDF"));
    }

    /// 走完整导出入口：产物名对得上，没排 PDF 时出包警告也照常进 Typst 结果。
    #[test]
    fn export_artifacts_names_the_package_and_keeps_warnings() {
        let temp = tempfile::tempdir().unwrap();
        let selection = crate::models::ExportSelection {
            markdown: false,
            docx: false,
            pdf: false,
            typst: true,
            receipt: false,
            overwrite: true,
        };
        let artifacts = crate::export::export_artifacts(
            temp.path(),
            &official_input(),
            "# 标题\n\n<!-- [正文] -->\n\n正文。\n\n![缺图](images/不存在.png)",
            &selection,
            &UnitDisplay::new(&[]),
            &crate::models::FontConfig::default(),
            &NumberingConfig::default(),
            None,
        )
        .unwrap();
        assert_eq!(artifacts.files.len(), 1);
        assert!(
            artifacts.files[0]
                .to_string_lossy()
                .ends_with("-Typst包.zip"),
            "{:?}",
            artifacts.files
        );
        let outcome = artifacts.typst.expect("出包警告应塞进 Typst 结果");
        assert!(
            outcome
                .warnings
                .iter()
                .any(|warning| warning.contains("images/不存在.png")),
            "{:?}",
            outcome.warnings
        );
    }

    #[test]
    fn pdf_failure_does_not_block_package_or_hide_existing_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let input = official_input();
        let markdown = "# 测试通知\n\n正文。";
        let display = UnitDisplay::new(&[]);
        let fonts = crate::models::FontConfig::default();
        let numbering = NumberingConfig::default();
        let mut selection = crate::models::ExportSelection {
            markdown: true,
            docx: false,
            pdf: false,
            typst: true,
            receipt: false,
            overwrite: true,
        };
        let first = crate::export::export_artifacts(
            temp.path(),
            &input,
            markdown,
            &selection,
            &display,
            &fonts,
            &numbering,
            None,
        )
        .unwrap();
        let package = first.files.last().unwrap();
        let stem = package
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .strip_suffix("-Typst包.zip")
            .unwrap();
        // 用同名目录让 PDF 写入必定失败；不修改进程环境或依赖机器字体状态。
        fs::create_dir(package.parent().unwrap().join(format!("{stem}.pdf"))).unwrap();
        selection.pdf = true;
        let second = crate::export::export_artifacts(
            temp.path(),
            &input,
            markdown,
            &selection,
            &display,
            &fonts,
            &numbering,
            None,
        )
        .unwrap();
        assert!(second.compile_error.is_some());
        assert_eq!(second.files, first.files);
        assert!(zip_entries(package).contains(&"gongwen.typ".to_string()));
    }
}
