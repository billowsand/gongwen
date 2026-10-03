//! `cite_check` 算子：政策依据核对（`docs/ai-agent-workbench.md` 16.14）。
//!
//! 从正文认出引用的政策文件（书名号里像文件名的、紧跟的文号、同一句里引号引的原文），找原文、
//! 逐项比对。能确定的程序比（名称差几个字、文号不同、括号写法、引文能否在原文里逐字找到），比不了的
//! （引文不是原话）才把原文里最接近的一段交模型判。名称与文号有明确改法，进审校抽屉；表述只提示。

use super::{Flow, check_cancel, note, param, phase, prompt, tool_line};
use crate::agent::board::{Finding, Fix};
use crate::agent::evidence::EvidenceDoc;
use crate::agent::skill::StepSpec;
use crate::agent::tools::{Permission, ToolCtx, short};
use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;

/// 书名号 + 可选的紧跟文号括注。
static CITATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"《([^《》\n]{2,80})》(?:\s*[（(]([^（）()\n]{2,40})[）)])?").expect("引用正则")
});
/// 文号：机关代字 + 年份 + 序号。年份的括号认几种写法，规范是六角括号〔〕。
static DOC_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\p{Han}A-Za-z]{1,12}[〔\[［(（【]\s*(\d{4})\s*[〕\]］)）】]\s*第?(\d{1,5})号")
        .expect("文号正则")
});
/// 引号里的原文表述。
static QUOTE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[“\x22「]([^”\x22」\n]{4,200})[”\x22」]").expect("引号正则"));

/// 像政策文件名的结尾（去掉「（试行）」这类括注后看）。
const DOC_SUFFIXES: [&str; 27] = [
    "法", "条例", "规定", "办法", "细则", "意见", "通知", "决定", "决议", "规划", "纲要", "方案",
    "计划", "标准", "准则", "规范", "指引", "指南", "措施", "批复", "公告", "通告", "要点", "规则",
    "章程", "制度", "清单",
];

/// 正文里的一处引用。区间都是正文里的字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Citation {
    pub(crate) title: String,
    /// 书名号里面那段文字的区间（不含《》）。
    pub(crate) title_span: Range<usize>,
    pub(crate) number: Option<(String, Range<usize>)>,
    pub(crate) quotes: Vec<String>,
    pub(crate) sentence: String,
}

fn looks_like_policy(title: &str) -> bool {
    let core = title
        .trim()
        .trim_end_matches([')', '）'])
        .split(['（', '('])
        .next()
        .unwrap_or(title)
        .trim();
    DOC_SUFFIXES.iter().any(|suffix| core.ends_with(suffix))
}

fn sentence_bounds(text: &str, pos: usize) -> Range<usize> {
    let is_end = |c: char| matches!(c, '。' | '！' | '？' | '；' | '\n');
    let start = text[..pos]
        .char_indices()
        .rev()
        .find(|(_, c)| is_end(*c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let end = text[pos..]
        .char_indices()
        .find(|(_, c)| is_end(*c))
        .map_or(text.len(), |(i, c)| pos + i + c.len_utf8());
    start..end
}

/// 认出正文里引用的政策文件。同名的只留第一处（改法落在第一处），引文收齐。
pub(crate) fn extract(text: &str) -> Vec<Citation> {
    let mut out: Vec<Citation> = Vec::new();
    for caps in CITATION.captures_iter(text) {
        let title_match = caps.get(1).expect("必有标题");
        let title = title_match.as_str().trim().to_string();
        if !looks_like_policy(&title) {
            continue;
        }
        let number = caps.get(2).and_then(|m| {
            DOC_NUMBER.find(m.as_str()).map(|n| {
                (
                    n.as_str().to_string(),
                    m.start() + n.start()..m.start() + n.end(),
                )
            })
        });
        let whole = caps.get(0).expect("必有整段");
        let bounds = sentence_bounds(text, whole.start());
        let sentence = text[bounds.clone()].trim().to_string();
        // 引文只看书名号之后、同一句里的。
        let quotes: Vec<String> = QUOTE
            .captures_iter(&text[whole.end()..bounds.end.max(whole.end())])
            .map(|q| q[1].trim().to_string())
            .collect();
        match out
            .iter_mut()
            .find(|c| normalize(&c.title) == normalize(&title))
        {
            Some(existing) => {
                for quote in quotes {
                    if !existing.quotes.contains(&quote) {
                        existing.quotes.push(quote);
                    }
                }
                if existing.number.is_none() {
                    existing.number = number;
                }
            }
            None => out.push(Citation {
                title,
                title_span: title_match.range(),
                number,
                quotes,
                sentence,
            }),
        }
    }
    out
}

/// 比对用的归一：去掉空白、标点与书名号。
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation() && !is_cjk_punct(*c))
        .collect()
}

fn is_cjk_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '《'
            | '》'
            | '（'
            | '）'
            | '【'
            | '】'
            | '〔'
            | '〕'
            | '［'
            | '］'
            | '「'
            | '」'
            | '—'
            | '…'
            | '·'
    )
}

/// 文号归一：括号统一成〔〕、去掉「第」和空白。
fn normalize_number(number: &str) -> String {
    DOC_NUMBER
        .captures(number)
        .map(|caps| {
            let whole = caps.get(0).expect("整段").as_str();
            let head: String = whole
                .chars()
                .take_while(|c| !matches!(c, '〔' | '[' | '［' | '(' | '（' | '【'))
                .collect();
            format!("{}〔{}〕{}号", head.trim(), &caps[1], &caps[2])
        })
        .unwrap_or_else(|| number.trim().to_string())
}

/// `short` 的字是否按顺序都出现在 `long` 里（只是漏了几个字）。
fn is_subsequence(short: &str, long: &str) -> bool {
    let mut rest = long.chars();
    short.chars().all(|c| rest.any(|l| l == c))
}

/// 两个标题算不算同一份文件：归一后相同；或者只是漏了 / 多了几个字（「进一步」「若干」「关于」）。
/// 字换了的（「森林」与「草原」）是另一份文件，不算。
/// 返回 Some(true) 完全一致，Some(false) 相近，None 不是同一份。
fn title_match(cited: &str, source: &str) -> Option<bool> {
    let (a, b) = (normalize(cited), normalize(source));
    if a == b {
        return Some(true);
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    let (short, long, ls, ll) = if la <= lb {
        (&a, &b, la, lb)
    } else {
        (&b, &a, lb, la)
    };
    if ls < 4 {
        return None;
    }
    let limit = (ll / 4).clamp(2, 6);
    (ll - ls <= limit && is_subsequence(short, long)).then_some(false)
}

/// 找到的原文。
struct Source {
    title: String,
    text: String,
    origin: &'static str,
}

/// 按标题找原文：知识库文档列表 → 按标题检索、片段所属文档对得上的读全文 → 步骤 `apis:` 列的数据接口。
/// 完全一致的优先于相近的。
fn find_source(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec, title: &str) -> Option<Source> {
    let kb = ctx.env.kb;
    if kb.enabled() {
        let listed = kb.list(None).unwrap_or_default();
        let best = listed
            .iter()
            .filter_map(|(id, doc_title, _)| {
                title_match(title, doc_title).map(|exact| (exact, *id))
            })
            .max_by_key(|(exact, _)| *exact);
        if let Some((_, id)) = best
            && let Ok(Some((doc_title, text))) = kb.read(id)
        {
            return Some(Source {
                title: doc_title,
                text,
                origin: "知识库",
            });
        }
        if let Ok((chunks, _)) = kb.search(title) {
            let best = chunks
                .iter()
                .filter_map(|chunk| {
                    title_match(title, &chunk.doc_title).map(|exact| (exact, chunk))
                })
                .max_by_key(|(exact, _)| *exact);
            if let Some((_, chunk)) = best {
                let (doc_title, text) = kb
                    .read(chunk.doc_id)
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| (chunk.doc_title.clone(), chunk.text.clone()));
                return Some(Source {
                    title: doc_title,
                    text,
                    origin: "知识库",
                });
            }
        }
    }
    let mut best: Option<(bool, usize)> = None;
    for (api, args) in super::api_sources(step) {
        for (_, id) in super::call_api_into(ctx, &api, args.as_ref(), title) {
            if let Some(evidence) = ctx.board.evidence.get(id)
                && let Some(exact) = title_match(title, &evidence.doc_title)
                && best.is_none_or(|(was, _)| exact && !was)
            {
                best = Some((exact, id));
            }
        }
    }
    let evidence = ctx.board.evidence.get(best?.1)?;
    Some(Source {
        title: evidence.doc_title.clone(),
        text: evidence.text.clone(),
        origin: "数据接口",
    })
}

/// 原文里和引文最接近的一段（按两字组重合度）。
fn closest_passage(source: &str, quote: &str) -> String {
    let grams = |text: &str| -> Vec<(char, char)> {
        let chars: Vec<char> = normalize(text).chars().collect();
        chars.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let wanted = grams(quote);
    let spans = crate::agent::gaps::sentence_spans(source);
    let sentences: Vec<&str> = spans
        .iter()
        .map(|span| source[span.clone()].trim())
        .collect();
    let mut best = (0usize, 0usize);
    for (index, sentence) in sentences.iter().enumerate() {
        let have = grams(sentence);
        let score = wanted.iter().filter(|g| have.contains(g)).count();
        if score > best.0 {
            best = (score, index);
        }
    }
    if best.0 == 0 {
        return String::new();
    }
    // 带上前后各一句，免得断章。
    let from = best.1.saturating_sub(1);
    let to = (best.1 + 2).min(sentences.len());
    sentences[from..to].join("")
}

pub(super) fn cite_check(ctx: &mut ToolCtx<'_, '_>, step: &StepSpec) -> anyhow::Result<Flow> {
    let document = ctx.board.document.clone();
    if document.trim().is_empty() {
        anyhow::bail!("正文还是空的，没有可核对的引用");
    }
    // 有选区只核选区里的引用；区间仍按全文算，改法才能落回正文。
    let region = ctx
        .board
        .selection
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .and_then(|selection| document.find(selection).map(|at| at..at + selection.len()))
        .unwrap_or(0..document.len());
    let max = param(ctx, step, &["max"], 12, 1..=40);
    let citations: Vec<Citation> = extract(&document)
        .into_iter()
        .filter(|c| region.contains(&c.title_span.start))
        .take(max)
        .collect();
    if citations.is_empty() {
        note(
            ctx,
            "正文里没有认出引用的政策文件（书名号里的法规、意见、通知等）。",
        );
        ctx.board.findings.push(Finding {
            group: "说明".into(),
            text: "没有认出引用的政策文件".into(),
            excerpt: String::new(),
            source: "只认书名号里以法、条例、规定、办法、意见、通知、方案等结尾的文件名".into(),
            fix: None,
        });
        return Ok(Flow::Next);
    }
    let mut findings = Vec::new();
    let mut checked = 0usize;
    for citation in &citations {
        check_cancel(ctx)?;
        phase(ctx, format!("核对《{}》…", short(&citation.title, 20)));
        let Some(source) = find_source(ctx, step, &citation.title) else {
            findings.push(Finding {
                group: "未找到原文".into(),
                text: format!(
                    "知识库与数据接口里没有找到《{}》，请人工核对名称、文号与引文",
                    citation.title
                ),
                excerpt: short(&citation.sentence, 60),
                source: String::new(),
                fix: None,
            });
            tool_line(
                ctx,
                "kb.search",
                Permission::Read,
                format!("《{}》：没有找到原文", short(&citation.title, 20)),
            );
            continue;
        };
        let key = format!("cite:{}", normalize(&source.title));
        let ids = ctx.board.evidence.absorb_docs(
            &citation.title,
            &[EvidenceDoc {
                key,
                title: source.title.clone(),
                section: String::new(),
                kind_label: source.origin.into(),
                text: source.text.clone(),
            }],
        );
        let label = format!("[K{}]《{}》（{}）", ids[0], source.title, source.origin);
        let before = findings.len();

        // 名称。
        if title_match(&citation.title, &source.title) == Some(false) {
            findings.push(Finding {
                group: "名称与原文不一致".into(),
                text: format!("引用的名称与原文不一致，原文为《{}》", source.title),
                excerpt: format!("《{}》", citation.title),
                source: label.clone(),
                fix: Some(Fix {
                    span: citation.title_span.clone(),
                    before: citation.title.clone(),
                    after: source.title.clone(),
                }),
            });
        }

        // 文号。
        if let Some((cited, span)) = &citation.number {
            let source_numbers: Vec<String> = DOC_NUMBER
                .find_iter(&source.text)
                .map(|m| m.as_str().to_string())
                .take(5)
                .collect();
            let cited_norm = normalize_number(cited);
            if let Some(first) = source_numbers.first()
                && !source_numbers
                    .iter()
                    .any(|n| normalize_number(n) == cited_norm)
            {
                let correct = normalize_number(first);
                findings.push(Finding {
                    group: "文号不符".into(),
                    text: format!("引用的文号与原文不符，原文为{correct}"),
                    excerpt: cited.clone(),
                    source: label.clone(),
                    fix: Some(Fix {
                        span: span.clone(),
                        before: cited.clone(),
                        after: correct,
                    }),
                });
            } else if !cited.contains('〔') {
                findings.push(Finding {
                    group: "文号写法".into(),
                    text: "文号的年份应使用六角括号〔〕".into(),
                    excerpt: cited.clone(),
                    source: "GB/T 9704 党政机关公文格式".into(),
                    fix: Some(Fix {
                        span: span.clone(),
                        before: cited.clone(),
                        after: cited_norm,
                    }),
                });
            }
        }

        // 引文。
        let source_norm = normalize(&source.text);
        for quote in &citation.quotes {
            let wanted = normalize(quote);
            if wanted.chars().count() < 4 || source_norm.contains(&wanted) {
                continue;
            }
            let passage = closest_passage(&source.text, quote);
            if passage.is_empty() {
                findings.push(Finding {
                    group: "表述与原文不一致".into(),
                    text: format!("引号里的「{}」在原文里找不到相近的表述", short(quote, 40)),
                    excerpt: short(&citation.sentence, 60),
                    source: label.clone(),
                    fix: None,
                });
                continue;
            }
            let ask = prompt(
                ctx,
                step,
                "prompt",
                "表述比对",
                &[
                    ("title", source.title.clone()),
                    ("quote", quote.clone()),
                    ("passage", passage.clone()),
                ],
            )?;
            let reply = super::assist(ctx, &ask)?;
            let first = reply
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("");
            if first.starts_with("一致") {
                continue;
            }
            let original = first
                .split_once(['｜', '|'])
                .map(|(_, rest)| rest.trim().to_string())
                .filter(|rest| !rest.is_empty())
                .unwrap_or_else(|| short(&passage, 80));
            findings.push(Finding {
                group: "表述与原文不一致".into(),
                text: format!(
                    "引号里的「{}」不是原文原话，原文为「{original}」",
                    short(quote, 40)
                ),
                excerpt: short(&citation.sentence, 60),
                source: label.clone(),
                fix: None,
            });
        }

        let issues = findings.len() - before;
        if issues == 0 {
            findings.push(Finding {
                group: "已核对".into(),
                text: format!("《{}》的名称、文号与引文和原文一致", citation.title),
                excerpt: short(&citation.sentence, 60),
                source: label.clone(),
                fix: None,
            });
        }
        checked += 1;
        tool_line(
            ctx,
            "cite_check",
            Permission::Check,
            format!(
                "核对《{}》：{}",
                short(&citation.title, 20),
                if issues == 0 {
                    "一致".to_string()
                } else {
                    format!("{issues} 处问题")
                }
            ),
        );
    }
    // 问题排前面，已核对的排最后。
    findings.sort_by_key(|f| match f.group.as_str() {
        "名称与原文不一致" | "文号不符" => 0,
        "表述与原文不一致" => 1,
        "文号写法" => 2,
        "未找到原文" => 3,
        _ => 4,
    });
    note(
        ctx,
        format!("认出 {} 处引用，找到原文 {checked} 份。", citations.len()),
    );
    ctx.board.findings.extend(findings);
    Ok(Flow::Next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_are_policy_titles_with_numbers_and_quotes() {
        let text = "根据《国务院办公厅关于加强森林防火工作的意见》（国办发[2024]12号）要求，\
                    各地要“压实属地责任，强化源头管控”。《红楼梦》不算。\n\
                    再次重申《国务院办公厅关于加强森林防火工作的意见》提出的“严禁野外用火”。\
                    依照《森林防火条例（修订）》执行。";
        let citations = extract(text);
        assert_eq!(citations.len(), 2, "{citations:?}");
        let first = &citations[0];
        assert_eq!(first.title, "国务院办公厅关于加强森林防火工作的意见");
        assert_eq!(&text[first.title_span.clone()], first.title);
        let (number, span) = first.number.clone().unwrap();
        assert_eq!(number, "国办发[2024]12号");
        assert_eq!(&text[span], "国办发[2024]12号");
        assert_eq!(
            first.quotes,
            ["压实属地责任，强化源头管控", "严禁野外用火"],
            "同名的引文收齐"
        );
        assert_eq!(citations[1].title, "森林防火条例（修订）");
        assert!(citations[1].number.is_none());
    }

    #[test]
    fn numbers_and_titles_are_compared_loosely_but_not_too_loosely() {
        assert_eq!(normalize_number("国办发[2024]12号"), "国办发〔2024〕12号");
        assert_eq!(
            normalize_number("国办发（2024）第12号"),
            "国办发〔2024〕12号"
        );
        assert_eq!(
            title_match("关于加强森林防火工作的意见", "关于加强森林防火工作的意见"),
            Some(true)
        );
        assert_eq!(
            title_match(
                "关于加强森林防火工作的意见",
                "关于进一步加强森林防火工作的意见"
            ),
            Some(false),
            "差几个字算相近"
        );
        assert_eq!(
            title_match("关于加强森林防火工作的意见", "安全生产法"),
            None
        );
        assert_eq!(
            title_match("关于加强森林防火工作的意见", "关于加强草原防火工作的意见"),
            None,
            "换了字的是另一份文件"
        );
        assert_eq!(title_match("意见", "意见"), Some(true));
    }

    #[test]
    fn the_closest_passage_is_found_by_shared_bigrams() {
        let source = "第一条 为了加强管理，制定本条例。第二条 各级人民政府要压实属地责任，强化源头管控，严防火灾发生。第三条 附则。";
        let passage = closest_passage(source, "压实属地责任，强化源头治理");
        assert!(passage.contains("压实属地责任，强化源头管控"), "{passage}");
    }
}
