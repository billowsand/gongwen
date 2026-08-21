//! 检索增强（RAG）纯函数层：切块、分词、向量编解码、余弦相似度、召回融合。
//!
//! 这一层不碰数据库与网络，全部可单测。切块把一篇公文 markdown 按标题层级
//! 拆成几百字的块；向量用小端 f32 字节流存进 SQLite；检索时用自研余弦做
//! 向量召回、用 jieba 预分词配合 FTS5 做关键词召回，两路 RRF 融合。

use crate::models::{RagConfig, TemplateKind};
use std::sync::OnceLock;

static JIEBA: OnceLock<jieba_rs::Jieba> = OnceLock::new();

fn jieba() -> &'static jieba_rs::Jieba {
    JIEBA.get_or_init(jieba_rs::Jieba::new)
}

/// 用 jieba 把文本切成空格分隔的词串，供 FTS5 索引/查询。
/// FTS5 默认 unicode61 分词器会把整段中文当作一个词，必须先分词才能按词命中。
pub fn tokenize(text: &str) -> String {
    jieba().cut(text, true).join(" ")
}

/// 切块参数。
#[derive(Debug, Clone, Copy)]
pub struct ChunkParams {
    pub target_chars: usize,
    pub max_chars: usize,
    pub overlap_chars: usize,
}

impl From<&RagConfig> for ChunkParams {
    /// 夹紧三个参数之间的关系，不信任 config.json 里的手填值：
    /// `target <= max`，且 `overlap <= target/2`。两者任一被违反都会让
    /// 封块后残留的重叠前缀重新触发封块条件，进而无限压块、吃光内存。
    fn from(config: &RagConfig) -> Self {
        let max_chars = config.chunk_max_chars.max(MIN_MAX_CHARS);
        let target_chars = config.chunk_target_chars.clamp(MIN_TARGET_CHARS, max_chars);
        let overlap_chars = config.chunk_overlap_chars.min(target_chars / 2);
        Self {
            target_chars,
            max_chars,
            overlap_chars,
        }
    }
}

/// 参数下限：块再小也要能装下一句公文。
const MIN_TARGET_CHARS: usize = 50;
const MIN_MAX_CHARS: usize = 100;
/// 不足此字数的尾块并回上一块，避免“特此函告。”这类碎块。
const MIN_TAIL_CHARS: usize = 40;
/// `##` 之前的开头部分（导语、称谓、第一段正文）的小节名。
const LEAD_SECTION: &str = "正文";
/// 附件区的小节名。
const ATTACHMENT_SECTION: &str = "附件";

/// 一个切好的块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub ord: usize,
    /// 标题链，如「正文 / 一、工作背景」；附件块为「附件」。
    pub section: String,
    pub text: String,
}

/// 按 `##` 划出的一个原始区段，尚未按字数拆分。
#[derive(Debug, Clone)]
struct Section {
    title: String,
    body: String,
}

/// 把公文 markdown 切成块。**分块主边界是 `##`**：每个二级标题起一块，
/// `##` 之前的开头部分归入「正文」，`<!-- [附件] -->` 之后归入「附件」。
///
/// `###` 及更深的标题不再另起块，标题文字并入所在 `##` 区段的正文——公文的
/// 「（一）……」这层通常只有一两段，单独成块反而割裂上下文，并入后其标题词
/// 也一并进入检索。区段超过 `max_chars` 时才在句读处硬切，并留重叠。
pub fn chunk_markdown(markdown: &str, params: &ChunkParams) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    for section in split_sections(markdown) {
        let body = normalize_body(&section.body);
        if body.is_empty() {
            continue;
        }
        split_section(&section.title, &body, params, &mut chunks);
    }
    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.ord = i;
    }
    chunks
}

/// 扫一遍 markdown，按 `##`（以及每个附件标记、附件正式标题 `#`）切出区段。
fn split_sections(markdown: &str) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut cur = Section {
        title: LEAD_SECTION.to_string(),
        body: String::new(),
    };
    let mut in_attachment = false;

    for raw_line in markdown.lines() {
        let line = raw_line.trim();
        // 导出器用 `<!-- [正文] -->` / `<!-- [附件] -->` 做区段标记。
        if line.starts_with("<!--") && line.ends_with("-->") {
            if line.contains(ATTACHMENT_SECTION) {
                in_attachment = true;
                start_section(&mut out, &mut cur, ATTACHMENT_SECTION.to_string());
            }
            continue;
        }
        match heading_level(line) {
            // `#`：文档大标题，或附件区里的附件正式标题。
            Some(1) => {
                let title = strip_heading(line);
                if in_attachment || title.starts_with(ATTACHMENT_SECTION) {
                    in_attachment = true;
                    start_section(&mut out, &mut cur, title);
                } else {
                    // 大标题本身不成正文；它会在建索引时单独并入每一块的检索文本。
                    start_section(&mut out, &mut cur, LEAD_SECTION.to_string());
                }
            }
            // `##`：分块的主边界。
            Some(2) => start_section(&mut out, &mut cur, strip_heading(line)),
            // `###` 及更深：留在当前块内，标题文字并入正文以便被检索到。
            Some(_) => push_line(&mut cur.body, &strip_heading(line)),
            None => {
                if line.is_empty() {
                    // 空行是自然段边界，用空行保留下来供硬切时优先断开。
                    if !cur.body.is_empty() && !cur.body.ends_with("\n\n") {
                        cur.body.push('\n');
                    }
                } else {
                    push_line(&mut cur.body, line);
                }
            }
        }
    }
    start_section(&mut out, &mut cur, String::new());
    out
}

/// 收尾当前区段（正文非空才收），并以 `title` 开启新区段。
fn start_section(out: &mut Vec<Section>, cur: &mut Section, title: String) {
    if !cur.body.trim().is_empty() {
        out.push(Section {
            title: std::mem::take(&mut cur.title),
            body: std::mem::take(&mut cur.body),
        });
    }
    cur.title = title;
    cur.body.clear();
}

fn push_line(body: &mut String, line: &str) {
    if line.is_empty() {
        return;
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    body.push_str(line);
}

/// 去掉首尾空白，并把 3 个以上连续换行压成段落边界的两个。
fn normalize_body(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut newlines = 0usize;
    for ch in body.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push(ch);
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out.trim().to_string()
}

/// 把一个 `##` 区段按字数拆成若干块。
///
/// 两个参数分工：`max_chars` 是**保持整段不拆**的上限——略超目标字数的小节
/// 宁可整块保留，也不为了凑字数把一个二级标题劈开；真的超了才拆，此时按
/// `target_chars` 填充，让每块大小稳定。
fn split_section(section: &str, body: &str, params: &ChunkParams, out: &mut Vec<Chunk>) {
    let start = out.len();
    if body.chars().count() <= params.max_chars {
        out.push(Chunk {
            ord: 0,
            section: section.to_string(),
            text: body.to_string(),
        });
        return;
    }
    // 区段里若有 markdown 表格，续块一律以表头开场，并且只在行边界断开。
    // 否则拆出来的续块长这样：「| 5 | 示例单位五 | 周海宁 | 109 |……」——
    // 没有列名、还从半行开始，作为参考片段基本没有价值。
    let table = table_header(body);
    let mut rest = body;
    let mut carry = String::new();
    loop {
        // overlap 已夹到 target/2，故 room 恒 ≥ target/2 ≥ 1，循环必然前进。
        let room = params
            .target_chars
            .saturating_sub(carry.chars().count())
            .max(1);
        let mut text = carry.clone();
        if rest.chars().count() <= room {
            push_paragraph(&mut text, rest);
            push_chunk(out, section, &text);
            break;
        }
        // hard_split_point 恒 ≥ 1，故 tail 严格短于 rest —— 循环必然前进。
        let take = hard_split_point(rest, room, table.is_some());
        let (head, tail) = split_at_char(rest, take);
        push_paragraph(&mut text, head.trim());
        push_chunk(out, section, &text);
        if tail.len() >= rest.len() {
            break; // 兜底：万一没前进就停，绝不空转。
        }
        rest = tail.trim_start();
        if rest.is_empty() {
            break;
        }
        carry = match &table {
            // 续块接的是表格行，就带上表头而不是上一块的尾巴。
            Some(header) if rest.trim_start().starts_with('|') => header.clone(),
            _ => overlap_prefix(text.trim(), params.overlap_chars),
        };
    }
    merge_short_tail(out, start, MIN_TAIL_CHARS);
}

/// 找出区段里第一个 markdown 表格的「表头行 + 分隔行」。
///
/// 分隔行形如 `| --- | :--: |`：以 `|` 开头、除 `|-: ` 外没有别的字符，且至少
/// 有一个 `-`。它的上一行就是列名行。
fn table_header(body: &str) -> Option<String> {
    let lines: Vec<&str> = body.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if i == 0 || !trimmed.starts_with('|') || !trimmed.contains('-') {
            continue;
        }
        if trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')) {
            let head = lines[i - 1].trim();
            if head.starts_with('|') {
                return Some(format!("{head}\n{trimmed}"));
            }
        }
    }
    None
}

fn push_paragraph(text: &mut String, addition: &str) {
    let addition = addition.trim();
    if addition.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(addition);
}

fn push_chunk(out: &mut Vec<Chunk>, section: &str, text: &str) {
    let body = text.trim();
    if body.is_empty() {
        return;
    }
    out.push(Chunk {
        ord: out.len(),
        section: section.to_string(),
        text: body.to_string(),
    });
}

/// 标题级别：`# `→1、`## `→2 …… 最多 6 级；其余返回 None。
fn heading_level(line: &str) -> Option<usize> {
    for level in (1..=6).rev() {
        if line.starts_with(&format!("{} ", "#".repeat(level))) {
            return Some(level);
        }
    }
    None
}

/// 去掉行首的 `#` 与空白，留下标题文字。
fn strip_heading(line: &str) -> String {
    line.trim_start_matches('#').trim().to_string()
}

/// 计算在 text 内不超过 max_chars 的切点，尽量落在句号/分号/换行之后。
/// 计算切点：尽量落在句读之后，且不超过 `max_chars`。
///
/// `line_only` 为真时只认换行——表格里的单元格文字常自带句号，按句号切会把
/// 一行数据劈成两半，切出「| 5 | 示例单位五 |」这种残行。此模式下若限额内
/// 一个换行都没有（一行比限额还长），**宁可超出限额也要切到整行末尾**：
/// 表格行是原子单位，切碎了这块就没有参考价值了。
fn hard_split_point(text: &str, max_chars: usize, line_only: bool) -> usize {
    let total = text.chars().count();
    let limit = max_chars.min(total);
    let mut best: Option<usize> = None;
    for (count, ch) in text.chars().enumerate() {
        if count >= limit {
            break;
        }
        let is_break = if line_only {
            ch == '\n'
        } else {
            matches!(ch, '。' | '；' | ';' | '\n' | '！' | '？')
        };
        if is_break {
            best = Some(count + 1); // 切在该分隔符之后
        }
    }
    if let Some(point) = best {
        return point.max(1);
    }
    if line_only {
        // 限额内没有换行：找限额之后的第一个换行，把这一整行带上。
        if let Some(next) = text
            .chars()
            .enumerate()
            .skip(limit)
            .find(|(_, ch)| *ch == '\n')
            .map(|(i, _)| i + 1)
        {
            return next;
        }
        return total.max(1); // 整段就一行，全取。
    }
    limit.max(1)
}

/// 按字符下标切分，返回 (前 n 字符, 剩余)。
fn split_at_char(text: &str, n: usize) -> (&str, &str) {
    let byte = text
        .char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    text.split_at(byte)
}

/// 取块尾 overlap_chars 个字符作为下一块前缀，回退到最近一个句号/分号之后，
/// 让重叠从一句的开头开始，避免半句开头。
fn overlap_prefix(body: &str, overlap_chars: usize) -> String {
    if overlap_chars == 0 || body.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = body.chars().collect();
    let take = overlap_chars.min(chars.len());
    let tail = &chars[chars.len() - take..];
    // 从尾巴里找最后一个句读，从其后开始，保证重叠是一句的完整开头。
    let start = tail
        .iter()
        .rposition(|c| matches!(c, '。' | '；' | ';' | '\n'))
        .map(|i| i + 1)
        .unwrap_or(0);
    tail[start..]
        .iter()
        .collect::<String>()
        .trim_start()
        .to_string()
}

/// 把不足 min_chars 的末尾块并入前一块（如“特此函告。”这类结语碎块）。
/// `start` 是本 `##` 区段在 chunks 里的起始下标——只在同一区段内合并，
/// 绝不跨 `##` 边界，否则会破坏“按二级标题分块”的归属。
fn merge_short_tail(chunks: &mut Vec<Chunk>, start: usize, min_chars: usize) {
    // 区段内至少要有两块才谈得上合并。
    if chunks.len().saturating_sub(start) < 2 {
        return;
    }
    let last_len = chunks.last().map(|c| c.text.chars().count()).unwrap_or(0);
    if last_len >= min_chars {
        return;
    }
    let last = chunks.pop().expect("已确认区段内长度≥2");
    if let Some(prev) = chunks.last_mut() {
        if !prev.text.is_empty() && !last.text.is_empty() {
            prev.text.push('\n');
        }
        prev.text.push_str(&last.text);
    }
}

/// 向量编解码与相似度。
pub mod vector {
    /// 小端 f32 字节流。len 必是 4 的倍数，decode 时校验。
    pub fn encode(v: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(v.len() * 4);
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Vec<f32>> {
        if !bytes.len().is_multiple_of(4) {
            return None;
        }
        Some(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect(),
        )
    }

    /// 余弦相似度。维度不等、空向量或任一向量零范数时返回 0（视为不相关，不 panic）。
    pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..a.len() {
            let (x, y) = (a[i] as f64, b[i] as f64);
            dot += x * y;
            na += x * x;
            nb += y * y;
        }
        if na == 0.0 || nb == 0.0 {
            return 0.0;
        }
        (dot / (na.sqrt() * nb.sqrt())) as f32
    }
}

/// 检索结果块（注入提示词前的载体）。
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    #[allow(dead_code)] // 预留：溯源定位
    pub chunk_id: i64,
    /// 所在知识库文档 id，预览整篇时用。
    pub doc_id: i64,
    pub doc_title: String,
    pub kind: TemplateKind,
    pub section: String,
    pub text: String,
    /// 向量余弦分（未参与向量召回为 0）。
    pub vector_score: f32,
    /// FTS bm25 归一化分（未参与关键词召回为 0）。
    pub bm25_score: f32,
    /// RRF 融合分。
    pub fused_score: f32,
    /// rerank 相关分（未 rerank 为 None）。
    pub rerank_score: Option<f32>,
}

/// RRF 融合：score = Σ 1/(k + rank_i)。输入两路的 (chunk_id, 归一化分) 已按相关度降序。
/// 返回 (chunk_id, fused_score) 按融合分降序的前 limit 条。
pub fn rrf_fuse(
    vector_ranked: &[(i64, f32)],
    fts_ranked: &[(i64, f32)],
    k: f32,
    limit: usize,
) -> Vec<(i64, f32)> {
    use std::collections::HashMap;
    let mut score: HashMap<i64, f32> = HashMap::new();
    let mut vm: HashMap<i64, f32> = HashMap::new();
    let mut fm: HashMap<i64, f32> = HashMap::new();
    for (rank, (id, s)) in vector_ranked.iter().enumerate() {
        *score.entry(*id).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
        vm.insert(*id, *s);
    }
    for (rank, (id, s)) in fts_ranked.iter().enumerate() {
        *score.entry(*id).or_insert(0.0) += 1.0 / (k + rank as f32 + 1.0);
        fm.insert(*id, *s);
    }
    let mut ids: Vec<i64> = score.keys().copied().collect();
    ids.sort_by(|a, b| {
        score[b]
            .partial_cmp(&score[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ids.truncate(limit);
    ids.into_iter().map(|id| (id, score[&id])).collect()
}

/// 一次检索的产物：命中块 + 降级告警。
///
/// `warnings` 是**给用户看**的降级说明：embedding 连不上、rerank 端点不对……
/// 这些以前都被静默吞掉，用户只能靠翻服务端日志才发现自己配的模型根本没生效。
#[derive(Debug, Clone, Default)]
pub struct RetrievalOutcome {
    pub chunks: Vec<RetrievedChunk>,
    pub warnings: Vec<String>,
}

/// 查询串上限：起草时的 query 是「标题 + 整段用户素材」，可能上千字。
/// 过长的 query 会稀释向量语义，也会让 FTS 的 OR 词表膨胀到匹配全库。
const MAX_QUERY_CHARS: usize = 300;

/// 截断过长的查询串（按字符），尽量断在句读处。
fn clamp_query(query: &str) -> String {
    let trimmed = query.trim();
    if trimmed.chars().count() <= MAX_QUERY_CHARS {
        return trimmed.to_string();
    }
    let take = hard_split_point(trimmed, MAX_QUERY_CHARS, false);
    split_at_char(trimmed, take).0.trim().to_string()
}

/// 检索入口：query（用户素材+草稿标题）→ 向量召回 + FTS 召回 → RRF 融合 → rerank 精排。
///
/// 在调用方（起草线程）内同步执行；自己开 `KnowledgeStore` 连接。任一步失败
/// 都尽量降级：embed 失败则只走 FTS，rerank 失败则用融合结果，整体不阻塞起草；
/// 但每次降级都会往 `warnings` 里记一条，由调用方显示给用户。
pub fn retrieve(
    config: &RagConfig,
    chat: &crate::models::LmStudioConfig,
    db_path: &std::path::Path,
    query: &str,
    kind_filter: Option<TemplateKind>,
) -> anyhow::Result<RetrievalOutcome> {
    use crate::knowledge::KnowledgeStore;
    use crate::rag_client;

    let mut store = KnowledgeStore::open(db_path)?;
    let mut warnings: Vec<String> = Vec::new();
    let query = clamp_query(query);
    let query = query.as_str();

    // 向量路：embed query → 全量余弦 → topK。embed 失败降级为空（只靠 FTS）。
    let query_vec = match rag_client::embed(&config.embedding, &[query.to_string()]) {
        Ok(mut v) if !v.is_empty() => Some(v.remove(0)),
        Ok(_) => {
            warnings.push("embedding 服务没有返回向量，本次只用关键词检索。".into());
            None
        }
        Err(error) => {
            warnings.push(format!(
                "embedding 不可用，本次只用关键词检索：{}",
                first_line(&format!("{error:#}"))
            ));
            None
        }
    };
    let vector_ranked: Vec<(i64, f32)> = match &query_vec {
        Some(qv) => {
            let mut scored: Vec<(i64, f32)> = store
                .all_embeddings(kind_filter)?
                .into_iter()
                .map(|(chunk_id, _doc_id, vec)| (chunk_id, vector::cosine(qv, &vec)))
                .filter(|(_, s)| *s > 0.0)
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(config.recall_top_k);
            scored
        }
        None => Vec::new(),
    };

    // 关键词路：jieba 分词 → FTS5 → topK。
    let query_tokens = tokenize(query);
    let fts_ranked = match store.fts_search(&query_tokens, kind_filter, config.fts_top_k) {
        Ok(hits) => hits,
        Err(error) => {
            warnings.push(format!(
                "关键词检索失败：{}",
                first_line(&format!("{error:#}"))
            ));
            Vec::new()
        }
    };

    // RRF 融合。
    let fused = rrf_fuse(
        &vector_ranked,
        &fts_ranked,
        config.rrf_k,
        config.fused_top_m,
    );
    if fused.is_empty() {
        return Ok(RetrievalOutcome {
            chunks: Vec::new(),
            warnings,
        });
    }

    // 取向量/FTS 分备用（注入与测试展示）。
    let vmap: std::collections::HashMap<i64, f32> = vector_ranked.iter().copied().collect();
    let fmap: std::collections::HashMap<i64, f32> = fts_ranked.iter().copied().collect();

    let fused_ids: Vec<i64> = fused.iter().map(|(id, _)| *id).collect();
    let fused_score: std::collections::HashMap<i64, f32> = fused.iter().copied().collect();

    // rerank 精排；不可用或失败则按融合分取 topN。
    let rows = store.chunks_by_ids(&fused_ids)?;
    let mut out: Vec<RetrievedChunk> = rows
        .into_iter()
        .map(|row| RetrievedChunk {
            chunk_id: row.chunk_id,
            doc_id: row.doc_id,
            doc_title: row.doc_title,
            kind: row.kind,
            section: row.section,
            text: row.text,
            vector_score: vmap.get(&row.chunk_id).copied().unwrap_or(0.0),
            bm25_score: fmap.get(&row.chunk_id).copied().unwrap_or(0.0),
            fused_score: fused_score.get(&row.chunk_id).copied().unwrap_or(0.0),
            rerank_score: None,
        })
        .collect();

    // 正文相同的块只留分最高的一个。入库时已按正文哈希跨来源去重，但仍可能
    // 出现同一篇内容以不同标题存了两份（只改了标题再导入一次）——那时每个片段
    // 都会命中两遍，白白吃掉一半的 rerank 名额和注入额度。
    out.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let removed = dedup_by_text(&mut out);
    if removed > 0 {
        warnings.push(format!(
            "有 {removed} 个重复片段已合并——知识库里可能存了内容相同的多篇文档，可在列表中删除多余的。"
        ));
    }

    let mode = config.effective_rerank_mode();
    if mode != crate::models::RerankMode::None {
        // out 此时已按融合分降序，下标即 rerank 的输入序号。
        let docs: Vec<String> = out.iter().map(|c| c.text.clone()).collect();
        let ranked = match mode {
            crate::models::RerankMode::Api => {
                rag_client::rerank(&config.rerank, query, &docs, config.rerank_top_n)
            }
            // 本地模型服务没有 rerank 端点时的替代路径：用对话模型打分。
            crate::models::RerankMode::Llm => {
                rag_client::rerank_with_llm(chat, query, &docs, config.rerank_top_n)
            }
            crate::models::RerankMode::None => unreachable!("已在上面排除"),
        };
        match ranked {
            Ok(ranked) => {
                let mut reranked: Vec<RetrievedChunk> = Vec::new();
                for (idx, score) in ranked.into_iter().take(config.rerank_top_n) {
                    if let Some(mut chunk) = out.get(idx).cloned() {
                        chunk.rerank_score = Some(score);
                        reranked.push(chunk);
                    }
                }
                if reranked.is_empty() {
                    warnings.push("重排没有返回可用结果，本次按融合分排序。".into());
                } else {
                    apply_score_floor(&mut reranked, config.min_score_ratio);
                    return Ok(RetrievalOutcome {
                        chunks: reranked,
                        warnings,
                    });
                }
            }
            Err(error) => warnings.push(format!(
                "重排未生效，本次按融合分排序：{}",
                first_line(&format!("{error:#}"))
            )),
        }
    }

    out.sort_by(|a, b| {
        b.fused_score
            .partial_cmp(&a.fused_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out.truncate(config.rerank_top_n);
    apply_score_floor(&mut out, config.min_score_ratio);
    Ok(RetrievalOutcome {
        chunks: out,
        warnings,
    })
}

/// 相关性下限：按**相对**首条的比例砍掉长尾，而不是设一个绝对分数——
/// rerank/余弦的分值范围因模型而异，绝对阈值换个模型就失准。首条永远保留，
/// 排序依据优先取 rerank 分，其次余弦分；两者都没有（纯 FTS 降级）时不过滤。
fn apply_score_floor(chunks: &mut Vec<RetrievedChunk>, ratio: f32) {
    if ratio <= 0.0 || chunks.len() < 2 {
        return;
    }
    let score_of = |c: &RetrievedChunk| c.rerank_score.unwrap_or(c.vector_score);
    let top = score_of(&chunks[0]);
    // 分值可能为负（部分 rerank 模型输出 logit），此时比例没有意义，跳过。
    if top <= 0.0 {
        return;
    }
    let floor = top * ratio.min(1.0);
    chunks.retain(|c| score_of(c) >= floor);
}

/// 去掉正文重复的块（保留先出现的，即分更高的），返回删掉的条数。
/// 比较时忽略空白差异，与入库去重的口径一致。
fn dedup_by_text(chunks: &mut Vec<RetrievedChunk>) -> usize {
    let before = chunks.len();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    chunks.retain(|chunk| {
        let key: String = chunk.text.split_whitespace().collect();
        seen.insert(key)
    });
    before - chunks.len()
}

/// 取错误信息的首行，避免把多层 context 的长串塞进状态栏。
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or(text).trim().to_string()
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn params() -> ChunkParams {
        ChunkParams {
            target_chars: 60,
            max_chars: 90,
            overlap_chars: 15,
        }
    }

    #[test]
    fn vector_encode_decode_roundtrip() {
        let v = vec![0.5f32, -1.25, 3.0, 0.0];
        let bytes = vector::encode(&v);
        assert_eq!(bytes.len() % 4, 0);
        let back = vector::decode(&bytes).expect("4 的倍数应解码成功");
        assert_eq!(back, v);
    }

    #[test]
    fn vector_decode_rejects_bad_length() {
        assert!(vector::decode(&[0u8; 3]).is_none());
    }

    #[test]
    fn cosine_orthogonal_parallel_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((vector::cosine(&a, &b)).abs() < 1e-6, "正交应≈0");
        let c = vec![2.0, 0.0];
        assert!((vector::cosine(&a, &c) - 1.0).abs() < 1e-5, "同向应≈1");
        let z = vec![0.0, 0.0];
        assert_eq!(vector::cosine(&a, &z), 0.0, "零范数应返回 0");
        assert_eq!(
            vector::cosine(&a, &[1.0, 2.0, 3.0]),
            0.0,
            "维度不等应返回 0"
        );
    }

    /// 分块主边界是 `##`：每个二级标题恰好一块，`##` 之前的开头独立成块。
    #[test]
    fn chunk_splits_on_level_two_headings() {
        let md = "# 关于某某的函\n\n正文开头一段。\n\n## 一、工作背景\n\n背景内容一段。\n\n## 二、工作安排\n\n安排内容一段。";
        let chunks = chunk_markdown(md, &params());
        let sections: Vec<&str> = chunks.iter().map(|c| c.section.as_str()).collect();
        assert_eq!(sections, vec!["正文", "一、工作背景", "二、工作安排"]);
        assert_eq!(chunks[0].text, "正文开头一段。");
        assert_eq!(chunks[1].text, "背景内容一段。");
        assert_eq!(chunks[2].text, "安排内容一段。");
    }

    /// 同一个 `##` 下的多个自然段留在一块里，不再按字数被拆散。
    #[test]
    fn chunk_keeps_one_section_together() {
        let md = "## 一、工作要求\n\n第一段。\n\n第二段。\n\n第三段。";
        let chunks = chunk_markdown(md, &params());
        assert_eq!(chunks.len(), 1, "同一 ## 下不该被拆开，实际 {chunks:#?}");
        assert!(chunks[0].text.contains("第一段。"));
        assert!(chunks[0].text.contains("第三段。"));
    }

    /// `###` 不另起块，其标题文字并入所属 `##` 块的正文，以便被检索到。
    #[test]
    fn chunk_folds_level_three_into_parent_section() {
        let md = "## 二、工作安排\n\n总述一句。\n\n### （一）压减材料\n\n把申请材料压减到三项。";
        let chunks = chunk_markdown(md, &params());
        assert_eq!(chunks.len(), 1, "### 不应另起块");
        assert_eq!(chunks[0].section, "二、工作安排");
        assert!(
            chunks[0].text.contains("（一）压减材料"),
            "### 标题文字应并入正文"
        );
        assert!(chunks[0].text.contains("把申请材料压减到三项。"));
    }

    /// 长表格被拆开时，每个续块都要带上列名行，并且只在行边界断开。
    /// 否则切出来的是「| 5 | 示例单位五 | 周海宁 | 109 |……」——没有列名、
    /// 还从半行开始，作为参考片段没有价值。
    #[test]
    fn chunk_repeats_table_header_and_splits_on_rows() {
        let mut rows = String::from("## 统计表\n\n| 序号 | 单位 | 完成率 |\n| --- | --- | --- |\n");
        for i in 1..=40 {
            // 单元格里带句号，用来验证不会按句号把一行劈成两半。
            rows.push_str(&format!("| {i} | 示例单位{i} | 已完成分类调整。 |\n"));
        }
        let chunks = chunk_markdown(&rows, &params());
        assert!(chunks.len() > 2, "长表格应被拆成多块");
        for (i, chunk) in chunks.iter().enumerate() {
            assert!(
                chunk.text.contains("| 序号 | 单位 | 完成率 |"),
                "第 {i} 块缺列名行：{}",
                chunk.text
            );
            // 除表头两行外，每一行都应是完整的表格行。
            for line in chunk.text.lines().skip(2).filter(|l| !l.trim().is_empty()) {
                assert!(
                    line.trim_start().starts_with('|') && line.trim_end().ends_with('|'),
                    "第 {i} 块出现残行：{line:?}"
                );
            }
        }
    }

    /// 附件区自成块，不与正文混在一起。
    #[test]
    fn chunk_separates_attachments() {
        let md = "# 某函\n\n正文一段。\n\n<!-- [附件] -->\n\n# 附件1\n\n附件正文一段。";
        let chunks = chunk_markdown(md, &params());
        assert!(chunks.iter().any(|c| c.section == "正文"));
        assert!(
            chunks.iter().any(|c| c.section == "附件1"),
            "附件应独立成块，实际 {chunks:#?}"
        );
    }

    #[test]
    fn chunk_splits_long_paragraph() {
        let sentence = "这是一个用来凑字数的长句子，反复出现以制造超长段落。";
        let long_para = sentence.repeat(20);
        let md = format!("# 标题\n\n{long_para}");
        let chunks = chunk_markdown(&md, &params());
        assert!(
            chunks.len() >= 2,
            "超长段落应被切成多块，实际 {}",
            chunks.len()
        );
        for c in &chunks {
            // 允许重叠前缀 + 单句硬切造成的少量超出。
            assert!(
                c.text.chars().count() <= params().max_chars + 60,
                "块长度 {} 超限",
                c.text.chars().count()
            );
        }
    }

    #[test]
    fn chunk_merges_short_tail() {
        let md = format!("# 标题\n\n{}\n\n特此函告。", "正文内容。".repeat(15));
        let chunks = chunk_markdown(&md, &params());
        // “特此函告。”很短，应并入前一块，不单独成块。
        assert!(
            !chunks.iter().any(|c| c.text.trim() == "特此函告。"),
            "短尾巴不应单独成块"
        );
    }

    /// 回归：`overlap >= target` 时封块后残留的重叠前缀会再次触发封块，
    /// 且没有句读可退，旧实现会无限压块直至吃光内存。参数必须先被夹紧。
    #[test]
    fn chunk_params_are_clamped_against_runaway_overlap() {
        let mut config = RagConfig::default();
        config.chunk_target_chars = 100;
        config.chunk_max_chars = 200;
        config.chunk_overlap_chars = 150; // > target，非法
        let params = ChunkParams::from(&config);
        assert!(
            params.overlap_chars * 2 <= params.target_chars,
            "overlap 应被夹到 target/2"
        );
        assert!(params.target_chars <= params.max_chars);

        // 一整段没有任何句读的超长文本——旧实现的死循环触发条件。
        let md = format!("# 标题\n\n{}", "安全生产检查要求落实到位".repeat(60));
        let chunks = chunk_markdown(&md, &params);
        assert!(!chunks.is_empty());
        assert!(chunks.len() < 100, "块数应有限，实际 {}", chunks.len());
    }

    /// target > max 这种手填反了的配置也要能收敛。
    #[test]
    fn chunk_params_clamp_target_to_max() {
        let mut config = RagConfig::default();
        config.chunk_target_chars = 4000;
        config.chunk_max_chars = 100;
        let params = ChunkParams::from(&config);
        assert_eq!(params.max_chars, 100);
        assert_eq!(params.target_chars, 100);
        let md = format!("## 小节\n\n{}", "工作要求。".repeat(200));
        let chunks = chunk_markdown(&md, &params);
        assert!(chunks.len() > 1);
        assert!(chunks.len() < 200, "块数应有限，实际 {}", chunks.len());
    }

    /// 过长的 query 会稀释向量语义、让 FTS 的 OR 词表匹配全库，必须截断。
    #[test]
    fn long_query_is_clamped() {
        let long = "关于开展安全生产大检查的通知。".repeat(80);
        let clamped = clamp_query(&long);
        assert!(clamped.chars().count() <= MAX_QUERY_CHARS);
        assert!(clamped.starts_with("关于开展安全生产"));
        // 短 query 原样保留。
        assert_eq!(clamp_query("  安全生产  "), "安全生产");
    }

    /// 正文相同的片段只留一个——库里存了两份内容相同的文档时，
    /// 以前每个片段都会命中两遍，白白吃掉一半的 rerank 名额。
    #[test]
    fn duplicate_chunks_are_collapsed() {
        let mk = |text: &str, fused: f32| RetrievedChunk {
            chunk_id: 0,
            doc_id: 0,
            doc_title: "t".into(),
            kind: TemplateKind::OfficialLetter,
            section: String::new(),
            text: text.into(),
            vector_score: 0.0,
            bm25_score: 0.0,
            fused_score: fused,
            rerank_score: None,
        };
        let mut chunks = vec![
            mk("为进一步推进标准化建设。", 0.9),
            mk("为进一步推进标准化建设。", 0.8), // 另一篇里的同一段
            mk("报送材料应当数据真实。", 0.7),
            mk("为进一步推进标准化建设。 ", 0.6), // 只差空白
        ];
        let removed = dedup_by_text(&mut chunks);
        assert_eq!(removed, 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].fused_score, 0.9, "应保留分最高的那份");
    }

    /// 相关性下限按与首条的比例砍长尾，首条永远保留。
    #[test]
    fn score_floor_drops_weak_tail_but_keeps_top() {
        let mk = |rerank: f32| RetrievedChunk {
            chunk_id: 0,
            doc_id: 0,
            doc_title: "t".into(),
            kind: TemplateKind::OfficialLetter,
            section: String::new(),
            text: String::new(),
            vector_score: 0.0,
            bm25_score: 0.0,
            fused_score: 0.0,
            rerank_score: Some(rerank),
        };
        let mut chunks = vec![mk(0.9), mk(0.6), mk(0.2), mk(0.05)];
        apply_score_floor(&mut chunks, 0.5); // 下限 0.45
        assert_eq!(chunks.len(), 2, "只应留下 0.9 与 0.6");

        // ratio=0 表示不过滤。
        let mut all = vec![mk(0.9), mk(0.01)];
        apply_score_floor(&mut all, 0.0);
        assert_eq!(all.len(), 2);

        // 首条分数非正（部分模型输出 logit）时比例无意义，不过滤。
        let mut negative = vec![mk(-1.0), mk(-8.0)];
        apply_score_floor(&mut negative, 0.5);
        assert_eq!(negative.len(), 2);
    }

    #[test]
    fn rrf_prefers_two_way_hits() {
        // id=1 两路都靠前，id=2/3 各一路。
        let vector = vec![(1i64, 0.9f32), (2, 0.8)];
        let fts = vec![(1i64, 0.7f32), (3, 0.6)];
        let fused = rrf_fuse(&vector, &fts, 60.0, 10);
        assert_eq!(fused[0].0, 1, "两路都命中的应排最前");
    }

    #[test]
    fn tokenize_splits_chinese() {
        let tokens = tokenize("关于开展工作的通知");
        assert!(tokens.contains(' '), "jieba 分词应产生空格分隔：{tokens}");
    }

    /// 端到端降级：embed/rerank 都不可达（指向无效端口）时，retrieve 仍应靠
    /// FTS 召回返回结果而不报错。
    #[test]
    fn retrieve_degrades_to_fts_when_services_unreachable() {
        use crate::knowledge::{
            KnowledgeSource, KnowledgeStore, NewKnowledgeChunk, NewKnowledgeDoc,
        };
        let dir = std::env::temp_dir().join(format!("gongwen_rag_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("knowledge_test.db");
        let _ = std::fs::remove_file(&db);

        {
            let mut store = KnowledgeStore::open(&db).unwrap();
            let meta = NewKnowledgeDoc {
                source: KnowledgeSource::Markdown,
                source_manuscript_id: None,
                source_path: "/tmp/a.md".into(),
                kind: TemplateKind::OfficialLetter,
                title: "安全生产通知".into(),
                content_markdown: "# 安全生产通知\n\n关于开展安全生产大检查。".into(),
                content_hash: crate::knowledge::content_hash("# 安全生产通知"),
                embed_model: "m".into(),
            };
            let chunk = NewKnowledgeChunk {
                ord: 0,
                section: "正文".into(),
                text: "关于开展安全生产大检查的通知".into(),
                tokens: tokenize("关于开展安全生产大检查的通知"),
                embedding: Some(vector::encode(&[1.0, 0.0, 0.0])),
                dims: 3,
            };
            store.replace_document(&meta, &[chunk]).unwrap();
        }

        let mut config = RagConfig::default();
        // 指向无效端口，embed/rerank 必然失败。
        config.embedding.base_url = "http://127.0.0.1:1/v1".into();
        config.embedding.timeout_seconds = 2;
        config.rerank.base_url = "http://127.0.0.1:1/v1".into();
        config.rerank.timeout_seconds = 2;
        config.rerank.model = "reranker".into();

        let chat = crate::models::LmStudioConfig::default();
        let outcome =
            retrieve(&config, &chat, &db, "安全生产检查", None).expect("降级检索不应报错");
        assert!(!outcome.chunks.is_empty(), "应靠 FTS 召回");
        assert!(outcome.chunks[0].text.contains("安全生产"));
        assert!(
            outcome.chunks[0].rerank_score.is_none(),
            "rerank 不可达应无 rerank 分"
        );
        // 关键：降级不能再是静默的，必须留下能显示给用户的说明。
        assert!(
            outcome.warnings.iter().any(|w| w.contains("embedding")),
            "embedding 不可达应有告警：{:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings.iter().any(|w| w.contains("rerank")),
            "rerank 不可达应有告警：{:?}",
            outcome.warnings
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
