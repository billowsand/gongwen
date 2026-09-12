//! 语料扫描：把稿件库与知识库的正文变成词频明细。
//!
//! 扫描是**增量**的：每个来源按正文哈希记账（`lexicon_scans`），正文没变就整篇
//! 跳过；变了就先删掉它上次的明细再重写，所以同一篇稿件反复保存也不会把词频
//! 算重。整批扫完再统一重算聚合列，不是每篇都重算。

use super::{LexiconStore, MAX_TERM_CHARS, MIN_TERM_CHARS, is_acceptable_term};
use crate::proofread;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// 来源种类，写进 `lexicon_scans.source_kind`。改名会让旧记账失配，别改。
pub const SOURCE_MANUSCRIPT: &str = "manuscript";
pub const SOURCE_KNOWLEDGE: &str = "knowledge";

/// 扫描范围。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// 把草稿也算进语料。默认关：草稿里半成品错字最多，是词表噪声的主要来源。
    pub include_drafts: bool,
    /// 把知识库里从外部 markdown 导入的文档也算进语料。
    /// 从稿件库勾选入库的那些不算——它们是稿件的副本，算两遍等于篇数翻倍。
    pub include_knowledge: bool,
    /// 全部重扫：忽略哈希记账，把每个来源都当成新的。
    pub rescan_all: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            include_drafts: false,
            include_knowledge: true,
            rescan_all: false,
        }
    }
}

/// 一轮扫描的结果摘要。
#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    /// 实际重新分词的来源数。
    pub scanned: usize,
    /// 正文没变、整篇跳过的来源数。
    pub skipped: usize,
    /// 来源已消失（稿件被删）而清掉的记账数。
    pub forgotten: usize,
    /// 本轮新出现的词条数（含已存在但重新计数的，按去重后的词形算）。
    pub distinct_terms: usize,
    /// 被校对词表黑名单挡下的词形数。
    pub blocked: usize,
    pub failed: Vec<(String, String)>,
}

impl ScanSummary {
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("扫描 {} 篇", self.scanned)];
        if self.skipped > 0 {
            parts.push(format!("跳过未变动 {} 篇", self.skipped));
        }
        if self.forgotten > 0 {
            parts.push(format!("清理已删来源 {} 篇", self.forgotten));
        }
        parts.push(format!("收词 {} 个", self.distinct_terms));
        if self.blocked > 0 {
            parts.push(format!("按校对词表挡下错别字 {} 个", self.blocked));
        }
        if !self.failed.is_empty() {
            parts.push(format!("{} 篇失败", self.failed.len()));
        }
        parts.join("，")
    }
}

/// 待扫的一个来源。
struct Source {
    kind: &'static str,
    id: i64,
    title: String,
    markdown: String,
}

/// 在后台线程里跑一整轮扫描。`progress(done, total, title)` 每篇回报一次。
///
/// 与 `knowledge::run_index_pipeline_with` 同形：自己开连接，不借用主线程的
/// store，失败逐篇记录而不中断整批。
pub fn run_scan_with<F: Fn(usize, usize, String)>(
    db_path: PathBuf,
    options: ScanOptions,
    proofread_config: crate::models::ProofreadConfig,
    vocabulary: Vec<crate::models::VocabularyEntry>,
    progress: F,
) -> Result<ScanSummary> {
    let mut store = LexiconStore::open(&db_path)?;
    let mut summary = ScanSummary::default();

    // 先并权威词：标准词库的专名与校对词表的建议写法不经语料也该在词表里，
    // 而且黑名单要在分词入库之前就位。
    store
        .sync_vocabulary(&vocabulary)
        .context("并入标准词库失败")?;
    let lexicon = proofread::Lexicon::resolved(&proofread_config);
    let (_, blacklist) = store.sync_proofread(&lexicon).context("并入校对词表失败")?;
    let blacklist: HashSet<String> = blacklist.into_iter().collect();
    let rejected = store.rejected_terms()?;

    let sources = collect_sources(&db_path, &options)?;
    // 来源消失（稿件被删）时，它贡献过的词频要一起消失，否则词表越攒越脏。
    let manuscript_ids: Vec<i64> = sources
        .iter()
        .filter(|source| source.kind == SOURCE_MANUSCRIPT)
        .map(|source| source.id)
        .collect();
    summary.forgotten += store.forget_missing_sources(SOURCE_MANUSCRIPT, &manuscript_ids)?;
    // 知识库这一路即使这次不扫也要清账：取消勾选「含知识库外部文档」之后，
    // 它们上次贡献的词频必须跟着消失，否则口径改了而数字不动。
    // 草稿那一路不用特殊处理——不勾选时它们本来就不在 `sources` 里，
    // 会被上面这次 forget 一并清掉。
    let knowledge_ids: Vec<i64> = sources
        .iter()
        .filter(|source| source.kind == SOURCE_KNOWLEDGE)
        .map(|source| source.id)
        .collect();
    summary.forgotten += store.forget_missing_sources(SOURCE_KNOWLEDGE, &knowledge_ids)?;

    let known = if options.rescan_all {
        HashMap::new()
    } else {
        store.scanned_hashes()?
    };
    // 挂上已接受的词再分词：标准词库刚并进来的专名（「新舆处」这类）自带词典
    // 不认识，不挂用户词典就会被切碎，语料里永远统计不到它们。
    if let Ok(dict) = store.jieba_user_dict() {
        super::segmenter::install_user_dict(&dict);
    }
    let jieba = super::segmenter::shared();
    let total = sources.len();
    let mut distinct: HashSet<String> = HashSet::new();
    let mut blocked: HashSet<String> = HashSet::new();

    for (index, source) in sources.iter().enumerate() {
        progress(index, total, source.title.clone());
        let hash = super::source_hash(&source.markdown);
        if known.get(&(source.kind.to_string(), source.id)) == Some(&hash) {
            summary.skipped += 1;
            continue;
        }
        let counts = count_terms(
            &jieba,
            &source.markdown,
            &blacklist,
            &rejected,
            &mut blocked,
        );
        for (term, _, _) in &counts {
            distinct.insert(term.clone());
        }
        match store.record_source(source.kind, source.id, &source.title, &hash, &counts) {
            Ok(()) => summary.scanned += 1,
            Err(error) => summary
                .failed
                .push((source.title.clone(), format!("{error:#}"))),
        }
    }
    progress(total, total, String::new());

    store.recompute_aggregates()?;
    summary.distinct_terms = distinct.len();
    summary.blocked = blocked.len();
    Ok(summary)
}

/// 读出待扫的正文。稿件与知识库同库不同表，这里一次性取全，扫描期间不再回查。
fn collect_sources(db_path: &std::path::Path, options: &ScanOptions) -> Result<Vec<Source>> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("无法打开稿件库 {}", db_path.display()))?;
    conn.busy_timeout(std::time::Duration::from_millis(10_000))?;
    let mut sources = Vec::new();

    // 默认只认已发布与已归档：草稿是半成品，错字与临时措辞最多。
    let statuses: &[&str] = if options.include_drafts {
        &["Draft", "Published", "Archived"]
    } else {
        &["Published", "Archived"]
    };
    let placeholders = statuses
        .iter()
        .enumerate()
        .map(|(index, _)| format!("?{}", index + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, title, content_markdown FROM manuscripts WHERE status IN ({placeholders})"
    );
    {
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(statuses.iter()), |row| {
            Ok(Source {
                kind: SOURCE_MANUSCRIPT,
                id: row.get(0)?,
                title: row.get::<_, String>(1)?,
                markdown: row.get::<_, String>(2)?,
            })
        })?;
        for row in rows {
            sources.push(row?);
        }
    }

    if options.include_knowledge {
        // source='manuscript' 的文档是稿件副本，算两遍会让篇数凭空翻倍。
        let mut stmt = conn.prepare(
            "SELECT id, title, content_markdown FROM knowledge_docs WHERE source <> 'manuscript'",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Source {
                kind: SOURCE_KNOWLEDGE,
                id: row.get(0)?,
                title: row.get::<_, String>(1)?,
                markdown: row.get::<_, String>(2)?,
            })
        })?;
        for row in rows {
            sources.push(row?);
        }
    }
    Ok(sources)
}

/// 对一篇正文分词计数。返回 `(词, 次数, jieba 自带词典里是否已有)`。
fn count_terms(
    jieba: &jieba_rs::Jieba,
    markdown: &str,
    blacklist: &HashSet<String>,
    rejected: &HashSet<String>,
    blocked: &mut HashSet<String>,
) -> Vec<(String, i64, bool)> {
    let text = plain_text(markdown);
    let mut counts: HashMap<String, i64> = HashMap::new();
    for token in jieba.cut(&text, true) {
        let word = token.word.trim();
        let chars = word.chars().count();
        if !(MIN_TERM_CHARS..=MAX_TERM_CHARS).contains(&chars) {
            continue;
        }
        if !is_acceptable_term(word) {
            continue;
        }
        // 错别字绝不入库：单独的词表工具从语料里扫到什么收什么，等于把自己
        // 写错过的字固化进输入法。
        if blacklist.contains(word) {
            blocked.insert(word.to_string());
            continue;
        }
        // 拒绝过的词也不必再记明细，省得它在候选表里刷存在感。
        if rejected.contains(word) {
            continue;
        }
        *counts.entry(word.to_string()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(term, count)| {
            let in_base_dict = jieba.has_word(&term);
            (term, count, in_base_dict)
        })
        .collect()
}

/// 把公文 Markdown 压成适合分词的纯文本。
///
/// jieba 只会切出连续汉字，标点与西文本来就不会成词，所以这里只处理真正会
/// **污染词频**的三类东西：代码块（整段原样计数）、HTML 注释里的附件标记，
/// 以及图片/链接的地址。行首的标题号、引用号、列表号一并去掉，免得
/// 「一、工作背景」被切成「一」+「工作背景」之外还多出别的碎片。
pub fn plain_text(markdown: &str) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut in_code = false;
    for raw in markdown.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        let mut line = strip_html_comments(line);
        line = strip_link_targets(&line);
        let trimmed = line
            .trim_start()
            .trim_start_matches(['#', '>', '-', '*', '+'])
            .trim_start();
        // markdown 表格：竖线当分隔符，`|---|---|` 这类分隔行整行丢掉。
        let trimmed = if trimmed.starts_with('|') {
            if trimmed
                .chars()
                .all(|ch| matches!(ch, '|' | '-' | ':' | ' ' | '\t'))
            {
                continue;
            }
            trimmed.replace('|', " ")
        } else {
            trimmed.to_string()
        };
        // 强调与链接的标记本身不成词，留着只会把「**贯彻**落实」切断，去掉。
        // 链接地址已经在 strip_link_targets 里摘走，这里收拾剩下的方括号。
        let cleaned: String = trimmed
            .chars()
            .map(|ch| {
                if matches!(ch, '*' | '_' | '`' | '~' | '[' | ']' | '!') {
                    ' '
                } else {
                    ch
                }
            })
            .collect();
        out.push_str(cleaned.trim());
        out.push('\n');
    }
    out
}

fn strip_html_comments(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            // 注释跨行时，本行剩下的部分整段丢掉。
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// 去掉 `![alt](path)` / `[文字](url)` 里的地址，保留可读的文字部分。
fn strip_link_targets(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find("](") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find(')') {
            Some(close) => rest = &rest[open + 2 + close + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_drops_code_blocks_and_markup() {
        let markdown = "# 关于开展专项整治的通知\n\n\
                        <!-- [附件] -->\n\
                        **贯彻**落实有关精神。\n\
                        ```\nfn main() { 代码里的中文不该计数 }\n```\n\
                        | 序号 | 单位 |\n| --- | --- |\n| 1 | 市新闻舆情处 |\n\
                        见 ![图](images/a.png) 与 [附表](files/b.xlsx)。\n";
        let text = plain_text(markdown);
        assert!(text.contains("关于开展专项整治的通知"));
        assert!(text.contains("贯彻"), "{text}");
        assert!(text.contains("落实有关精神"), "{text}");
        assert!(!text.contains("代码里的中文"), "代码块不该进语料：{text}");
        assert!(!text.contains("images"), "图片地址不该进语料：{text}");
        assert!(text.contains("市新闻舆情处"), "表格单元格该保留：{text}");
        assert!(text.contains("附表"), "链接文字该保留：{text}");
        assert!(!text.contains('['), "方括号该一并清掉：{text}");
    }

    #[test]
    fn count_terms_honours_blacklist_and_rejections() {
        let jieba = super::super::segmenter::shared();
        let mut blacklist = HashSet::new();
        blacklist.insert("布署".to_string());
        let mut rejected = HashSet::new();
        rejected.insert("有关事项".to_string());
        let mut blocked = HashSet::new();
        let counts = count_terms(
            &jieba,
            "布署工作，现将有关事项通知如下，专项整治专项整治。",
            &blacklist,
            &rejected,
            &mut blocked,
        );
        let terms: HashSet<String> = counts.iter().map(|(term, _, _)| term.clone()).collect();
        assert!(!terms.contains("布署"), "错别字不该进词表");
        assert!(blocked.contains("布署"));
        assert!(!terms.contains("有关事项"), "拒绝过的词不该复活");
        // 同一篇里出现两次的词，次数要累加。
        let repeated = counts.iter().find(|(term, _, _)| term == "专项整治");
        if let Some((_, count, _)) = repeated {
            assert_eq!(*count, 2);
        }
    }

    /// 建一个装着若干稿件的临时库，返回库路径。
    fn seeded_db(entries: &[(&str, crate::models::ManuscriptStatus)]) -> tempfile::TempDir {
        use crate::manuscript::{ManuscriptStore, NewManuscript};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        let mut store = ManuscriptStore::open(&path).unwrap();
        for (markdown, status) in entries {
            store
                .create(
                    &NewManuscript {
                        snapshot: crate::models::DraftInput::default(),
                        content_markdown: (*markdown).to_string(),
                        notes: String::new(),
                        status: *status,
                        created_at: None,
                        updated_at: None,
                        published_at: None,
                        archived_at: None,
                    },
                    None,
                )
                .unwrap();
        }
        dir
    }

    fn terms_of(path: &std::path::Path) -> std::collections::HashMap<String, (i64, i64)> {
        let store = LexiconStore::open(path).unwrap();
        store
            .list(&super::super::TermFilter::default())
            .unwrap()
            .into_iter()
            .map(|term| (term.term, (term.freq_total, term.doc_count)))
            .collect()
    }

    /// 两篇正文都提到的那个词。用它做断言而不是某个特定长词，是为了不把
    /// 用例绑在 jieba 对某个词的切法上——这里要验的是累积与记账，不是分词。
    const SHARED_TERM: &str = "专项";

    #[test]
    fn scanning_accumulates_across_manuscripts_and_skips_unchanged_ones() {
        use crate::models::ManuscriptStatus;
        let dir = seeded_db(&[
            (
                "# 关于开展专项整治的通知\n\n现将专项整治有关安排通知如下。",
                ManuscriptStatus::Published,
            ),
            (
                "# 关于专项整治进展的报告\n\n专项整治已经收尾。",
                ManuscriptStatus::Archived,
            ),
            // 草稿默认不进语料。
            ("# 草稿\n\n专项整治尚未定稿。", ManuscriptStatus::Draft),
        ]);
        let path = dir.path().join("m.db");
        let scan = || {
            run_scan_with(
                path.clone(),
                ScanOptions::default(),
                crate::models::ProofreadConfig::default(),
                Vec::new(),
                |_, _, _| {},
            )
            .unwrap()
        };

        let first = scan();
        assert_eq!(first.scanned, 2, "只该扫已发布与已归档这两篇");
        assert_eq!(first.skipped, 0);
        let (freq, docs) = terms_of(&path)[SHARED_TERM];
        assert_eq!(docs, 2, "草稿不计入，出现在两篇里");
        assert_eq!(freq, 4, "两篇合计出现四次");

        // 正文一个字没改，第二轮整篇跳过，统计一模一样。
        let second = scan();
        assert_eq!(second.scanned, 0);
        assert_eq!(second.skipped, 2);
        assert_eq!(terms_of(&path)[SHARED_TERM], (4, 2));

        // 全部重扫也不该把词频算成两倍——来源明细是覆盖写，不是累加。
        let third = run_scan_with(
            path.clone(),
            ScanOptions {
                rescan_all: true,
                ..ScanOptions::default()
            },
            crate::models::ProofreadConfig::default(),
            Vec::new(),
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(third.scanned, 2);
        assert_eq!(terms_of(&path)[SHARED_TERM], (4, 2));
    }

    #[test]
    fn narrowing_the_scope_takes_effect_on_a_plain_scan() {
        use crate::models::ManuscriptStatus;
        let body = "# 通知\n\n现将专项整治有关安排通知如下。";
        let dir = seeded_db(&[
            (body, ManuscriptStatus::Published),
            (body, ManuscriptStatus::Draft),
        ]);
        let path = dir.path().join("m.db");

        run_scan_with(
            path.clone(),
            ScanOptions {
                include_drafts: true,
                ..ScanOptions::default()
            },
            crate::models::ProofreadConfig::default(),
            Vec::new(),
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(terms_of(&path)[SHARED_TERM].1, 2, "含草稿时两篇都算");

        // 取消「含草稿」后普通扫一次，草稿贡献的词频就要跟着消失——
        // 不该要求用户必须点「全部重扫」才能让口径生效。
        let narrowed = run_scan_with(
            path.clone(),
            ScanOptions::default(),
            crate::models::ProofreadConfig::default(),
            Vec::new(),
            |_, _, _| {},
        )
        .unwrap();
        assert_eq!(narrowed.forgotten, 1);
        assert_eq!(
            terms_of(&path)[SHARED_TERM].1,
            1,
            "移出口径的来源不再贡献篇数"
        );
    }

    #[test]
    fn single_characters_and_non_han_never_enter() {
        let jieba = super::super::segmenter::shared();
        let mut blocked = HashSet::new();
        let counts = count_terms(
            &jieba,
            "〔2026〕5 号 Word 的 你 我 他",
            &HashSet::new(),
            &HashSet::new(),
            &mut blocked,
        );
        assert!(counts.is_empty(), "文号、西文与单字都不该成词：{counts:?}");
    }
}
