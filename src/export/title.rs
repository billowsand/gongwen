//! 公文标题的排布：超出一行不多于 2 个全角字宽时**横向压缩字形**保持单行（字高不变），
//! 超出更多时用 jieba 分词在词边界均衡换行，词不被拆到两行。
//!
//! 断行点在词边界之外还要守几条排版规矩（见 [`title_words`]）：标点不落行首、
//! 开括号不落行尾、短括注整体不拆、“的”不起行、“第”与数字、数字与量词不分家。
//! 研究报告封面题名（[`cover_title_lines`]）用同一套算法，只是换了字号与框宽。

/// 标题基准字号：二号（22 pt）。汉字为方形，字宽=字高=字号。
pub const TITLE_BASE_SIZE_PT: usize = 22;
/// 红头呈批件首页标题：旧模板使用小二号，标题仅占批示栏左侧约 10cm。
pub const RED_APPROVAL_TITLE_SIZE_PT: usize = 18;
/// 版心宽度（约 15.6cm）：以 OOXML 半磅体系的 pt 计，8845 twip = 442.25 pt；
/// LaTeX 的 156mm 约 443.9 TeX pt，二者对应同一物理宽度，取较小值使压缩结果偏保守，
/// 两套引擎都放得下。
pub const TITLE_LINE_WIDTH_PT: f64 = 8_845.0 / 20.0;
/// 红头呈批件首页左侧正文/标题栏宽度（96mm）：红色竖线在 100mm 处，
/// 标题与正文一样要给竖线让出 4mm 留白，见 `export::red::RED_APPROVAL_GUTTER_MM`。
pub const RED_APPROVAL_TITLE_WIDTH_PT: f64 = super::red::RED_APPROVAL_NARROW_MM / 25.4 * 72.0;

/// 研究报告封面题名：小标宋一号（26 pt），题名框宽 150 mm（版心 160 mm 两侧各让
/// 5 mm），与 mdx `cover::layout` 一致。一行放 16 个字。
pub const COVER_TITLE_SIZE_PT: usize = 26;
pub const COVER_TITLE_WIDTH_PT: f64 = 150.0 / 25.4 * 72.0;

/// 标题的排布方案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TitlePlan {
    /// 单行，字号保持二号。
    SingleLine,
    /// 单行，字高保持二号、仅横向收窄字形到一行放得下。
    Compressed,
    /// 多行：jieba 分词后在词边界均衡换行，每行不超 `chars_per_line` 个全角字宽。
    Wrapped(Vec<String>),
}

/// 一行（二号字号）能容纳的汉字数。
pub fn chars_per_line() -> usize {
    chars_per_line_for(TITLE_LINE_WIDTH_PT, TITLE_BASE_SIZE_PT)
}

pub fn chars_per_line_for(width_pt: f64, size_pt: usize) -> usize {
    (width_pt / size_pt as f64).floor() as usize
}

pub fn red_approval_chars_per_line() -> usize {
    chars_per_line_for(RED_APPROVAL_TITLE_WIDTH_PT, RED_APPROVAL_TITLE_SIZE_PT)
}

/// 压缩单行时的横向缩放百分比：字高不变（仍为二号），仅横向收窄字形。
/// 100 = 原宽，小于 100 为横向压缩比例；按版心宽度与显示字宽反推并限制在不小于 80%。
pub fn compressed_scale_percent(title: &str) -> usize {
    compressed_scale_percent_for(title, TITLE_LINE_WIDTH_PT, TITLE_BASE_SIZE_PT)
}

pub fn compressed_scale_percent_for(title: &str, width_pt: f64, size_pt: usize) -> usize {
    let count = display_units(title).max(2) as f64 / 2.0;
    let scale = width_pt / (count * size_pt as f64);
    (scale * 100.0).floor().clamp(80.0, 100.0) as usize
}

/// 决定标题排布：不超一行单行；超出一行不超过 2 个全角字宽横向压缩单行；否则 jieba 换行。
pub fn title_plan(title: &str, chars_per_line: usize) -> TitlePlan {
    let width = display_units(title);
    let line_units = chars_per_line * 2;
    if width <= line_units {
        return TitlePlan::SingleLine;
    }
    if width <= line_units + 4 {
        return TitlePlan::Compressed;
    }
    TitlePlan::Wrapped(wrap_units(&title_units(title), chars_per_line))
}

/// 研究报告封面题名的分行：一行放得下就一行，否则按公文标题的规矩在词边界
/// 均衡换行（首行不短于末行）。封面题名不做横向压缩——封面留白足，分两行
/// 比把字压扁更庄重。
pub fn cover_title_lines(title: &str) -> Vec<String> {
    let title = title.trim();
    if title.is_empty() {
        return Vec::new();
    }
    let per_line = chars_per_line_for(COVER_TITLE_WIDTH_PT, COVER_TITLE_SIZE_PT);
    if display_units(title) <= per_line * 2 {
        return vec![title.to_string()];
    }
    wrap_units(&title_units(title), per_line)
}

/// 行首不能出现的字符：句读、点号与各种闭括号、闭引号。
const NO_LINE_START: &str = "）》〉」』】〕〗”’、，。：；！？…·%％)]},.:;!?";
/// 行尾不能出现的字符：各种开括号、开引号。
const NO_LINE_END: &str = "（《〈「『【〔〖“‘([{";
/// 括注整体不拆的上限（全角字宽，含括号）：短括注如“（二期）”“（试行）”
/// “（2026—2030年）”拆开很难看；太长的括注拆不拆由词边界决定。
const KEEP_BRACKET_UNITS: usize = 24;
/// 不起行的虚词：拆到下一行行首读起来断了气，归到上一行行尾。
const NO_LINE_START_WORDS: [&str; 4] = ["的", "之", "等", "了"];
/// 紧跟数字的量词、单位：数字与它们不分家（“2026年”“第3期”）。
const NUMBER_SUFFIX: &str = "年月日号期届次版章条款项批个位名家";

/// 标题的一个断行单元：一个或几个并在一起、中间不许换行的词。
/// 首尾两个词的词性与字数留着，用来给单元之间的断点打分。
#[derive(Debug, Clone)]
struct Unit {
    text: String,
    head_tag: String,
    head_chars: usize,
    tail_tag: String,
    tail_chars: usize,
}

impl Unit {
    fn new(word: &str, tag: &str) -> Self {
        let chars = word.chars().count();
        Self {
            text: word.to_string(),
            head_tag: tag.to_string(),
            head_chars: chars,
            tail_tag: tag.to_string(),
            tail_chars: chars,
        }
    }

    fn absorb(&mut self, next: &Unit) {
        self.text.push_str(&next.text);
        self.tail_tag.clone_from(&next.tail_tag);
        self.tail_chars = next.tail_chars;
    }
}

/// 标题的断行单元：jieba 分词并标词性（带本单位词表，专名不拆），再把排版上
/// 不许断开的相邻词并起来。
fn title_units(title: &str) -> Vec<Unit> {
    let words: Vec<Unit> = crate::lexicon::segmenter::tagged(title)
        .iter()
        .map(|(word, tag)| Unit::new(word, tag))
        .collect();
    glue_units(words)
}

fn glue_units(words: Vec<Unit>) -> Vec<Unit> {
    let text: String = words.iter().map(|unit| unit.text.as_str()).collect();
    let keep = short_bracket_spans(&text);
    let mut out: Vec<Unit> = Vec::new();
    let mut offset = 0usize;
    for word in words {
        let start = offset;
        offset += word.text.len();
        let Some(last) = out.last_mut() else {
            out.push(word);
            continue;
        };
        let before = last.text.chars().last();
        let after = word.text.chars().next();
        let forbidden = before.is_some_and(|ch| NO_LINE_END.contains(ch))
            || after.is_some_and(|ch| NO_LINE_START.contains(ch))
            || NO_LINE_START_WORDS.contains(&word.text.as_str())
            || last.text.ends_with('第')
            || (before.is_some_and(|ch| ch.is_ascii_digit())
                && after.is_some_and(|ch| ch.is_ascii_digit() || NUMBER_SUFFIX.contains(ch)))
            || keep
                .iter()
                .any(|span| span.start < start && start < span.end);
        if forbidden {
            last.absorb(&word);
        } else {
            out.push(word);
        }
    }
    out
}

/// 找出较短的成对括注（字节区间，含两侧括号），换行不得落在其中。
fn short_bracket_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    const PAIRS: [(char, char); 6] = [
        ('（', '）'),
        ('(', ')'),
        ('《', '》'),
        ('〔', '〕'),
        ('【', '】'),
        ('“', '”'),
    ];
    let mut spans = Vec::new();
    for (open, close) in PAIRS {
        let mut stack = Vec::new();
        for (index, ch) in text.char_indices() {
            if ch == open {
                stack.push(index);
            } else if ch == close
                && let Some(start) = stack.pop()
            {
                let end = index + ch.len_utf8();
                if display_units(&text[start..end]) <= KEEP_BRACKET_UNITS {
                    spans.push(start..end);
                }
            }
        }
    }
    spans
}

/// 标题的显示宽度（半角单位）：1 个全角字符 = 2，1 个半角英数 = 1，空白不计。
pub(crate) fn display_units(text: &str) -> usize {
    text.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum()
}

/// 在两个单元之间换行的代价（越小越好）。依据公文标题回行“词意完整”的要求：
/// - 最好断在“的”之后、连词介词（与、和、及、关于、对……）之前，意群在这里自然分开；
/// - 断在名词与名词之间多半是拆开了一个复合词（“数字|政府”“数据共享|平台”）；
/// - 单字词多是词缀或没切准的半个词（“大|模型”“数据|局”），挨着它断最难看；
/// - 西文词之间的空格是天然断点。
fn break_cost(left: &Unit, right: &Unit) -> f32 {
    let nominal = |tag: &str| tag.starts_with('n') || matches!(tag, "vn" | "an" | "j");
    let function = |tag: &str| matches!(tag, "c" | "p" | "uj" | "u" | "ul");
    if left.text.ends_with(['的', '之']) || function(&right.head_tag) {
        return 0.0;
    }
    if left.text.ends_with([' ', '\u{3000}']) || right.text.starts_with([' ', '\u{3000}']) {
        return 0.5;
    }
    // 括注、引号收尾处：后面紧跟名词时多是“（二期）建设项目”“‘十五五’时期”
    // 这样的定中结构，按普通词边界算；否则是个不错的断点。
    if left.text.ends_with(['）', '》', '”', '〕', '】', ')']) {
        return if nominal(&right.head_tag) { 3.0 } else { 1.0 };
    }
    let single = |chars: usize, tag: &str, text_is_cjk: bool| {
        chars == 1 && text_is_cjk && !function(tag) && tag != "x" && tag != "m"
    };
    let left_cjk = left.text.chars().last().is_some_and(|ch| !ch.is_ascii());
    let right_cjk = right.text.chars().next().is_some_and(|ch| !ch.is_ascii());
    if single(left.tail_chars, &left.tail_tag, left_cjk)
        || single(right.head_chars, &right.head_tag, right_cjk)
    {
        return 8.0;
    }
    if right.text.starts_with(|ch: char| NO_LINE_END.contains(ch)) {
        return 4.0;
    }
    if nominal(&left.tail_tag) && nominal(&right.head_tag) {
        return 6.0;
    }
    3.0
}

/// 在单元边界、每行不超过 `chars_per_line` 个全角字宽的约束下分行，全局取代价最小的
/// 断法。代价 = 各断点的 [`break_cost`] + 各行字数偏离平均的总量 + 多用一行的罚分。
/// “头不轻脚不重”（首行不短于末行）是硬要求：只要有断法满足，就只在这些断法里挑。
/// 行数只在“最少行数”与“多一行”之间比较——多一行能换来词意完整时才值得。
fn wrap_units(units: &[Unit], chars_per_line: usize) -> Vec<String> {
    let line_units = chars_per_line * 2;
    let widths: Vec<usize> = units.iter().map(|unit| display_units(&unit.text)).collect();
    let total: usize = widths.iter().sum();
    if total == 0 {
        return Vec::new();
    }
    let min_lines = lines_needed(&widths, line_units);
    if min_lines <= 1 {
        return vec![units.iter().map(|unit| unit.text.as_str()).collect()];
    }
    // 断点下标 i 表示在 units[i-1] 与 units[i] 之间换行。
    let costs: Vec<f32> = (1..units.len())
        .map(|i| break_cost(&units[i - 1], &units[i]))
        .collect();
    // 每种行数各取一个最优：同一行数里“头不轻脚不重”的断法优先。
    let mut per_count: Vec<(bool, f32, Vec<usize>)> = Vec::new();
    for lines in min_lines..=min_lines + 1 {
        if lines > units.len() {
            break;
        }
        let mean = total as f32 / lines as f32;
        let mut best: Option<(bool, f32, Vec<usize>)> = None; // (头轻脚重, 代价, 断点)
        let mut splits = Vec::new();
        search_splits(&widths, 0, lines, line_units, &mut splits, &mut |splits| {
            let bounds: Vec<usize> = std::iter::once(0)
                .chain(splits.iter().copied())
                .chain(std::iter::once(units.len()))
                .collect();
            let lens: Vec<usize> = bounds
                .windows(2)
                .map(|w| widths[w[0]..w[1]].iter().sum())
                .collect();
            let light_head = lens[0] < lens[lens.len() - 1];
            let ragged: f32 = lens
                .iter()
                .map(|&len| (len as f32 - mean).abs() / 2.0)
                .sum();
            let breaks: f32 = splits.iter().map(|&i| costs[i - 1]).sum();
            let cost = breaks + ragged + (lines - min_lines) as f32 * 6.0;
            if best.as_ref().is_none_or(|(best_light, best_cost, _)| {
                (light_head, cost) < (*best_light, *best_cost)
            }) {
                best = Some((light_head, cost, splits.to_vec()));
            }
        });
        per_count.extend(best);
    }
    // 跨行数比较时，头轻脚重只记罚分，不一票否决：多拆一行换来的往往更难看。
    let Some((_, _, splits)) = per_count.into_iter().min_by(|a, b| {
        let score =
            |(light, cost, _): &(bool, f32, Vec<usize>)| cost + if *light { 4.0 } else { 0.0 };
        score(a).total_cmp(&score(b))
    }) else {
        return vec![units.iter().map(|unit| unit.text.as_str()).collect()];
    };
    let bounds: Vec<usize> = std::iter::once(0)
        .chain(splits)
        .chain(std::iter::once(units.len()))
        .collect();
    bounds
        .windows(2)
        .map(|w| {
            units[w[0]..w[1]]
                .iter()
                .map(|unit| unit.text.as_str())
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

/// 枚举把 `widths[start..]` 分成 `lines` 行、每行不超过 `limit` 的全部断法。
/// 标题不过几十个字，穷举足够快，而且比贪心可靠：贪心在长括注前后会把自己逼进死角。
fn search_splits(
    widths: &[usize],
    start: usize,
    lines: usize,
    limit: usize,
    splits: &mut Vec<usize>,
    visit: &mut dyn FnMut(&[usize]),
) {
    let rest: usize = widths[start..].iter().sum();
    if lines == 1 {
        if rest <= limit {
            visit(splits);
        }
        return;
    }
    if rest > limit * lines {
        return;
    }
    let mut width = 0usize;
    for end in start + 1..widths.len() {
        width += widths[end - 1];
        if width > limit {
            break;
        }
        splits.push(end);
        search_splits(widths, end, lines - 1, limit, splits, visit);
        splits.pop();
    }
}

/// 每行不超过 `limit_units` 个半角单位（单元不拆开）时，贪心能分成的最少行数。
fn lines_needed(widths: &[usize], limit_units: usize) -> usize {
    let mut lines = 0usize;
    let mut current = 0usize;
    for &width in widths {
        if current != 0 && current + width > limit_units {
            lines += 1;
            current = 0;
        }
        current += width;
    }
    if current > 0 {
        lines += 1;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_one_line_within_capacity() {
        assert_eq!(
            title_plan("关于开展测试工作的函", chars_per_line()),
            TitlePlan::SingleLine
        );
    }

    #[test]
    fn small_overflow_compresses_horizontally_only() {
        // 一行约 20 个全角字宽：21/22 个全角字宽属于“超出一行不超过 2 个全角字宽”。
        let title = "关于认真做好网络安全与信息化重点工作验收的函";
        assert_eq!(display_units(title), 44);
        assert_eq!(title_plan(title, 20), TitlePlan::Compressed);
        // 字高不变（字号仍是二号），仅横向收窄字形。
        let scale = compressed_scale_percent(title);
        assert!((80..100).contains(&scale), "横向缩放应小于原宽：{scale}");
    }

    #[test]
    fn red_approval_title_uses_small_two_and_fifteen_character_column() {
        assert_eq!(red_approval_chars_per_line(), 15);
        assert_eq!(
            title_plan(
                "一二三四五六七八九十一二三四五",
                red_approval_chars_per_line()
            ),
            TitlePlan::SingleLine
        );
        assert_eq!(
            title_plan(
                "一二三四五六七八九十一二三四五六七",
                red_approval_chars_per_line()
            ),
            TitlePlan::Compressed
        );
        assert!(matches!(
            title_plan(
                "关于认真做好网络安全与信息化重点工作验收的请示",
                red_approval_chars_per_line()
            ),
            TitlePlan::Wrapped(_)
        ));
    }

    #[test]
    fn half_width_characters_count_as_half_a_full_width_char() {
        assert_eq!(display_units("2026"), 4);
        assert_eq!(display_units("2026年"), 6);
        assert_eq!(display_units("关于2026年"), 10);

        // 32 个半角英数只占 16 个全角字宽，仍可单行。
        let half = "ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        assert_eq!(display_units(half), 32);
        assert_eq!(title_plan(half, chars_per_line()), TitlePlan::SingleLine);

        // 21 个汉字 + 2 个半角字符 = 22 个全角字宽，按实际宽度应走横向压缩。
        let base = "关于认真做好网络安全与信息化重点工作验收的函";
        let mixed = format!("{}AB", &base[..base.len() - "函".len()]);
        assert_eq!(display_units(&mixed), 44);
        assert_eq!(title_plan(&mixed, 20), TitlePlan::Compressed);
    }

    #[test]
    fn large_overflow_wraps_at_word_boundaries() {
        let title = "关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知";
        assert!(display_units(title) > 44);
        let TitlePlan::Wrapped(lines) = title_plan(title, 20) else {
            panic!("应走 jieba 换行");
        };
        assert!(lines.len() >= 2);
        for line in &lines {
            assert!(display_units(line) <= 40, "单行不得超过上限：{line}");
            assert!(!line.is_empty());
        }
        // 换行按词边界进行：拼回原标题，字符不增删。
        assert_eq!(lines.join(""), title);
    }

    #[test]
    fn wrapped_title_never_has_light_head_and_heavy_foot() {
        // 首行不短于末行（头不轻脚不重），且相邻行长短不悬殊。
        let titles = [
            "关于转发国家互联网信息办公室有关网络安全和信息化工作重点任务实施方案的通知",
            "关于开展2026年度网络安全考核评估暨重点工作任务落实情况检查的通知",
            "关于认真做好信息化建设及网络安全管理有关重点事项整改落实工作的通知",
        ];
        for title in titles {
            let TitlePlan::Wrapped(lines) = title_plan(title, 20) else {
                panic!("应走 jieba 换行：{title}");
            };
            assert_eq!(lines.join(""), title);
            let first = display_units(&lines[0]);
            let last = display_units(lines.last().unwrap());
            assert!(
                first >= last,
                "首行（{first} 半角单位）不得短于末行（{last} 半角单位）：{title} -> {lines:?}"
            );
            for line in &lines {
                assert!(display_units(line) <= 40, "单行超上限：{line}");
            }
        }
    }

    fn glued(words: &[&str]) -> Vec<String> {
        let units = words.iter().map(|word| Unit::new(word, "n")).collect();
        glue_units(units)
            .into_iter()
            .map(|unit| unit.text)
            .collect()
    }

    #[test]
    fn short_brackets_and_punctuation_are_never_split() {
        assert_eq!(
            glued(&["全市", "平台", "（", "二期", "）", "建设"]),
            ["全市", "平台", "（二期）", "建设"]
        );
        // 开括号不落行尾、闭括号与顿号不起行，即使括注很长。
        let long = [
            "关于",
            "（",
            "一二三四五六七八九十一二三",
            "四五",
            "）",
            "、",
            "通知",
        ];
        assert_eq!(
            glued(&long),
            ["关于", "（一二三四五六七八九十一二三", "四五）、", "通知"]
        );
    }

    #[test]
    fn particles_numbers_and_ordinals_stay_with_their_neighbours() {
        assert_eq!(
            glued(&["算力", "网络", "的", "若干", "建议"]),
            ["算力", "网络的", "若干", "建议"]
        );
        assert_eq!(glued(&["第", "3", "期", "简报"]), ["第3期", "简报"]);
        assert_eq!(glued(&["2026", "年度", "工作"]), ["2026年度", "工作"]);
    }

    #[test]
    fn cover_titles_wrap_at_sixteen_characters_and_balance() {
        assert_eq!(
            chars_per_line_for(COVER_TITLE_WIDTH_PT, COVER_TITLE_SIZE_PT),
            16
        );
        assert_eq!(
            cover_title_lines("人工智能风险管理框架（1.0版）"),
            ["人工智能风险管理框架（1.0版）"]
        );
        let title = "全市一体化政务数据共享平台（二期）建设项目";
        let lines = cover_title_lines(title);
        assert_eq!(lines.join(""), title);
        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .iter()
                .all(|line| !line.contains('（') || line.contains('）')),
            "括注不得拆开：{lines:?}"
        );
        assert!(
            display_units(&lines[0]) >= display_units(&lines[1]),
            "{lines:?}"
        );
        for title in [
            "关于加快构建全市一体化算力网络的若干建议",
            "大模型在政务服务中的应用风险与治理对策研究",
        ] {
            let lines = cover_title_lines(title);
            assert_eq!(lines.join(""), title);
            assert!(
                lines.iter().all(|line| display_units(line) <= 32),
                "{lines:?}"
            );
            assert!(!lines[1].starts_with('的'), "{lines:?}");
        }
    }

    #[test]
    fn wrapped_lines_rejoin_to_original() {
        let title = "关于组织召开2026年度网络安全工作推进会商会议的通知";
        let TitlePlan::Wrapped(lines) = title_plan(title, 20) else {
            panic!("应走 jieba 换行");
        };
        assert_eq!(lines.join(""), title, "换行不得增删字符");
        for line in &lines {
            assert!(display_units(line) <= 40, "单行超上限：{line}");
        }
    }
}
