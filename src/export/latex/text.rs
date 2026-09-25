//! 文本工具：LaTeX 转义、密级指令、标题内容与署名展开。
//!
//! 由 src/export/latex.rs 拆分而来：本文件是模块 `export::latex::text`，与其它子模块共享
//! `export::latex` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::export::title;
use crate::export::title::TitlePlan;
use crate::export::{
    MarkdownBlock, RedlineKind, attachment_names, inline_segments, is_redline_sentinel, plain_text,
    redline_chunks, redline_slice_lines,
};
use crate::models::{DraftInput, TemplateKind, split_period_digits};

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
        // 花脸稿哨兵是私用区码位，字体里没有这些字形，漏到 TeX 里就是纸上一个
        // 豆腐块。正常路径（`body_text_to_tex`、表格单元格）在调用这里之前就把
        // 它们切掉了，这一道是兜底：将来谁新写一条渲染路径忘了处理，最坏也只是
        // 不显示标记，而不会印出乱码。
        if is_redline_sentinel(ch) {
            continue;
        }
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

/// 正文段转 TeX：Markdown 加粗转为 `\GwBold`——类文件里它默认就是 `\textbf`，
/// 由字体的 AutoFakeBold 实现；设置里改选专用粗体字体时由导言区换成真正的字面；
/// 完整圆括号/方头括号及其中内容用四号楷体，其余保持正文三号仿宋。
/// 标题（文档标题、各级标题、附件标签）不经由此处，不受此规则影响。
/// 括号部分用花括号限定 `\kai\zihao{4}` 的作用域，闭合后自动回到正文三号仿宋。
pub(crate) fn body_text_to_tex(text: &str) -> String {
    let mut out = String::new();
    // 先按花脸稿哨兵切块，再对每块跑原来的加粗/括号逻辑。没有哨兵时只有一块
    // `Same`，走的路径与从前完全一样。
    for chunk in redline_chunks(text) {
        let segments = inline_segments(&chunk.text);
        let count = segments.len();
        for (index, segment) in segments.iter().enumerate() {
            // 样式包在标注宏**外面**：xeCJKfntef 逐字处理宏内文字，宏内的
            // 字体切换（加粗、括号楷体）只作用到第一个字。
            let mut content = redline_macro(chunk.kind, index, count, &tex_escape(&segment.text));
            if segment.bold {
                content = format!("\\GwBold{{{content}}}");
            }
            if segment.parenthesized {
                // 新增框的高度要在换小一号字之前定死，否则这一截框比前后矮。
                let freeze = if chunk.kind == RedlineKind::Added {
                    "\\GwBoxFreeze"
                } else {
                    ""
                };
                out.push_str(&format!("{{{freeze}\\kai\\enkai\\zihao{{4}} {content}}}"));
            } else {
                out.push_str(&content);
            }
        }
    }
    out
}

/// 给一段已转义的文字套花脸稿宏。样式（加粗、括号楷体）要包在宏外面，
/// 所以一块标注会按样式切成 `count` 段：删除线逐段画，接起来仍是一条；
/// 新增框只在首段画左边、末段画右边（`\GwAddOpen` / `\GwAddMid` /
/// `\GwAddClose`），中间不断框，读起来还是一个框。
pub(crate) fn redline_macro(
    kind: RedlineKind,
    index: usize,
    count: usize,
    content: &str,
) -> String {
    match kind {
        RedlineKind::Same => content.to_string(),
        RedlineKind::Deleted => format!("\\GwDel{{{content}}}"),
        RedlineKind::Added => {
            let name = match (index == 0, index + 1 == count) {
                (true, true) => "GwAdd",
                (true, false) => "GwAddOpen",
                (false, true) => "GwAddClose",
                (false, false) => "GwAddMid",
            };
            format!("\\{name}{{{content}}}")
        }
    }
}

/// 生成密级相关命令：密级、保密期限，以及“指人专办”标记（勾选后非空）。
/// 数字年限的保密期限把前导数字用 `\ttfamily` 排成等宽，如 `{\ttfamily 10}年`。
pub(crate) fn security_commands(input: &DraftInput) -> String {
    let (level, period) = crate::export::element_display::security_parts(input);
    if level.is_empty() {
        return String::new();
    }
    let special = if input.kind != TemplateKind::PlainDocument && input.profile.special_handling {
        "指人专办"
    } else {
        ""
    };
    let (digits, rest) = split_period_digits(period);
    let period = if digits.is_empty() {
        tex_escape(period)
    } else {
        format!("{{\\ttfamily {digits}}}{}", tex_escape(rest))
    };
    format!(
        "\\renewcommand{{\\SecurityLevel}}{{{}}}\n\\renewcommand{{\\SecurityPeriod}}{{{}}}\n\\renewcommand{{\\SpecialHandling}}{{{}}}\n",
        tex_escape(level),
        period,
        tex_escape(special)
    )
}

/// 附件概要：正文结束后、落款之前，与正文之间空两行、首行缩进两个汉字，
/// 按顺序列出附件名称。单个附件写“附件：名称”；多个附件只有第一行写“附件N：名称”，
/// 每行都用 `\phantom{附件}` 建立相同的序号起点，首行再叠印“附件”二字。
pub(crate) fn attachment_summary_tex(blocks: &[MarkdownBlock]) -> Option<String> {
    let names = attachment_names(blocks);
    if names.is_empty() {
        return None;
    }
    let mut out = String::new();
    // 与正文之间空两行。
    out.push_str("\\vspace{2\\baselineskip}\n");
    for (index, name) in names.iter().enumerate() {
        // 多个附件的每一行都经过同一个 phantom 盒子，使盒子到数字之间的字间距
        // 完全一致；首行用零宽盒叠印“附件”，避免真实中文文本与 phantom 盒子后
        // 接西文数字时 XeCJK 采用不同的字间距而产生细微错位。
        let prefix = if names.len() == 1 {
            "附件"
        } else if index == 0 {
            "\\rlap{附件}\\phantom{附件}"
        } else {
            "\\phantom{附件}"
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

/// 标题路径的标注支持：与 `tex_escape(&plain_text(text))` 完全同口径，只是
/// 带花脸稿哨兵时删除块包 `\GwDel`、新增块包 `\GwAdd`（两者都能随文字
/// 断行，与正文同一套规矩）。
/// 视觉 diff 引擎在解析后注入哨兵，标题改动因此也就地标注（方案需求结论
/// 第 12 条），不再需要单独的说明页。
pub(crate) fn marked_tex_escape(text: &str) -> String {
    if !text.contains([
        crate::export::REDLINE_DEL_OPEN,
        crate::export::REDLINE_DEL_CLOSE,
        crate::export::REDLINE_ADD_OPEN,
        crate::export::REDLINE_ADD_CLOSE,
    ]) {
        return tex_escape(&plain_text(text));
    }
    let mut out = String::new();
    for chunk in redline_chunks(text) {
        let inner = tex_escape(&plain_text(&chunk.text));
        match chunk.kind {
            RedlineKind::Same => out.push_str(&inner),
            RedlineKind::Deleted => out.push_str(&format!("\\GwDel{{{inner}}}")),
            RedlineKind::Added => out.push_str(&format!("\\GwAdd{{{inner}}}")),
        }
    }
    out
}

/// 公文标题整行（编号前缀 + 文字）。新增标题整体加框时（方案规则 8：
/// 含编号），编号与文字进同一个 `\GwAdd` 框；其余情况编号照常排在标注之外。
pub(crate) fn marked_heading_tex(number: &str, text: &str) -> String {
    use crate::export::whole_chunk_kind;
    if whole_chunk_kind(text) != Some(RedlineKind::Added) {
        return format!("{number}{}", marked_tex_escape(text));
    }
    format!(
        "\\GwAdd{{{}}}",
        tex_escape(&format!("{number}{}", plain_text(text)))
    )
}

/// 标题内容 TeX：按标题字数与 jieba 排布。
/// 单行保持二号；超出一行不超过 2 字用 `\scalebox` 只缩横向、字高不变；
/// 超出更多在词边界均衡换行（`\\` 分段）。带花脸稿标记时排布仍在纯文本上
/// 计算，再按行切开逐行注宏（`\\` 不能落在 `\GwDel` / `\GwAdd` 内部）。
pub(crate) fn title_content_tex(title: &str) -> String {
    let plain = plain_text(title);
    match title::title_plan(&plain, title::chars_per_line()) {
        TitlePlan::SingleLine => {
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {}}}",
                marked_tex_escape(title)
            )
        }
        TitlePlan::Compressed => {
            // \scalebox{横向}[1] 只压缩字形宽度，纵向保持 1，即字高不变。
            let scale = title::compressed_scale_percent(&plain);
            let scale_f = scale as f64 / 100.0;
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}}\\scalebox{{{scale_f}}}[1]{{{}}}}}",
                marked_tex_escape(title)
            )
        }
        TitlePlan::Wrapped(lines) => {
            let body = redline_slice_lines(title, &lines)
                .iter()
                .map(|line| marked_tex_escape(line))
                .collect::<Vec<_>>()
                .join("\\\\");
            format!(
                "{{\\bs\\enbt\\zihao{{2}}\\setlength{{\\baselineskip}}{{\\BodyBaselineSkip}} {body}}}"
            )
        }
    }
}

/// 红头呈批件首页标题：小二号、约 10cm 左栏（15 个全角字宽）。
/// 带花脸稿标记时与正文标题同一套处理（按行切开逐行注宏）。
pub(crate) fn red_approval_title_content_tex(title: &str) -> String {
    let plain = plain_text(title);
    match title::title_plan(&plain, title::red_approval_chars_per_line()) {
        TitlePlan::SingleLine => format!(
            "{{\\bs\\enbt\\fontsize{{18bp}}{{\\BodyBaselineSkip}}\\selectfont {}}}",
            marked_tex_escape(title)
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
                marked_tex_escape(title)
            )
        }
        TitlePlan::Wrapped(lines) => {
            let body = redline_slice_lines(title, &lines)
                .iter()
                .map(|line| marked_tex_escape(line))
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
