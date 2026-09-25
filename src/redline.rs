//! 花脸稿：两版正文经视觉 diff 引擎比较，标注层序列化成带哨兵的 Markdown，
//! 交给现有导出器出 PDF / Word。
//!
//! 花脸稿是送签用的纸：**删掉的字留在原位画红色直删除线，新增的字就地套
//! 蓝色方框**（docs/version-diff-redesign.md 需求结论第 10 条），领导一眼
//! 就能看出这一稿动了哪儿。它和屏幕上的版本对照是两回事——对照是左右分栏，
//! 各看各的；花脸稿只有一张纸，增删必须交织在同一段里。
//!
//! 实现上不走「往源码里插哨兵」的老路：哨兵在**解析后**由视觉模型重新生成
//! 的标注稿里注入（见 `visual_diff`），标题、表格、公式都能安全携带标记，
//! 一切就地标注，不再附「花脸稿说明」页。研究报告的 Word / TeX 由 mdx
//! 转换器生成，哨兵原样穿过转换，产物落地后由 `visual_diff::postprocess`
//! 换成删除线 / 边框（Word）或 `\GwDel` / `\GwAdd` 宏（TeX）。

use crate::export;
use crate::models::{DraftInput, FontConfig, NumberingConfig};
use crate::texcompile;
use crate::units::UnitDisplay;
use crate::visual_diff;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 一份生成好的花脸稿。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedlineDoc {
    /// 带哨兵的 Markdown，直接交给 `export` 的导出器。
    pub markdown: String,
    /// 标注稿里每个块的来历，版本对照据此在预览与代码 diff 之间互跳。
    pub(crate) spans: Vec<visual_diff::MarkedSpan>,
    /// 公文要素的就地标注（版头 / 版记）：旧值删除线、新值加框，整字段替换。
    /// 正文没动、只改要素时 `markdown` 不带哨兵，标注全在这里。
    pub(crate) elements: visual_diff::ElementMarks,
}

impl RedlineDoc {
    /// 有没有任何改动。两版完全一致时花脸稿没有意义，调用方据此提示用户。
    /// 要素改动也算——正文一字未动、只改密级或日期的稿子同样要能出花脸稿。
    pub fn is_empty(&self) -> bool {
        !self
            .markdown
            .chars()
            .any(crate::export::is_redline_sentinel)
            && self.elements.is_empty()
    }
}

/// 生成花脸稿。`old` 是旧版正文，`new` 是新版正文，方向与版本对照一致。
///
/// 标注层附着在新版视觉块上：词级比较 + 归并、段落移动、编号顺移不标、
/// 表格逐格、公式整体（见 `visual_diff` 模块头注释的规则清单）。公文要素
/// 不在正文里，这个入口不带要素对照（要素标注见 [`build_with_inputs`]）。
#[allow(dead_code)] // 不带要素对照的兼容入口，测试使用。
pub fn build(old: &str, new: &str) -> RedlineDoc {
    build_with_elements(old, new, visual_diff::ElementMarks::default())
}

/// 生成花脸稿：正文走视觉 diff，公文要素按新旧两版 `DraftInput` 就地标注
/// （方案需求第 9 条）。要素的显示值与三处渲染同源（`export::element_display`），
/// 预览版留白两侧一致时不产生标注。
pub fn build_with_inputs(
    old: &str,
    new: &str,
    old_input: &DraftInput,
    new_input: &DraftInput,
    display: &UnitDisplay,
) -> RedlineDoc {
    let elements = visual_diff::element_marks(old_input, new_input, display);
    build_with_elements(old, new, elements)
}

fn build_with_elements(old: &str, new: &str, elements: visual_diff::ElementMarks) -> RedlineDoc {
    let overlay = visual_diff::diff_documents(
        &visual_diff::DocumentModel::from_markdown(old),
        &visual_diff::DocumentModel::from_markdown(new),
    );
    let (markdown, spans) = visual_diff::to_marked_markdown_with_spans(&overlay);
    RedlineDoc {
        markdown,
        spans,
        elements,
    }
}

/// 要导出哪些格式。至少选一个，调用方保证。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedlineFormats {
    pub pdf: bool,
    pub docx: bool,
}

/// 生成花脸稿文件。返回写出的文件路径。
///
/// 复用定稿那条导出链路：同一个 `.cls`、同一套字体和版心，所以花脸稿看起来就是
/// 这份公文本身，只是多了增删标记。唯一的不同是文件名带 `-花脸稿`，且落在自己的
/// 子目录里，不会和定稿混在一起被误发。研究报告走 mdx 转换器，标记在产物落地后
/// 注入。
pub fn export_files(
    output_dir: &Path,
    input: &DraftInput,
    doc: &RedlineDoc,
    formats: RedlineFormats,
    display: &UnitDisplay,
    fonts: &FontConfig,
    numbering: &NumberingConfig,
) -> Result<Vec<PathBuf>> {
    let markdown = &doc.markdown;
    // 标题取自正文 H1，哨兵已在生成时避开标题语法位置，这里再兜一层底：
    // 文件名里绝不能出现私用区字符。
    let title = export::strip_redline(&export::extract_title(markdown, &input.title_hint));
    let stem = format!("{}-花脸稿", export::safe_filename(&title));
    let dir = output_dir.join(&stem);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("无法创建花脸稿目录：{}", dir.display()))?;

    let mut files = Vec::new();
    if formats.docx {
        let path = dir.join(format!("{stem}.docx"));
        if input.kind.is_research() {
            export::write_docx_research(&path, input, markdown, numbering)?;
            visual_diff::redline_research_docx(&path)?;
        } else {
            export::write_docx_with_numbering(&path, input, markdown, display, fonts, numbering)?;
        }
        files.push(path);
    }
    if formats.pdf {
        let tex = dir.join(format!("{stem}.tex"));
        export::write_tex_for_kind(&tex, input, markdown, display, fonts, numbering)?;
        files.push(tex.clone());
        if input.kind.is_research() {
            // mdx 转换出的分章 TeX 也要换宏、主文件注入导言区定义。
            visual_diff::redline_research_tex_files(&dir)?;
            let outcome = texcompile::compile_research_pdf(&tex)
                .with_context(|| "花脸稿 PDF 编译失败".to_string())?;
            if let Some(pdf) = outcome.pdf {
                files.push(pdf);
            }
        } else {
            let outcome = texcompile::compile_pdf_with_proof(&tex, fonts)
                .with_context(|| "花脸稿 PDF 编译失败".to_string())?;
            if let Some(pdf) = outcome.pdf {
                files.push(pdf);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::strip_redline;
    use crate::export::{REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN};

    /// 把哨兵换成好读的记号，断言起来直观：删除 ~…~、新增 […]。
    fn readable(text: &str) -> String {
        text.chars()
            .map(|ch| match ch {
                REDLINE_DEL_OPEN | REDLINE_DEL_CLOSE => "~".to_string(),
                REDLINE_ADD_OPEN => "[".to_string(),
                REDLINE_ADD_CLOSE => "]".to_string(),
                other => other.to_string(),
            })
            .collect()
    }

    #[test]
    fn an_unchanged_document_produces_no_marks() {
        let text = "第一段。\n\n第二段。";
        let doc = build(text, text);
        assert!(doc.is_empty());
        assert_eq!(doc.markdown, text);
    }

    #[test]
    fn element_only_changes_still_produce_a_redline() {
        // 正文一字未动、只改密级：花脸稿要有——要素标注在 RedlineDoc.elements，
        // 正文的 markdown 仍不带哨兵（要素不进正文，就地标注在版头）。
        use crate::models::TemplateKind;
        let text = "第一段。";
        let mut old = DraftInput {
            kind: TemplateKind::OfficialLetter,
            ..DraftInput::default()
        };
        old.profile.security_level.clear();
        old.profile.security_period.clear();
        let mut new = old.clone();
        new.profile.security_level = "秘密".into();
        new.profile.security_period = "10年".into();
        let display = UnitDisplay::new(&[]);
        let doc = build_with_inputs(text, text, &old, &new, &display);
        assert!(!doc.is_empty(), "要素改动也算改动");
        assert_eq!(doc.markdown, text, "正文没动，markdown 原样");
        assert_eq!(
            doc.elements.security().change(),
            Some(("", "秘密★10年")),
            "密级整字段替换"
        );
    }

    #[test]
    fn a_rewritten_paragraph_interleaves_both_versions() {
        let doc = build(
            "同意你单位关于报送情况的请示。",
            "同意你单位关于开展检查的请示。",
        );
        assert_eq!(
            readable(&doc.markdown),
            "同意你单位关于~报送情况~[开展检查]的请示。"
        );
    }

    #[test]
    fn stripping_the_marks_gives_back_clean_text() {
        // 花脸稿去掉标记后应当是新版文本，且不能残留哨兵字符：它们一旦漏进
        // 标题或文件名，纸上就是缺字符。
        let doc = build("原来的表述。", "修改后的表述。");
        let clean = strip_redline(&doc.markdown);
        // 摘掉的只是哨兵：删掉的旧字仍占版面（花脸稿比定稿长的原因），
        // 与新增的文字交织在同一句里；哨兵一个不留。
        assert!(clean.contains("修改后的表述。"), "新文字在：{clean}");
        assert!(clean.contains("原来"), "删掉的旧字留在原位：{clean}");
        assert!(!clean.contains('\u{E000}') && !clean.contains('\u{E003}'));
    }

    #[test]
    fn a_whole_new_paragraph_is_boxed() {
        let doc = build("第一段。", "第一段。\n\n新增的第二段。");
        assert_eq!(readable(&doc.markdown), "第一段。\n\n[新增的第二段。]");
    }

    #[test]
    fn a_deleted_paragraph_stays_on_the_page_with_a_strike() {
        let doc = build("第一段。\n\n多余的第二段。", "第一段。");
        assert_eq!(readable(&doc.markdown), "第一段。\n\n~多余的第二段。~");
    }

    #[test]
    fn markdown_prefixes_stay_outside_the_marks() {
        // 标记跑到 `- ` 前面，这一行就不再是列表项了。
        let doc = build("- 原来的条目", "- 改过的条目");
        assert!(doc.markdown.starts_with("1. "), "编号前缀必须留在标记外面");
        // 列表项末尾标点由解析器统一规范化（补句号），标注在规范化后的文字上。
        assert_eq!(readable(&doc.markdown), "1. ~原来~[改过]的条目。");
    }

    #[test]
    fn heading_changes_are_marked_in_place() {
        // 新样式：标题也就地标注（旧实现是列入「花脸稿说明」）。
        let doc = build("## 报送内容\n\n正文。", "## 报送要求\n\n正文。");
        let marked = readable(&doc.markdown);
        assert!(
            marked.contains("## 报送~内容~[要求]"),
            "标题就地标注：{marked}"
        );
        assert!(!marked.contains("花脸稿说明"), "不再有说明页：{marked}");
    }

    #[test]
    fn table_cells_are_marked_in_place_and_the_grid_survives() {
        let doc = build(
            "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |",
            "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 9月 |",
        );
        let marked = readable(&doc.markdown);
        // 日期是整体 token：整格删旧插新。
        assert!(marked.contains("~8月~[9月]"), "单元格应就地标注：{marked}");
        assert!(marked.contains("| 备案 |"), "未改的格子不该动：{marked}");
        assert!(marked.contains("| --- | --- |"), "分隔行不能被动：{marked}");
        assert_eq!(marked.lines().count(), 3, "不该有空行插进表里：{marked}");
        assert!(marked.lines().all(
            |line| line.trim_start_matches(['~', '[', '|']).starts_with('|')
                || line.trim_start_matches(['~', '[']).starts_with("事项")
                || line.contains('|')
        ));
    }

    #[test]
    fn an_untouched_table_comes_through_byte_for_byte() {
        let table = "正文一段。\n\n| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |\n\n正文二段。";
        let doc = build(table, table);
        assert!(doc.is_empty());
        assert_eq!(doc.markdown, table, "未改动的表格必须原样过来");
    }

    #[test]
    fn a_table_with_a_different_column_count_replaces_the_whole_table() {
        // 列数变了逐格对不上号：整表删旧插新，不再有说明页。
        let doc = build(
            "| 事项 | 时限 |\n| --- | --- |\n| 备案 | 8月 |",
            "| 事项 | 时限 | 责任人 |\n| --- | --- | --- |\n| 备案 | 8月 | 张三 |",
        );
        let marked = readable(&doc.markdown);
        assert!(
            marked.contains("~备案~") && marked.contains("[张三]"),
            "整表删旧插新：{marked}"
        );
        assert!(!marked.contains("花脸稿说明"), "说明页已取消：{marked}");
    }

    #[test]
    fn a_deleted_heading_is_not_printed_as_a_heading() {
        // 删除的标题不排成标题行：编号会重复。它是原位置的一段删除线文字。
        let doc = build("## 将被删掉\n\n正文。", "正文。");
        let marked = readable(&doc.markdown);
        assert!(
            !marked.lines().any(|line| line.starts_with("## ~")),
            "删掉的标题不能照排成标题：{marked}"
        );
        assert!(
            marked.contains("~将被删掉~"),
            "文字留在原位画删除线：{marked}"
        );
    }
}

#[cfg(test)]
mod tex_tests {
    use super::*;
    use crate::export::write_tex;
    use crate::models::TemplateProfile;

    /// 端到端：花脸稿 Markdown 走定稿那条导出链路，产出的 TeX 里必须是
    /// `\GwDel` / `\GwAdd`，而不是把哨兵字符原样漏进去——那在纸上就是缺字符。
    #[test]
    fn redline_markdown_becomes_gw_macros_in_tex() {
        let doc = build(
            "# 关于报送情况的函\n\n同意你单位关于报送情况的请示。",
            "# 关于报送情况的函\n\n同意你单位关于开展检查的请示。",
        );
        let mut input = DraftInput {
            kind: crate::models::TemplateKind::PlainDocument,
            ..Default::default()
        };
        input.profile = TemplateProfile::for_kind(input.kind);

        let dir = tempfile::tempdir().expect("临时目录");
        let tex = dir.path().join("花脸稿.tex");
        write_tex(
            &tex,
            &input,
            &doc.markdown,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .expect("写 TeX");
        let content = std::fs::read_to_string(&tex).expect("读 TeX");

        assert!(content.contains("\\GwDel{"), "缺少删除标记：{content}");
        assert!(content.contains("\\GwAdd{"), "缺少新增标记");
        assert!(
            !content.contains('\u{E000}') && !content.contains('\u{E002}'),
            "哨兵字符漏进了 TeX"
        );
    }

    /// 哨兵绝不能出现在 TeX 里——它是私用区码位，字体里没有字形，印出来是豆腐块。
    ///
    /// 这条把哨兵**硬塞**进标题、各级小标题、表格、列表、附件标题等所有位置，
    /// 包括那些不做标记的路径，验证兜底过滤（`tex_escape` / `plain_text` /
    /// 表格列宽）确实拦得住。将来谁新写一条渲染路径忘了处理，这条会先炸。
    #[test]
    fn no_sentinel_can_reach_the_tex_through_any_path() {
        use crate::export::{
            REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN,
        };
        let d0 = REDLINE_DEL_OPEN;
        let d1 = REDLINE_DEL_CLOSE;
        let a0 = REDLINE_ADD_OPEN;
        let a1 = REDLINE_ADD_CLOSE;
        let markdown = format!(
            "# 标题{d0}删{d1}里也塞\n\n## 二级{a0}增{a1}标题\n\n正文{d0}删{d1}与{a0}增{a1}。\n\n             - 列表{d0}删{d1}项\n\n1. 有序{a0}增{a1}项\n\n             | 表头{d0}删{d1} | 姓名 |\n| --- | --- |\n| 单元{a0}增{a1}格 | 李四 |\n\n             <!-- [附件] -->\n\n# 附件{d0}删{d1}标题\n\n附件正文{a0}增{a1}。"
        );

        for kind in [
            crate::models::TemplateKind::PlainDocument,
            crate::models::TemplateKind::OfficialLetter,
            crate::models::TemplateKind::WhitePaper,
            crate::models::TemplateKind::RedHeadApproval,
        ] {
            let input = DraftInput {
                kind,
                profile: TemplateProfile::for_kind(kind),
                ..Default::default()
            };
            let dir = tempfile::tempdir().expect("临时目录");
            let tex = dir.path().join("t.tex");
            write_tex(
                &tex,
                &input,
                &markdown,
                &UnitDisplay::new(&[]),
                &FontConfig::default(),
            )
            .expect("写 TeX");
            let content = std::fs::read_to_string(&tex).expect("读 TeX");
            for (name, ch) in [
                ("删除起", d0),
                ("删除止", d1),
                ("新增起", a0),
                ("新增止", a1),
            ] {
                assert!(!content.contains(ch), "{kind:?} 的 TeX 里漏出了{name}哨兵");
            }
        }
    }

    /// Word 侧同理：哨兵不能进 OOXML。docx 是 zip，直接读整个字节流找码位的
    /// UTF-8 编码就够——XML 里存的就是它。
    #[test]
    fn no_sentinel_can_reach_the_docx() {
        use crate::export::{REDLINE_ADD_OPEN, REDLINE_DEL_OPEN};
        let markdown = format!(
            "# 标题{d}删{c}\n\n正文{a}增{b}与{d}删{c}。\n\n| 表头{d}删{c} | 值 |\n| --- | --- |\n| 格{a}增{b} | 1 |",
            d = REDLINE_DEL_OPEN,
            c = crate::export::REDLINE_DEL_CLOSE,
            a = REDLINE_ADD_OPEN,
            b = crate::export::REDLINE_ADD_CLOSE,
        );
        let input = DraftInput {
            kind: crate::models::TemplateKind::PlainDocument,
            profile: TemplateProfile::for_kind(crate::models::TemplateKind::PlainDocument),
            ..Default::default()
        };
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("t.docx");
        export::write_docx(&path, &input, &markdown, &UnitDisplay::new(&[])).expect("写 docx");

        let bytes = std::fs::read(&path).expect("读 docx");
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("解包");
        for index in 0..zip.len() {
            use std::io::Read;
            let mut entry = zip.by_index(index).expect("条目");
            let name = entry.name().to_string();
            let mut buffer = String::new();
            if entry.read_to_string(&mut buffer).is_err() {
                continue; // 二进制条目（图片等）跳过
            }
            for ch in ['\u{E000}', '\u{E001}', '\u{E002}', '\u{E003}'] {
                assert!(!buffer.contains(ch), "{name} 里漏出了哨兵");
            }
        }
    }

    /// 类文件里的标记宏必须与样式约定一致：删除 = 红色直删除线，新增 = 蓝色
    /// 方框（方案需求结论第 10 条；旧实现是穿过字身的波浪线 + 黑色框）。
    /// 类文件是编译进二进制的，这里直接查它的文本。
    #[test]
    fn the_class_wires_the_new_mark_styles() {
        let class = include_str!("../gonghan-gwa.cls");
        assert!(
            class.contains("symbol=\\GwStrikeUnit"),
            "\\GwDel 必须画直删除线：不再用自绘波浪单元"
        );
        assert!(
            class.contains("\\providecolor{GwaDelColor}")
                || class.contains("\\definecolor{GwaDelColor}"),
            "删除色必须定义为 GwaDelColor"
        );
        assert!(class.contains("C00000"), "删除必须是红 #C00000");
        assert!(
            class.contains("\\providecolor{GwaAddColor}")
                || class.contains("\\definecolor{GwaAddColor}"),
            "新增色必须定义为 GwaAddColor"
        );
        assert!(class.contains("1F4E9E"), "新增必须是蓝 #1F4E9E");
        // 删掉的字也是红色：xeCJKfntef 装盒放字，颜色 special 到不了汉字上
        // （第 ③ 期测试 F2 实测汉字是黑的），只能走字体的 Color 属性。
        for (name, source) in [
            ("gonghan-gwa.cls", class),
            ("研究报告导言区", crate::visual_diff::REDLINE_PREAMBLE_TEX),
        ] {
            assert!(
                source
                    .contains("\\addfontfeatures{Color=C00000}\\addCJKfontfeatures{Color=C00000}"),
                "{name} 的 \\GwDel 要用字体颜色染红删掉的字"
            );
            // 新增框的竖边不能与框里的字断开到两行（第 ③ 期测试 F3：表格窄列里
            // 左竖边落在上一行行尾）：留白用 \kern（\hspace 是胶、是断点），竖边
            // 与字之间 \nobreak，断点只留在框前面。
            assert!(
                source
                    .contains("\\GwBoxBarL}{\\penalty5000\\GwBoxBar\\rlap{\\GwBoxStubs}\\nobreak}")
                    && source.contains("\\GwBoxBarR}{\\nobreak\\llap{\\GwBoxStubs}\\GwBoxBar}"),
                "{name} 的竖边要粘住框里的字、框前留断点"
            );
            assert!(
                source.contains("\\GwAddLines{\\kern1.5pt#1\\kern1.5pt}"),
                "{name} 的 \\GwAdd 留白要用 \\kern"
            );
            let add_macros = source
                .lines()
                .filter(|line| line.contains("command{\\GwAdd"))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !add_macros.contains("\\hspace"),
                "{name} 的新增框宏里不能再有 \\hspace：{add_macros}"
            );
        }
    }

    /// 反向保证：没有花脸稿标记的普通稿件，产出的 TeX 里不该出现这两个宏。
    /// 这条守着"改动不影响存量导出"。
    #[test]
    fn an_ordinary_document_gains_no_redline_macros() {
        let mut input = DraftInput {
            kind: crate::models::TemplateKind::PlainDocument,
            ..Default::default()
        };
        input.profile = TemplateProfile::for_kind(input.kind);

        let dir = tempfile::tempdir().expect("临时目录");
        let tex = dir.path().join("定稿.tex");
        write_tex(
            &tex,
            &input,
            "# 普通公文\n\n这是一段普通正文，没有任何增删标记。",
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
        )
        .expect("写 TeX");
        let content = std::fs::read_to_string(&tex).expect("读 TeX");
        assert!(!content.contains("\\GwDel"));
        assert!(!content.contains("\\GwAdd"));
    }
}

#[cfg(test)]
mod consistency_tests {
    //! 一致性测试：同一份标注稿分别生成 DOCX run 序列与 TeX 片段序列，
    //! 两边的 `(文字, 类型)` 序列必须完全相同（方案 4.3 节：一份标注层喂
    //! 多处，从结构上保证「预览 = PDF = Word」）。
    //!
    //! 覆盖正文段、列表项、对齐行：三者在两条链路里都走
    //! `redline_chunks` → 标记宏 / 字符格式。标题与表格的标注在两边各有
    //! 专门的入口（`marked_runs` / `marked_tex_escape`、逐格切块），由各自
    //! 的单测把关。

    use super::build;
    use crate::export::{MarkdownBlock, RedlineKind, parse_markdown};
    use crate::export::{body_runs, body_text_to_tex};

    /// DOCX 侧：用真实的 `body_runs` 生成 run，再按 run 属性取回
    /// `(文字, 类型)`。删除 = 删除线，新增 = 字符边框。
    fn docx_sequence(markdown: &str) -> Vec<(String, RedlineKind)> {
        let mut seq = Vec::new();
        for block in parse_markdown(markdown) {
            let text = match &block {
                MarkdownBlock::Paragraph(text)
                | MarkdownBlock::OrderedListItem { text, .. }
                | MarkdownBlock::Aligned { text, .. } => text,
                _ => continue,
            };
            for run in &body_runs(text, None) {
                let text: String = run
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        docx_rs::RunChild::Text(t) => Some(t.text.as_str()),
                        _ => None,
                    })
                    .collect();
                if text.is_empty() {
                    continue;
                }
                let kind = if run.run_property.strike.is_some() {
                    RedlineKind::Deleted
                } else if run.run_property.text_border.is_some() {
                    RedlineKind::Added
                } else {
                    RedlineKind::Same
                };
                push_merged(&mut seq, text, kind);
            }
        }
        seq
    }

    /// TeX 侧：用真实的 `body_text_to_tex` 生成片段，再解析 `\GwDel` /
    /// `\GwAdd` 取回 `(文字, 类型)`；`\GwBold`、括号楷体组是透明包装。
    fn tex_sequence(markdown: &str) -> Vec<(String, RedlineKind)> {
        let mut seq = Vec::new();
        for block in parse_markdown(markdown) {
            let text = match &block {
                MarkdownBlock::Paragraph(text)
                | MarkdownBlock::OrderedListItem { text, .. }
                | MarkdownBlock::Aligned { text, .. } => text,
                _ => continue,
            };
            for (text, kind) in extract_tex_fragments(&body_text_to_tex(text)) {
                push_merged(&mut seq, text, kind);
            }
        }
        seq
    }

    fn push_merged(seq: &mut Vec<(String, RedlineKind)>, text: String, kind: RedlineKind) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = seq.last_mut()
            && last.1 == kind
        {
            last.0.push_str(&text);
            return;
        }
        seq.push((text, kind));
    }

    /// 解析 `body_text_to_tex` 的输出：`\GwDel{…}` / `\GwAdd{…}` 换类型，
    /// 其余包装（`\GwBold`、括号楷体组、普通分组）透明穿过，转义还原。
    fn extract_tex_fragments(tex: &str) -> Vec<(String, RedlineKind)> {
        const KAI_GROUP: &str = "{\\kai\\enkai\\zihao{4} ";
        const KAI_GROUP_FROZEN: &str = "{\\GwBoxFreeze\\kai\\enkai\\zihao{4} ";
        let chars: Vec<char> = tex.chars().collect();
        let mut stack: Vec<RedlineKind> = vec![RedlineKind::Same];
        let mut buf = String::new();
        let mut out: Vec<(String, RedlineKind)> = Vec::new();
        fn flush(out: &mut Vec<(String, RedlineKind)>, stack: &[RedlineKind], buf: &mut String) {
            if !buf.is_empty() {
                push_merged(out, std::mem::take(buf), *stack.last().expect("栈非空"));
            }
        }
        let mut index = 0usize;
        while index < chars.len() {
            let rest: String = chars[index..].iter().collect();
            if rest.starts_with("\\GwDel{") {
                flush(&mut out, &stack, &mut buf);
                stack.push(RedlineKind::Deleted);
                index += "\\GwDel{".len();
            } else if let Some(name) = ["\\GwAdd{", "\\GwAddOpen{", "\\GwAddMid{", "\\GwAddClose{"]
                .into_iter()
                .find(|name| rest.starts_with(name))
            {
                flush(&mut out, &stack, &mut buf);
                stack.push(RedlineKind::Added);
                index += name.len();
            } else if rest.starts_with("\\GwBold{") {
                stack.push(*stack.last().expect("栈非空"));
                index += "\\GwBold{".len();
            } else if let Some(group) = [KAI_GROUP, KAI_GROUP_FROZEN]
                .into_iter()
                .find(|group| rest.starts_with(group))
            {
                stack.push(*stack.last().expect("栈非空"));
                index += group.len();
            } else if let Some((ch, width)) = tex_unescape(&rest) {
                buf.push(ch);
                index += width;
            } else {
                match chars[index] {
                    '{' => stack.push(*stack.last().expect("栈非空")),
                    '}' => {
                        // 组关闭前先冲刷：组内文字属于即将弹出的这层类型。
                        flush(&mut out, &stack, &mut buf);
                        stack.pop();
                    }
                    other => buf.push(other),
                }
                index += 1;
            }
        }
        flush(&mut out, &stack, &mut buf);
        out
    }

    /// 反转义：返回 (字符, 消耗的字符数)。不是转义序列时返回 None。
    fn tex_unescape(rest: &str) -> Option<(char, usize)> {
        const ESCAPES: [(&str, char); 3] = [
            ("\\textasciitilde{}", '~'),
            ("\\textasciicircum{}", '^'),
            ("\\textbackslash{}", '\\'),
        ];
        for (prefix, ch) in ESCAPES {
            if rest.starts_with(prefix) {
                return Some((ch, prefix.chars().count()));
            }
        }
        let mut chars = rest.chars();
        if chars.next() == Some('\\')
            && let Some(next) = chars.next()
            && next.is_ascii_punctuation()
        {
            return Some((next, 2));
        }
        None
    }

    #[test]
    fn docx_runs_and_tex_fragments_carry_the_same_marks() {
        let old = concat!(
            "请于8月10日前报送材料，逾期视为放弃。",
            "\n\n各单位要高度重视，加强组织领导。",
            "\n\n- 第一项工作。",
            "\n\n- 第二项工作。",
            "\n\n<!-- [居中] -->\n（联系人：张三）",
            "\n\n本通知自印发之日起施行。",
        );
        let new = concat!(
            "请于8月15日前报送材料，逾期视为自动放弃。",
            "\n\n各单位务必高度重视，切实加强组织领导。",
            "\n\n- 第一项工作。",
            "\n\n- 第二项工作。",
            "\n\n- 新增第三项工作。",
            "\n\n<!-- [居中] -->\n（联系人：李四）",
            "\n\n本通知自印发之日起施行。",
        );
        let doc = build(old, new);
        let docx = docx_sequence(&doc.markdown);
        let tex = tex_sequence(&doc.markdown);
        assert_eq!(
            docx,
            tex,
            "DOCX run 序列与 TeX 片段序列必须一致\nmarkdown:\n{}\ndocx: {docx:?}\ntex:  {tex:?}",
            crate::export::strip_redline(&doc.markdown),
        );
        // 第三方：预览的排版任务（方案 4.3「预览 = PDF = Word」）。
        let preview = crate::preview::marks::body_sequence(&doc.markdown);
        assert_eq!(docx, preview, "预览片段序列必须与 DOCX 一致：{preview:?}");
        // 序列里确实带着标注（不是两边都退化成全 Same 的假一致）。
        assert!(
            docx.iter().any(|(_, kind)| *kind == RedlineKind::Deleted)
                && docx.iter().any(|(_, kind)| *kind == RedlineKind::Added),
            "序列应包含删与增两类标记：{docx:?}"
        );
        // 括号内容在两边都触发楷体分段，分类型仍须一致。
        assert!(
            docx.iter().any(|(text, _)| text.contains('（')),
            "括号片段在序列里：{docx:?}"
        );
    }

    #[test]
    fn aligned_blocks_share_the_sequence_too() {
        // 对齐行单独走一条断言路径：标记跨居中 / 居右区时两边一致。
        let doc = build(
            "<!-- [居中] -->\n旧的第一行\n<!-- [居右] -->\n旧的第二行",
            "<!-- [居中] -->\n新的第一行\n<!-- [居右] -->\n新的第二行",
        );
        let docx = docx_sequence(&doc.markdown);
        let tex = tex_sequence(&doc.markdown);
        assert_eq!(docx, tex, "对齐行的双端序列一致：{docx:?}");
        let preview = crate::preview::marks::body_sequence(&doc.markdown);
        assert_eq!(docx, preview, "对齐行的预览序列一致：{preview:?}");
    }
}

#[cfg(test)]
mod research_tests {
    //! 研究报告的花脸稿导出：Word / TeX 由 mdx 转换器生成，哨兵在产物落地后
    //! 由 `visual_diff::postprocess` 换成删除线 / 边框或 `\GwDel` / `\GwAdd` 宏。
    //! 这里验证转换结果真的带上了标记（PDF 编译走 texcompile 的既有测试）。

    use super::{RedlineFormats, build, export_files};
    use crate::models::{DraftInput, FontConfig, TemplateKind};
    use crate::units::UnitDisplay;

    fn research_input() -> DraftInput {
        let mut input = DraftInput {
            kind: TemplateKind::ResearchReport,
            title_hint: "测试报告".into(),
            ..Default::default()
        };
        input.research.institution = "测试单位".into();
        input
    }

    #[test]
    fn research_redline_docx_marks_deleted_and_added_runs() {
        let old = "<!-- [正文] -->\n\n## 研究背景\n\n由$x^{2}$可知，样本容量为8月。";
        let new = "<!-- [正文] -->\n\n## 研究背景\n\n由$x^{3}$可知，样本容量为9月。";
        let doc = build(old, new);
        assert!(!doc.is_empty());

        let dir = tempfile::tempdir().expect("临时目录");
        let files = export_files(
            dir.path(),
            &research_input(),
            &doc,
            RedlineFormats {
                pdf: false,
                docx: true,
            },
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &crate::models::NumberingConfig::default(),
        )
        .expect("研究报告花脸稿 Word 应导出成功");
        let path = files
            .iter()
            .find(|path| path.extension().is_some_and(|ext| ext == "docx"))
            .expect("有 docx");
        let bytes = std::fs::read(path).expect("读 docx");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("解包");
        use std::io::Read as _;
        let mut xml = String::new();
        archive
            .by_name("word/document.xml")
            .expect("document.xml 在")
            .read_to_string(&mut xml)
            .unwrap();
        assert!(xml.contains("<w:strike />"), "删除部分应有删除线：{xml}");
        assert!(
            xml.contains("<w:bdr w:val=\"single\""),
            "新增部分应有字符边框：{xml}"
        );
        for ch in ['\u{E000}', '\u{E001}', '\u{E002}', '\u{E003}'] {
            assert!(!xml.contains(ch), "哨兵不得残留：{xml}");
        }
    }

    #[test]
    fn research_redline_tex_swaps_sentinels_for_gw_macros() {
        let old = "<!-- [正文] -->\n\n## 研究背景\n\n由$x^{2}$可知。";
        let new = "<!-- [正文] -->\n\n## 研究背景\n\n由$x^{3}$可知，另见附件。";
        let doc = build(old, new);

        // 与 export_files 的 PDF 分支相同的前半段：转换 + 换宏。
        // 编译交给 texcompile 的既有测试，这里不真跑 Tectonic。
        let dir = tempfile::tempdir().expect("临时目录");
        let tex_path = dir.path().join("报告-花脸稿.tex");
        crate::export::write_tex_for_kind(
            &tex_path,
            &research_input(),
            &doc.markdown,
            &UnitDisplay::new(&[]),
            &FontConfig::default(),
            &crate::models::NumberingConfig::default(),
        )
        .expect("研究报告花脸稿 TeX 应生成成功");
        crate::visual_diff::redline_research_tex_files(dir.path()).expect("换宏");
        let main = std::fs::read_to_string(&tex_path).expect("读主 TeX");
        assert!(
            main.contains("\\providecommand{\\GwDel}"),
            "主 TeX 应注入花脸稿宏定义：{main}"
        );
        // 分章 TeX 里的哨兵应已换成宏。
        let chapter =
            std::fs::read_to_string(tex_path.parent().unwrap().join("data/chapter01.tex"))
                .expect("读分章 TeX");
        assert!(chapter.contains("\\GwDel{"), "分章应有删除宏：{chapter}");
        // 新增的一句里带公式：公式不能进 xeCJKfntef 的宏（编译失败），整体装盒
        // 画框（`\GwAddAtom`），文字部分走 `\GwAddLines`，首尾各一条竖边。
        assert!(
            chapter.contains("\\GwDelAtom{\\(x^{2}\\)}"),
            "删掉的公式整体标注：{chapter}"
        );
        assert!(
            chapter.contains("\\GwBoxBarL\\GwAddAtom{\\kern1.5pt\\(x^{3}\\)}")
                && chapter.contains("\\GwAddLines{可知，另见附件\\kern1.5pt}\\GwBoxBarR{}"),
            "分章应有新增宏：{chapter}"
        );
        for ch in ['\u{E000}', '\u{E001}', '\u{E002}', '\u{E003}'] {
            assert!(!chapter.contains(ch), "哨兵不得残留：{chapter}");
        }
    }
}
