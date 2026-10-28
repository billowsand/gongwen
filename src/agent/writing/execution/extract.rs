//! 提炼素材：按模型窗口大块投送，几份材料拼进同一次请求，观点按要点归纳。
//!
//! 真机（qwen3.6-35b-a3b，32k 窗口估值）上的教训：每块 1000 字左右、逐句逐字摘录，66 KB 材料
//! 切成二十多块，一句原句没对上整块作废；分节备料再按「节数 × 块数」重读一遍。现在一次投送
//! 约半个窗口的原文，观点按完整意思归纳；依据尽量用原文措辞，能在原文里找到就记下精确位置，
//! 找不到就如实标成「概括」，不再因此作废整批。

use super::*;

/// 一次提炼投送多少原文（估算 token）：窗口的一半，且不超过扣掉固定提示词与输出余量后的输入预算。
/// 60k 窗口约投 30k token，32k 窗口约 16k。
pub(super) fn extract_tokens(window: usize, fixed: &str) -> usize {
    let input = crate::agent::budget::input_budget(window)
        .saturating_sub(crate::lmstudio::context::estimate_tokens(fixed) + 1024);
    (window / 2).min(input).max(512)
}

/// 估算 token 折成原文字节：按最密的中文算（UTF-8 一字 3 字节约 1.1 token），英文只会更宽松。
pub(super) fn tokens_to_bytes(tokens: usize) -> usize {
    tokens.saturating_mul(27) / 10
}

/// 一批里的一段原文：哪份材料、哪个范围。
#[derive(Clone, Copy)]
struct Segment {
    source: usize,
    start: usize,
    end: usize,
}

const SYSTEM: &str = "你是公文材料提炼助手。通读给出的全部材料，归纳与总主题有关的观点要点：每条观点概括一个完整意思，可以合并同一份材料里多处相关表述，不要逐句摘抄，也不要拆得过细；保留条件、否定、数字与统计口径，不外推、不补造。每条按三行输出：\n观点：一句话概括\n依据：材料中的关键表述，尽量用原文措辞，可节选\n来源：材料编号，例如 材料2\n条目之间空一行。与主题无关的材料不输出；全部无关只输出：无相关内容。";

/// 把未完成的内容块按预算拼成若干批，逐批提炼。返回失败的块数；取消、超预算等全局错误立即返回。
pub(super) fn extract_content(
    task: &mut WritingTask,
    model: &dyn crate::agent::backend::ModelBackend,
    save: &mut dyn FnMut(&WritingTask) -> anyhow::Result<()>,
) -> anyhow::Result<usize> {
    let query = task.topic_query();
    let mut pending = Vec::new();
    for (index, source) in task.sources.iter().enumerate() {
        if source.role != SourceRole::Content {
            continue;
        }
        for (start, end) in chunk_ranges(&source.text, task.chunk_bytes) {
            if same_chunk(task, source, source.role, None, &query, start, end).is_none() {
                pending.push(Segment {
                    source: index,
                    start,
                    end,
                });
            }
        }
    }
    let batches = pack(&pending, task.chunk_bytes);
    let mut failures = 0;
    for (number, batch) in batches.iter().enumerate() {
        task.status = format!(
            "提炼素材第 {}/{} 批（{} 段原文）",
            number + 1,
            batches.len(),
            batch.len()
        );
        save(task)?;
        match extract_batch(task, model, batch, &query) {
            Ok(()) => save(task)?,
            Err(error) if is_fatal(&error) => return Err(error),
            Err(error) => {
                failures += batch.len();
                mark_failed(
                    task,
                    batch,
                    &query,
                    &format!("{error:#}"),
                    model.cancelled(),
                );
                if let Err(save_error) = save(task) {
                    return Err(fatal(format!("保存失败：{save_error:#}")));
                }
            }
        }
    }
    Ok(failures)
}

/// 只重试一段：界面上「重试此块」。
pub(super) fn retry_segment(
    task: &mut WritingTask,
    source_id: &str,
    start: usize,
    end: usize,
    model: &dyn crate::agent::backend::ModelBackend,
    save: &mut dyn FnMut(&WritingTask) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let source = task
        .sources
        .iter()
        .position(|source| source.id == source_id)
        .ok_or_else(|| anyhow::anyhow!("材料已移除，无法重试此块"))?;
    let query = task.topic_query();
    let batch = [Segment { source, start, end }];
    match extract_batch(task, model, &batch, &query) {
        Ok(()) => save(task),
        Err(error) => {
            if !is_fatal(&error) {
                mark_failed(
                    task,
                    &batch,
                    &query,
                    &format!("{error:#}"),
                    model.cancelled(),
                );
                save(task)?;
            }
            Err(error)
        }
    }
}

/// 按字节预算装批：相邻小段拼在一起，单段本身就是按预算切出来的，不会超。
fn pack(segments: &[Segment], budget: usize) -> Vec<Vec<Segment>> {
    let mut batches: Vec<Vec<Segment>> = Vec::new();
    let mut used = 0;
    for segment in segments {
        let size = segment.end - segment.start;
        if batches.is_empty() || used + size > budget {
            batches.push(Vec::new());
            used = 0;
        }
        used += size;
        if let Some(batch) = batches.last_mut() {
            batch.push(*segment);
        }
    }
    batches
}

fn extract_batch(
    task: &mut WritingTask,
    model: &dyn crate::agent::backend::ModelBackend,
    batch: &[Segment],
    query: &str,
) -> anyhow::Result<()> {
    let mut body = String::new();
    let mut chars = 0;
    for (number, segment) in batch.iter().enumerate() {
        let source = &task.sources[segment.source];
        let text = &source.text[segment.start..segment.end];
        chars += text.chars().count();
        body.push_str(&format!(
            "【材料{}】《{}》（原文 {}..{}）\n{}\n\n",
            number + 1,
            source.title,
            segment.start,
            segment.end,
            text
        ));
    }
    // 粒度：约每 700–1500 字一条，按完整意思归纳，不逐句拆。
    let fewest = (chars / 1500).max(3);
    let most = (chars / 700).clamp(fewest + 2, 60);
    let prompt = format!(
        "文种：{}\n总主题：{}\n本批共 {} 份材料，约 {} 字，建议归纳 {}–{} 条观点。\n\n{}",
        task.kind,
        task.topic,
        batch.len(),
        chars,
        fewest,
        most,
        body
    );
    let mut last_output = String::new();
    for attempt in 0..=MAX_RETRIES {
        let retry = if attempt == 0 {
            ""
        } else {
            "\n\n上次没有按格式输出。每条必须有「观点：」「依据：」「来源：材料编号」三行；全部无关只输出：无相关内容。"
        };
        let response = complete(
            task,
            model,
            crate::agent::backend::ModelRole::Assist,
            SYSTEM,
            &format!("{prompt}{retry}"),
        )?;
        if is_no_relevant(&response) {
            record(task, batch, query, Vec::new());
            return Ok(());
        }
        let cards = parse(&response, task, batch, query);
        if !cards.is_empty() {
            record(task, batch, query, cards);
            return Ok(());
        }
        last_output = response;
    }
    anyhow::bail!(
        "模型没有按「观点 / 依据 / 来源」格式输出；最后一次回答开头：{}",
        clip_chars(&last_output, 200)
    )
}

/// 成功的一批：卡片入库，每段按有没有卡片记「已提炼」或「无相关内容」。
fn record(task: &mut WritingTask, batch: &[Segment], query: &str, cards: Vec<MaterialCard>) {
    for segment in batch {
        let source = task.sources[segment.source].clone();
        let found = cards.iter().any(|card| {
            card.source_id == source.id && card.start >= segment.start && card.end <= segment.end
        });
        upsert_chunk(
            task,
            ChunkProgress {
                source_id: source.id.clone(),
                chunk_hash: chunk_hash(&source, segment.start, segment.end),
                role: source.role,
                section_id: None,
                start: segment.start,
                end: segment.end,
                status: if found {
                    ChunkStatus::Extracted
                } else {
                    ChunkStatus::NoRelevantContent
                },
                error: None,
                output: String::new(),
                query: query.to_string(),
            },
        );
    }
    task.topic_cards.extend(cards);
}

fn mark_failed(
    task: &mut WritingTask,
    batch: &[Segment],
    query: &str,
    error: &str,
    cancelled: bool,
) {
    for segment in batch {
        let source = task.sources[segment.source].clone();
        upsert_chunk(
            task,
            ChunkProgress {
                source_id: source.id.clone(),
                chunk_hash: chunk_hash(&source, segment.start, segment.end),
                role: source.role,
                section_id: None,
                start: segment.start,
                end: segment.end,
                status: if cancelled {
                    ChunkStatus::Pending
                } else {
                    ChunkStatus::Failed
                },
                error: Some(format!(
                    "《{}》原文 {}..{}：{error}",
                    source.title, segment.start, segment.end
                )),
                output: String::new(),
                query: query.to_string(),
            },
        );
    }
}

/// 解析「观点 / 依据 / 来源」三行一组的回答。缺依据或来源对不上的条目丢掉，其余照收；
/// 依据能在原文里找到就记精确位置，找不到记为概括、位置标在它所属的那段。
fn parse(raw: &str, task: &WritingTask, batch: &[Segment], query: &str) -> Vec<MaterialCard> {
    #[derive(Default)]
    struct Item {
        point: String,
        evidence: String,
        source: Option<usize>,
    }
    let mut items: Vec<Item> = Vec::new();
    for line in raw.lines() {
        let line = line
            .trim()
            .trim_start_matches(['-', '*', ' '])
            .trim_matches('*');
        if let Some(value) = field(line, "观点") {
            items.push(Item {
                point: value.to_string(),
                ..Item::default()
            });
        } else if let Some(value) = field(line, "依据").or_else(|| field(line, "原句")) {
            if let Some(item) = items.last_mut() {
                item.evidence = value
                    .trim_matches(|c| matches!(c, '"' | '“' | '”'))
                    .to_string();
            }
        } else if let Some(value) = field(line, "来源")
            && let Some(item) = items.last_mut()
        {
            let digits: String = value
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(char::is_ascii_digit)
                .collect();
            item.source = digits
                .parse::<usize>()
                .ok()
                .and_then(|number| number.checked_sub(1));
        }
    }
    let mut cards = Vec::new();
    for item in items {
        if item.point.is_empty() || item.evidence.is_empty() {
            continue;
        }
        let text_of =
            |segment: &Segment| &task.sources[segment.source].text[segment.start..segment.end];
        // 来源编号缺了或写错时，按依据在哪段原文里找得到来认；只有一段时就是它。
        let segment = item
            .source
            .and_then(|index| batch.get(index))
            .or_else(|| {
                batch
                    .iter()
                    .find(|segment| find_quote(text_of(segment), &item.evidence).is_some())
            })
            .or_else(|| (batch.len() == 1).then(|| &batch[0]));
        let Some(segment) = segment else {
            continue;
        };
        let source = &task.sources[segment.source];
        let text = text_of(segment);
        let card = match find_quote(text, &item.evidence) {
            Some((start, end)) => MaterialCard {
                source_id: source.id.clone(),
                quote: text[start..end].to_string(),
                point: item.point,
                start: segment.start + start,
                end: segment.start + end,
                status: "依据已在原文定位，观点未核实".into(),
                query: query.to_string(),
                located: true,
                ..MaterialCard::default()
            },
            None => MaterialCard {
                source_id: source.id.clone(),
                quote: item.evidence,
                point: item.point,
                start: segment.start,
                end: segment.end,
                status: "依据为模型概括，未逐字定位，观点未核实".into(),
                query: query.to_string(),
                located: false,
                ..MaterialCard::default()
            },
        };
        cards.push(card);
    }
    cards
}

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?;
    let rest = rest.strip_prefix('：').or_else(|| rest.strip_prefix(':'))?;
    Some(rest.trim())
}
