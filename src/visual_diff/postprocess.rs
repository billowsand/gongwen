//! 研究报告的花脸稿后处理：研究报告的 TeX / Word 由 mdx 转换器生成，
//! 哨兵字符原样穿过转换（mdx 不做私用区过滤），所以在产物落地后把哨兵对
//! 换成 `\GwDel` / `\GwAdd` 宏（TeX）或删除线 / 字符边框 run（OOXML）。
//!
//! 宏定义注入主 TeX 的导言区；md2tex.cls 已加载 xcolor，xeCJKfntef 在
//! 离线 texbundle 里（公文类文件已依赖），这里按需补加载。

use crate::export::{
    REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN, RedlineKind,
};
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 删除标记色（方案第八节）：红 #C00000。
pub(crate) const DEL_COLOR: &str = "C00000";
/// 新增标记色：蓝 #1F4E9E。
pub(crate) const ADD_COLOR: &str = "1F4E9E";

/// 注入主 TeX 导言区的花脸稿宏定义（mdx 的 md2tex.cls 不认公文类的宏）。
pub(crate) const REDLINE_PREAMBLE_TEX: &str = concat!(
    "% gongwen 花脸稿标记（视觉 diff 引擎注入）\n",
    "\\makeatletter\n",
    "\\@ifpackageloaded{xeCJKfntef}{}{\\RequirePackage{xeCJKfntef}}\n",
    "\\makeatother\n",
    "\\providecolor{GwaDelColor}{HTML}{C00000}\n",
    "\\providecolor{GwaAddColor}{HTML}{1F4E9E}\n",
    "\\providecommand{\\GwWaveDepth}{-0.5em}\n",
    "\\providecommand{\\GwStrikeUnit}{\\hbox{\\color{GwaDelColor}\\rule{0.34em}{0.6pt}}}\n",
    "\\providecommand{\\GwDel}[1]{\\textcolor{GwaDelColor}{\\CJKunderwave[symbol=\\GwStrikeUnit, depth=\\GwWaveDepth]{#1}}}\n",
    "\\providecommand{\\GwBoxTop}{0.96em}\n",
    "\\providecommand{\\GwBoxBottom}{0.24em}\n",
    "\\providecommand{\\GwBoxBar}{\\textcolor{GwaAddColor}{\\rule[-\\GwBoxBottom]{0.5pt}{\\dimexpr\\GwBoxTop+\\GwBoxBottom\\relax}}}\n",
    "\\providecommand{\\GwAddLines}[1]{\\CJKunderdblline[depth=-\\GwBoxTop, gap=\\dimexpr\\GwBoxTop+\\GwBoxBottom-0.5pt\\relax, thickness=0.5pt, skip=false, format=\\color{GwaAddColor}]{#1}}\n",
    "\\providecommand{\\GwAdd}[1]{\\GwBoxBar\\GwAddLines{\\hspace{1.5pt}#1\\hspace{1.5pt}}\\GwBoxBar}\n",
);

/// 把一段 TeX 源码里的哨兵对换成 `\GwDel{...}` / `\GwAdd{...}`。
/// 未配对的哨兵直接剥掉（防御：宁可丢标记也不把私用区码位漏进纸面），
/// 哨兵里的文字始终原样保留。
pub(crate) fn inject_tex_redline(tex: &str) -> String {
    let mut out = String::with_capacity(tex.len());
    let mut buffer = String::new();
    let mut state = RedlineKind::Same;
    for ch in tex.chars() {
        match ch {
            REDLINE_DEL_OPEN | REDLINE_ADD_OPEN => {
                if state != RedlineKind::Same {
                    continue; // 嵌套起始哨兵：丢弃
                }
                out.push_str(&buffer);
                buffer.clear();
                state = if ch == REDLINE_DEL_OPEN {
                    RedlineKind::Deleted
                } else {
                    RedlineKind::Added
                };
            }
            REDLINE_DEL_CLOSE | REDLINE_ADD_CLOSE => {
                if state == RedlineKind::Same {
                    continue; // 孤立的收尾哨兵：丢弃
                }
                if state == RedlineKind::Deleted {
                    out.push_str(&wrap_tex_macro("GwDel", &buffer));
                } else {
                    // 新增框能随正文断行（与公文侧同一个 \GwAdd 定义），整段一个框。
                    out.push_str(&wrap_tex_macro("GwAdd", &buffer));
                }
                buffer.clear();
                state = RedlineKind::Same;
            }
            other => buffer.push(other),
        }
    }
    out.push_str(&buffer);
    out
}

/// 把内容包进花脸稿宏。内容里 `{` `}` 若不平衡（哨兵跨过 `\textbf{` 这类
/// 命令的花括号时会发生），在宏内补齐到平衡、宏外补回对应的另一半——
/// 宁可标记范围不准也不让 TeX 编译挂在不配对的括号上。
fn wrap_tex_macro(macro_name: &str, content: &str) -> String {
    let opens = content.chars().filter(|ch| *ch == '{').count();
    let closes = content.chars().filter(|ch| *ch == '}').count();
    if opens == closes {
        return format!("\\{macro_name}{{{content}}}");
    }
    if opens > closes {
        let pad = opens - closes;
        format!(
            "\\{macro_name}{{{content}{}}}{}",
            "}".repeat(pad),
            "{".repeat(pad)
        )
    } else {
        let pad = closes - opens;
        let wrapped = format!("\\{macro_name}{{{content}}}");
        format!("{}{wrapped}{}", "{".repeat(pad), "}".repeat(pad))
    }
}

/// 研究报告导出目录里所有 TeX（主文件 + 分章 + 附录）的哨兵换宏，
/// 并在主文件导言区注入宏定义。返回处理过的文件。
pub(crate) fn redline_research_tex_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_tex(dir, &mut files);
    let mut touched = Vec::new();
    for path in files {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("无法读取研究报告 TeX：{}", path.display()))?;
        let has_sentinels = content.contains([
            REDLINE_DEL_OPEN,
            REDLINE_DEL_CLOSE,
            REDLINE_ADD_OPEN,
            REDLINE_ADD_CLOSE,
        ]);
        // 主文档（含 \documentclass）需要注入宏定义，无论它自己是否带哨兵
        // （哨兵只落在分章 / 附录里）；分章文件只在自己带哨兵时改写。
        let is_main = content.contains("\\documentclass");
        if !has_sentinels && !is_main {
            continue;
        }
        let mut content = inject_tex_redline(&content);
        if is_main && !content.contains("gongwen 花脸稿标记") {
            content = inject_preamble(&content);
        }
        fs::write(&path, content)
            .with_context(|| format!("无法写回研究报告 TeX：{}", path.display()))?;
        touched.push(path);
    }
    Ok(touched)
}

fn collect_tex(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tex(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "tex") {
            files.push(path);
        }
    }
}

/// 在 `\documentclass` 行之后插入导言区定义。
fn inject_preamble(tex: &str) -> String {
    let marker = "\\begin{document}";
    if let Some(at) = tex.find(marker) {
        format!("{}{}{}", &tex[..at], REDLINE_PREAMBLE_TEX, &tex[at..])
    } else {
        format!("{REDLINE_PREAMBLE_TEX}{tex}")
    }
}

/// 研究报告 Word（mdx 生成的 docx）：把 document.xml 里带哨兵的 run
/// 文本按哨兵切开，删除部分加红色删除线、新增部分加蓝色字符边框。
/// 哨兵可以跨 run（标记跨过加粗 / 公式等格式边界时），状态在 run 间延续。
pub(crate) fn redline_research_docx(path: &Path) -> Result<()> {
    let file = fs::File::open(path)
        .with_context(|| format!("无法打开研究报告 Word：{}", path.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("无法解包：{}", path.display()))?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let mut document_xml: Option<String> = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        if name == "word/document.xml" {
            document_xml = Some(String::from_utf8_lossy(&bytes).to_string());
        } else {
            entries.push((name, bytes));
        }
    }
    let Some(xml) = document_xml else {
        anyhow::bail!("研究报告 Word 缺少 word/document.xml");
    };
    let transformed = inject_ooxml_redline(&xml);
    let temp = path.with_extension("redline-tmp");
    {
        let out = fs::File::create(&temp)?;
        let mut writer = zip::ZipWriter::new(out);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("word/document.xml", options)?;
        writer.write_all(transformed.as_bytes())?;
        for (name, bytes) in entries {
            writer.start_file(name, options)?;
            writer.write_all(&bytes)?;
        }
        writer.finish()?;
    }
    fs::rename(&temp, path)
        .with_context(|| format!("无法写回研究报告 Word：{}", path.display()))?;
    Ok(())
}

/// OOXML run 级注入（纯字符串手术，mdx 的 document.xml 是规整的机器输出）。
/// 只处理「rPr + 单个 w:t」的 run；其余 run 原样穿过（哨兵状态照常推进）。
pub(crate) fn inject_ooxml_redline(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    let mut state = RedlineKind::Same;
    while let Some(start) = rest.find("<w:r>") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + "<w:r>".len()..];
        let end = after_open.find("</w:r>").map(|at| at + "</w:r>".len());
        let (run_xml, tail_at) = match end {
            Some(end) => (
                &rest[start..start + "<w:r>".len() + end],
                start + "<w:r>".len() + end,
            ),
            None => (rest[start..].trim_end(), rest.len()),
        };
        out.push_str(&rewrite_run(run_xml, &mut state));
        rest = &rest[tail_at..];
    }
    out.push_str(rest);
    out
}

fn rewrite_run(run: &str, state: &mut RedlineKind) -> String {
    let Some(text_start) = run.find("<w:t") else {
        return run.to_string();
    };
    let text_open_end = run[text_start..]
        .find('>')
        .map(|at| text_start + at + 1)
        .expect("w:t 有闭合尖括号");
    let Some(text_close) = run[text_open_end..].find("</w:t>") else {
        return run.to_string();
    };
    let text = &run[text_open_end..text_open_end + text_close];
    if !text.contains([
        REDLINE_DEL_OPEN,
        REDLINE_DEL_CLOSE,
        REDLINE_ADD_OPEN,
        REDLINE_ADD_CLOSE,
    ]) {
        return run.to_string();
    }
    // rPr 原文（可能为空），以及标记属性的插入点。
    let rpr = run.find("<w:rPr>").and_then(|open| {
        run[open..]
            .find("</w:rPr>")
            .map(|close| (open, open + close + "</w:rPr>".len()))
    });
    let rpr_inner: String = match rpr {
        Some((open, close)) => run[open + "<w:rPr>".len()..close - "</w:rPr>".len()].to_string(),
        None => String::new(),
    };
    let mut out = String::new();
    for (piece, kind) in split_by_sentinels(text, state) {
        if piece.is_empty() {
            continue;
        }
        let extra = match kind {
            RedlineKind::Same => String::new(),
            RedlineKind::Deleted => {
                "<w:strike /><w:color w:val=\"".to_string() + DEL_COLOR + "\" />"
            }
            RedlineKind::Added => {
                "<w:bdr w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"".to_string()
                    + ADD_COLOR
                    + "\" />"
            }
        };
        out.push_str(&format!(
            "<w:r><w:rPr>{rpr_inner}{extra}</w:rPr><w:t xml:space=\"preserve\">{piece}</w:t></w:r>"
        ));
    }
    out
}

/// 按哨兵切文本；`state` 记录跨 run 的未闭合标记。
fn split_by_sentinels(text: &str, state: &mut RedlineKind) -> Vec<(String, RedlineKind)> {
    let mut pieces = Vec::new();
    let mut buffer = String::new();
    for ch in text.chars() {
        match ch {
            REDLINE_DEL_OPEN | REDLINE_ADD_OPEN => {
                if !buffer.is_empty() || !matches!(*state, RedlineKind::Same) {
                    pieces.push((std::mem::take(&mut buffer), *state));
                }
                *state = if ch == REDLINE_DEL_OPEN {
                    RedlineKind::Deleted
                } else {
                    RedlineKind::Added
                };
            }
            REDLINE_DEL_CLOSE | REDLINE_ADD_CLOSE => {
                pieces.push((std::mem::take(&mut buffer), *state));
                *state = RedlineKind::Same;
            }
            other => buffer.push(other),
        }
    }
    if !buffer.is_empty() {
        pieces.push((buffer, *state));
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tex_sentinel_pairs_become_gw_macros() {
        let input = format!(
            "正文 {REDLINE_DEL_OPEN}删掉{REDLINE_DEL_CLOSE}与{REDLINE_ADD_OPEN}新增{REDLINE_ADD_CLOSE}。"
        );
        let out = inject_tex_redline(&input);
        assert_eq!(
            out, "正文 \\GwDel{删掉}与\\GwAdd{新增}。",
            "哨兵对换成宏：{out}"
        );
    }

    #[test]
    fn an_unpaired_tex_sentinel_is_dropped() {
        let input = format!("正文{REDLINE_DEL_OPEN}没了下文。");
        let out = inject_tex_redline(&input);
        assert_eq!(out, "正文没了下文。", "未配对哨兵剥掉：{out}");
    }

    #[test]
    fn preamble_is_inserted_before_begin_document() {
        let tex = "\\documentclass{md2tex}\n\\begin{document}\n正文\n\\end{document}\n";
        let out = inject_preamble(tex);
        let at = out.find("\\begin{document}").unwrap();
        assert!(out[..at].contains("\\providecommand{\\GwDel}"));
        assert!(out.contains("\\documentclass{md2tex}"));
    }

    #[test]
    fn ooxml_runs_are_split_by_sentinel() {
        let xml = format!(
            "<w:p><w:r><w:rPr><w:rFonts w:ascii=\"仿宋\" /></w:rPr><w:t xml:space=\"preserve\">正文{REDLINE_DEL_OPEN}删掉{REDLINE_DEL_CLOSE}完。</w:t></w:r></w:p>"
        );
        let out = inject_ooxml_redline(&xml);
        assert!(out.contains("<w:strike />"), "删除部分加删除线：{out}");
        assert!(
            out.contains("<w:color w:val=\"C00000\" />"),
            "删除为红色：{out}"
        );
        assert!(
            out.contains("<w:t xml:space=\"preserve\">正文</w:t>"),
            "同部分原样：{out}"
        );
        assert!(!out.contains(REDLINE_DEL_OPEN), "哨兵不得残留：{out}");
    }

    #[test]
    fn ooxml_marks_carry_across_runs() {
        // 标记跨过两个 run（加粗边界）：状态在 run 间延续。
        let xml = format!(
            "<w:p><w:r><w:rPr><w:b /></w:rPr><w:t>前段{REDLINE_ADD_OPEN}</w:t></w:r><w:r><w:rPr /><w:t>后段{REDLINE_ADD_CLOSE}尾。</w:t></w:r></w:p>"
        );
        let out = inject_ooxml_redline(&xml);
        // 标记从第一个 run 的尾部开启：「前段」仍是普通文字，「后段」带框，
        // 收尾后的「尾。」恢复普通文字。
        assert_eq!(out.matches("<w:bdr ").count(), 1, "只有后段带边框：{out}");
        assert!(
            out.contains("<w:t xml:space=\"preserve\">前段</w:t>"),
            "前段原样：{out}"
        );
        assert!(out.contains("尾。"), "收尾文字保留：{out}");
        assert!(
            !out.contains(REDLINE_ADD_OPEN) && !out.contains(REDLINE_ADD_CLOSE),
            "哨兵不得残留：{out}"
        );
    }

    #[test]
    fn tex_marks_across_textbf_stay_brace_balanced() {
        // 哨兵跨过 \textbf{ 这类命令的花括号：宏内补齐到平衡、宏外补回，
        // 编译不会被不配对的括号卡住。
        let input = format!("前 {REDLINE_DEL_OPEN}\\textbf{{重点}}文字{REDLINE_DEL_CLOSE} 后");
        let out = inject_tex_redline(&input);
        let opens = out.chars().filter(|ch| *ch == '{').count();
        let closes = out.chars().filter(|ch| *ch == '}').count();
        assert_eq!(opens, closes, "花括号必须平衡：{out}");
        assert!(out.contains("\\GwDel{"), "删除宏在：{out}");
        assert!(!out.contains(REDLINE_DEL_OPEN), "哨兵不得残留：{out}");
    }

    #[test]
    fn tex_long_addition_stays_one_breakable_box() {
        // \GwAdd 能随正文断行，长插入整段一个框，不再按标点切成多个框。
        let long = "这是第一句，这是第二句，这是第三句";
        let input = format!("{REDLINE_ADD_OPEN}{long}{REDLINE_ADD_CLOSE}");
        let out = inject_tex_redline(&input);
        assert_eq!(out, format!("\\GwAdd{{{long}}}"));
    }

    #[test]
    fn ooxml_without_sentinels_passes_through() {
        let xml = "<w:p><w:r><w:rPr /><w:t>干净文字。</w:t></w:r></w:p>";
        assert_eq!(inject_ooxml_redline(xml), xml);
    }
}
