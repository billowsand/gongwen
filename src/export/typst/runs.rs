//! 行内文字 → 片段：花脸稿哨兵、加粗、括号楷体，以及中西文间隙。
//!
//! 与 TeX 路径一一对应：`body_runs` 对应 `body_text_to_tex`，`marked_runs` 对应
//! `marked_tex_escape`，`heading_runs` 对应 `marked_heading_tex`。

use std::ops::Range;

use super::data::{Run, Runs};
use crate::export::{
    RedlineKind, inline_segments, numbered_inline_segments, plain_text, redline_chunks,
    whole_chunk_kind,
};

/// xeCJK 的 CJKecglue：`\hskip 2pt`（TeX pt）。
const CJK_LATIN_GAP_PT: f64 = 2.0 * 72.0 / 72.27;

fn mark_of(kind: RedlineKind) -> Option<&'static str> {
    match kind {
        RedlineKind::Same => None,
        RedlineKind::Deleted => Some("del"),
        RedlineKind::Added => Some("add"),
    }
}

/// 正文段落：先按花脸稿哨兵切块，块内再切加粗 / 括号楷体。
pub(crate) fn body_runs(text: &str) -> Runs {
    numbered_body_runs(text, &[])
}

/// 同 [`body_runs`]，`numbers` 是段内列表编号占的可见字符范围（解析器生成，见
/// `LocatedBlock::generated_prefixes`）：编号是版式不是括号注释，「(1)」「（1）」这类
/// 编号不排括号楷体，与正文同字面。
pub(crate) fn numbered_body_runs(text: &str, numbers: &[Range<usize>]) -> Runs {
    let out = numbered_inline_segments(text, numbers)
        .into_iter()
        .filter(|piece| !piece.segment.text.is_empty())
        .map(|piece| Run {
            t: Some(piece.segment.text),
            b: piece.segment.bold,
            k: piece.segment.parenthesized,
            m: mark_of(piece.kind),
            ..Run::default()
        })
        .collect();
    with_punct_kerning(with_cjk_latin_gaps(out))
}

/// 要素、标题这类纯文字：剥掉 Markdown 标记，只保留花脸稿标注。
pub(crate) fn marked_runs(text: &str) -> Runs {
    let mut out = Vec::new();
    for chunk in redline_chunks(text) {
        let plain = plain_text(&chunk.text);
        if plain.is_empty() {
            continue;
        }
        out.push(Run {
            t: Some(plain),
            m: mark_of(chunk.kind),
            ..Run::default()
        });
    }
    with_punct_kerning(with_cjk_latin_gaps(out))
}

/// 表格数据格：花脸稿 + 加粗，不切括号楷体（与 `data_cell_tex` 一致）。
pub(crate) fn cell_runs(text: &str) -> Runs {
    let mut out = Vec::new();
    for chunk in redline_chunks(text) {
        for segment in inline_segments(&chunk.text) {
            if segment.text.is_empty() {
                continue;
            }
            out.push(Run {
                t: Some(segment.text),
                b: segment.bold,
                m: mark_of(chunk.kind),
                ..Run::default()
            });
        }
    }
    with_punct_kerning(with_cjk_latin_gaps(out))
}

/// 标题整行（编号 + 文字）。新增标题整体加框时编号一并进框（方案规则 8）。
pub(crate) fn heading_runs(number: &str, text: &str) -> Runs {
    if whole_chunk_kind(text) == Some(RedlineKind::Added) {
        return with_cjk_latin_gaps(vec![Run {
            t: Some(format!("{number}{}", plain_text(text))),
            m: Some("add"),
            ..Run::default()
        }]);
    }
    let mut out = Vec::new();
    if !number.is_empty() {
        out.push(Run::text(number));
    }
    out.extend(marked_runs(text));
    with_punct_kerning(with_cjk_latin_gaps(out))
}

/// 不带任何标注的纯文字。
pub(crate) fn plain_runs(text: &str) -> Runs {
    if text.is_empty() {
        return Vec::new();
    }
    with_cjk_latin_gaps(vec![Run::text(text)])
}

/// xeCJK 在汉字类字符与西文字符之间插的间隙。实测（公函文号「18号」、密级「★1年」）：
/// 汉字、★ 这类与西文相邻时有 2pt，全角标点与西文相邻时没有。
fn is_cjk_glue_side(ch: char) -> bool {
    if is_cjk_punctuation(ch) {
        return false;
    }
    matches!(ch as u32,
        0x2E80..=0x2FFF   // 部首
        | 0x3040..=0x30FF // 假名
        | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0x20000..=0x3FFFF
        | 0x2605 | 0x2606 // ★ ☆
    )
}

fn is_cjk_punctuation(ch: char) -> bool {
    matches!(ch as u32,
        0x3000..=0x303F | 0xFF00..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65
        | 0x2010..=0x2027 // 破折号、引号、省略号
    )
}

fn is_latin_side(ch: char) -> bool {
    ch.is_ascii_graphic()
}

fn needs_gap(prev: char, next: char) -> bool {
    (is_cjk_glue_side(prev) && is_latin_side(next))
        || (is_latin_side(prev) && is_cjk_glue_side(next))
}

/// 全角标点（相邻标点挤压用，与 Typst 的 CJK 标点判定同一批）。
fn is_squeezable_punct(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '！'
            | '？'
            | '）'
            | '」'
            | '』'
            | '》'
            | '】'
            | '〕'
            | '（'
            | '「'
            | '『'
            | '《'
            | '【'
            | '〔'
            | '“'
            | '”'
            | '‘'
            | '’'
    )
}

/// 片段的字号：括号楷体四号 14pt，其余正文三号 16pt。
fn run_size_pt(run: &Run) -> f64 {
    if run.k { 14.0 } else { 16.0 }
}

/// 相邻标点挤压（「。（」「）。」这类）。Typst 只在同一段文字内部挤，换字体、加粗、
/// 花脸稿标注都会切成两段，跨段的一对就不挤了；TeX（xeCJK）照挤。实测 TeX 挤掉
/// 两者中较小那个字号的半个字宽（「。」三号接「（」四号：挤 7pt），这里在跨段处补
/// 一段等量的负间隙。
pub(crate) fn with_punct_kerning(runs: Runs) -> Runs {
    let mut out: Runs = Vec::with_capacity(runs.len());
    for run in runs {
        if let (Some(first), Some(prev)) = (
            run.t.as_deref().and_then(|t| t.chars().next()),
            out.last().filter(|p| p.t.is_some()),
        ) && let Some(last) = prev.t.as_deref().and_then(|t| t.chars().last())
            && is_squeezable_punct(last)
            && is_squeezable_punct(first)
        {
            let squeeze = run_size_pt(prev).min(run_size_pt(&run)) / 2.0;
            let mark = shared_mark(prev, &run);
            out.push(Run {
                m: mark,
                ..Run::gap(-squeeze)
            });
        }
        out.push(run);
    }
    out
}

/// 间隙夹在两段之间时的花脸稿标注：两边同属一个删除 / 新增块，间隙也算进去，
/// 否则一个新增框会在「10月」这样的中西文交界处断成两个。
fn shared_mark(prev: &Run, next: &Run) -> Option<&'static str> {
    if prev.m == next.m { next.m } else { None }
}

/// 在片段序列里按字符相邻关系补中西文间隙；跨片段的边界也算。
pub(crate) fn with_cjk_latin_gaps(runs: Runs) -> Runs {
    let mut out: Runs = Vec::with_capacity(runs.len());
    let mut prev: Option<char> = None;
    let mut prev_mark: Option<&'static str> = None;
    for run in runs {
        let Some(text) = run.t.as_deref() else {
            // 占位、间隙打断相邻关系。
            prev = None;
            out.push(run);
            continue;
        };
        let mut piece = String::new();
        for ch in text.chars() {
            // 标注块的边界上 TeX 不插间隙：\GwDel / \GwAdd 宏把 xeCJK 的字类判断隔断了。
            let crosses_mark = piece.is_empty() && prev_mark != run.m;
            if let Some(p) = prev
                && !crosses_mark
                && needs_gap(p, ch)
            {
                // 能走到这里，两边必是同一标注。
                let mark = run.m;
                if !piece.is_empty() {
                    out.push(Run {
                        t: Some(std::mem::take(&mut piece)),
                        ..run.clone()
                    });
                }
                out.push(Run {
                    m: mark,
                    ..Run::gap(CJK_LATIN_GAP_PT)
                });
            }
            piece.push(ch);
            prev = Some(ch);
        }
        prev_mark = run.m;
        if !piece.is_empty() {
            out.push(Run {
                t: Some(piece),
                ..run.clone()
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(runs: &Runs) -> Vec<String> {
        runs.iter()
            .map(|r| match (&r.t, r.g) {
                (Some(t), _) => t.clone(),
                (None, Some(_)) => "␣".into(),
                _ => "□".into(),
            })
            .collect()
    }

    #[test]
    fn gaps_only_between_han_and_latin() {
        let runs = plain_runs("某民呈〔2026〕18号");
        assert_eq!(texts(&runs), ["某民呈〔2026〕18", "␣", "号"]);
        let runs = plain_runs("秘密★1年");
        assert_eq!(texts(&runs), ["秘密★", "␣", "1", "␣", "年"]);
        let runs = plain_runs("联系电话：010-12345678");
        assert_eq!(texts(&runs), ["联系电话：010-12345678"]);
    }

    #[test]
    fn gaps_cross_run_boundaries() {
        let runs = body_runs("共排查**站点312**个");
        assert_eq!(texts(&runs), ["共排查", "站点", "␣", "312", "␣", "个"]);
        assert!(runs[1].b && runs[3].b && !runs[5].b);
    }

    #[test]
    fn adjacent_punctuation_across_runs_is_squeezed() {
        // 括号楷体在「）」处结束，下一段正文以「。」开头：补 -7pt（较小的四号的半字）。
        let runs = body_runs("说明（附后）。");
        let gaps: Vec<_> = runs.iter().filter_map(|r| r.g).collect();
        assert_eq!(gaps, [-7.0]);
        // 同一段里的相邻标点由 Typst 自己挤，不另补。
        assert!(body_runs("好。」").iter().all(|r| r.g.is_none()));
    }

    #[test]
    fn inline_list_numbers_are_not_kai() {
        // 「（1）」是段内列表编号：不排括号楷体，与前后正文并成一段；正文里的括号照旧。
        let text = "要求如下：（1）落实**责任**（试行）；（2）加强";
        let numbers = [5..8, 17..20];
        let runs = numbered_body_runs(text, &numbers);
        let plain: Vec<_> = runs
            .iter()
            .filter(|r| !r.k && !r.b)
            .filter_map(|r| r.t.clone())
            .collect();
        assert_eq!(plain, ["要求如下：（1）落实", "；（2）加强"]);
        let kai: Vec<_> = runs
            .iter()
            .filter(|r| r.k)
            .filter_map(|r| r.t.clone())
            .collect();
        assert_eq!(kai, ["（试行）"]);
        assert_eq!(
            runs.iter().filter_map(|r| r.t.clone()).collect::<String>(),
            "要求如下：（1）落实责任（试行）；（2）加强"
        );
    }

    #[test]
    fn parentheses_become_kai() {
        let runs = body_runs("评估（办法另行印发）。");
        let kai: Vec<_> = runs
            .iter()
            .filter(|r| r.k)
            .filter_map(|r| r.t.clone())
            .collect();
        assert_eq!(kai, ["（办法另行印发）"]);
    }

    #[test]
    fn redline_chunks_become_marks() {
        let text = format!(
            "原{}旧{}新{}增{}文",
            crate::export::REDLINE_DEL_OPEN,
            crate::export::REDLINE_DEL_CLOSE,
            crate::export::REDLINE_ADD_OPEN,
            crate::export::REDLINE_ADD_CLOSE
        );
        let runs = body_runs(&text);
        let marks: Vec<_> = runs.iter().map(|r| r.m).collect();
        assert_eq!(texts(&runs), ["原", "旧", "新", "增", "文"]);
        assert_eq!(marks, [None, Some("del"), None, Some("add"), None]);
    }
}
