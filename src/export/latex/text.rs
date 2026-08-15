//! 文本工具：LaTeX 转义、密级指令、标题内容与署名展开。
//!
//! 由 src/export/latex.rs 拆分而来：本文件是模块 `export::latex::text`，与其它子模块共享
//! `export::latex` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{DraftInput, TemplateKind, split_period_digits};
use crate::export::{MarkdownBlock, attachment_names, inline_segments, plain_text};
use crate::export::title::{TitlePlan};
use crate::export::title;

/// 规格 §3.2/§6 姓名宽度处理：2 字姓名中间加 1em 空格，4 字姓名压缩到 3 字宽，
/// 保证版记联系人列与表格姓名列在视觉上整齐对齐。
pub(crate) fn latex_name(value: &str) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    match chars.len() {
        2 => format!("{}\\hspace{{1em}}{}", chars[0], chars[1]),
        4 => format!("\\resizebox{{3em}}{{0.9em}}{{{}}}", tex_escape(value)),
        _ => tex_escape(value),
    }
}

pub(crate) fn tex_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\textbackslash{}"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '%' => out.push_str("\\%"),
            '&' => out.push_str("\\&"),
            '_' => out.push_str("\\_"),
            '^' => out.push_str("\\textasciicircum{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            _ => out.push(ch),
        }
    }
    out
}

/// 正文段转 TeX：Markdown 加粗转为 `\textbf`，由字体的 AutoFakeBold 实现；
/// 完整圆括号/方头括号及其中内容用四号楷体，其余保持正文三号仿宋。
/// 标题（文档标题、各级标题、附件标签）不经由此处，不受此规则影响。
/// 括号部分用花括号限定 `\kai\zihao{4}` 的作用域，闭合后自动回到正文三号仿宋。
pub(crate) fn body_text_to_tex(text: &str) -> String {
    let mut out = String::new();
    for segment in inline_segments(text) {
        let mut content = tex_escape(&segment.text);
        if segment.bold {
            content = format!("\\textbf{{{content}}}");
        }
        if segment.parenthesized {
            out.push_str(&format!("{{\\kai\\enkai\\zihao{{4}} {content}}}"));
        } else {
            out.push_str(&content);
        }
    }
    out
}

/// 生成密级相关命令：密级、保密期限，以及“指人专办”标记（勾选后非空）。
/// 数字年限的保密期限把前导数字用 `\ttfamily` 排成等宽，如 `{\ttfamily 10}年`。
pub(crate) fn security_commands(input: &DraftInput) -> String {
    if input.profile.security_level.trim().is_empty() {
        return String::new();
    }
    let special = if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        "指人专办"
    } else {
        ""
    };
    let (digits, rest) = split_period_digits(&input.profile.security_period);
    let period = if digits.is_empty() {
        tex_escape(&input.profile.security_period)
    } else {
        format!("{{\\ttfamily {digits}}}{}", tex_escape(rest))
    };
    format!(
        "\\renewcommand{{\\SecurityLevel}}{{{}}}\n\\renewcommand{{\\SecurityPeriod}}{{{}}}\n\\renewcommand{{\\SpecialHandling}}{{{}}}\n",
        tex_escape(&input.profile.security_level),
        period,
        tex_escape(special)
    )
}

/// 附件概要：正文结束后、落款之前，与正文之间空两行、首行缩进两个汉字，
/// 按顺序列出附件名称。单个附件写“附件：名称”；多个附件只有第一行写“附件N：名称”，
/// 其余行的“附件”二字用 `\qquad`（两个汉字宽）占位对齐、不再重复。
pub(crate) fn attachment_summary_tex(blocks: &[MarkdownBlock]) -> Option<String> {
    let names = attachment_names(blocks);
    if names.is_empty() {
        return None;
    }
    let mut out = String::new();
    // 与正文之间空两行。
    out.push_str("\\vspace{2\\baselineskip}\n");
    for (index, name) in names.iter().enumerate() {
        // 多个附件只有第一行保留“附件”二字，其余行用 \qquad 占位，使序号列对齐。
        let prefix = if names.len() == 1 || index == 0 {
            "附件"
        } else {
            "\\qquad "
        };
        let label = if names.len() == 1 {
            format!("：{name}")
        } else {
            format!("{}：{name}", index + 1)
        };
        out.push_str(&format!(
            "\\noindent\\hspace*{{2em}}{}{}\\par",
            prefix,
            body_text_to_tex(&label)
        ));
    }
    Some(out)
}

/// 标题内容 TeX：按标题字数与 jieba 排布。
/// 单行保持二号；超出一行不超过 2 字用 `\scalebox` 只缩横向、字高不变；
/// 超出更多在词边界均衡换行（`\\` 分段）。
pub(crate) fn title_content_tex(title: &str) -> String {
    let plain = plain_text(title);
    match title::title_plan(&plain, title::chars_per_line()) {
        TitlePlan::SingleLine => {
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {}}}",
                tex_escape(&plain)
            )
        }
        TitlePlan::Compressed => {
            // \scalebox{横向}[1] 只压缩字形宽度，纵向保持 1，即字高不变。
            let scale = title::compressed_scale_percent(&plain);
            let scale_f = scale as f64 / 100.0;
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}}\\scalebox{{{scale_f}}}[1]{{{}}}}}",
                tex_escape(&plain)
            )
        }
        TitlePlan::Wrapped(lines) => {
            let mut body = String::new();
            for (index, line) in lines.iter().enumerate() {
                if index > 0 {
                    body.push_str("\\\\");
                }
                body.push_str(&tex_escape(line));
            }
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {body}}}"
            )
        }
    }
}

/// 红头呈批件首页标题：小二号、约 10cm 左栏（15 个全角字宽）。
pub(crate) fn red_approval_title_content_tex(title: &str) -> String {
    let plain = plain_text(title);
    match title::title_plan(&plain, title::red_approval_chars_per_line()) {
        TitlePlan::SingleLine => format!(
            "{{\\bs\\enbt\\fontsize{{18bp}}{{\\BodyBaselineSkip}}\\selectfont {}}}",
            tex_escape(&plain)
        ),
        TitlePlan::Compressed => {
            let scale = title::compressed_scale_percent_for(
                &plain,
                title::RED_APPROVAL_TITLE_WIDTH_PT,
                title::RED_APPROVAL_TITLE_SIZE_PT,
            ) as f64
                / 100.0;
            format!(
                "{{\\bs\\enbt\\fontsize{{18bp}}{{\\BodyBaselineSkip}}\\selectfont\\scalebox{{{scale}}}[1]{{{}}}}}",
                tex_escape(&plain)
            )
        }
        TitlePlan::Wrapped(lines) => {
            let body = lines
                .iter()
                .map(|line| tex_escape(line))
                .collect::<Vec<_>>()
                .join("\\\\");
            format!("{{\\bs\\enbt\\fontsize{{18bp}}{{\\BodyBaselineSkip}}\\selectfont {body}}}")
        }
    }
}

/// LaTeX 中普通空格会被忽略或压缩，无法表达逐字间距；
/// 电话通知落款须把半角空格改写为受控空格命令 `\ `（每个空格独立有效）。
/// 必须在 `tex_escape` 之后替换，否则 `\` 会被转义为 `\textbackslash{}`。
pub(crate) fn tex_spaced(value: &str) -> String {
    tex_escape(value).replace(' ', "\\ ")
}

/// 落款单位单行：少于 5 字时逐字用 `\hspace*` 分散对齐到 5 字宽
/// （字距以 em 计，随落款字号缩放），与预览/Word 各端一致；否则原样转义。
pub(crate) fn tex_spread_signature(text: &str) -> String {
    match crate::units::spread_gap(text) {
        Some(gap) => text
            .chars()
            .enumerate()
            .map(|(index, ch)| {
                let escaped = tex_escape(&ch.to_string());
                if index == 0 {
                    escaped
                } else {
                    format!("\\hspace*{{{gap}em}}{escaped}")
                }
            })
            .collect(),
        None => tex_escape(text),
    }
}
