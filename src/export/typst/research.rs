//! 研究报告的 Typst 排版：mdx 整理出排版数据（`mdx::typst_research`），这里补上
//! 字体家族名、把公式排成 SVG，交给 `assets/typst/research.typ`。
//!
//! 版式对照的是 mdx 的 `md2tex.cls` 与 `template.tex`；两者的对照与已知差异见
//! `docs/typst-engine.md`。

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::export::research::ResearchSourceBundle;
use crate::models::{DraftInput, NumberingConfig};
use crate::typst_engine::{self, Template, TypstJob, TypstOutcome};

/// 研究报告用到的字体（随包 `runtime/fonts` 里的文件，见 `portable_runtime`）。
fn fonts() -> Result<Value> {
    let dir =
        crate::portable_runtime::find_font_dir().context("找不到内置字体目录 runtime/fonts")?;
    crate::portable_runtime::validate_research_fonts(&dir)?;
    let family = |file: &str| typst_engine::bundled_family(file);
    Ok(serde_json::json!({
        "song": family("FZShuSong.ttf")?,
        "hei": family("FZHei.ttf")?,
        "kai": family("GWKai.ttf")?,
        "xbs": family("XiaoBiaoSong.ttf")?,
        "latin": family("texgyretermes-regular.otf")?,
        "mono": family("JetBrainsMono-Regular.ttf")?,
        // 方正书宋覆盖 GBK，兼任兜底（与公文同一份）；GBK 以外的字由模板继续
        // 向系统字体回退。
        "fallback": family("FZShuSong.ttf")?,
    }))
}

/// 模板数据（JSON）、只在内存里的文件（公式 SVG）与提示。
type Prepared = (String, HashMap<String, Vec<u8>>, Vec<String>);

/// 生成模板数据与公式 SVG。`source` 是 mdx 源码目录里的 Markdown（插图、文献都在旁边）。
fn document(source: &Path, input: &DraftInput, numbering: &NumberingConfig) -> Result<Prepared> {
    let doc = mdx::typst_research::build_with_body_normalizer(source, |body| {
        super::research_body::normalize(body, numbering)
    })
    .context("研究报告转换失败")?;
    let mut warnings = doc.warnings.clone();
    let mut data = serde_json::to_value(&doc).context("无法序列化研究报告排版数据")?;
    let mut files = HashMap::new();
    // 花脸稿的哨兵先换成标注：公式源码里的哨兵剥掉才排得出来。
    super::research_redline::apply(&mut data);
    let terminal = input.research.template == crate::models::ResearchTemplate::Terminal;
    let dark = terminal && input.research.palette == crate::models::ResearchPalette::Dark;
    super::math::render_all_with_color(
        &mut data,
        &mut files,
        &mut warnings,
        if dark {
            latex_rust::Color::rgb(231, 237, 240)
        } else {
            latex_rust::Color::rgb(0, 0, 0)
        },
    );
    data["fonts"] = fonts()?;
    data["terminal"] = terminal.into();
    data["dark"] = dark.into();
    let data = serde_json::to_string(&data).context("无法序列化研究报告排版数据")?;
    Ok((data, files, warnings))
}

/// 排版并写出 PDF。插图按 `base_dir` 解析（正常是用户配置目录）。
pub(crate) fn write_pdf_with_base(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
    base_dir: &Path,
) -> Result<TypstOutcome> {
    let bundle = ResearchSourceBundle::create(
        input,
        markdown,
        numbering,
        crate::mermaid::Format::Pdf,
        base_dir,
    )?;
    let (data, files, warnings) = document(&bundle.markdown, input, numbering)?;
    let set = typst_engine::font_set(&crate::models::FontConfig::default())?;
    let mut outcome = typst_engine::compile(
        &TypstJob {
            data,
            base_dir: bundle.root(),
            template: Template::Research,
            files,
        },
        &set,
    )?;
    for warning in warnings.into_iter().rev() {
        outcome.warnings.insert(0, warning);
    }
    std::fs::write(path, &outcome.pdf)
        .with_context(|| format!("无法写入 PDF：{}", path.display()))?;
    Ok(outcome)
}

/// 同上，插图按用户配置目录解析。
pub(crate) fn write_pdf(
    path: &Path,
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
) -> Result<TypstOutcome> {
    write_pdf_with_base(
        path,
        input,
        markdown,
        numbering,
        &crate::storage::config_dir()?,
    )
}

/// 按完整定稿排版数物理页，封面、目录、附录与自动补的空白页均计入。
pub(crate) fn page_count(
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
) -> Result<usize> {
    page_count_with_base(input, markdown, numbering, &crate::storage::config_dir()?)
}

fn page_count_with_base(
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
    base_dir: &Path,
) -> Result<usize> {
    let bundle = ResearchSourceBundle::create(
        input,
        markdown,
        numbering,
        crate::mermaid::Format::Pdf,
        base_dir,
    )?;
    let (data, files, _) = document(&bundle.markdown, input, numbering)?;
    let set = typst_engine::font_set(&crate::models::FontConfig::default())?;
    typst_engine::page_count(
        &TypstJob {
            data,
            base_dir: bundle.root(),
            template: Template::Research,
            files,
        },
        &set,
    )
}

/// 测试用：只生成模板数据，落盘查看。
#[cfg(test)]
pub(crate) fn document_json(
    input: &DraftInput,
    markdown: &str,
    numbering: &NumberingConfig,
    base_dir: &Path,
) -> Result<String> {
    let bundle = ResearchSourceBundle::create(
        input,
        markdown,
        numbering,
        crate::mermaid::Format::Pdf,
        base_dir,
    )?;
    Ok(document(&bundle.markdown, input, numbering)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdx::typst_research::{Doc, Item, build};

    /// 西文数字一律 Termes：标题类（章节标题、图表题标签、封面黑体字与大标题、目录章条目）
    /// 用 Bold，正文、页码、列表序号用 Regular。
    #[test]
    fn latin_is_termes_bold_in_headings_and_regular_elsewhere() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let base = tempfile::tempdir().unwrap();
        let input = DraftInput {
            kind: crate::models::TemplateKind::ResearchReport,
            ..Default::default()
        };
        let markdown = concat!(
            "# 报告C1题名

<!-- [摘要] -->

摘要正文。

",
            "<!-- [目录] -->

<!-- [正文] -->

## 背景H2

",
            "正文B3里写**加粗D4**。

### 方法S5

",
            "表：数据T6说明

| 项目 | 数值 |
| --- | --- |
| 测试 | 123 |

",
            "1. 第一条
2. 第二条
3. 第三条
4. 第四条

",
        );
        let bundle = ResearchSourceBundle::create(
            &input,
            markdown,
            &NumberingConfig::default(),
            crate::mermaid::Format::Pdf,
            base.path(),
        )
        .unwrap();
        let (data, files, _) =
            document(&bundle.markdown, &input, &NumberingConfig::default()).unwrap();
        let set = typst_engine::font_set(&crate::models::FontConfig::default()).unwrap();
        let items = typst_engine::text_fonts_for_test(
            &TypstJob {
                data,
                base_dir: bundle.root(),
                template: Template::Research,
                files,
            },
            &set,
        )
        .unwrap();
        let termes = typst_engine::bundled_family("texgyretermes-regular.otf").unwrap();
        let weights = |needle: &str| -> Vec<(String, u16)> {
            items
                .iter()
                .filter(|item| item.text.contains(needle))
                .map(|item| (item.family.clone(), item.weight))
                .collect()
        };
        for (needle, weight) in [
            ("C1", 700),
            ("H2", 700),
            ("B3", 400),
            ("D4", 700),
            ("T6", 400),
            ("123", 400),
        ] {
            let found = weights(needle);
            assert!(
                !found.is_empty() && found.iter().all(|(f, w)| *f == termes && *w == weight),
                "「{needle}」应是 Termes {weight}：{found:?}"
            );
        }
        // 节标题：正文里是 Bold，目录的节条目照 TeX 用正文字面，是 Regular。
        let section = weights("S5");
        assert!(section.iter().all(|(f, _)| *f == termes), "{section:?}");
        assert!(section.iter().any(|(_, w)| *w == 700), "{section:?}");
        assert!(section.iter().any(|(_, w)| *w == 400), "{section:?}");
        // 「1.1」三处：目录节条目（Regular）、节标题与表题标签「表 1.1」（Bold）。
        let numbers = weights("1.1");
        assert_eq!(
            numbers.iter().filter(|(_, w)| *w == 700).count(),
            2,
            "{numbers:?}"
        );
        // 数字一律 Termes（页码、目录页码、封面版本号等）。
        let digits: Vec<_> = items
            .iter()
            .filter(|item| item.text.chars().any(|c| c.is_ascii_digit()))
            .filter(|item| item.family != termes)
            .collect();
        assert!(digits.is_empty(), "数字应一律是 Termes：{digits:?}");
    }

    #[test]
    fn research_physical_page_count_matches_pdf_including_cover_toc_and_blank_pages() {
        // 页数要与真实排版的 PDF 对得上，本机没有 runtime/fonts（CI）时跳过。
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let base = tempfile::tempdir().unwrap();
        let input = DraftInput {
            kind: crate::models::TemplateKind::ResearchReport,
            ..Default::default()
        };
        let markdown = concat!(
            "# 页数测试报告\n\n<!-- [摘要] -->\n\n摘要正文。\n\n",
            "<!-- [目录] -->\n\n<!-- [正文] -->\n\n## 背景\n\n正文。\n\n",
            "$$x=\\frac{1}{2}$$\n\n## 措施\n\n| 项目 | 数值 |\n| --- | --- |\n| 测试 | 123 |\n\n",
            "<!-- [附录] -->\n\n## 数据说明\n\n附录正文。\n"
        );
        let numbering = NumberingConfig::default();
        let count = page_count_with_base(&input, markdown, &numbering, base.path()).unwrap();
        let outcome = write_pdf_with_base(
            &base.path().join("research.pdf"),
            &input,
            markdown,
            &numbering,
            base.path(),
        )
        .unwrap();
        assert_eq!(
            count,
            crate::manuscript_io::send_package::page_count(&outcome.pdf).unwrap()
        );
        assert!(
            count >= 6,
            "应计入封面、摘要、目录、多章及附录，实际 {count}"
        );
    }

    #[test]
    fn terminal_research_chapter_sheets_start_on_separate_odd_pages() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let base = tempfile::tempdir().unwrap();
        let mut input = DraftInput {
            kind: crate::models::TemplateKind::ResearchReport,
            ..Default::default()
        };
        input.research.template = crate::models::ResearchTemplate::Terminal;
        let markdown = "# 研究报告\n\n<!-- [目录] -->\n\n<!-- [部分] -->\n\n# 现状与问题\n\n## 研究背景\n\n正文。\n\n## 主要问题\n\n正文。\n\n# 实施路径\n\n## 对策建议\n\n正文。\n";
        let bundle = ResearchSourceBundle::create(
            &input,
            markdown,
            &NumberingConfig::default(),
            crate::mermaid::Format::Pdf,
            base.path(),
        )
        .unwrap();
        let (data, files, _) =
            document(&bundle.markdown, &input, &NumberingConfig::default()).unwrap();
        let set = typst_engine::font_set(&crate::models::FontConfig::default()).unwrap();
        let items = typst_engine::text_fonts_for_test(
            &TypstJob {
                data,
                base_dir: bundle.root(),
                template: Template::Research,
                files,
            },
            &set,
        )
        .unwrap();
        let mut pages: Vec<_> = items
            .iter()
            .filter(|t| t.text.contains("CHAPTER 0"))
            .map(|t| t.page)
            .collect();
        pages.dedup();
        assert_eq!(pages.len(), 3, "三个章必须各有独立章首页：{pages:?}");
        assert!(
            pages.iter().all(|p| p % 2 == 1),
            "章必须起在奇数页：{pages:?}"
        );
        assert!(pages.windows(2).all(|w| w[0] < w[1]));
    }

    fn build_str(markdown: &str) -> Doc {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("r.md");
        std::fs::write(&path, markdown).unwrap();
        build(&path).unwrap()
    }

    fn kinds(doc: &Doc) -> Vec<String> {
        doc.blocks
            .iter()
            .map(|b| {
                serde_json::to_value(b).unwrap()["k"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn numbers_chapters_sections_tables_and_appendices_like_latex() {
        let doc = build_str(concat!(
            "---\n文件名称: 测试\n---\n\n",
            "## 背景\n\n### 一节\n\n#### 小节\n\n表：甲\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n",
            "## 现状\n\n### 二节\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n",
            "<!-- [附录] -->\n\n## 附一\n\n### 附节\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        ));
        let json = serde_json::to_string(&doc.blocks).unwrap();
        for expected in [
            r#""number":"1","prefix":"第1章""#,
            r#""number":"1.1""#,
            r#""number":"1.1.1""#,
            r#""number":"2.1""#,
            r#""number":"A","prefix":"附录A""#,
            r#""number":"A.1""#,
        ] {
            assert!(json.contains(expected), "{expected}: {json}");
        }
        let tables: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Item::Table(t) => Some(t.number.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(tables, ["1.1", "2.1", "A.1"]);
    }

    #[test]
    fn unnumbered_chapters_use_running_numbers_and_parts_count_in_chinese() {
        let doc = build_str(concat!(
            "<!-- [不编号] -->\n## 前言\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n",
            "> [!专栏] 甲\n\n",
            "<!-- [部分] -->\n\n# 现状 {#part:a}\n\n## 背景\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n",
            "> [!专栏] 乙\n\n> [!案例] 丙\n\n# 对策\n\n## 思路\n\n",
            "<!-- [不编号] -->\n## 结语\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        ));
        let json = serde_json::to_string(&doc.blocks).unwrap();
        assert!(json.contains(r#""k":"part","number":"一""#), "{json}");
        assert!(json.contains(r#""number":"二""#), "{json}");
        assert!(
            json.contains(r#""prefix":"第2章""#),
            "章号跨部分连续：{json}"
        );
        let numbers: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Item::Table(t) => Some(t.number.clone()),
                Item::Box { number, .. } => Some(number.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(numbers, ["1", "1", "1.1", "1.1", "1.1", "2"]);
    }

    #[test]
    fn deeper_list_items_join_their_parent_paragraph_and_numbering_continues() {
        let doc = build_str("## 章\n\n1. 甲\n2. 乙\n    1. 子一\n    2. 子二\n3. 丙\n");
        let lists: Vec<Vec<(u8, String)>> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Item::List { items } => {
                    Some(items.iter().map(|e| (e.level, e.label.clone())).collect())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            lists,
            vec![
                vec![(1, "⑴".to_string())],
                vec![(1, "⑵".into()), (2, "①".into()), (2, "②".into())],
                vec![(1, "⑶".into())],
            ]
        );
    }

    #[test]
    fn abstract_toc_and_reference_section_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("references.bib"),
            "@book{a, title={T}, author={A}, year={2020}}\n",
        )
        .unwrap();
        let path = dir.path().join("r.md");
        std::fs::write(
            &path,
            concat!(
                "---\nbibliography: references.bib\n---\n\n",
                "<!-- [摘要] -->\n\n摘要。\n\n<!-- [目录] -->\n\n<!-- [正文] -->\n\n",
                "## 背景\n\n正文[@a]。\n\n<!-- [参考文献] -->\n\n## 参考文献\n",
            ),
        )
        .unwrap();
        let doc = build(&path).unwrap();
        assert_eq!(
            kinds(&doc),
            [
                "front", "abstract", "toc", "chapter", "par", "chapter", "bib"
            ]
        );
        assert!(matches!(
            doc.blocks.last(),
            Some(Item::Bib { titled: false })
        ));
    }

    #[test]
    fn missing_images_are_dropped_with_a_warning() {
        let doc = build_str("## 章\n\n![图](images/none.png)\n");
        assert!(!kinds(&doc).contains(&"figure".to_string()));
        assert_eq!(doc.warnings.len(), 1, "{:?}", doc.warnings);
    }

    /// 整条链：mdx 数据 → 公式 SVG → 模板排版，出得来 PDF，封面、目录、文献都在。
    #[test]
    fn typesets_a_research_report_end_to_end() {
        if crate::portable_runtime::find_font_dir().is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut input = crate::models::DraftInput {
            kind: crate::models::TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        input.research.bibliography_content =
            "@book{a, title={书名}, author={作者}, year={2020}}\n".into();
        let path = dir.path().join("报告.pdf");
        let outcome = write_pdf_with_base(
            &path,
            &input,
            concat!(
                "<!-- [摘要] -->\n\n摘要。\n\n<!-- [目录] -->\n\n<!-- [正文] -->\n\n",
                "## 研究背景 {#chap:bg}\n\n正文[@a]，见第{@chap:bg}章，$x^2$。\n\n",
                "$$\\frac{1}{3}$$\n\n> [!案例] 标题\n>\n> 内容。\n\n",
                "<!-- [参考文献] -->\n\n## 参考文献\n",
            ),
            &crate::models::NumberingConfig::default(),
            dir.path(),
        )
        .unwrap();
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        let pdf = lopdf::Document::load(&path).unwrap();
        let pages: Vec<u32> = pdf.get_pages().keys().copied().collect();
        let text = pdf
            .extract_text(&pages)
            .unwrap()
            .replace(char::is_whitespace, "");
        for expected in [
            "测试报告",
            "目录",
            "摘要",
            "研究背景",
            "案例",
            "参考文献",
            "书名",
        ] {
            assert!(text.contains(expected), "PDF 缺少 {expected}：{text}");
        }
    }
}
