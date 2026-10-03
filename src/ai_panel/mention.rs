//! 输入框里的 `/` 技能与 `@` 文章：识别光标前的触发符、过滤排序、插入与引用解析
//! （第 ⑥ 期，`docs/ai-agent-workbench.md` 16.13）。纯逻辑，界面在 `composer_ui.rs`。
//!
//! 位置一律按**字符**计（与 egui 文本框的光标一致），不按字节。

use crate::agent::board::Reference;
use crate::models::TemplateKind;
use pinyin::ToPinyin;
use regex::Regex;
use std::sync::LazyLock;

/// 输入框里的引用记号 `@《……》`。
static MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[@＠]《[^《》]*》").expect("引用记号正则"));

/// 触发符后面最多跟几个字还算在过滤；再长多半是正常打字，不弹了。
const MAX_QUERY_CHARS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerKind {
    /// `/`：选技能。
    Skill,
    /// `@`：引用文章。
    Article,
}

/// 光标前正在输入的一个触发：`/润` 或 `@防火`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Trigger {
    pub(crate) kind: TriggerKind,
    /// 触发符所在的字符位置。
    pub(crate) start: usize,
    /// 触发符与光标之间的字。
    pub(crate) query: String,
}

/// 光标前有没有正在输入的触发。
///
/// - `/`（或全角 `／`）只认行首或空白之后，免得「1/2」「和/或」误弹；
/// - `@`（或 `＠`）前面不能紧跟英文字母数字（邮箱地址），中文紧跟着打可以；
/// - 触发符与光标之间不能有空白，`@` 后面不能已经是 `《》` 记号。
pub(crate) fn detect(text: &str, caret: usize) -> Option<Trigger> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    for start in (0..caret).rev() {
        let c = chars[start];
        if c.is_whitespace() || caret - start > MAX_QUERY_CHARS + 1 {
            return None;
        }
        let kind = match c {
            '/' | '／' => TriggerKind::Skill,
            '@' | '＠' => TriggerKind::Article,
            _ => continue,
        };
        let before = start.checked_sub(1).map(|i| chars[i]);
        let ok = match kind {
            TriggerKind::Skill => before.is_none_or(char::is_whitespace),
            TriggerKind::Article => before.is_none_or(|b| !b.is_ascii_alphanumeric()),
        };
        let query: String = chars[start + 1..caret].iter().collect();
        if !ok || query.contains(['《', '》', '/', '／', '@', '＠']) {
            return None;
        }
        return Some(Trigger { kind, start, query });
    }
    None
}

/// 把 `[trigger.start, caret)` 换成 `replacement`，返回新文字与新的光标位置。
pub(crate) fn replace_trigger(
    text: &str,
    trigger: &Trigger,
    caret: usize,
    replacement: &str,
) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.clamp(trigger.start, chars.len());
    let mut out: String = chars[..trigger.start].iter().collect();
    out.push_str(replacement);
    // 删掉 `/技能` 后两边都是空白时并成一个，不留双空格。
    let rest: String = chars[caret..].iter().collect();
    let rest = if replacement.is_empty() && out.ends_with(char::is_whitespace) {
        rest.trim_start_matches(' ').to_string()
    } else {
        rest
    };
    let new_caret = trigger.start + replacement.chars().count();
    out.push_str(&rest);
    (out, new_caret)
}

/// 文字的全拼与拼音首字母（非汉字原样小写）：「防火通知」→ (fanghuotongzhi, fhtz)。
fn pinyin_keys(text: &str) -> (String, String) {
    let mut full = String::new();
    let mut initials = String::new();
    for ch in text.chars() {
        if let Some(py) = ch.to_pinyin() {
            let syllable = py.plain().replace('ü', "v");
            full.push_str(&syllable);
            if let Some(first) = syllable.chars().next() {
                initials.push(first);
            }
        } else {
            full.extend(ch.to_lowercase());
            initials.extend(ch.to_lowercase());
        }
    }
    (full, initials)
}

/// 一项对查询词的匹配分；不匹配返回 None。名称打头 > 名称包含 > 首字母打头 > 拼音包含。
/// `extra` 是 id 一类的别名，只认打头。
pub(crate) fn score(query: &str, name: &str, extra: &str) -> Option<u8> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let name_lower = name.to_lowercase();
    if name_lower.starts_with(&query) || extra.to_lowercase().starts_with(&query) {
        return Some(5);
    }
    if name_lower.contains(&query) {
        return Some(4);
    }
    if !query.is_ascii() {
        return None;
    }
    let (full, initials) = pinyin_keys(name);
    if initials.starts_with(&query) || full.starts_with(&query) {
        Some(3)
    } else if initials.contains(&query) {
        Some(2)
    } else if full.contains(&query) {
        Some(1)
    } else {
        None
    }
}

/// 按查询词过滤并排序，返回下标；分数相同保持原顺序。
pub(crate) fn rank<T>(query: &str, items: &[T], key: impl Fn(&T) -> (&str, &str)) -> Vec<usize> {
    let mut scored: Vec<(usize, u8)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let (name, extra) = key(item);
            score(query, name, extra).map(|s| (index, s))
        })
        .collect();
    scored.sort_by_key(|&(index, s)| (std::cmp::Reverse(s), index));
    scored.into_iter().map(|(index, _)| index).collect()
}

/// `@` 列表里的一篇：引用本身加上显示用的文种与日期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogItem {
    pub(crate) reference: Reference,
    pub(crate) kind: TemplateKind,
    /// 成文日期或更新日期，只显示用。
    pub(crate) date: String,
}

/// 引用在输入框里留的记号。
pub(crate) fn mark(title: &str) -> String {
    format!("@《{title}》")
}

/// 输入框里的记号还在的引用（用户可能直接把记号删了），按出现先后、去重。
pub(crate) fn live_refs(text: &str, refs: &[Reference]) -> Vec<Reference> {
    let mut live: Vec<(usize, Reference)> = Vec::new();
    for reference in refs {
        if let Some(at) = text.find(&mark(&reference.title))
            && !live
                .iter()
                .any(|(_, r)| r.source == reference.source && r.id == reference.id)
        {
            live.push((at, reference.clone()));
        }
    }
    live.sort_by_key(|(at, _)| *at);
    live.into_iter().map(|(_, r)| r).collect()
}

/// 发给技能的原话：`@《标题》` 记号去掉 `@`，读起来就是「参照《标题》」。
pub(crate) fn plain_request(text: &str) -> String {
    text.replace("@《", "《").replace("＠《", "《")
}

/// 选技能时看的文字：引用的记号与书名都去掉。触发词按命中长度打分，「参照@《……》」
/// 这种长书名会把「仿照 / 参照」类技能的分数撑大，盖过真正的要求。
pub(crate) fn routing_text(text: &str, refs: &[Reference]) -> String {
    let mut out = text.to_string();
    for reference in refs {
        out = out
            .replace(&mark(&reference.title), "")
            .replace(&format!("《{}》", reference.title), "");
    }
    MARK.replace_all(&out, "").into_owned()
}

/// 去掉一个引用：记号换回不带 `@` 的书名，句子还通顺。
pub(crate) fn unlink(text: &str, title: &str) -> String {
    text.replacen(&mark(title), &format!("《{title}》"), 1)
}

/// 弹出层里的按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PopupKey {
    Up,
    Down,
    /// 回车或 Tab：选中高亮项。
    Pick,
    /// Esc：关掉，这一个触发不再弹。
    Close,
}

/// `/`、`@` 弹出层的状态，存在输入区里。
#[derive(Debug, Default)]
pub(crate) struct PopupState {
    /// 高亮的是第几项。
    pub(crate) highlight: usize,
    /// 当前开着的触发。
    pub(crate) open: Option<Trigger>,
    /// 按 Esc 关掉的触发（种类与位置）：同一个触发不再弹，换个地方再敲才弹。
    dismissed: Option<(TriggerKind, usize)>,
    /// `@` 列表：弹出时读一次，关掉就扔，下次重读（稿件库可能刚存了新稿）。
    pub(crate) catalog: Option<Vec<CatalogItem>>,
}

impl PopupState {
    /// 按光标前识别出的触发更新状态，返回此刻该开着的那个。查询词一变，高亮回到第一项。
    pub(crate) fn sync(&mut self, trigger: Option<Trigger>) -> Option<&Trigger> {
        let Some(trigger) = trigger else {
            self.open = None;
            self.dismissed = None;
            self.catalog = None;
            return None;
        };
        if self.dismissed == Some((trigger.kind, trigger.start)) {
            self.open = None;
            return None;
        }
        self.dismissed = None;
        if self.open.as_ref() != Some(&trigger) {
            self.highlight = 0;
        }
        if trigger.kind != TriggerKind::Article {
            self.catalog = None;
        }
        self.open = Some(trigger);
        self.open.as_ref()
    }

    /// 处理一个按键；选中时返回选中项的下标。`len` 是当前列表的长度。
    pub(crate) fn key(&mut self, key: PopupKey, len: usize) -> Option<usize> {
        match key {
            PopupKey::Up if len > 0 => self.highlight = (self.highlight + len - 1) % len,
            PopupKey::Down if len > 0 => self.highlight = (self.highlight + 1) % len,
            PopupKey::Pick if len > 0 => {
                let picked = self.highlight.min(len - 1);
                self.open = None;
                return Some(picked);
            }
            PopupKey::Close => {
                self.dismissed = self.open.take().map(|t| (t.kind, t.start));
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::board::RefSource;

    fn at(text: &str) -> Option<Trigger> {
        detect(text, text.chars().count())
    }

    #[test]
    fn slash_only_triggers_at_line_start_or_after_space() {
        assert_eq!(
            at("/润"),
            Some(Trigger {
                kind: TriggerKind::Skill,
                start: 0,
                query: "润".into()
            })
        );
        assert_eq!(at("压缩一下 /").map(|t| t.start), Some(5));
        assert_eq!(at("第一段\n／rs").map(|t| t.query), Some("rs".into()));
        assert_eq!(at("1/2"), None, "分数不弹");
        assert_eq!(at("和/或"), None);
        assert_eq!(at("/润色 只改"), None, "空格之后就不算在过滤了");
    }

    #[test]
    fn at_sign_triggers_inside_chinese_but_not_in_email() {
        let trigger = at("参照@防火").unwrap();
        assert_eq!(trigger.kind, TriggerKind::Article);
        assert_eq!(trigger.start, 2);
        assert_eq!(trigger.query, "防火");
        assert_eq!(at("参照＠").map(|t| t.query), Some(String::new()));
        assert_eq!(at("mail me@abc"), None, "邮箱不弹");
        assert_eq!(at("参照@《去年通知》写"), None, "已经插好的记号不再弹");
        // 光标在中间：只看光标前面。
        assert_eq!(
            detect("参照@防火通知", 4).map(|t| t.query),
            Some("防".into())
        );
        let long = format!("@{}", "字".repeat(MAX_QUERY_CHARS + 1));
        assert_eq!(at(&long), None);
    }

    #[test]
    fn filtering_matches_names_ids_and_pinyin_initials() {
        assert_eq!(score("润", "润色", "polish"), Some(5));
        assert_eq!(score("pol", "润色", "polish"), Some(5));
        assert_eq!(score("色", "润色", "polish"), Some(4));
        assert_eq!(score("rs", "润色", "polish"), Some(3));
        assert_eq!(score("runse", "润色", "polish"), Some(3));
        assert_eq!(score("ht", "防火通知", ""), Some(2));
        assert_eq!(score("tongzhi", "防火通知", ""), Some(1));
        assert_eq!(score("xyz", "润色", "polish"), None);
        assert_eq!(score("压缩", "润色", "polish"), None);
        assert_eq!(score("", "润色", ""), Some(0), "空查询全列");

        let names = ["要点提炼", "润色", "仿写", "语气调整"];
        assert_eq!(
            rank("y", &names, |name| (name, "")),
            [0, 3],
            "同分保持原顺序"
        );
        assert_eq!(rank("", &names, |name| (name, "")), [0, 1, 2, 3]);
        let names = ["润色", "色彩", "仿写"];
        assert_eq!(
            rank("s", &names, |name| (name, "")),
            [1, 0],
            "首字母打头的排在首字母包含之前"
        );
        assert_eq!(rank("色", &names, |name| (name, "")), [1, 0]);
    }

    #[test]
    fn picking_replaces_the_trigger_and_moves_the_caret() {
        let text = "参照@防火写今年的";
        let trigger = detect(text, 5).unwrap();
        let (out, caret) = replace_trigger(text, &trigger, 5, &mark("2025年冬季森林防火通知"));
        assert_eq!(out, "参照@《2025年冬季森林防火通知》写今年的");
        assert_eq!(caret, 2 + "@《2025年冬季森林防火通知》".chars().count());

        let text = "压缩 /精 到八百字";
        let trigger = detect(text, 5).unwrap();
        let (out, caret) = replace_trigger(text, &trigger, 5, "");
        assert_eq!(out, "压缩 到八百字", "去掉 /技能 不留双空格");
        assert_eq!(caret, 3);
    }

    #[test]
    fn references_follow_the_marks_left_in_the_text() {
        let a = Reference {
            source: RefSource::Manuscript,
            id: 7,
            title: "甲通知".into(),
        };
        let b = Reference {
            source: RefSource::Knowledge,
            id: 3,
            title: "乙办法".into(),
        };
        let refs = vec![a.clone(), b.clone(), a.clone()];
        let text = "依据@《乙办法》，参照@《甲通知》写";
        assert_eq!(
            live_refs(text, &refs),
            [b.clone(), a.clone()],
            "按出现先后、去重"
        );
        assert_eq!(
            live_refs("参照@《甲通知》", &refs),
            std::slice::from_ref(&a)
        );
        assert_eq!(live_refs("记号删掉了", &refs), []);
        assert_eq!(plain_request(text), "依据《乙办法》，参照《甲通知》写");
        assert_eq!(routing_text(text, &refs), "依据，参照写");
        assert_eq!(
            routing_text("参照《甲通知》压缩", &refs),
            "参照压缩",
            "发送时记号已换成书名，照样去掉"
        );
        assert_eq!(routing_text("看@《别的》", &[]), "看");
        assert_eq!(unlink(text, "甲通知"), "依据@《乙办法》，参照《甲通知》写");
    }
    #[test]
    fn keyboard_moves_wraps_picks_and_dismisses() {
        let mut popup = PopupState::default();
        let trigger = at("/r").unwrap();
        assert!(popup.sync(Some(trigger.clone())).is_some());
        assert_eq!(popup.key(PopupKey::Up, 3), None);
        assert_eq!(popup.highlight, 2, "从第一项往上绕到最后一项");
        popup.key(PopupKey::Down, 3);
        assert_eq!(popup.highlight, 0);
        popup.key(PopupKey::Down, 3);
        // 查询词没变，高亮不动；变了回到第一项。
        popup.sync(Some(trigger.clone()));
        assert_eq!(popup.highlight, 1);
        popup.sync(at("/rs"));
        assert_eq!(popup.highlight, 0);
        popup.key(PopupKey::Down, 3);
        assert_eq!(popup.key(PopupKey::Pick, 3), Some(1));
        assert_eq!(popup.key(PopupKey::Pick, 0), None, "空列表回车不选");

        // Esc 关掉之后，同一个触发继续打字也不再弹；删掉重敲才弹。
        popup.sync(at("/r"));
        popup.key(PopupKey::Close, 3);
        assert!(popup.sync(at("/r")).is_none());
        assert!(popup.sync(at("/ru")).is_none());
        assert!(popup.sync(None).is_none());
        assert!(popup.sync(at("/r")).is_some());
    }

    #[test]
    fn the_article_list_is_dropped_when_the_popup_closes() {
        let mut popup = PopupState {
            catalog: Some(Vec::new()),
            ..PopupState::default()
        };
        popup.sync(at("@防"));
        assert!(popup.catalog.is_some());
        popup.sync(None);
        assert!(popup.catalog.is_none(), "下次弹出时重读");
    }
}
