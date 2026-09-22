//! 候选生成：按模式分派查询，整句转换与词级查找，位置展开。

use super::*;
use crate::engine::fuma::FumaCodes;

mod english_tail;
mod expanded;
mod result;
mod snapshot;

use expanded::Expanded;

pub(crate) use english_tail::EnglishTail;
pub use result::Query;
pub(super) use result::join_marked;
pub(super) use snapshot::QuerySnapshot;

impl Engine {
    /// 解析当前缓冲区并生成排好序的候选。**不带译文**，译文由 [`Self::annotate`] 补。
    ///
    /// 光标停在拼音中间时只按光标前的那段算候选（`ni|hao` 出 你），光标后的拼音留着，
    /// 上屏之后接着组句；见 [`Composition::scope`]。
    pub fn query(&self) -> Result<Query, ParseError> {
        self.last_rescored.set(false);
        let mut query = match self.query_inner() {
            Ok(query) => query,
            Err(error) => {
                if !self
                    .custom_phrases
                    .iter()
                    .any(|p| p.enabled && p.code == self.composition.scope())
                {
                    return Err(error);
                }
                Query::custom_only(
                    self.composition.text(),
                    self.composition.cursor(),
                    self.shuangpin.is_some(),
                    self.composition.scope(),
                    self.marked_rest(self.composition.rest()),
                )
            }
        };
        self.insert_custom_phrases(&mut query.candidates.items);
        // 给输入日志留个摘要：上屏时才知道选了什么，这里才知道看到了什么
        let pinyin = match &query.correction {
            Some(correction) => correction.segmentation.joined("'"),
            None => join_marked(&query.segmentations, &query.tail),
        };
        *self.last_query.borrow_mut() = Some(QuerySnapshot {
            scope: self.composition.scope().to_owned(),
            pinyin,
            corrected: query.correction.is_some(),
            candidates: query
                .candidates
                .items
                .iter()
                .take(QuerySnapshot::MAX_CANDIDATES)
                .map(|c| c.text.clone())
                .collect(),
            rescored: self.last_rescored.get(),
        });
        Ok(query)
    }

    pub(super) fn query_inner(&self) -> Result<Query, ParseError> {
        let start = Instant::now();
        let keys = self.composition.scope();
        let rest = self.marked_rest(self.composition.rest());
        if self.english_mode {
            return Ok(self.query_english(keys, rest, start));
        }
        if is_raw(keys, self.shuangpin, self.fuma_enabled()) {
            return Ok(self.query_raw(keys, rest, start));
        }
        // 双拼先解成全拼（音节间已用 `'` 连好，切分没有歧义），之后与全拼同路；解不动的键当尾巴。
        // 辅码两码都敲了时 decode 已把它们剥掉，这里留着敲出的码供过滤 / 置顶用
        let decoded = self.decode(keys);
        let fuma_codes = self.fuma_codes(keys);
        // 只有两码那档排除别的候选；首码那档不排他，英文词与 emoji 照常出
        let fuma_filters = matches!(fuma_codes, Some(FumaCodes::Both(_)));
        let scope: &str = decoded.as_ref().map_or(keys, |d| d.pinyin());
        // 末尾是英文词（`woxiangxuehaorust`）：拼音候选与整句只按头段算，尾段整个跟在整句后面。
        // 整段也能读成拼音时（`database`、`…rust` 当简拼）两种读法比分，英文赢了才按头段算，
        // 输了整段按拼音读、英文读法排在拼音整句后面
        let english_tail = if decoded.is_none() {
            self.split_english_tail(keys)
        } else {
            None
        };
        let head_wins = english_tail
            .as_ref()
            .is_some_and(|t| !t.competes || self.mixed_beats_plain(keys, t));
        let parsed = match (&decoded, &english_tail) {
            (Some(d), _) => d
                .segmentation()
                .map(|s| (vec![s], d.tail()))
                .ok_or(ParseError::NoSegmentation),
            (None, Some(tail)) if head_wins => {
                parser::segment(&keys[..tail.head_len]).map(|s| (s, ""))
            }
            _ => segment_longest_prefix(keys),
        };
        // 连第一个字母都切不动（`impor`）：拼音这边没戏，但英文词 / 补全、快捷候选还可以有
        let (segmentations, tail) = match parsed {
            Ok(parsed) => parsed,
            Err(error) => {
                let mut items = Vec::new();
                self.insert_english(&mut items, true);
                self.insert_shortcuts(&mut items, keys);
                if items.is_empty() {
                    return Err(error);
                }
                return Ok(Query {
                    segmentations: Vec::new(),
                    candidates: CandidateList { items },
                    tail: keys.to_owned(),
                    text: self.composition.text().to_owned(),
                    cursor: self.composition.cursor(),
                    rest,
                    decoded_keys: self.shuangpin.is_some(),
                    typed_display: decoded.as_ref().map(|d| d.marked()),
                    // 连第一个字母都切不动，辅码不可能激活
                    fuma: None,
                    correction: None,
                    timings: Timings {
                        parse: start.elapsed(),
                        lookup: Duration::ZERO,
                        rank: Duration::ZERO,
                    },
                });
            }
        };
        // 拼音「不像话」时试拼写纠错；纠正生效则按纠正后的切分查词，原串只用来记学习与显示
        let unlikely = correction::unlikely_pinyin(segmentations.first(), tail)
            || correction::trailing_single_letter(segmentations.first());
        let correction = if unlikely {
            self.active_correction(scope)
        } else {
            None
        };
        let (segmentations, tail): (Vec<Segmentation>, &str) = match &correction {
            Some(c) => (vec![c.segmentation.clone()], ""),
            None => (segmentations, tail),
        };
        let parse = start.elapsed();

        let start = Instant::now();
        let mut scored = Vec::new();
        // 不同切分共享很多前缀（`zh g d o…` 的各种切法前几段一样），同一次查询里同一个模式只查一遍
        let mut memo: HashMap<String, Vec<Match<'_>>> = HashMap::new();
        for segmentation in &segmentations {
            let mut patterns = segmentation.patterns();
            let count = patterns.len();
            let last = &segmentation.syllables[count - 1];
            // 最后一个音节即使打完了也可能还没打完（`xia` 可能是 `xiang` 的前缀），按前缀查；双拼两键就是定局
            if last.complete && decoded.is_none() && parser::is_syllable_prefix(&last.text) {
                patterns[count - 1].complete = false;
            }
            // 词级候选只按敲的原样查，敲错变体只进整句词图（它的候选从那边插进来）：
            // 词级排序把音节数对得上的排最前，敲错命中的词（`kaif` → 咖啡）会把更长的原样词挤到后面
            let positions: Vec<Vec<qingjian_dictionary::SyllablePattern<'_>>> = patterns
                .iter()
                .map(|p| {
                    vec![qingjian_dictionary::SyllablePattern {
                        text: p.text,
                        complete: p.complete,
                    }]
                })
                .collect();
            let positions_ref: &[Vec<qingjian_dictionary::SyllablePattern<'_>>] = &positions;
            let abbreviated = abbreviated_count(&patterns);
            // 每条命中都是敲的原音节，penalty 直接给 0（单字母简拼能命中几万条）
            let hits = self.lookup_all(positions_ref);
            scored.reserve(hits.len());
            for hit in hits {
                let full_last = last.complete
                    && hit.syllables().nth(count - 1) == Some(patterns[count - 1].text);
                scored.push(Scored {
                    hit,
                    full_last,
                    coverage: segmentation.letters(),
                    abbreviated,
                    weight: self.learner.weight(hit.text),
                    fuma_hit: false,
                    penalty: 0.0,
                });
            }
            // 输入的前缀也出候选（`kaifazhe` → 开发、开），否则长句没法逐词上屏。
            // 只收音节数正好等于前缀长度的词，更长的词会与输入后面的音节冲突。
            // 前缀不含最后一个位置，因此可复用上面的扩展结果。
            for prefix_len in (1..count).rev() {
                let prefix = &patterns[..prefix_len];
                let prefix_letters: usize = prefix.iter().map(|p| p.text.len()).sum();
                let hits = memo
                    .entry(pattern_key(prefix))
                    .or_insert_with(|| self.lookup_exact_all(&positions[..prefix_len]));
                let abbreviated = abbreviated_count(prefix);
                for hit in hits.iter().copied() {
                    scored.push(Scored {
                        // 对整个输入来说它不是精确命中，只是覆盖了前面一部分
                        hit: Match {
                            exact: false,
                            ..hit
                        },
                        full_last: true,
                        coverage: prefix_letters,
                        abbreviated,
                        weight: self.learner.weight(hit.text),
                        fuma_hit: false,
                        penalty: 0.0,
                    });
                }
            }
        }
        let lookup = start.elapsed();

        // 辅码：两码都敲了就严格过滤，词级候选按文本首末字形码对敲出的两码，对不上不出（语义同水杉）；
        // 只敲了首码时不排除任何候选（`ljm` 的 蓝莓 还要出），只把首码对得上的标出来由排序顶到最前。
        // 前缀候选也按它们自己的文本算（开发 覆不满 kai'fa'zhe 的时候同样过这道闸）
        match fuma_codes {
            Some(FumaCodes::Both(codes)) => {
                scored.retain(|item| self.fuma_expected(item.hit.text) == Some(codes));
            }
            Some(codes @ FumaCodes::First(_)) => {
                for item in &mut scored {
                    item.fuma_hit = self
                        .fuma_expected(item.hit.text)
                        .is_some_and(|expected| codes.admits(expected));
                }
            }
            None => {}
        }

        let start = Instant::now();
        // 再往后翻也翻不到的候选不必再造：单字母简拼能命中两万个词，排完序只留前面这些。
        // 同输入串（候选覆盖的那段字母）下选过的优先；上下文是上一个上屏的词（句首为 None）：
        // `ba` 在「做了」后面出 吧、句首出 把
        let log_total = (self.total_frequency() as f64).max(1.0).ln();
        let letters = choice_key(scope, scope.len());
        ranking::rank(&mut scored, MAX_CANDIDATES, |item| {
            let hit = &item.hit;
            // 纠错生效时覆盖的是纠正后的字母，换算回原串再查「这个输入串下选过什么」
            let covered = correction
                .as_ref()
                .map_or(item.coverage, |c| c.edit.to_original(item.coverage));
            let choice = letters
                .get(..covered)
                .map_or(0, |input| self.learner.choice_weight(input, hit.text));
            let log_prob = sentence::transition_log_prob(
                &*self.language_model,
                self.personal(),
                self.chain.context(),
                hit.text,
                sentence::fallback_log_prob(hit.frequency, log_total),
            );
            (choice, log_prob)
        });
        let mut items: Vec<Candidate> = scored
            .into_iter()
            .map(|s| Candidate {
                text: s.hit.text.to_owned(),
                kind: CandidateKind::Chinese,
                syllables: s.hit.syllables().map(str::to_owned).collect(),
                reading: None,
                translation: None,
                fuma: None,
            })
            .collect();
        // 中文优先：整句先进去占第一，英文词紧跟其后（第二）；关掉时英文词先进、整句排在开头的英文后面。
        // 辅码两码那档不出 emoji 与英文补全：敲辅码就是在选字。但整串正好是个英文词时（`rust` 被读成
        // ru + 辅码 st）英文词照出，排在辅码筛出的字后面（筛空了就是第一），不然空格 / 回车只剩 ru
        if self.chinese_first || fuma_filters {
            self.insert_sentence(
                &mut items,
                &segmentations,
                correction.is_none(),
                english_tail.as_ref().filter(|_| correction.is_none()),
                head_wins,
            );
            self.insert_english(&mut items, unlikely && !fuma_filters);
        } else {
            self.insert_english(&mut items, unlikely);
            self.insert_sentence(
                &mut items,
                &segmentations,
                correction.is_none(),
                english_tail.as_ref().filter(|_| correction.is_none()),
                head_wins,
            );
        }
        // 快捷候选按敲的键认（`rq` 日期），双拼下也是
        self.insert_shortcuts(&mut items, keys);
        if !fuma_filters {
            self.insert_emoji(&mut items);
        }
        // 敲了辅码就给候选标上各自的辅码：用户看着 蓝 cm、缆 cf 才知道下次该敲哪个码。
        // 没敲辅码时不标，候选窗照旧（标注那一栏要留给译文）
        if fuma_codes.is_some() {
            for item in &mut items {
                item.fuma = self
                    .fuma_expected(&item.text)
                    .map(|codes| codes.iter().collect());
            }
        }
        let rank = start.elapsed();

        // 按头段算时英文尾段不参与拼音候选，显示上跟在切分后面：`wo'xiang'xue'hao'rust`
        let tail = english_tail
            .as_ref()
            .filter(|_| head_wins)
            .map_or(tail, |t| &keys[t.head_len..]);
        let typed_display = decoded.as_ref().map(|d| d.marked());
        Ok(Query {
            segmentations,
            candidates: CandidateList { items },
            tail: tail.to_owned(),
            text: self.composition.text().to_owned(),
            cursor: self.composition.cursor(),
            rest,
            decoded_keys: self.shuangpin.is_some(),
            typed_display,
            // 拼音行单独画出敲的那两个辅码键（解码已把它们剥掉，不画就一点痕迹都没有）
            fuma: self.fuma_keys().map(str::to_owned),
            correction,
            timings: Timings {
                parse,
                lookup,
                rank,
            },
        })
    }

    /// 英文直输段：唯一候选就是原文（`no-way`），空格 / 回车都上屏它；preedit 原样显示。
    pub(super) fn query_raw(&self, scope: &str, rest: String, start: Instant) -> Query {
        let items = vec![Candidate {
            text: scope.to_owned(),
            kind: CandidateKind::English,
            syllables: Vec::new(),
            reading: None,
            translation: None,
            fuma: None,
        }];
        Query {
            segmentations: Vec::new(),
            candidates: CandidateList { items },
            tail: scope.to_owned(),
            text: self.composition.text().to_owned(),
            cursor: self.composition.cursor(),
            rest,
            decoded_keys: self.shuangpin.is_some(),
            typed_display: None,
            // 这些模式下辅码不激活（辅码要求前缀是完整双拼）
            fuma: None,
            correction: None,
            timings: Timings {
                parse: Duration::ZERO,
                lookup: Duration::ZERO,
                rank: start.elapsed(),
            },
        }
    }

    /// 英文模式：敲的字母原样显示，候选是英文词表的精确词、前缀补全与拼错纠正（见 [`english::suggest`]），
    /// 词表没装就没有候选。emoji 照配，但排在所有词后面：选词靠上下键，emoji 夹在词中间会挡路。
    pub(super) fn query_english(&self, scope: &str, rest: String, start: Instant) -> Query {
        let mut items: Vec<Candidate> = english::suggest(
            &self.english_lists(),
            scope,
            |text| self.learner.weight(text),
            ENGLISH_MODE_CANDIDATES,
        )
        .into_iter()
        .map(|text| Candidate {
            text,
            kind: CandidateKind::English,
            syllables: Vec::new(),
            reading: None,
            translation: None,
            fuma: None,
        })
        .collect();
        self.insert_emoji(&mut items);
        items.sort_by_key(|c| c.kind == CandidateKind::Emoji);
        Query {
            segmentations: Vec::new(),
            candidates: CandidateList { items },
            tail: scope.to_owned(),
            text: self.composition.text().to_owned(),
            cursor: self.composition.cursor(),
            rest,
            decoded_keys: self.shuangpin.is_some(),
            typed_display: None,
            // 这些模式下辅码不激活（辅码要求前缀是完整双拼）
            fuma: None,
            correction: None,
            timings: Timings {
                parse: Duration::ZERO,
                lookup: Duration::ZERO,
                rank: start.elapsed(),
            },
        }
    }

    /// 整句候选。没有英文尾段时是整段拼音的转换（[`Self::plain_sentence`]），排在开头的英文候选之后。
    /// 有英文尾段且英文读法胜出（`head_wins`）时，头段的转换加上那个词排第一（`woxiangxuehaorust` → 我想学好rust），
    /// 整段也能读成拼音的再把拼音读法的整句放在第二；英文读法输了就不出（`diaoyong` 不出 掉Yong），
    /// 免得把真正要的候选往后挤。
    /// `typos` 为假时词图里不加敲错边（整段一处编辑的纠错已经生效，不在纠正后的拼音上再猜第二处）。
    pub(super) fn insert_sentence(
        &self,
        items: &mut Vec<Candidate>,
        segmentations: &[Segmentation],
        typos: bool,
        english_tail: Option<&EnglishTail>,
        head_wins: bool,
    ) {
        let Some(best) = segmentations.first() else {
            return;
        };
        let keys = self.composition.scope();
        let first_segmentation = |text: &str| parser::segment(text).ok()?.into_iter().next();
        match english_tail {
            Some(tail) if head_wins => {
                if let Some(mixed) = self.mixed_sentence(best, tail, typos) {
                    items.insert(0, mixed);
                }
                if tail.competes
                    && let Some(full) = first_segmentation(keys)
                    && let Some(plain) = self.plain_sentence(items, &full, typos)
                {
                    let position = items.len().min(1);
                    items.insert(position, plain);
                }
            }
            _ => {
                if let Some(plain) = self.plain_sentence(items, best, typos) {
                    let position = leading_english(items);
                    items.insert(position, plain);
                }
            }
        }
    }

    /// 整段拼音的整句候选：最优切分至少两个音节、且最优路径不止一个词时才有（空格上屏的就是它）。
    /// 整段本身就是词库里的词时不重复；有音节没转成字的不算句子。
    /// 词级候选里已有同文本同读音的候选时不出（那条留在词级排序给它的位置），同文本不同读音的从 `items` 里去掉。
    pub(super) fn plain_sentence(
        &self,
        items: &mut Vec<Candidate>,
        best: &Segmentation,
        typos: bool,
    ) -> Option<Candidate> {
        if best.syllables.len() < 2 {
            return None;
        }
        let mut conversion = self.convert_sentence(&best.patterns(), typos)?;
        // 不按原样读的路径（敲错边 / 模糊音）不许压过「敲的拼音本身就是一个词」：`jineng` 按 `jin eng` 切时
        // 词图里没有 技能，敲错边读出 近藤；`ceshi` 读出 的是。词级候选里有音节正好拼成整段输入的词时退回原样的路径
        if conversion.altered() {
            let letters = best.joined("");
            let spelled_exactly = items
                .iter()
                .any(|c| c.kind == CandidateKind::Chinese && c.syllables.concat() == letters);
            if spelled_exactly {
                conversion = self.convert_sentence(&best.patterns(), false)?;
            }
        }
        if conversion.has_placeholder() {
            return None;
        }
        // 辅码激活时整句同规则过滤（首末字形码）：对不上就不出
        if !self.fuma_admits(&conversion.text) {
            return None;
        }
        // 整段本来就是一个词时不出整句；但路径靠敲错变体把整段读成的一个词（`meiganxi` → 没关系）是噪声信道的判断，
        // 词级查询按原样查不到它，作为普通词候选插到最前。只读了一部分（末尾没打完的音节没算进去）的不插
        let kind = if conversion.word_count() >= 2 {
            CandidateKind::Sentence
        } else if conversion.altered() && conversion.syllables.len() == best.syllables.len() {
            CandidateKind::Chinese
        } else {
            return None;
        };
        // 词级候选里已经有同样的文本：读音也相同就是同一个候选，不重复插、词留在词级排序给它的位置
        //（先是 / 有的 这种整句恰好拼成一个词的，词级排序更可信）；读音不同的是按别的读音对上的词
        //（云端学来的错读音用户词 `我的 wo di` 靠敲错变体对上 `wode`），那条不是这个候选，去掉它，整句以正确读音顶上
        if let Some(index) = items.iter().position(|c| c.text == conversion.text) {
            if items[index].syllables == conversion.syllables {
                return None;
            }
            items.remove(index);
        }
        Some(Candidate {
            text: conversion.text,
            kind,
            syllables: conversion.syllables,
            reading: None,
            translation: None,
            fuma: None,
        })
    }

    /// 跑一次整句转换：主词库 + 用户词（含模糊音与敲错写法，命中的按代价扣分），静态语言模型与个人 n-gram 插值，用户选择次数加分。
    /// `typos` 为假时不加敲错边。
    pub(super) fn convert_sentence(
        &self,
        patterns: &[qingjian_dictionary::SyllablePattern<'_>],
        typos: bool,
    ) -> Option<Conversion> {
        self.convert_sentence_with(patterns, typos, false)
    }

    /// 同 [`Self::convert_sentence`]，`whole` 为真时末尾单字母也读（[`sentence::convert_whole`]），只给比分用。
    /// 接了神经重打分器时取前 [`RESCORE_PATHS`] 条路径，按「路径分 + λ·(神经分 − 静态分)」重排（[`Self::rescore_paths`]）：
    /// 神经分替换的是静态二元模型那部分判断，个人 n-gram 插值、用户加分、敲错代价原样保留，尺度也不变（纠错代价等常数照旧适用）。
    /// 返回重排后的第一条（`score` 换成重排后的分，好与别的读法比）；只有一条路径或模型还没给分时原样返回。
    pub(super) fn convert_sentence_with(
        &self,
        patterns: &[qingjian_dictionary::SyllablePattern<'_>],
        typos: bool,
        whole: bool,
    ) -> Option<Conversion> {
        let dictionaries = self.all_dictionaries();
        let expanded = self.expand_positions(patterns, typos);
        let k = if self.has_sentence_scorer() {
            RESCORE_PATHS
        } else {
            1
        };
        let mut paths = sentence::convert_paths(
            &dictionaries,
            &expanded.positions(),
            whole,
            k,
            &*self.language_model,
            self.personal(),
            |text| self.learner.weight(text),
            |index, syllable| expanded.cost(index, syllable),
            &mut self.span_cache.borrow_mut(),
        );
        // 与最优路径差得太远的不参与：那种差距多半是个人 n-gram 拉开的
        if paths.len() > 1 {
            let floor = paths[0].score - self.neural_margin;
            paths.retain(|p| p.score >= floor);
            self.rescore_paths(&mut paths);
        }
        paths.into_iter().next()
    }

    /// 每个位置的写法：敲的原样，再加音节级敲错变体（`correction::typo`）当带代价的边，
    /// 代价按类别定、按个人敲错表打折。太短的输入（不到 [`correction::MIN_LETTERS`]）、双拼、非末尾带简拼的切分不加敲错变体：
    /// 短串一处编辑几乎总能凑出别的词，双拼敲错一键换掉的是整个声母 / 韵母。不完整的位置（简拼、前缀）本来就按前缀查，不加。
    pub(super) fn expand_positions(
        &self,
        patterns: &[qingjian_dictionary::SyllablePattern<'_>],
        typos: bool,
    ) -> Expanded {
        let mut expanded = Expanded::new(
            patterns
                .iter()
                .map(|p| (vec![(p.text.to_owned(), 0.0)], p.complete)),
        );
        if !typos {
            return expanded;
        }
        let letters: usize = patterns.iter().map(|p| p.text.len()).sum();
        // 非末尾有简拼 / 残缺音节的切分（`kai f a`）本来就不是用户敲的原话，不在它上面再猜敲错
        let inner_abbreviated = patterns
            .iter()
            .take(patterns.len().saturating_sub(1))
            .any(|p| !p.complete);
        if self.shuangpin.is_some() || letters < correction::MIN_LETTERS || inner_abbreviated {
            return expanded;
        }
        for (index, pattern) in patterns.iter().enumerate() {
            if !pattern.complete {
                continue;
            }
            for (text, kind) in typo::variants(pattern.text) {
                let accepted = self.learner.typo_count(pattern.text, text);
                expanded.push_alternative(index, text, self.typo_costs.typo_cost(*kind, accepted));
            }
        }
        expanded
    }

    /// 主词库与用户词一起查（每个位置多种写法）。用户词是用户自己选过的（云联想接受的词等），排序上靠 weight 自然靠前。
    pub(super) fn lookup_all(
        &self,
        positions: &[Vec<qingjian_dictionary::SyllablePattern<'_>>],
    ) -> Vec<Match<'_>> {
        let mut hits = self.dictionary.lookup_pattern_alt(positions);
        for dictionary in self.all_dictionaries().into_iter().skip(1) {
            hits.extend(dictionary.lookup_pattern_alt(positions));
        }
        hits
    }

    /// 只要音节数正好等于位置数的词，主词库与用户词一起查。
    pub(super) fn lookup_exact_all(
        &self,
        positions: &[Vec<qingjian_dictionary::SyllablePattern<'_>>],
    ) -> Vec<Match<'_>> {
        let mut hits = self.dictionary.lookup_exact_alt(positions);
        for dictionary in self.all_dictionaries().into_iter().skip(1) {
            hits.extend(dictionary.lookup_exact_alt(positions));
        }
        hits
    }
}

/// 排在开头的英文候选有几条（整段是英文词、不像拼音带出的英文补全）：整句插在它们后面。
fn leading_english(items: &[Candidate]) -> usize {
    items
        .iter()
        .take_while(|c| c.kind == CandidateKind::English)
        .count()
}
