//! 小鹤音形：简码与开头四码，叠在双拼整句上。
//!
//! 使用者的打法是音形四码与双拼整句混着用（需求见 `docs/ime-yinxing-requirements.md`）：
//!
//! * 缓冲区从空开始敲 1–4 键，码表里这个码的字词排在候选最前：一简、二简、三简、四码，空格上屏；
//!   不选、接着敲第 5 键就当整句双拼继续打；
//! * 句中不认音形码：缓冲区一过 4 键、或者不是从空开始敲的（半段上屏后剩下的），码表就不参与；
//! * 可选的「四码唯一自动上屏」（默认关）：恰好敲满 4 键、这个码只对应一个**词组**（二字及以上）
//!   就上屏，单字四码（双拼 + 形码）不自动。打开后整句要先敲 `'` 再打——不然每 4 键都可能撞上
//!   某个词组的四码被顶上屏，实测 10 句常用公文错 7 处（「统一」→同意、「任务」→人物）。
//!
//! 码表是使用者自己导入的（搜狗自定义短语格式 `编码,位置=字词`），许可只限私人使用：
//! 不随包、不进仓库，导入后存在 `config_dir()/ime/yinxing/`。码表里没有的公文词表专名按
//! `lexicon::flypy` 的规则补码，排在同码的码表词之后。
//!
//! 码表候选以 `CandidateKind::Shortcut` 交给引擎：上屏时吃掉整段编码、不进引擎的词频学习，
//! 也不占固定位置（`Custom` 占位会和使用者的自定义短语互相覆盖）。重码的调频由这里自己记。

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context as _;
use qingjian_core::{Candidate, CandidateKind, ShuangpinScheme};

use super::data;
use super::session::Ime;
use crate::lexicon::LexiconTerm;

/// 码长上限：音形最长四码。
const MAX_CODE: usize = 4;

/// 这个码长及以下的重码顺序固定、不调频：一简、二简是盲打的根基。
const FIXED_ORDER_MAX: usize = 2;

/// 编码提示只管这么长以上的词：二字词的四码就是双拼全码，提示了等于没提示。
const HINT_MIN_CHARS: usize = 3;

/// 编码提示从一段上屏文字里找词时，最长看几个字。
const HINT_MAX_CHARS: usize = 12;

/// 同一个词最多提示几次：学会了就别再唠叨。
const HINT_TIMES: u8 = 3;

/// 调频数据的文件名，落在学习数据目录。
const FREQ_FILE: &str = "yinxing-freq.tsv";

/// 编码能不能当音形码用：1–4 个小写字母。
fn valid_code(code: &str) -> bool {
    (1..=MAX_CODE).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_lowercase())
}

/// 使用者导入的音形码表。
#[derive(Debug, Default)]
pub(crate) struct Table {
    /// 编码 → 字词，按码表里的位置排好。
    by_code: HashMap<String, Vec<String>>,

    /// 条数。
    entries: usize,
}

impl Table {
    /// 解析搜狗自定义短语格式：`编码,位置=字词`，`#` 开头的行是注释。
    ///
    /// 编码不是 1–4 个小写字母的行跳过（码表里偶有超长的怪码），同码同词只留一条。
    pub(crate) fn parse(text: &str) -> Self {
        let mut rows: HashMap<String, Vec<(usize, usize, String)>> = HashMap::new();
        for (line_no, line) in text.lines().enumerate() {
            let line = line.trim_start_matches('\u{feff}').trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((left, word)) = line.split_once('=') else {
                continue;
            };
            let (code, position) = match left.split_once(',') {
                Some((code, position)) => (code.trim(), position.trim().parse().unwrap_or(1)),
                None => (left.trim(), 1),
            };
            if !valid_code(code) || word.is_empty() {
                continue;
            }
            rows.entry(code.to_owned())
                .or_default()
                .push((position, line_no, word.to_owned()));
        }
        let mut entries = 0;
        let by_code = rows
            .into_iter()
            .map(|(code, mut words)| {
                words.sort_by_key(|&(position, line_no, _)| (position, line_no));
                let mut ordered: Vec<String> = Vec::with_capacity(words.len());
                for (_, _, word) in words {
                    if !ordered.contains(&word) {
                        ordered.push(word);
                    }
                }
                entries += ordered.len();
                (code, ordered)
            })
            .collect();
        Self { by_code, entries }
    }

    /// 条数。
    pub(crate) fn len(&self) -> usize {
        self.entries
    }
}

/// 音形的全部状态：码表、词表补的码、调频、编码提示的记账。
#[derive(Debug, Default)]
pub(crate) struct Yinxing {
    /// 导入的码表；没导入时为 `None`，音形整个不参与。
    table: Option<Table>,

    /// 公文词表补的码：编码 → 字词。
    lexicon: HashMap<String, Vec<String>>,

    /// 字词 → 最短编码（码表与词表合起来），编码提示用。只收三字及以上的词。
    shortest: HashMap<String, String>,

    /// 调频：（编码，字词）→ 选用次数。
    freq: HashMap<(String, String), u32>,

    /// 调频有没写盘的改动。
    freq_dirty: bool,

    /// 每个词提示过几次编码（只在本次运行里记）。
    hinted: HashMap<String, u8>,
}

impl Yinxing {
    /// 换码表。
    pub(crate) fn set_table(&mut self, table: Option<Table>) {
        self.table = table;
        self.rebuild_shortest();
    }

    /// 码表条数；没导入时为 `None`。
    pub(crate) fn table_len(&self) -> Option<usize> {
        self.table.as_ref().map(Table::len)
    }

    /// 换公文词表补的码：（编码，字词）。码表里同码已有的词不重复收。
    pub(crate) fn set_lexicon(&mut self, entries: impl IntoIterator<Item = (String, String)>) {
        let mut lexicon: HashMap<String, Vec<String>> = HashMap::new();
        for (code, word) in entries {
            if !valid_code(&code) {
                continue;
            }
            let words = lexicon.entry(code).or_default();
            if !words.contains(&word) {
                words.push(word);
            }
        }
        self.lexicon = lexicon;
        self.rebuild_shortest();
    }

    /// 这个码的字词：码表的在前（按码表位置），词表补的在后；三码、四码再按选用次数调频。
    pub(crate) fn lookup(&self, code: &str) -> Vec<String> {
        let Some(table) = &self.table else {
            return Vec::new();
        };
        let mut words: Vec<String> = table.by_code.get(code).cloned().unwrap_or_default();
        for word in self.lexicon.get(code).into_iter().flatten() {
            if !words.contains(word) {
                words.push(word.clone());
            }
        }
        if code.len() > FIXED_ORDER_MAX {
            // 稳定排序：次数一样的保持码表顺序
            words.sort_by_key(|word| {
                std::cmp::Reverse(
                    self.freq
                        .get(&(code.to_owned(), word.clone()))
                        .copied()
                        .unwrap_or(0),
                )
            });
        }
        words
    }

    /// 记一次选用。只记这个码真有的字词。
    pub(crate) fn record(&mut self, code: &str, word: &str) {
        if !self.lookup(code).iter().any(|w| w == word) {
            return;
        }
        *self
            .freq
            .entry((code.to_owned(), word.to_owned()))
            .or_insert(0) += 1;
        self.freq_dirty = true;
    }

    /// 编码提示：从一段整句上屏的文字里找最长的、有音形码的三字及以上词。
    /// 同一个词提示满 [`HINT_TIMES`] 次就不再提示。
    pub(crate) fn hint(&mut self, text: &str) -> Option<(String, String)> {
        // 没导入码表就不提示（词表补的码单独不算数）
        self.table.as_ref()?;
        let chars: Vec<char> = text.chars().collect();
        let mut best: Option<(String, String)> = None;
        for start in 0..chars.len() {
            let longest = (chars.len() - start).min(HINT_MAX_CHARS);
            for len in (HINT_MIN_CHARS..=longest).rev() {
                let word: String = chars[start..start + len].iter().collect();
                let Some(code) = self.shortest.get(&word) else {
                    continue;
                };
                let longer = best
                    .as_ref()
                    .is_none_or(|(found, _)| found.chars().count() < len);
                let tired = self.hinted.get(&word).copied().unwrap_or(0) >= HINT_TIMES;
                if longer && !tired {
                    best = Some((word, code.clone()));
                }
                break;
            }
        }
        if let Some((word, _)) = &best {
            *self.hinted.entry(word.clone()).or_insert(0) += 1;
        }
        best
    }

    /// 读调频数据：`编码\t字词\t次数`。读不出来就从零开始，不挡输入法。
    pub(crate) fn load_freq(&mut self, dir: &Path) {
        let Ok(text) = std::fs::read_to_string(dir.join(FREQ_FILE)) else {
            return;
        };
        self.freq = text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\t');
                let code = fields.next()?;
                let word = fields.next()?;
                let count = fields.next()?.parse().ok()?;
                Some(((code.to_owned(), word.to_owned()), count))
            })
            .collect();
    }

    /// 调频有改动就写盘。
    pub(crate) fn save_freq(&mut self, dir: &Path) {
        if !self.freq_dirty {
            return;
        }
        let mut rows: Vec<_> = self.freq.iter().collect();
        rows.sort();
        let text: String = rows
            .into_iter()
            .map(|((code, word), count)| format!("{code}\t{word}\t{count}\n"))
            .collect();
        match std::fs::write(dir.join(FREQ_FILE), text) {
            Ok(()) => self.freq_dirty = false,
            Err(error) => eprintln!("[ime] 音形调频数据写盘失败：{error}"),
        }
    }

    /// 重算「字词 → 最短编码」。
    fn rebuild_shortest(&mut self) {
        let mut shortest: HashMap<String, String> = HashMap::new();
        let table = self.table.iter().flat_map(|table| table.by_code.iter());
        for (code, words) in table.chain(self.lexicon.iter()) {
            for word in words {
                if word.chars().count() < HINT_MIN_CHARS {
                    continue;
                }
                let slot = shortest.entry(word.clone()).or_insert_with(|| code.clone());
                if code.len() < slot.len() {
                    *slot = code.clone();
                }
            }
        }
        self.shortest = shortest;
    }
}

/// 一个码表候选。
fn candidate(text: String) -> Candidate {
    Candidate {
        text,
        kind: CandidateKind::Shortcut,
        syllables: Vec::new(),
        reading: None,
        translation: None,
        fuma: None,
    }
}

impl Ime {
    /// 音形开着：设置里打开、双拼是小鹤、码表导入了。
    pub(crate) fn yinxing_active(&self) -> bool {
        self.settings.yinxing
            && self.settings.shuangpin == Some(ShuangpinScheme::Xiaohe)
            && self.yinxing.table_len().is_some()
    }

    /// 导入的码表条数；没导入时为 `None`。
    pub(crate) fn yinxing_entries(&self) -> Option<usize> {
        self.yinxing.table_len()
    }

    /// 当前缓冲区能不能当音形码查：音形开着、从空开始敲的、光标在末尾、1–4 个小写字母。
    pub(super) fn yinxing_code(&self) -> Option<String> {
        if !self.yinxing_active() || !self.fresh {
            return None;
        }
        let composition = self.engine()?.composition();
        let text = composition.text();
        (composition.cursor() == text.len() && valid_code(text)).then(|| text.to_owned())
    }

    /// 把码表候选并进引擎的候选：排在最前，引擎候选里同字的去掉。
    /// 固定位置的自定义短语不动（布局按位置钉住它们）。
    pub(super) fn merge_yinxing(&self, items: &mut Vec<Candidate>) {
        let Some(code) = self.yinxing_code() else {
            return;
        };
        let words = self.yinxing.lookup(&code);
        if words.is_empty() {
            return;
        }
        items.retain(|item| {
            matches!(item.kind, CandidateKind::Custom(_)) || !words.contains(&item.text)
        });
        items.splice(0..0, words.into_iter().map(candidate));
    }

    /// 刚敲满开头四码：这个码只对应一个词组就自动上屏。返回上屏的文字。
    pub(super) fn yinxing_auto_commit(&mut self) -> Option<String> {
        if !self.settings.yinxing_auto_commit {
            return None;
        }
        let code = self.yinxing_code().filter(|code| code.len() == MAX_CODE)?;
        let words = self.yinxing.lookup(&code);
        let [word] = words.as_slice() else {
            return None;
        };
        // 单字四码（双拼 + 形码）不自动：整句开头的两个音节很容易撞上某个生僻字的四码
        if word.chars().count() < 2 {
            return None;
        }
        let page_size = self.settings.page_size.max(1);
        let index = (0..self.layout.len().min(page_size)).find(|&index| {
            self.layout
                .candidate(index)
                .is_some_and(|c| c.kind == CandidateKind::Shortcut && &c.text == word)
        })?;
        self.execute_guarded(super::keys::Action::CommitIndex(index))
            .commit
    }

    /// 上屏之后：码表候选记调频，整句打出的长词提示音形码。
    pub(super) fn after_commit(&mut self, candidate: &Candidate, code: Option<&str>) {
        self.fresh = false;
        match candidate.kind {
            CandidateKind::Shortcut => {
                if let Some(code) = code {
                    self.yinxing.record(code, &candidate.text);
                }
            }
            CandidateKind::Sentence | CandidateKind::Chinese
                if self.settings.yinxing_hint && self.yinxing_active() =>
            {
                if let Some((word, code)) = self.yinxing.hint(&candidate.text) {
                    self.notice = Some(format!("音形码：{word} → {code}"));
                }
            }
            _ => {}
        }
    }

    /// 导入音形码表：读使用者选的文件（UTF-16 / GBK / UTF-8 都认），转成 UTF-8 存进用户目录再加载。
    /// 返回条数。
    pub(crate) fn import_yinxing_table(&mut self, source: &Path) -> anyhow::Result<usize> {
        let content = crate::text_file::read_to_string(source)
            .with_context(|| format!("读取音形码表失败：{}", source.display()))?;
        let table = Table::parse(&content);
        anyhow::ensure!(
            table.len() > 0,
            "没有认出码表条目：要的是「编码,位置=字词」一行一条的文本"
        );
        let target = data::yinxing_path().context("无法确定音形码表目录")?;
        std::fs::write(&target, content)
            .with_context(|| format!("写入音形码表失败：{}", target.display()))?;
        let entries = table.len();
        self.yinxing.set_table(Some(table));
        Ok(entries)
    }

    /// 启动时读码表与调频数据。码表不在就当音形关着。
    pub(super) fn load_yinxing(&mut self) {
        if let Some(path) = data::yinxing_path().filter(|path| path.is_file()) {
            match std::fs::read_to_string(&path) {
                Ok(text) => self.yinxing.set_table(Some(Table::parse(&text))),
                Err(error) => eprintln!(
                    "[ime] 音形码表读取失败，音形关着：{}（{error}）",
                    path.display()
                ),
            }
        }
        if let Some(dir) = data::learning_dir() {
            self.yinxing.load_freq(&dir);
        }
    }

    /// 公文词表补码：词表里（没被拒绝的）二字及以上的词按小鹤规则出码，改过的短码优先。
    pub(super) fn sync_yinxing_lexicon(&mut self, terms: &[LexiconTerm]) {
        self.yinxing.set_lexicon(
            terms
                .iter()
                .filter(|term| term.char_count() >= 2)
                .filter_map(|term| Some((term.code().ok()?, term.term.clone()))),
        );
    }

    /// 调频数据写盘。
    pub(super) fn flush_yinxing(&mut self) {
        if let Some(dir) = data::learning_dir() {
            self.yinxing.save_freq(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{feff}#小鹤音形文本码表\n\
                          #注释\n\
                          d,1=的\n\
                          q,2=期\n\
                          q,1=去\n\
                          aq,1=安全\n\
                          lild,2=历朝历代\n\
                          lild,1=理论\n\
                          vhrg,1=中华人民共和国\n\
                          jiyk,1=诘\n\
                          toolong,1=坏\n\
                          BAD,1=坏\n\
                          noequals\n";

    fn yinxing() -> Yinxing {
        let mut yinxing = Yinxing::default();
        yinxing.set_table(Some(Table::parse(SAMPLE)));
        yinxing
    }

    /// 解析：注释、坏行跳过，同码按位置排。
    #[test]
    fn parses_the_sogou_phrase_format() {
        let table = Table::parse(SAMPLE);
        assert_eq!(table.len(), 8);
        let yinxing = yinxing();
        assert_eq!(yinxing.lookup("q"), ["去", "期"]);
        assert_eq!(yinxing.lookup("lild"), ["理论", "历朝历代"]);
        assert!(yinxing.lookup("toolong").is_empty());
    }

    /// 三码、四码调频；一简、二简固定。
    #[test]
    fn longer_codes_follow_usage_short_ones_stay_fixed() {
        let mut yinxing = yinxing();
        yinxing.record("lild", "历朝历代");
        assert_eq!(yinxing.lookup("lild"), ["历朝历代", "理论"]);
        yinxing.record("q", "期");
        yinxing.record("q", "期");
        assert_eq!(yinxing.lookup("q"), ["去", "期"], "一简不调频");
        // 码里没有的词不记
        yinxing.record("lild", "乱码");
        assert!(!yinxing.freq.contains_key(&("lild".into(), "乱码".into())));
    }

    /// 词表补的码排在同码的码表词之后，码表里已有的不重复。
    #[test]
    fn lexicon_codes_come_after_the_table() {
        let mut yinxing = yinxing();
        yinxing.set_lexicon([
            ("lild".to_owned(), "临理督导".to_owned()),
            ("lild".to_owned(), "理论".to_owned()),
            ("zhcs".to_owned(), "综合处室".to_owned()),
        ]);
        assert_eq!(yinxing.lookup("lild"), ["理论", "历朝历代", "临理督导"]);
        assert_eq!(yinxing.lookup("zhcs"), ["综合处室"]);
    }

    /// 编码提示：找最长的三字及以上词，同一个词提示满三次就停。
    #[test]
    fn hints_the_longest_word_a_few_times() {
        let mut yinxing = yinxing();
        let text = "坚持中华人民共和国宪法";
        for _ in 0..HINT_TIMES {
            assert_eq!(
                yinxing.hint(text),
                Some(("中华人民共和国".to_owned(), "vhrg".to_owned()))
            );
        }
        assert_eq!(yinxing.hint(text), None);
        assert_eq!(yinxing.hint("安全"), None, "二字词不提示");
    }

    /// 使用者真实的码表（UTF-16 搜狗自定义短语）能认出来。
    ///
    /// 标 `#[ignore]`：码表只限私人使用、不进仓库，只有放了这份文件的机器能跑：
    /// `cargo test yinxing -- --ignored`，路径可用 `GONGWEN_YINXING_TABLE` 指定。
    #[test]
    #[ignore = "需要使用者自己的小鹤音形码表"]
    fn reads_the_real_code_table() {
        let path = std::env::var("GONGWEN_YINXING_TABLE")
            .unwrap_or_else(|_| "assets/fuma/搜狗拼音win版自定义短语.txt".to_owned());
        let text = crate::text_file::read_to_string(&path).expect("读码表");
        let mut yinxing = Yinxing::default();
        let table = Table::parse(&text);
        assert!(table.len() > 50_000, "条数太少：{}", table.len());
        yinxing.set_table(Some(table));
        assert_eq!(yinxing.lookup("d").first().map(String::as_str), Some("的"));
        assert!(yinxing.lookup("vhrg").iter().any(|w| w == "中华人民共和国"));
        assert!(yinxing.lookup("lild").iter().any(|w| w == "历朝历代"));
    }

    /// 调频数据写盘再读回来。
    #[test]
    fn frequency_round_trips_through_disk() {
        let dir = tempfile::tempdir().expect("临时目录");
        let mut yinxing = yinxing();
        yinxing.record("lild", "历朝历代");
        yinxing.save_freq(dir.path());
        let mut loaded = self::yinxing();
        loaded.load_freq(dir.path());
        assert_eq!(loaded.lookup("lild"), ["历朝历代", "理论"]);
    }
}
