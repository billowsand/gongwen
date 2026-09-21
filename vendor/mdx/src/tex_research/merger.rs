//! Markdown → LaTeX 转换（纯 Rust 实现，不依赖 pandoc）。
//!
//! 使用 parser::parse 进行 Markdown 解析，使用 TexResearchEmitter 生成 LaTeX。

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::common::front_matter::{self, Metadata};
use crate::parser;
use crate::tex_research_emitter::TexResearchEmitter;

use super::resources;

pub struct Merger {
    _placeholder: (),
}

type CoverInfo = Metadata;

impl Merger {
    pub fn new() -> Self {
        Self { _placeholder: () }
    }

    /// Full processing pipeline
    pub fn process(
        &mut self,
        input: &Path,
        is_dir: bool,
        user_template: Option<&Path>,
        output_tex: &Path,
        compile_pdf: bool,
    ) -> Result<()> {
        // 1. 读取并解析 markdown
        let (markdown_content, doc_title) = if is_dir {
            self.merge_markdown_files(input)?
        } else {
            self.read_markdown_file(input)?
        };

        // 2.5 剥离 front matter 封面字段（标题可被 front matter 覆盖）
        let (cover, markdown_content) = front_matter::parse(&markdown_content);
        let doc_title = cover.title.clone().or(doc_title);

        println!("正在解析 markdown...");
        let mut blocks = parser::parse(&markdown_content);

        let base_dir = if is_dir {
            input
        } else {
            input.parent().unwrap_or(Path::new("."))
        };
        let citations =
            crate::common::citation::validate(&blocks, cover.bibliography.as_deref(), base_dir)?;

        // 所有引用检查均在创建输出目录之前完成
        crate::common::crossref::check_or_bail(&blocks, crate::common::crossref::Support::Full)?;

        // 2. 校验完成后才能释放资源、复制 Bib 与图片
        let out_dir = output_tex.parent().unwrap_or(Path::new("."));
        resources::extract(out_dir).context("释放嵌入资源失败")?;
        if let Some(path) = citations.copy_to(out_dir)? {
            println!("  Bib 文件已复制到 {}", path.display());
        }
        for (_, dst) in crate::common::images::relocate(&mut blocks, base_dir, out_dir) {
            println!("  图片已复制到 {}", dst.display());
        }

        // 3. 使用 Rust emitter 生成 LaTeX
        println!("正在生成 LaTeX...");
        let mut emitter = TexResearchEmitter::new();
        emitter.emit_all(&blocks);

        // 如果有摘要，完成摘要收集
        emitter.finish_abstract();

        let (body, parts) = emitter.finish();

        // 4. 包装成完整文档
        let template = if let Some(path) = user_template {
            fs::read_to_string(path).with_context(|| format!("读取模板 {} 失败", path.display()))?
        } else {
            resources::TEMPLATE_TEX.to_string()
        };
        let tex = render_template(
            &template,
            &body,
            doc_title.as_deref(),
            &cover,
            citations.has_citations,
        );
        let tex = configure_bibliography(&tex, citations.has_citations);

        // 5. 写入输出文件（主文件 + data/ 与 appendix/ 分章部件）
        fs::write(output_tex, &tex)
            .with_context(|| format!("写入 {} 失败", output_tex.display()))?;
        crate::common::parts::write_parts(out_dir, &parts)
            .with_context(|| format!("写入分章部件到 {} 失败", out_dir.display()))?;

        if compile_pdf {
            crate::tex_compile::compile_pdf_if_available(output_tex)?;
        }

        println!("转换成功: {}", output_tex.display());
        Ok(())
    }

    /// 读取并合并目录中的所有 markdown 文件
    fn merge_markdown_files(&mut self, input_dir: &Path) -> Result<(String, Option<String>)> {
        let mut md_files: Vec<PathBuf> = WalkDir::new(input_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_type().is_file()
                    && e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext.eq_ignore_ascii_case("md"))
                        .unwrap_or(false)
            })
            .map(|e| e.path().to_path_buf())
            .collect();

        if md_files.is_empty() {
            anyhow::bail!("在目录 '{}' 中未找到任何 .md 文件", input_dir.display());
        }

        md_files.sort_by(|a, b| {
            let name_a = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let name_b = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name_a.cmp(name_b)
        });

        println!("找到 {} 个 markdown 文件:", md_files.len());
        for f in &md_files {
            println!(
                "  - {}",
                f.file_name().unwrap_or_default().to_string_lossy()
            );
        }

        let mut merged = String::new();
        let mut doc_title = None;

        for path in &md_files {
            println!(
                "处理文件: {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            let content = fs::read_to_string(path)
                .with_context(|| format!("读取文件 {} 失败", path.display()))?;
            let content = strip_utf8_bom(&content);

            // 提取标题（第一个 # 标题）
            if doc_title.is_none() {
                doc_title = extract_title_from_content(content);
                if doc_title.is_some() {
                    merged.push_str(&remove_first_h1(content));
                    merged.push_str("\n\n");
                    continue;
                }
            }

            merged.push_str(content);
            merged.push_str("\n\n");
        }

        Ok((merged, doc_title))
    }

    /// 读取单个 markdown 文件
    fn read_markdown_file(&mut self, input_file: &Path) -> Result<(String, Option<String>)> {
        println!("处理文件: {}", input_file.display());

        let content = fs::read_to_string(input_file)
            .with_context(|| format!("读取文件 {} 失败", input_file.display()))?;
        let content = strip_utf8_bom(&content);

        let doc_title = extract_title_from_content(content);

        // 移除第一个 # 标题行（因为它已经被提取为 title）
        let content_without_first_h1 = remove_first_h1(content);

        Ok((content_without_first_h1, doc_title))
    }
}

/// 移除文件开头的 UTF-8 BOM；正文内部的 U+FEFF 保持不变。
fn strip_utf8_bom(content: &str) -> &str {
    content.trim_start_matches('\u{feff}')
}

/// 从 markdown 内容中提取第一个 # 标题
fn extract_title_from_content(content: &str) -> Option<String> {
    let heading_regex = regex::Regex::new(r"^#\s+(.+?)(?:\s*\{[^}]*\})?\s*$").ok()?;

    for line in content.lines() {
        if let Some(caps) = heading_regex.captures(line) {
            let title = crate::common::heading::clean(caps.get(1)?.as_str().trim());
            if !title.is_empty() {
                println!("提取文档标题: {}", title);
                return Some(title);
            }
        }
    }
    None
}

/// 移除 markdown 内容中的第一个 # 标题行
fn remove_first_h1(content: &str) -> String {
    let heading_regex = regex::Regex::new(r"^#\s+.+\s*$").ok();

    if let Some(re) = heading_regex {
        let mut result = String::new();
        let mut first_found = false;

        for line in content.lines() {
            if !first_found && re.is_match(line) {
                first_found = true;
                continue; // 跳过第一行
            }
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(line);
        }
        result
    } else {
        content.to_string()
    }
}

/// 渲染 pandoc 风格的简单模板变量。
///
/// 这里不重新实现完整 pandoc template 语言，只覆盖当前内置模板与常见外部模板
/// 使用到的 `$body$`、`$title$`、`$if(title)$...$else$...$endif$`，以及封面字段
/// `$security$` / `$securityyears$` / `$doctype$` / `$docnumber$` / `$docversion$` /
/// `$institution$` / `$submitdate$`。
///
/// 密级与年限是两个独立变量，星号连接由模板负责（见 template.tex 的 `\ding{72}`）。
fn render_template(
    template: &str,
    body: &str,
    title: Option<&str>,
    cover: &CoverInfo,
    has_citations: bool,
) -> String {
    let escaped_title = title.map(escape_latex);
    let mut rendered = template.to_string();

    if let Ok(re) = regex::Regex::new(r"(?s)\$if\(title\)\$(.*?)\$else\$(.*?)\$endif\$") {
        rendered = re
            .replace_all(&rendered, |caps: &regex::Captures| {
                if escaped_title.is_some() {
                    caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string()
                } else {
                    caps.get(2).map(|m| m.as_str()).unwrap_or("").to_string()
                }
            })
            .to_string();
    }

    if let Ok(re) = regex::Regex::new(r"(?s)\$if\(bibliography\)\$(.*?)\$endif\$") {
        rendered = re
            .replace_all(&rendered, |caps: &regex::Captures| {
                if has_citations {
                    caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string()
                } else {
                    String::new()
                }
            })
            .to_string();
    }

    // 封面字段：缺省值与旧版模板保持一致；日期缺省用 LaTeX 当前年月（到月）
    let field = |value: Option<&String>, default: &str| {
        value
            .map(|v| escape_latex(v))
            .unwrap_or_else(|| default.to_string())
    };
    let date = cover
        .date
        .as_ref()
        .map(|d| escape_latex(&front_matter::normalize_date(d)))
        .unwrap_or_else(|| r"\the\year 年 \the\month 月".to_string());

    rendered = rendered.replace("$body$", body);
    rendered = rendered.replace("$title$", escaped_title.as_deref().unwrap_or(""));
    rendered = rendered.replace("$security$", &field(cover.security.as_ref(), "公开"));
    // 年限缺省为空：模板据此决定是否输出 "★年限"
    rendered = rendered.replace("$securityyears$", &field(cover.security_years.as_ref(), ""));
    rendered = rendered.replace("$doctype$", &field(cover.doc_type.as_ref(), "研究报告"));
    rendered = rendered.replace("$docnumber$", &field(cover.doc_number.as_ref(), ""));
    rendered = rendered.replace("$docversion$", &field(cover.version.as_ref(), "V1.0"));
    rendered = rendered.replace(
        "$institution$",
        &field(cover.institution.as_ref(), "某某单位"),
    );
    rendered = rendered.replace("$submitdate$", &date);
    rendered
}

fn escape_latex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '$' => out.push_str("\\$"),
            '#' => out.push_str("\\#"),
            '_' => out.push_str("\\_"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            other => out.push(other),
        }
    }
    out
}

fn configure_bibliography(content: &str, enabled: bool) -> String {
    let mut result = content.to_string();
    let nocite = regex::Regex::new(r"\\nocite\{\*\}").expect("invalid nocite cleanup pattern");
    result = nocite.replace_all(&result, "").to_string();
    if enabled {
        return result;
    }

    for pattern in [
        r"\\usepackage\[[^\]]*?\]\{biblatex\}\s*\n?",
        r"\\usepackage\{biblatex\}\s*\n?",
        r"\\addbibresource\{[^}]*\}\s*%?.*\n?",
        r"\\renewcommand\{\\bibname\}\{[^}]*\}\s*\n?",
        r"\\printbibliography\s*\n?",
        r"\\usepackage\{gbt7714\}\s*\n?",
        r"\\citestyle\{numbers\}\s*\n?",
        r"\\bibliography\{references\}\s*\n?",
    ] {
        let re = regex::Regex::new(pattern).expect("invalid bibliography cleanup pattern");
        result = re.replace_all(&result, "").to_string();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn template_renders_title_and_body() {
        let template = "$if(title)$T:$title$$else$UNTITLED$endif$\n$body$";
        let rendered = render_template(template, "BODY", Some("A&B"), &CoverInfo::default(), false);
        assert_eq!(rendered, "T:A\\&B\nBODY");
    }

    #[test]
    fn template_uses_else_without_title() {
        let template = "$if(title)$T:$title$$else$UNTITLED$endif$\n$body$";
        let rendered = render_template(template, "BODY", None, &CoverInfo::default(), false);
        assert_eq!(rendered, "UNTITLED\nBODY");
    }

    #[test]
    fn template_conditionally_renders_bibliography() {
        let template = "$if(bibliography)$BIB$endif$\n$body$";
        assert_eq!(
            render_template(template, "BODY", None, &CoverInfo::default(), true),
            "BIB\nBODY"
        );
        assert_eq!(
            render_template(template, "BODY", None, &CoverInfo::default(), false),
            "\nBODY"
        );
    }

    #[test]
    fn validation_failure_does_not_create_research_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("paper.md");
        let output = dir.path().join("out/report.tex");
        fs::write(
            &input,
            "---\nbibliography: missing.bib\n---\n# 标题\n正文 [@a]\n",
        )
        .unwrap();

        let err = Merger::new()
            .process(&input, false, None, &output, false)
            .unwrap_err();

        assert!(err.to_string().contains("missing.bib"));
        assert!(!output.exists());
        assert!(!output.parent().unwrap().exists());
    }

    #[test]
    fn configure_bibliography_removes_nocite_and_disabled_setup() {
        let input = "\\usepackage[style=gb7714-2015]{biblatex}\n\\addbibresource{references.bib}\n\\nocite{*}\n\\printbibliography\n";
        let enabled = configure_bibliography(input, true);
        assert!(enabled.contains("biblatex"));
        assert!(enabled.contains("printbibliography"));
        assert!(!enabled.contains("nocite"));

        let disabled = configure_bibliography(input, false);
        assert!(!disabled.contains("biblatex"));
        assert!(!disabled.contains("addbibresource"));
        assert!(!disabled.contains("printbibliography"));
        assert!(!disabled.contains("nocite"));
    }

    #[test]
    fn configure_bibliography_removes_inline_nocite() {
        let input = "\\AtBeginDocument{\\nocite{*}}\n";
        let configured = configure_bibliography(input, true);
        assert!(!configured.contains("\\nocite{*}"), "{configured}");
        assert_eq!(configured, "\\AtBeginDocument{}\n");
    }

    #[test]
    fn template_renders_cover_fields() {
        let template = "$security$|$doctype$|$docnumber$|$docversion$|$institution$|$submitdate$";
        let cover = CoverInfo {
            security: Some("内部".into()),
            security_years: None,
            doc_type: Some("技术报告".into()),
            doc_number: Some("XX-2026-001".into()),
            version: Some("V2.1".into()),
            institution: Some("某研究所".into()),
            date: Some("2026-07".into()),
            title: None,
            bibliography: None,
        };
        let rendered = render_template(template, "", None, &cover, false);
        assert_eq!(
            rendered,
            "内部|技术报告|XX-2026-001|V2.1|某研究所|2026 年 7 月"
        );
    }

    #[test]
    fn template_cover_defaults() {
        let template = "$security$|$doctype$|$docnumber$|$docversion$|$institution$|$submitdate$";
        let rendered = render_template(template, "", None, &CoverInfo::default(), false);
        assert_eq!(
            rendered,
            r"公开|研究报告||V1.0|某某单位|\the\year 年 \the\month 月"
        );
    }

    #[test]
    fn template_renders_security_years_separately() {
        let template = "$security$/$securityyears$";
        let cover = CoverInfo {
            security: Some("机密".into()),
            security_years: Some("5年".into()),
            ..CoverInfo::default()
        };
        let rendered = render_template(template, "", None, &cover, false);
        assert_eq!(rendered, "机密/5年");
    }

    #[test]
    fn template_security_years_defaults_to_empty() {
        // 年限缺省为空串，模板据此不输出五角星
        let template = "$security$/$securityyears$";
        let rendered = render_template(template, "", None, &CoverInfo::default(), false);
        assert_eq!(rendered, "公开/");
    }

    #[test]
    fn builtin_template_joins_security_and_years_with_ding() {
        // 内置模板：密级与年限之间使用 pifont 的 \ding{72}
        let cover = CoverInfo {
            security: Some("机密".into()),
            security_years: Some("5年".into()),
            ..CoverInfo::default()
        };
        let rendered = render_template(resources::TEMPLATE_TEX, "", None, &cover, false);
        assert!(rendered.contains("\\ding{72}"), "{rendered}");
        assert!(
            rendered.contains("\\newcommand{\\securityyears}{5年}"),
            "{rendered}"
        );
    }

    #[test]
    fn parses_front_matter_cover_fields() {
        let md = "---\n密级: 内部\n文件类型：技术报告\n文件编号: XX-2026-001\n版本: V2.1\n撰写单位: 某研究所\n撰写时间: 2026-07\n---\n\n# 标题\n\n正文\n";
        let (cover, body) = front_matter::parse(md);
        assert_eq!(cover.security.as_deref(), Some("内部"));
        assert_eq!(cover.doc_type.as_deref(), Some("技术报告"));
        assert_eq!(cover.doc_number.as_deref(), Some("XX-2026-001"));
        assert_eq!(cover.version.as_deref(), Some("V2.1"));
        assert_eq!(cover.institution.as_deref(), Some("某研究所"));
        assert_eq!(cover.date.as_deref(), Some("2026-07"));
        assert!(body.starts_with("# 标题") || body.starts_with("\n# 标题"));
        assert!(!body.contains("密级"));
    }

    #[test]
    fn unclosed_front_matter_left_untouched() {
        let md = "---\n密级: 内部\n\n正文\n";
        let (cover, body) = front_matter::parse(md);
        assert!(cover.security.is_none());
        assert_eq!(body, md);
    }

    #[test]
    fn normalizes_cover_date_to_month() {
        assert_eq!(front_matter::normalize_date("2026-07"), "2026 年 7 月");
        assert_eq!(front_matter::normalize_date("2026/7"), "2026 年 7 月");
        assert_eq!(front_matter::normalize_date("2026年07月"), "2026 年 7 月");
        assert_eq!(
            front_matter::normalize_date("二〇二六年七月"),
            "二〇二六年七月"
        );
    }

    #[test]
    fn directory_merge_removes_first_h1_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("01.md"), "# 一、测试报告\n\n## 背景\n正文").unwrap();
        fs::write(dir.path().join("02.md"), "# 第二章 后续\n内容").unwrap();

        let mut merger = Merger::new();
        let (merged, title) = merger.merge_markdown_files(dir.path()).expect("merge");
        assert_eq!(title.as_deref(), Some("测试报告"));
        assert!(!merged.contains("# 一、测试报告"));
        assert!(merged.contains("# 第二章 后续"));
    }

    #[test]
    fn directory_merge_strips_bom_from_each_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("01.md"), "# 测试报告\n\n## 第一章 概述\n").unwrap();
        fs::write(
            dir.path().join("02.md"),
            "\u{feff}## 第三章 感知类研制任务\n\n正文\n",
        )
        .unwrap();

        let mut merger = Merger::new();
        let (merged, _) = merger.merge_markdown_files(dir.path()).expect("merge");
        let blocks = parser::parse(&merged);
        let mut emitter = TexResearchEmitter::new();
        emitter.emit_all(&blocks);
        let (main, parts) = emitter.finish();
        let body = main + &parts.into_iter().map(|(_, c)| c).collect::<String>();

        assert!(body.contains("\\chapter{感知类研制任务}"), "{body}");
        assert!(!body.contains("\\#\\# 第三章"), "{body}");
    }
}
