//! 风格管理（`docs/ai-agent-workbench.md` 16.15 C）：从指定的几篇稿子学出一份「风格档案」，
//! 起草、改写时按这次的内容挑一份带进提示词。
//!
//! - **档案**：写法描述（模型学、用户可改）、统计特征（程序算）、范例片段、适用文种与场合、学自哪些稿件；
//! - **学**：程序先算统计特征、挑候选范例；样稿放不进上下文时分批先出小结；模型写写法描述并从候选里
//!   定范例。只学写法、不学事实：样稿里的单位、人名、日期、数字出现在描述里的，标出来让用户删；
//! - **用**：按文种与场合关键词给档案打分，只有一份最高就用它，几份分不出交辅助模型挑，都对不上不用；
//!   用户也可以在输入框底栏指定或关掉。范例标明只学写法、不得照搬，也不进证据包——照抄进稿子的事实
//!   照样会被来源不明检查查出来；
//! - **存**：配置目录 `styles.json`，只存本机；可导出导入（`.json`）。
//!
//! 与润色预设分工：预设是一句指令，风格是学出来的一整套写法。

use super::backend::{ModelBackend, ModelRole};
use super::board::RefSource;
use super::tools::{ASSIST_SYSTEM, short};
use crate::lmstudio::context::estimate_tokens;
use crate::models::{TemplateKind, VocabularyEntry};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

const FILE: &str = "styles.json";
/// 一次最多学几篇。
pub(crate) const MAX_SAMPLES: usize = 10;
/// 范例最多几段。
const MAX_EXAMPLES: usize = 5;
/// 一段范例最多多少字。
const EXAMPLE_CHARS: usize = 220;

/// 一份风格档案。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StyleProfile {
    pub(crate) id: String,
    pub(crate) name: String,
    /// 一句话说明。
    pub(crate) note: String,
    /// 适用文种；空表示不限。
    pub(crate) kinds: Vec<TemplateKind>,
    /// 适用场合关键词：「讲话」「调研报告」「对下部署」。
    pub(crate) occasions: Vec<String>,
    /// 写法描述：基调、开头、主体、常用表达、结尾、用词偏好与忌用、数字与时间。
    pub(crate) description: String,
    pub(crate) stats: StyleStats,
    pub(crate) examples: Vec<StyleExample>,
    pub(crate) sources: Vec<StyleSource>,
    /// RFC3339。
    pub(crate) learned_at: String,
    pub(crate) uses: u32,
    pub(crate) enabled: bool,
    /// 这些文种分不出时默认用它。
    pub(crate) default_for: Vec<TemplateKind>,
}

impl Default for StyleProfile {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            note: String::new(),
            kinds: Vec::new(),
            occasions: Vec::new(),
            description: String::new(),
            stats: StyleStats::default(),
            examples: Vec::new(),
            sources: Vec::new(),
            learned_at: String::new(),
            uses: 0,
            enabled: true,
            default_for: Vec::new(),
        }
    }
}

/// 程序算的统计特征。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StyleStats {
    pub(crate) docs: usize,
    pub(crate) chars: usize,
    /// 平均句长、段长（字）。
    pub(crate) sentence_chars: usize,
    pub(crate) paragraph_chars: usize,
    /// 小标题与条目的编号样式及出现次数：「一、」「（一）」「1.」「一是」……
    pub(crate) numbering: Vec<(String, usize)>,
    /// 常用开头语、结尾语。
    pub(crate) openers: Vec<String>,
    pub(crate) closers: Vec<String>,
    /// 常用四字词与惯用语。
    pub(crate) phrases: Vec<String>,
}

impl StyleStats {
    /// 人看的要点，也是交给模型的「统计特征」。
    pub(crate) fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "样稿 {} 篇、约 {} 字；句子平均约 {} 字，段落平均约 {} 字",
            self.docs, self.chars, self.sentence_chars, self.paragraph_chars
        )];
        if !self.numbering.is_empty() {
            let items: Vec<String> = self
                .numbering
                .iter()
                .map(|(style, count)| format!("{style}（{count} 处）"))
                .collect();
            lines.push(format!("编号样式：{}", items.join("、")));
        }
        if !self.openers.is_empty() {
            lines.push(format!("常用开头：{}", self.openers.join("、")));
        }
        if !self.closers.is_empty() {
            lines.push(format!("常用结尾：{}", self.closers.join("、")));
        }
        if !self.phrases.is_empty() {
            lines.push(format!("常用说法：{}", self.phrases.join("、")));
        }
        lines
    }
}

/// 一段范例。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StyleExample {
    /// 开头 / 过渡 / 结尾 / 正文。
    pub(crate) role: String,
    pub(crate) text: String,
    /// 出自哪篇（标题）。
    pub(crate) source: String,
}

/// 学自哪篇。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StyleSource {
    pub(crate) source: RefSource,
    pub(crate) id: i64,
    pub(crate) title: String,
    /// 学的时候的内容指纹；对不上说明样稿后来改了。
    pub(crate) hash: String,
}

/// 全部风格档案（`styles.json`）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct StyleBook {
    pub(crate) styles: Vec<StyleProfile>,
}

impl StyleBook {
    pub(crate) fn load() -> anyhow::Result<Self> {
        super::api::read_json(FILE)
    }

    pub(crate) fn save(&self) -> anyhow::Result<()> {
        super::api::write_json(FILE, self)
    }

    pub(crate) fn get(&self, id: &str) -> Option<&StyleProfile> {
        self.styles.iter().find(|style| style.id == id)
    }

    /// 有同 id 的就换掉，没有就加在最后。
    pub(crate) fn upsert(&mut self, profile: StyleProfile) {
        match self.styles.iter_mut().find(|style| style.id == profile.id) {
            Some(slot) => *slot = profile,
            None => self.styles.push(profile),
        }
    }

    pub(crate) fn remove(&mut self, id: &str) {
        self.styles.retain(|style| style.id != id);
    }

    /// 某份档案用了一次。
    pub(crate) fn record_use(&mut self, id: &str) {
        if let Some(style) = self.styles.iter_mut().find(|style| style.id == id) {
            style.uses += 1;
        }
    }

    /// 导出成 `.json`（与 `styles.json` 同格式），给同事导入。
    pub(crate) fn export(&self, path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// 导入：同 id 的覆盖，其余追加；返回导入了几份。
    pub(crate) fn import(&mut self, path: &std::path::Path) -> anyhow::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("不是风格档案文件：{e}"))?;
        // 整本（`{"styles": [...]}`）或单份档案都认。
        let incoming: StyleBook = if value.get("styles").is_some() {
            serde_json::from_value(value)
        } else {
            serde_json::from_value::<StyleProfile>(value).map(|one| StyleBook { styles: vec![one] })
        }
        .map_err(|e| anyhow::anyhow!("不是风格档案文件：{e}"))?;
        let count = incoming.styles.len();
        for style in incoming.styles {
            self.upsert(style);
        }
        Ok(count)
    }
}

/// 一篇样稿。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sample {
    pub(crate) source: RefSource,
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) kind: Option<TemplateKind>,
    pub(crate) text: String,
}

/// 内容指纹（样稿有没有改过）。
pub(crate) fn content_hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.trim().as_bytes())[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 样稿后来改过的那几篇（标题）。`current` 给出各篇现在的内容指纹，读不到的不算。
pub(crate) fn stale_sources(
    profile: &StyleProfile,
    current: &dyn Fn(&StyleSource) -> Option<String>,
) -> Vec<String> {
    profile
        .sources
        .iter()
        .filter(|source| current(source).is_some_and(|hash| hash != source.hash))
        .map(|source| source.title.clone())
        .collect()
}

/// 去掉 Markdown 记号后的非空行。
fn plain_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['#', '>', '*', '-', ' '])
                .replace("**", "")
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

static NUMBERING: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("一、", r"^[一二三四五六七八九十]+、"),
        ("（一）", r"^[（(][一二三四五六七八九十]+[）)]"),
        ("1.", r"^\d{1,2}[.．、]\s*\D"),
        ("（1）", r"^[（(]\d{1,2}[）)]"),
        ("第一，", r"^第[一二三四五六七八九十]+[，,、]"),
        ("首先……其次", r"^(首先|其次|再次|最后)[，,]"),
    ]
    .into_iter()
    .map(|(label, pattern)| (label, Regex::new(pattern).expect("编号正则")))
    .collect()
});

static INLINE_NUMBERING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[一二三四五六七八九十]是").expect("一是正则"));

/// 程序算统计特征。
pub(crate) fn compute_stats(samples: &[Sample]) -> StyleStats {
    let mut stats = StyleStats {
        docs: samples.len(),
        ..StyleStats::default()
    };
    let mut sentences = (0usize, 0usize);
    let mut paragraphs = (0usize, 0usize);
    let mut numbering: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut openers: BTreeMap<String, usize> = BTreeMap::new();
    let mut closers: Vec<String> = Vec::new();
    let mut phrase_docs: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let jieba = crate::lexicon::segmenter::shared();
    for sample in samples {
        let lines = plain_lines(&sample.text);
        stats.chars += lines.iter().map(|l| l.chars().count()).sum::<usize>();
        // 标题（`#` 开头的第一行）与主送（冒号结尾）不算正文段落。
        let titled = sample
            .text
            .lines()
            .find(|l| !l.trim().is_empty())
            .is_some_and(|l| l.trim_start().starts_with('#'));
        let mut body: Vec<&str> = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if index == 0 && titled {
                continue;
            }
            let heading = NUMBERING.iter().find(|(_, re)| re.is_match(line));
            if let Some((label, _)) = heading {
                *numbering.entry(label).or_default() += 1;
            }
            let inline = INLINE_NUMBERING.find_iter(line).count();
            if inline >= 2 {
                *numbering.entry("一是……二是").or_default() += 1;
            }
            let len = line.chars().count();
            // 短的编号行是小标题、冒号结尾的是主送，都不算段落。
            if (heading.is_some() && len <= 30) || line.ends_with(['：', ':']) {
                continue;
            }
            body.push(line);
            paragraphs.0 += len;
            paragraphs.1 += 1;
            for sentence in line.split(['。', '！', '？', '；']) {
                let n = sentence.chars().filter(|c| !c.is_whitespace()).count();
                if n >= 4 {
                    sentences.0 += n;
                    sentences.1 += 1;
                }
            }
            // 开头语：正文段段首到第一个逗号，4–12 字。
            if len >= 15
                && let Some(head) = line.split(['，', ',']).next()
            {
                let n = head.chars().count();
                if (4..=12).contains(&n) && head.chars().all(|c| !c.is_ascii_digit()) {
                    *openers.entry(head.to_string()).or_default() += 1;
                }
            }
        }
        // 结尾语：最后一段的最后一句。
        if let Some(last) = lines.iter().rev().find(|l| l.chars().count() >= 4) {
            let sentence = last
                .trim_end_matches(['。', '！'])
                .rsplit(['。', '！', '？'])
                .next()
                .unwrap_or(last)
                .trim();
            if (2..=24).contains(&sentence.chars().count())
                && !closers.contains(&sentence.to_string())
            {
                closers.push(sentence.to_string());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        let joined = body.join("\n");
        let hanzi = |word: &str, n: usize| {
            word.chars().count() == n
                && word.chars().all(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        };
        // 四字词：分词就是四个字的，或两个相邻的双字词（「统筹」+「推进」）。
        let tokens: Vec<&str> = jieba
            .cut(&joined, false)
            .into_iter()
            .map(|t| t.word)
            .collect();
        let mut words: Vec<String> = tokens
            .iter()
            .filter(|w| hanzi(w, 4))
            .map(|w| w.to_string())
            .collect();
        words.extend(
            tokens
                .windows(2)
                .filter(|pair| hanzi(pair[0], 2) && hanzi(pair[1], 2))
                .map(|pair| format!("{}{}", pair[0], pair[1])),
        );
        for word in words {
            let entry = phrase_docs.entry(word.clone()).or_default();
            entry.1 += 1;
            if seen.insert(word) {
                entry.0 += 1;
            }
        }
    }
    stats.sentence_chars = sentences.0.checked_div(sentences.1).unwrap_or(0);
    stats.paragraph_chars = paragraphs.0.checked_div(paragraphs.1).unwrap_or(0);
    let mut numbering: Vec<(String, usize)> = numbering
        .into_iter()
        .map(|(label, count)| (label.to_string(), count))
        .collect();
    numbering.sort_by_key(|item| std::cmp::Reverse(item.1));
    stats.numbering = numbering;
    let mut openers: Vec<(String, usize)> = openers.into_iter().filter(|(_, n)| *n >= 2).collect();
    openers.sort_by_key(|item| std::cmp::Reverse(item.1));
    stats.openers = openers.into_iter().take(8).map(|(text, _)| text).collect();
    stats.closers = closers.into_iter().take(6).collect();
    let need_docs = if samples.len() >= 2 { 2 } else { 1 };
    let mut phrases: Vec<(String, (usize, usize))> = phrase_docs
        .into_iter()
        .filter(|(_, (docs, times))| *docs >= need_docs && *times >= 2)
        .collect();
    phrases.sort_by_key(|item| std::cmp::Reverse(item.1));
    stats.phrases = phrases.into_iter().take(16).map(|(word, _)| word).collect();
    stats
}

/// 候选范例：每篇的开头段、一段过渡（「为此」「同时」「一是」开头的）与结尾段。
pub(crate) fn candidate_examples(samples: &[Sample]) -> Vec<StyleExample> {
    let mut out = Vec::new();
    for sample in samples {
        let paragraphs: Vec<String> = plain_lines(&sample.text)
            .into_iter()
            .filter(|line| line.chars().count() >= 30)
            .collect();
        let mut push = |role: &str, text: &str| {
            let text = short(text, EXAMPLE_CHARS);
            if !out.iter().any(|e: &StyleExample| e.text == text) {
                out.push(StyleExample {
                    role: role.into(),
                    text,
                    source: sample.title.clone(),
                });
            }
        };
        if let Some(first) = paragraphs.first() {
            push("开头", first);
        }
        if let Some(middle) = paragraphs.iter().skip(1).find(|p| {
            [
                "为此",
                "同时",
                "此外",
                "一是",
                "要",
                "各地",
                "各单位",
                "下一步",
            ]
            .iter()
            .any(|lead| p.starts_with(lead))
        }) {
            push("过渡", middle);
        }
        if paragraphs.len() >= 2
            && let Some(last) = paragraphs.last()
        {
            push("结尾", last);
        }
    }
    out
}

fn numbered(candidates: &[StyleExample]) -> String {
    candidates
        .iter()
        .enumerate()
        .map(|(i, e)| format!("[{}]（{}·《{}》）{}", i + 1, e.role, e.source, e.text))
        .collect::<Vec<_>>()
        .join("\n")
}

fn samples_text(samples: &[Sample]) -> String {
    samples
        .iter()
        .enumerate()
        .map(|(i, s)| format!("【样稿 {}《{}》】\n{}", i + 1, s.title, s.text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 一批样稿的写法小结（样稿放不进上下文时先分批）。
fn batch_prompt(samples: &str) -> String {
    format!(
        "下面是同一单位的几篇公文样稿。只看写法，不管内容：用 200 字以内概括这几篇的总体基调、开头怎么起、\
         主体怎么组织（层次、小标题、段落长短）、常用表达与句式、结尾怎么收、用词偏好。\
         不要写出样稿里的单位名称、人名、日期、数字和具体事项。\n\n{samples}"
    )
}

/// 合成风格档案的提示词。
fn learn_prompt(stats: &StyleStats, material: &str, candidates: &[StyleExample]) -> String {
    format!(
        "下面是同一单位的几篇公文样稿（或它们的写法小结）和程序统计的特征。请总结这组样稿的**写法风格**，\
         供以后起草同类公文时模仿。\n\
         规矩：只学写法，不学事实——样稿里的单位名称、人名、日期、数字、具体事项一律不得写进描述，\
         需要举例时用「××」代替。\n\n\
         严格按下面的格式输出，不要别的内容：\n\
         名称：（6–12 字，概括这种风格，如「对下部署类通知」）\n\
         适用场合：（2–5 个关键词，用顿号分隔，如「部署、通知、对下」）\n\
         写法描述：\n\
         总体基调：……\n开头：……\n主体：……\n常用表达：……\n结尾：……\n用词偏好与忌用：……\n数字与时间：……\n\
         范例：（从下面的候选范例里挑 3–5 段最能代表这种风格的，写编号，用顿号分隔）\n\n\
         【统计特征】\n{}\n\n【候选范例】\n{}\n\n{material}",
        stats.lines().join("\n"),
        numbered(candidates)
    )
}

/// 模型的回复 → (名称, 场合, 写法描述, 范例编号)。
fn parse_learned(reply: &str) -> (String, Vec<String>, String, Vec<usize>) {
    let mut name = String::new();
    let mut occasions = Vec::new();
    let mut description = Vec::new();
    let mut picks = Vec::new();
    let mut in_description = false;
    for raw in reply.lines() {
        let line = raw
            .trim()
            .trim_start_matches(['*', '#', ' '])
            .replace("**", "");
        let line = line.trim();
        let value = |key: &str| {
            line.strip_prefix(key)
                .map(|rest| rest.trim_start_matches(['：', ':', ' ']).trim().to_string())
        };
        if let Some(v) = value("名称") {
            name = v.trim_matches(['「', '」', '“', '”', '"']).to_string();
            in_description = false;
        } else if let Some(v) = value("适用场合") {
            occasions = v
                .split(['、', '，', ',', '；', ';', ' '])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            in_description = false;
        } else if let Some(v) = value("写法描述") {
            in_description = true;
            if !v.is_empty() {
                description.push(v);
            }
        } else if let Some(v) = value("范例") {
            in_description = false;
            static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("编号"));
            picks = NUMBER
                .find_iter(&v)
                .filter_map(|m| m.as_str().parse::<usize>().ok())
                .collect();
        } else if in_description && !line.is_empty() {
            description.push(line.to_string());
        }
    }
    (name, occasions, description.join("\n"), picks)
}

/// 学的结果：档案草稿与要用户看一眼的事实。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Learned {
    pub(crate) profile: StyleProfile,
    /// 档案描述里出现的样稿事实值（单位、人名、日期、数字……），让用户删。
    pub(crate) flagged: Vec<String>,
}

/// 档案的名称、场合、写法描述里出现的样稿事实（只学写法，不学事实）。范例是原文，不查。
pub(crate) fn fact_flags(
    profile: &StyleProfile,
    samples: &[Sample],
    vocabulary: &[VocabularyEntry],
) -> Vec<String> {
    let text = format!(
        "{}\n{}\n{}",
        profile.name,
        profile.occasions.join("、"),
        profile.description
    );
    let mut flagged = Vec::new();
    for sample in samples {
        for fact in crate::ai_guard::extract_key_facts(&sample.text, vocabulary) {
            let value = fact.value.trim().to_string();
            if value.chars().count() >= 2 && text.contains(&value) && !flagged.contains(&value) {
                flagged.push(value);
            }
        }
    }
    flagged
}

/// 从样稿学一份风格档案。`on_phase` 报进度。
pub(crate) fn learn(
    model: &dyn ModelBackend,
    samples: &[Sample],
    vocabulary: &[VocabularyEntry],
    on_phase: &mut dyn FnMut(String),
) -> anyhow::Result<Learned> {
    if samples.is_empty() {
        anyhow::bail!("没有可学的样稿");
    }
    let stats = compute_stats(samples);
    let candidates = candidate_examples(samples);
    let window = model.window(ModelRole::Assist).tokens;
    let fixed = learn_prompt(&stats, "", &candidates);
    let room = super::budget::room_chars(window, &format!("{ASSIST_SYSTEM}{fixed}"));
    let all = samples_text(samples);
    let material = if all.chars().count() <= room {
        format!("【样稿】\n{all}")
    } else {
        // 放不下：分批先出小结，再合成。
        let batch_room = super::budget::room_chars(window, &batch_prompt(""));
        let mut batches: Vec<String> = Vec::new();
        let mut current = String::new();
        for (i, sample) in samples.iter().enumerate() {
            let one = format!(
                "【样稿 {}《{}》】\n{}",
                i + 1,
                sample.title,
                sample.text.trim()
            );
            for piece in super::budget::split_by_budget(&one, batch_room) {
                if current.chars().count() + piece.chars().count() > batch_room
                    && !current.is_empty()
                {
                    batches.push(std::mem::take(&mut current));
                }
                current.push_str(&piece);
                current.push_str("\n\n");
            }
        }
        if !current.trim().is_empty() {
            batches.push(current);
        }
        let mut summaries = Vec::new();
        for (i, batch) in batches.iter().enumerate() {
            if model.cancelled() {
                anyhow::bail!("已停止");
            }
            on_phase(format!(
                "样稿较多，分批小结（{}/{}）…",
                i + 1,
                batches.len()
            ));
            let reply = model.complete(
                ModelRole::Assist,
                ASSIST_SYSTEM,
                &batch_prompt(batch),
                &mut |_| {},
            )?;
            summaries.push(format!("【第 {} 批小结】\n{}", i + 1, reply.content.trim()));
        }
        summaries.join("\n\n")
    };
    on_phase("总结写法风格…".into());
    let reply = model.complete(
        ModelRole::Assist,
        ASSIST_SYSTEM,
        &learn_prompt(&stats, &material, &candidates),
        &mut |_| {},
    )?;
    let (name, occasions, description, picks) = parse_learned(&reply.content);
    if description.trim().is_empty() {
        anyhow::bail!("模型没按格式给出写法描述，可以再学一次");
    }
    let mut examples: Vec<StyleExample> = picks
        .iter()
        .filter_map(|n| n.checked_sub(1).and_then(|i| candidates.get(i)).cloned())
        .take(MAX_EXAMPLES)
        .collect();
    if examples.is_empty() {
        examples = candidates.iter().take(3).cloned().collect();
    }
    let mut kinds: Vec<TemplateKind> = Vec::new();
    for kind in samples.iter().filter_map(|s| s.kind) {
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
    }
    let profile = StyleProfile {
        name: if name.is_empty() {
            "新风格".into()
        } else {
            short(&name, 20)
        },
        occasions,
        description,
        stats,
        examples,
        kinds,
        sources: samples
            .iter()
            .map(|s| StyleSource {
                source: s.source,
                id: s.id,
                title: s.title.clone(),
                hash: content_hash(&s.text),
            })
            .collect(),
        learned_at: chrono::Local::now().to_rfc3339(),
        ..StyleProfile::default()
    };
    let flagged = fact_flags(&profile, samples, vocabulary);
    Ok(Learned { profile, flagged })
}

/// 输入框底栏的风格选择。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum StyleChoice {
    /// 按文种与场合自动挑。
    #[default]
    Auto,
    /// 指定一份（档案 id）。
    Fixed(String),
    /// 这次不用风格。
    Off,
}

/// 自动挑的结果。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ranked {
    Picked(Box<StyleProfile>),
    /// 几份分不出高下，交辅助模型挑。
    Tie(Vec<StyleProfile>),
    Nothing,
}

/// 按文种与场合关键词给档案打分（16.15 C.3）：适用文种必须对得上（不限的也算），场合关键词每命中一个
/// 2 分，设为这个文种的默认再加 1 分；0 分不用。
pub(crate) fn rank(styles: &[StyleProfile], kind: TemplateKind, text: &str) -> Ranked {
    let mut scored: Vec<(usize, &StyleProfile)> = styles
        .iter()
        .filter(|s| s.enabled && (s.kinds.is_empty() || s.kinds.contains(&kind)))
        .map(|s| {
            let hits = s
                .occasions
                .iter()
                .filter(|word| !word.trim().is_empty() && text.contains(word.trim()))
                .count();
            (hits * 2 + usize::from(s.default_for.contains(&kind)), s)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by_key(|item| std::cmp::Reverse(item.0));
    match scored.as_slice() {
        [] => Ranked::Nothing,
        [(top, first), rest @ ..] => {
            let tied: Vec<StyleProfile> = rest
                .iter()
                .filter(|(score, _)| score == top)
                .map(|(_, s)| (*s).clone())
                .collect();
            if tied.is_empty() {
                Ranked::Picked(Box::new((*first).clone()))
            } else {
                let mut all = vec![(*first).clone()];
                all.extend(tied);
                Ranked::Tie(all)
            }
        }
    }
}

/// 几份分不出时让辅助模型挑。返回下标；模型说都不合适返回 None。
pub(crate) fn choose_by_model(
    model: &dyn ModelBackend,
    candidates: &[StyleProfile],
    request: &str,
) -> anyhow::Result<Option<usize>> {
    let list: Vec<String> = candidates
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}. {}（场合：{}）{}",
                i + 1,
                s.name,
                s.occasions.join("、"),
                short(&s.description, 80)
            )
        })
        .collect();
    let prompt = format!(
        "用户要写一篇公文，要求如下。从下面几种写法风格里挑最合适的一种，只回答编号；都不合适回答 0。\n\n\
         【要求】\n{}\n\n【风格】\n{}",
        short(request, 600),
        list.join("\n")
    );
    let reply = model.complete(ModelRole::Assist, ASSIST_SYSTEM, &prompt, &mut |_| {})?;
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("编号"));
    Ok(NUMBER
        .find(&reply.content)
        .and_then(|m| m.as_str().parse::<usize>().ok())
        .and_then(|n| n.checked_sub(1))
        .filter(|i| *i < candidates.len()))
}

/// 带进提示词的样子（`{style}`）。超出 `budget`（token）时先砍范例、再截写法描述。
pub(crate) fn render(profile: &StyleProfile, budget: usize) -> String {
    let head = format!(
        "【写法风格：{}】（从本单位样稿学来的写法，照着写。范例只学写法：里面的单位、人名、日期、数字和具体事项\
         一律不得照搬，也不能当出处）",
        profile.name
    );
    let stats = profile
        .stats
        .lines()
        .into_iter()
        .skip(1)
        .collect::<Vec<_>>()
        .join("；");
    let body = |examples: &[StyleExample], description: &str| {
        let mut text = format!("{head}\n{}", description.trim());
        if profile.stats.sentence_chars > 0 {
            text.push_str(&format!(
                "\n程序统计：句子平均约 {} 字，段落平均约 {} 字",
                profile.stats.sentence_chars, profile.stats.paragraph_chars
            ));
            if !stats.is_empty() {
                text.push('；');
                text.push_str(&stats);
            }
        }
        if !examples.is_empty() {
            text.push_str("\n范例（只学写法）：");
            for example in examples {
                text.push_str(&format!("\n［{}］{}", example.role, example.text));
            }
        }
        text
    };
    let mut examples = profile.examples.clone();
    loop {
        let text = body(&examples, &profile.description);
        if estimate_tokens(&text) <= budget {
            return text;
        }
        if examples.pop().is_none() {
            let room =
                super::budget::tokens_to_chars(budget).saturating_sub(head.chars().count() + 20);
            return body(&[], &short(&profile.description, room.max(40)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::ScriptedModel;
    use super::*;

    fn sample(id: i64, title: &str, text: &str) -> Sample {
        Sample {
            source: RefSource::Manuscript,
            id,
            title: title.into(),
            kind: Some(TemplateKind::PlainDocument),
            text: text.into(),
        }
    }

    const NOTICE_A: &str = "# 关于做好冬季森林防火工作的通知\n\n各县（区）人民政府，市直有关单位：\n\n为深入贯彻落实上级决策部署，切实做好今冬明春森林防火工作，确保人民群众生命财产安全，现就有关事项通知如下。\n\n一、提高思想认识\n\n各地要充分认识森林防火工作的重要性，压实责任，统筹推进，坚决守住不发生重特大森林火灾的底线。\n\n二、强化工作措施\n\n一是加强巡查，二是严控火源，三是做好应急准备。各地要统筹推进，压实责任。\n\n特此通知。";
    const NOTICE_B: &str = "# 关于开展安全生产大检查的通知\n\n各县（区）人民政府：\n\n为深入贯彻落实安全生产工作部署，切实防范化解重大安全风险，现就开展安全生产大检查有关事项通知如下。\n\n一、检查范围\n\n各地要压实责任，统筹推进，对辖区内企业开展拉网式排查，做到全覆盖、无死角。\n\n二、工作要求\n\n一是加强组织领导，二是强化督导检查，三是严格责任追究。\n\n特此通知。";

    #[test]
    fn stats_capture_numbering_openers_closers_and_phrases() {
        let stats = compute_stats(&[sample(1, "防火", NOTICE_A), sample(2, "安全", NOTICE_B)]);
        assert_eq!(stats.docs, 2);
        let styles: Vec<&str> = stats.numbering.iter().map(|(s, _)| s.as_str()).collect();
        assert!(styles.contains(&"一、"), "{styles:?}");
        assert!(styles.contains(&"一是……二是"), "{styles:?}");
        assert!(
            stats.closers.contains(&"特此通知".to_string()),
            "{:?}",
            stats.closers
        );
        assert!(
            stats.phrases.contains(&"统筹推进".to_string()),
            "{:?}",
            stats.phrases
        );
        assert!(
            stats.phrases.contains(&"压实责任".to_string()),
            "{:?}",
            stats.phrases
        );
        assert!(stats.sentence_chars > 10);
        assert!(stats.lines()[0].starts_with("样稿 2 篇"));
    }

    #[test]
    fn candidates_take_openings_transitions_and_endings() {
        let candidates = candidate_examples(&[sample(1, "防火", NOTICE_A)]);
        let roles: Vec<&str> = candidates.iter().map(|e| e.role.as_str()).collect();
        assert_eq!(roles, ["开头", "过渡", "结尾"], "{candidates:?}");
        assert!(candidates[0].text.starts_with("为深入贯彻落实"));
        assert!(candidates.iter().all(|e| e.source == "防火"));
    }

    #[test]
    fn learning_parses_the_reply_and_flags_copied_facts() {
        let model = ScriptedModel::new(|_, _| {
            "名称：对下部署类通知\n适用场合：部署、通知、对下\n写法描述：\n总体基调：庄重、干脆。\n开头：「为深入贯彻落实……现就有关事项通知如下」。\n主体：一、二、三分条，条内用一是二是。\n常用表达：压实责任、统筹推进。\n结尾：特此通知。\n用词偏好与忌用：不用口语。\n数字与时间：今冬明春这类说法；如森林防火工作。\n范例：1、3".into()
        });
        let samples = [sample(1, "防火", NOTICE_A), sample(2, "安全", NOTICE_B)];
        let learned = learn(&model, &samples, &[], &mut |_| {}).unwrap();
        let profile = &learned.profile;
        assert_eq!(profile.name, "对下部署类通知");
        assert_eq!(profile.occasions, ["部署", "通知", "对下"]);
        assert!(profile.description.starts_with("总体基调：庄重、干脆。"));
        assert!(profile.description.contains("结尾：特此通知。"));
        assert_eq!(profile.examples.len(), 2);
        assert_eq!(profile.examples[0].role, "开头");
        assert_eq!(profile.sources.len(), 2);
        assert_eq!(profile.sources[0].hash, content_hash(NOTICE_A));
        assert_eq!(profile.kinds, [TemplateKind::PlainDocument]);
        // 「今冬明春」不是事实；「森林防火」这类事项没进事实提取，只认单位、人名、日期、数字、文件名。
        let prompt = &model.calls.borrow()[0].1;
        assert!(prompt.contains("只学写法，不学事实"), "{prompt}");
        assert!(
            prompt.contains("【候选范例】\n[1]（开头·《防火》）"),
            "{prompt}"
        );
    }

    #[test]
    fn copied_facts_in_the_description_are_flagged() {
        let samples = [sample(
            1,
            "会议",
            "定于2025年11月3日召开全市森林防火工作会议，会期1天。",
        )];
        let profile = StyleProfile {
            description: "开头写「定于2025年11月3日召开」。".into(),
            ..StyleProfile::default()
        };
        let flagged = fact_flags(&profile, &samples, &[]);
        assert!(
            flagged.iter().any(|f| f.contains("2025年11月3日")),
            "{flagged:?}"
        );
    }

    #[test]
    fn many_samples_are_summarised_in_batches() {
        let model = ScriptedModel::new(|_, prompt| {
            if prompt.starts_with("下面是同一单位的几篇公文样稿。只看写法") {
                "写法小结：庄重。".into()
            } else {
                "名称：长稿风格\n适用场合：报告\n写法描述：\n总体基调：平实。\n范例：1".into()
            }
        })
        .with_window(8192);
        let long = format!(
            "{}\n",
            "为深入贯彻落实上级部署，各地要压实责任、统筹推进。".repeat(120)
        );
        let samples: Vec<Sample> = (0..3)
            .map(|i| sample(i, &format!("长稿{i}"), &long))
            .collect();
        let mut phases = Vec::new();
        let learned = learn(&model, &samples, &[], &mut |p| phases.push(p)).unwrap();
        assert_eq!(learned.profile.name, "长稿风格");
        assert!(model.asked("只看写法") >= 2, "分批小结");
        assert!(phases.iter().any(|p| p.contains("分批小结")), "{phases:?}");
        let last = model.calls.borrow().last().unwrap().1.clone();
        assert!(last.contains("【第 1 批小结】"), "合成时用的是小结");
    }

    fn profile(name: &str, kinds: &[TemplateKind], occasions: &[&str]) -> StyleProfile {
        StyleProfile {
            name: name.into(),
            kinds: kinds.to_vec(),
            occasions: occasions.iter().map(|s| s.to_string()).collect(),
            description: format!("{name}的写法"),
            ..StyleProfile::default()
        }
    }

    #[test]
    fn styles_are_ranked_by_kind_and_occasion() {
        let plain = TemplateKind::PlainDocument;
        let letter = TemplateKind::OfficialLetter;
        let styles = vec![
            profile("部署通知", &[plain], &["部署", "通知"]),
            profile("讲话稿", &[], &["讲话"]),
            profile("复函", &[letter], &["复函"]),
        ];
        assert!(
            matches!(rank(&styles, plain, "起草一份部署防火的通知"), Ranked::Picked(p) if p.name == "部署通知")
        );
        assert!(
            matches!(rank(&styles, plain, "写个领导讲话"), Ranked::Picked(p) if p.name == "讲话稿")
        );
        assert_eq!(
            rank(&styles, letter, "写个通知"),
            Ranked::Nothing,
            "文种对不上不用"
        );
        assert_eq!(
            rank(&styles, plain, "随便写写"),
            Ranked::Nothing,
            "都对不上不用"
        );
        // 设为默认：分不出时用它。
        let mut with_default = styles.clone();
        with_default[0].default_for = vec![plain];
        assert!(
            matches!(rank(&with_default, plain, "随便写写"), Ranked::Picked(p) if p.name == "部署通知")
        );
        // 同分：交模型。
        let tie = vec![profile("甲", &[], &["通知"]), profile("乙", &[], &["通知"])];
        assert!(matches!(rank(&tie, plain, "写个通知"), Ranked::Tie(list) if list.len() == 2));
        // 停用的不参与。
        let mut off = styles;
        off[0].enabled = false;
        assert_eq!(rank(&off, plain, "部署通知"), Ranked::Nothing);
    }

    #[test]
    fn the_model_breaks_ties_or_declines() {
        let candidates = vec![profile("甲", &[], &["通知"]), profile("乙", &[], &["通知"])];
        let model = ScriptedModel::new(|_, _| "2".into());
        assert_eq!(
            choose_by_model(&model, &candidates, "写个通知").unwrap(),
            Some(1)
        );
        let model = ScriptedModel::new(|_, _| "0".into());
        assert_eq!(
            choose_by_model(&model, &candidates, "写个通知").unwrap(),
            None
        );
    }

    #[test]
    fn rendering_respects_the_budget_and_drops_examples_first() {
        let mut style = profile("部署通知", &[], &["通知"]);
        style.description = "总体基调：庄重。".into();
        style.examples = (0..4)
            .map(|i| StyleExample {
                role: "开头".into(),
                text: format!("范例{i}：{}", "为深入贯彻落实".repeat(20)),
                source: "防火".into(),
            })
            .collect();
        let full = render(&style, 10_000);
        assert!(full.contains("范例（只学写法）"));
        assert!(full.contains("不得照搬"));
        assert_eq!(full.matches("［开头］").count(), 4);
        let tight = render(&style, 300);
        assert!(
            estimate_tokens(&tight) <= 300,
            "{}",
            estimate_tokens(&tight)
        );
        assert!(tight.contains("总体基调：庄重。"));
        assert!(tight.matches("［开头］").count() < 4);
    }

    #[test]
    fn books_round_trip_and_import_merges_by_id() {
        let dir = std::env::temp_dir().join(format!("gongwen-styles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut book = StyleBook::default();
        let a = profile("甲", &[], &["通知"]);
        book.upsert(a.clone());
        book.record_use(&a.id);
        let path = dir.join("export.json");
        book.export(&path).unwrap();
        let mut other = StyleBook::default();
        other.upsert(profile("乙", &[], &[]));
        assert_eq!(other.import(&path).unwrap(), 1);
        assert_eq!(other.styles.len(), 2);
        assert_eq!(other.get(&a.id).unwrap().uses, 1);
        assert_eq!(other.import(&path).unwrap(), 1, "同 id 覆盖，不重复");
        assert_eq!(other.styles.len(), 2);
        // 单份档案的文件也认。
        std::fs::write(&path, serde_json::to_string(&a).unwrap()).unwrap();
        let mut third = StyleBook::default();
        assert_eq!(third.import(&path).unwrap(), 1);
    }

    /// 连真实模型从样稿文件学一份风格档案，打出来并写到 `tmp/live-style-<序号>.json`。默认忽略；
    /// 环境变量：`GONGWEN_LIVE_LLM_URL` / `_MODEL` / `_KEY`，`GONGWEN_LIVE_SAMPLES`（样稿文件，分号分隔），
    /// `GONGWEN_LIVE_STYLE_OUT`（输出文件名）。
    #[test]
    #[ignore = "需要真实模型"]
    fn live_style_learn() {
        let env = |key: &str| std::env::var(key).unwrap_or_default();
        if env("GONGWEN_LIVE_LLM_URL").is_empty() || env("GONGWEN_LIVE_SAMPLES").is_empty() {
            eprintln!("未设置联机测试的环境变量，跳过");
            return;
        }
        let mut config = crate::models::AppConfig::default();
        config.lm_studio.base_url = env("GONGWEN_LIVE_LLM_URL");
        config.lm_studio.model = env("GONGWEN_LIVE_LLM_MODEL");
        config.lm_studio.api_key = env("GONGWEN_LIVE_LLM_KEY");
        config.lm_studio.timeout_seconds = 300;
        let model = crate::agent::backend::LmBackend::new(
            &config,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let samples: Vec<Sample> = env("GONGWEN_LIVE_SAMPLES")
            .split(';')
            .enumerate()
            .map(|(i, path)| {
                let text = std::fs::read_to_string(path.trim()).expect("样稿文件");
                let title = text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches('#')
                    .trim()
                    .to_string();
                Sample {
                    source: RefSource::Manuscript,
                    id: i as i64 + 1,
                    title,
                    kind: Some(TemplateKind::PlainDocument),
                    text,
                }
            })
            .collect();
        let started = std::time::Instant::now();
        let learned = learn(&model, &samples, &[], &mut |p| eprintln!("  {p}")).unwrap();
        eprintln!("—— 用时 {:?} ——", started.elapsed());
        let profile = &learned.profile;
        eprintln!(
            "名称：{}\n场合：{:?}\n{}",
            profile.name, profile.occasions, profile.description
        );
        for line in profile.stats.lines() {
            eprintln!("  统计：{line}");
        }
        for example in &profile.examples {
            eprintln!(
                "  范例［{}·{}］{}",
                example.role, example.source, example.text
            );
        }
        eprintln!("  要删的事实：{:?}", learned.flagged);
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join(match env("GONGWEN_LIVE_STYLE_OUT") {
                name if name.is_empty() => "live-style.json".to_string(),
                name => name,
            });
        std::fs::write(&out, serde_json::to_string_pretty(profile).unwrap()).unwrap();
        eprintln!("{}", out.display());
    }

    #[test]
    fn edited_samples_are_reported_stale() {
        let style = StyleProfile {
            sources: vec![StyleSource {
                source: RefSource::Manuscript,
                id: 1,
                title: "防火".into(),
                hash: content_hash("旧"),
            }],
            ..StyleProfile::default()
        };
        assert!(stale_sources(&style, &|_| Some(content_hash("旧"))).is_empty());
        assert_eq!(
            stale_sources(&style, &|_| Some(content_hash("新"))),
            ["防火"]
        );
        assert!(stale_sources(&style, &|_| None).is_empty(), "读不到的不算");
    }
}
