//! 研究报告 Word 的花脸稿后处理：研究报告的 Word 由 mdx 转换器生成，哨兵字符
//! 原样穿过转换（mdx 不做私用区过滤），所以在产物落地后把哨兵对换成删除线 /
//! 字符边框 run（OOXML）。PDF 不经这里：Typst 路径在排版数据里直接换成标注。

use crate::export::{
    REDLINE_ADD_CLOSE, REDLINE_ADD_OPEN, REDLINE_DEL_CLOSE, REDLINE_DEL_OPEN, RedlineKind,
};
use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

/// 删除标记色（方案第八节）：红 #C00000。
pub(crate) const DEL_COLOR: &str = "C00000";
/// 新增标记色：蓝 #1F4E9E。
pub(crate) const ADD_COLOR: &str = "1F4E9E";

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
