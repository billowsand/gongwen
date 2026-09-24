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
///
/// 除了与公文类文件同一套 `\GwDel` / `\GwAdd`，研究报告还有公式、交叉引用、
/// 文献引用这类不能放进 xeCJKfntef 宏里的「原子」（`\(…\)` 进了逐字处理的宏
/// 直接编译失败），它们整体装盒后另画标记：
/// - `\GwDelAtom`：红色，盒子竖直中线画一道删除线；
/// - `\GwAddAtom`：只画上下边，与两侧文字的新增框接成一个框（竖边由首尾的
///   `\GwBoxBar` 画）；内容比框高时上下边让开；
/// - `\GwAddDisplay`：独立公式整块套框。
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
    "\\ifdefined\\GwAtomBox\\else\\newsavebox{\\GwAtomBox}\\fi\n",
    "\\ifdefined\\GwAtomTop\\else\\newdimen\\GwAtomTop\\fi\n",
    "\\ifdefined\\GwAtomBottom\\else\\newdimen\\GwAtomBottom\\fi\n",
    "\\providecommand{\\GwDelAtom}[1]{\\sbox\\GwAtomBox{\\textcolor{GwaDelColor}{#1}}",
    "\\rlap{\\textcolor{GwaDelColor}{\\rule[\\dimexpr(\\ht\\GwAtomBox-\\dp\\GwAtomBox)/2-0.3pt\\relax]{\\wd\\GwAtomBox}{0.6pt}}}",
    "\\usebox\\GwAtomBox}\n",
    "\\providecommand{\\GwAddAtom}[1]{\\sbox\\GwAtomBox{#1}",
    "\\GwAtomTop=\\dimexpr\\GwBoxTop\\relax",
    "\\ifdim\\dimexpr\\ht\\GwAtomBox+1pt\\relax>\\GwAtomTop\\GwAtomTop=\\dimexpr\\ht\\GwAtomBox+1pt\\relax\\fi",
    "\\GwAtomBottom=\\dimexpr\\GwBoxBottom\\relax",
    "\\ifdim\\dimexpr\\dp\\GwAtomBox+1pt\\relax>\\GwAtomBottom\\GwAtomBottom=\\dimexpr\\dp\\GwAtomBox+1pt\\relax\\fi",
    "\\rlap{\\textcolor{GwaAddColor}{\\rule[-\\GwAtomBottom]{\\wd\\GwAtomBox}{0.5pt}}}",
    "\\rlap{\\textcolor{GwaAddColor}{\\rule[\\dimexpr\\GwAtomTop-0.5pt\\relax]{\\wd\\GwAtomBox}{0.5pt}}}",
    "\\usebox\\GwAtomBox}\n",
    "\\providecommand{\\GwAddDisplay}[1]{{\\color{GwaAddColor}\\fboxrule=0.5pt\\fboxsep=3pt",
    "\\fbox{\\normalcolor$\\displaystyle #1$}}}\n",
);

/// 把一段 TeX 源码里的哨兵对换成花脸稿宏。
///
/// 标记里的内容先按顶层切段（[`segments`]）：加粗 / 斜体这类样式命令包在宏
/// **外面**（xeCJKfntef 的宏里字体切换只作用到第一个字，与公文类文件同一条
/// 规则），公式、引用等原子整体另画；只有纯文字时仍是一个 `\GwDel{…}` /
/// `\GwAdd{…}`。独立公式（`\[…\]`）里的哨兵换成整块标注。
/// 未配对的哨兵直接剥掉（防御：宁可丢标记也不把私用区码位漏进纸面），
/// 哨兵里的文字始终原样保留。
pub(crate) fn inject_tex_redline(tex: &str) -> String {
    let mut out = String::with_capacity(tex.len());
    let mut buffer = String::new();
    let mut state = RedlineKind::Same;
    let mut in_display = false;
    let mut chars = tex.chars();
    while let Some(ch) = chars.next() {
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
                if in_display {
                    out.push_str(&mark_display_math(state, &buffer));
                } else {
                    out.push_str(&mark_tex_span(state, &buffer));
                }
                buffer.clear();
                state = RedlineKind::Same;
            }
            '\\' => {
                // 控制序列整个吃掉：`\\[2pt]` 这类换行不能误认成 `\[`。
                buffer.push('\\');
                if let Some(next) = chars.next() {
                    buffer.push(next);
                    if state == RedlineKind::Same {
                        match next {
                            '[' => in_display = true,
                            ']' => in_display = false,
                            _ => {}
                        }
                    }
                }
            }
            other => buffer.push(other),
        }
    }
    out.push_str(&buffer);
    out
}

/// 独立公式里的标注：整块。删除 = 红色 + 中线；新增 = 套框。
fn mark_display_math(kind: RedlineKind, content: &str) -> String {
    let content = content.trim();
    match kind {
        RedlineKind::Deleted => format!("\\GwDelAtom{{$\\displaystyle {content}$}}"),
        RedlineKind::Added => format!("\\GwAddDisplay{{{content}}}"),
        RedlineKind::Same => content.to_string(),
    }
}

/// 标记内容的一段（顶层切分）。
#[derive(Debug, PartialEq)]
enum Segment<'a> {
    /// 普通文字（含 `\%`、`\textasciicircum{}` 这类转义）。
    Text(&'a str),
    /// 样式命令：`open` 是 `\textbf{` 这样的开头，`inner` 是组内再切的段。
    Styled {
        open: &'a str,
        inner: Vec<Segment<'a>>,
    },
    /// 不能进 xeCJKfntef 宏的整体：公式、交叉引用、文献引用、链接。
    Atom(&'a str),
    /// 脚注：装进盒子里会丢，原样放在标记外面。
    Raw(&'a str),
}

/// 包在宏外面的样式命令。
const STYLE_COMMANDS: [&str; 4] = ["\\textbf{", "\\textit{", "\\texttt{", "\\emph{"];
/// 原子命令与其参数组个数。
const ATOM_COMMANDS: [(&str, usize); 5] = [
    ("\\ref{", 1),
    ("\\eqref{", 1),
    ("\\cite{", 1),
    ("\\url{", 1),
    ("\\href{", 2),
];

/// 按顶层切段。花括号不平衡（哨兵跨过了命令的括号）或公式没闭合时返回 None，
/// 调用方退回整段一个宏的老办法。
fn segments<'a>(text: &'a str) -> Option<Vec<Segment<'a>>> {
    let mut out = Vec::new();
    let mut text_start = 0usize;
    let mut index = 0usize;
    let flush = |out: &mut Vec<Segment<'a>>, from: usize, to: usize| {
        if from < to {
            out.push(Segment::Text(&text[from..to]));
        }
    };
    while index < text.len() {
        let rest = &text[index..];
        if let Some(inner) = rest.strip_prefix("\\(") {
            let end = index + 2 + inner.find("\\)")? + 2;
            flush(&mut out, text_start, index);
            out.push(Segment::Atom(&text[index..end]));
            index = end;
            text_start = end;
        } else if let Some(open) = STYLE_COMMANDS.iter().find(|open| rest.starts_with(**open)) {
            let body = index + open.len();
            let close = group_end(text, body - 1)?;
            flush(&mut out, text_start, index);
            out.push(Segment::Styled {
                open: &text[index..body],
                inner: segments(&text[body..close])?,
            });
            index = close + 1;
            text_start = index;
        } else if let Some((command, groups)) = ATOM_COMMANDS
            .iter()
            .find(|(command, _)| rest.starts_with(*command))
        {
            let mut end = index + command.len() - 1;
            for group in 0..*groups {
                if group > 0 && !text[end..].starts_with('{') {
                    return None;
                }
                end = group_end(text, end)? + 1;
            }
            flush(&mut out, text_start, index);
            out.push(Segment::Atom(&text[index..end]));
            index = end;
            text_start = end;
        } else if rest.starts_with("\\footnote{") {
            let end = group_end(text, index + "\\footnote".len())? + 1;
            flush(&mut out, text_start, index);
            out.push(Segment::Raw(&text[index..end]));
            index = end;
            text_start = end;
        } else if let Some(after) = rest.strip_prefix('\\') {
            // 其余控制序列（转义、无参命令）算普通文字：连同后一个字符一起跳过。
            index += 1 + after.chars().next().map_or(0, char::len_utf8);
        } else if rest.starts_with('{') {
            // 普通分组（如 `\textasciicircum{}` 的 `{}`）整体算文字。
            index = group_end(text, index)? + 1;
        } else if rest.starts_with('}') {
            return None;
        } else {
            index += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    flush(&mut out, text_start, text.len());
    Some(out)
}

/// `open` 处的 `{` 对应的 `}` 的位置（跳过 `\{` `\}`）。
fn group_end(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => index += 1,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// 一段带标记的 TeX：纯文字时一个宏；否则按段各自标注，样式在宏外。
fn mark_tex_span(kind: RedlineKind, content: &str) -> String {
    let macro_name = if kind == RedlineKind::Deleted {
        "GwDel"
    } else {
        "GwAdd"
    };
    let Some(parts) = segments(content) else {
        return wrap_tex_macro(macro_name, content);
    };
    if parts.iter().all(|part| matches!(part, Segment::Text(_))) {
        return wrap_tex_macro(macro_name, content);
    }
    let leaves = count_leaves(&parts);
    let mut out = String::new();
    let mut seen = 0usize;
    if kind == RedlineKind::Added {
        out.push_str("\\GwBoxBar");
    }
    render_segments(&parts, kind, leaves, &mut seen, &mut out);
    if kind == RedlineKind::Added {
        // 收尾的 `{}` 不能省：XeTeX 里汉字是字母，`\GwBoxBar为` 会被读成
        // 一个叫「GwBoxBar为」的控制序列。
        out.push_str("\\GwBoxBar{}");
    }
    out
}

fn count_leaves(parts: &[Segment<'_>]) -> usize {
    parts
        .iter()
        .map(|part| match part {
            Segment::Text(_) | Segment::Atom(_) => 1,
            Segment::Styled { inner, .. } => count_leaves(inner),
            Segment::Raw(_) => 0,
        })
        .sum()
}

/// 逐段写宏。新增框的左右留白只加在第一段 / 最后一段里（与 `\GwAdd` 一致）。
fn render_segments(
    parts: &[Segment<'_>],
    kind: RedlineKind,
    leaves: usize,
    seen: &mut usize,
    out: &mut String,
) {
    for part in parts {
        match part {
            Segment::Styled { open, inner } => {
                out.push_str(open);
                render_segments(inner, kind, leaves, seen, out);
                out.push('}');
            }
            Segment::Raw(raw) => out.push_str(raw),
            Segment::Text(content) | Segment::Atom(content) => {
                let first = *seen == 0;
                *seen += 1;
                let last = *seen == leaves;
                let atom = matches!(part, Segment::Atom(_));
                if kind == RedlineKind::Deleted {
                    let name = if atom { "GwDelAtom" } else { "GwDel" };
                    out.push_str(&format!("\\{name}{{{content}}}"));
                    continue;
                }
                let pad = "\\hspace{1.5pt}";
                let body = format!(
                    "{}{content}{}",
                    if first { pad } else { "" },
                    if last { pad } else { "" }
                );
                let name = if atom { "GwAddAtom" } else { "GwAddLines" };
                out.push_str(&format!("\\{name}{{{body}}}"));
            }
        }
    }
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
        // 夹在一对哨兵中间、自己不带哨兵的 run（标记跨过加粗 / 公式时的中段）
        // 整个带上标记；从前原样穿过，一段新增只有首尾两个 run 有框。
        return match state {
            RedlineKind::Same => run.to_string(),
            kind => with_run_props(run, &mark_props(*kind)),
        };
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
        let extra = mark_props(kind);
        out.push_str(&format!(
            "<w:r><w:rPr>{rpr_inner}{extra}</w:rPr><w:t xml:space=\"preserve\">{piece}</w:t></w:r>"
        ));
    }
    out
}

/// 标记对应的 run 属性：删除 = 删除线 + 红字，新增 = 蓝色字符边框。
fn mark_props(kind: RedlineKind) -> String {
    match kind {
        RedlineKind::Same => String::new(),
        RedlineKind::Deleted => format!("<w:strike /><w:color w:val=\"{DEL_COLOR}\" />"),
        RedlineKind::Added => {
            format!("<w:bdr w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"{ADD_COLOR}\" />")
        }
    }
}

/// 给整个 run 追加属性，run 里的其余内容原样保留。
fn with_run_props(run: &str, props: &str) -> String {
    if let Some(close) = run.find("</w:rPr>") {
        return format!("{}{props}{}", &run[..close], &run[close..]);
    }
    for empty in ["<w:rPr />", "<w:rPr/>"] {
        if let Some(at) = run.find(empty) {
            return format!(
                "{}<w:rPr>{props}</w:rPr>{}",
                &run[..at],
                &run[at + empty.len()..]
            );
        }
    }
    match run.strip_prefix("<w:r>") {
        Some(rest) => format!("<w:r><w:rPr>{props}</w:rPr>{rest}"),
        None => run.to_string(),
    }
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

    /// 第 ③ 期测试 F5：新增里的加粗包在宏里，只有第一个字是粗体。
    /// 样式命令要包在宏外面，一段新增按样式切开、仍拼成一个框。
    #[test]
    fn tex_styles_wrap_outside_the_mark_macros() {
        let input = format!(
            "确定{REDLINE_ADD_OPEN}，\\textbf{{重点报送}}（含说明），\\textbf{{并按季度校准}}{REDLINE_ADD_CLOSE}。"
        );
        let out = inject_tex_redline(&input);
        assert_eq!(
            out,
            "确定\\GwBoxBar\\GwAddLines{\\hspace{1.5pt}，}\\textbf{\\GwAddLines{重点报送}}\
             \\GwAddLines{（含说明），}\\textbf{\\GwAddLines{并按季度校准\\hspace{1.5pt}}}\\GwBoxBar{}。"
        );
        let input = format!("前{REDLINE_DEL_OPEN}\\textbf{{权重}}由专家{REDLINE_DEL_CLOSE}后");
        assert_eq!(
            inject_tex_redline(&input),
            "前\\textbf{\\GwDel{权重}}\\GwDel{由专家}后"
        );
    }

    /// 第 ③ 期测试 F4：新增里带行内公式，公式进了 xeCJKfntef 的宏，编译失败。
    /// 公式、引用是原子，整体装盒另画标记。
    #[test]
    fn tex_formulas_and_references_are_marked_as_atoms() {
        let input =
            format!("取{REDLINE_ADD_OPEN}\\(p \\ge 0.8\\)的样本\\cite{{a,b}}{REDLINE_ADD_CLOSE}。");
        assert_eq!(
            inject_tex_redline(&input),
            "取\\GwBoxBar\\GwAddAtom{\\hspace{1.5pt}\\(p \\ge 0.8\\)}\\GwAddLines{的样本}\
             \\GwAddAtom{\\cite{a,b}\\hspace{1.5pt}}\\GwBoxBar{}。"
        );
        let input = format!("其中{REDLINE_DEL_OPEN}\\(w_i\\){REDLINE_DEL_CLOSE}为权重");
        assert_eq!(
            inject_tex_redline(&input),
            "其中\\GwDelAtom{\\(w_i\\)}为权重"
        );
        // 脚注装进盒子会丢：原样留在标记外面。
        let input = format!("{REDLINE_ADD_OPEN}正文\\footnote{{注释}}{REDLINE_ADD_CLOSE}");
        assert_eq!(
            inject_tex_redline(&input),
            "\\GwBoxBar\\GwAddLines{\\hspace{1.5pt}正文\\hspace{1.5pt}}\\footnote{注释}\\GwBoxBar{}"
        );
    }

    /// 第 ③ 期测试 F6：独立公式里的哨兵换成整块标注，不能把 `\GwDel` 塞进数学模式。
    #[test]
    fn tex_display_math_is_marked_as_a_whole_block() {
        let input = format!(
            "\\[\n{REDLINE_DEL_OPEN}E = mc^{{2}}{REDLINE_DEL_CLOSE}\n\\]\n\n\\[\n{REDLINE_ADD_OPEN}E = mc^{{3}}{REDLINE_ADD_CLOSE}\n\\]\n"
        );
        assert_eq!(
            inject_tex_redline(&input),
            "\\[\n\\GwDelAtom{$\\displaystyle E = mc^{2}$}\n\\]\n\n\\[\n\\GwAddDisplay{E = mc^{3}}\n\\]\n"
        );
        // 表格里的 `\\[2pt]` 换行不是独立公式。
        let input = format!("甲\\\\[2pt]{REDLINE_DEL_OPEN}乙{REDLINE_DEL_CLOSE}");
        assert_eq!(inject_tex_redline(&input), "甲\\\\[2pt]\\GwDel{乙}");
    }

    /// 第 ③ 期测试 F8：一段新增跨过加粗 / 公式，中间那几个自己不带哨兵的 run
    /// 从前原样穿过，只有首尾两个 run 有框。中段 run 要整个带上标记，
    /// 原有属性（加粗）与 run 里的其余内容保留。
    #[test]
    fn ooxml_runs_between_sentinels_get_the_mark_too() {
        let xml = format!(
            "<w:p><w:r><w:rPr /><w:t>甲{REDLINE_ADD_OPEN}，</w:t></w:r>\
             <w:r><w:rPr><w:b /></w:rPr><w:t>重点</w:t></w:r>\
             <w:r><w:rPr /><w:t>$x$</w:t></w:r>\
             <w:r><w:t>无属性</w:t></w:r>\
             <w:r><w:rPr /><w:t>尾{REDLINE_ADD_CLOSE}。</w:t></w:r>\
             <w:r><w:rPr /><w:t>之后</w:t></w:r></w:p>"
        );
        let out = inject_ooxml_redline(&xml);
        let border = "<w:bdr w:val=\"single\" w:sz=\"4\" w:space=\"1\" w:color=\"1F4E9E\" />";
        assert!(
            out.contains(&format!("<w:rPr><w:b />{border}</w:rPr><w:t>重点</w:t>")),
            "加粗中段带框、保留加粗：{out}"
        );
        assert!(
            out.contains(&format!("<w:rPr>{border}</w:rPr><w:t>$x$</w:t>")),
            "公式 run 带框：{out}"
        );
        assert!(
            out.contains(&format!("<w:r><w:rPr>{border}</w:rPr><w:t>无属性</w:t>")),
            "没有 rPr 的 run 补上：{out}"
        );
        assert!(
            out.contains("<w:r><w:rPr /><w:t>之后</w:t></w:r>"),
            "标记收尾后的 run 原样：{out}"
        );
        // 删除同理。
        let xml = format!(
            "<w:r><w:rPr /><w:t>{REDLINE_DEL_OPEN}甲</w:t></w:r><w:r><w:rPr><w:b /></w:rPr><w:t>乙</w:t></w:r><w:r><w:rPr /><w:t>丙{REDLINE_DEL_CLOSE}</w:t></w:r>"
        );
        let out = inject_ooxml_redline(&xml);
        assert_eq!(out.matches("<w:strike />").count(), 3, "三段都划掉：{out}");
    }

    #[test]
    fn ooxml_without_sentinels_passes_through() {
        let xml = "<w:p><w:r><w:rPr /><w:t>干净文字。</w:t></w:r></w:p>";
        assert_eq!(inject_ooxml_redline(xml), xml);
    }
}
