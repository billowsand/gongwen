//! 「把 AI 工作稿提交到正文」这类话：程序认，不交给模型。
//!
//! 正文只能由用户合并（红线 1），模型没有、也不能有写正文的工具，所以这句话发给模型它只会
//! 不知所措，或者把它当成又一次改稿要求。这里在发送前按确定性规则认出来，侧栏改为弹出
//! 「写入正文？」确认卡，用户点「接受」走的是与结果卡「采用」同一个 `accept_ai_proposal`
//! ——同样的事实核对与闸门。认错了也只是多出一张卡，点「算了」即可，所以宁宽勿漏。

/// 命令写法。
const COMMANDS: [&str; 4] = ["/采用", "/接受", "/accept", "/commit"];
/// 写入的动作。
const VERBS: [&str; 16] = [
    "提交",
    "写入",
    "写进",
    "放进",
    "放到",
    "放入",
    "合并",
    "合入",
    "落到",
    "落入",
    "同步到",
    "更新到",
    "应用到",
    "替换掉",
    "覆盖",
    "转成",
];
/// 说的是哪份稿子。
const SOURCES: [&str; 13] = [
    "工作稿",
    "提案",
    "修改稿",
    "改稿",
    "改好的",
    "这版",
    "这稿",
    "这一版",
    "结果",
    "AI稿",
    "它",
    "这个",
    "上面",
];
/// 不带「正文」也算的说法：动词后面紧跟着说的是哪份（「采用这个提案」「接受修改」）。
/// 要紧跟着：「采用更正式的表述修改一下」是改稿要求，不是采纳。
static ADOPT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(采用|接受|应用|采纳)(这个|这份|这一版|该|你的|AI的|ai的|全部|所有)?(提案|修改|改动|工作稿|这版|这稿)",
    )
    .expect("采纳说法正则")
});
/// 否定：「先不要提交到正文」。
const NEGATIONS: [&str; 6] = ["不要", "别", "不用", "先不", "暂不", "不必"];
/// 改的是正文本身（「把正文里的数字替换成……」），不是把稿子写进正文。
const BODY_EDITS: [&str; 3] = ["正文里", "正文中", "正文的"];
/// 再长多半是一段真正的改稿要求，只是顺带提到了正文。
const MAX_CHARS: usize = 40;

/// 这句话是不是要把待确认的 AI 提案写入正文。
pub(crate) fn is_commit_request(text: &str) -> bool {
    let text: String = text
        .trim()
        .trim_end_matches(['。', '！', '!', '.', '～', '~'])
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if text.is_empty() {
        return false;
    }
    if text.starts_with('/') {
        return COMMANDS
            .iter()
            .any(|command| text.eq_ignore_ascii_case(command));
    }
    if text.chars().count() > MAX_CHARS
        || NEGATIONS.iter().any(|word| text.contains(word))
        || BODY_EDITS.iter().any(|word| text.contains(word))
    {
        return false;
    }
    let has = |words: &[&str]| words.iter().any(|word| text.contains(word));
    let into_body = text.contains("正文") && has(&VERBS) && has(&SOURCES);
    let adopt = ADOPT.is_match(&text);
    into_body || adopt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_to_commit_the_workspace_are_recognised() {
        for text in [
            "请把现在的AI工作稿提交到正文",
            "把工作稿写入正文。",
            "把 AI 工作稿 放进正文",
            "提案合并到正文吧",
            "把改好的同步到正文",
            "把它提交到正文",
            "采用这个提案",
            "接受修改",
            "/采用",
            "/accept",
        ] {
            assert!(is_commit_request(text), "应当认出：{text}");
        }
    }

    #[test]
    fn edits_negations_and_long_requests_are_left_to_the_skills() {
        for text in [
            "先不要提交到正文",
            "别把工作稿写入正文",
            "把正文里的数字替换成阿拉伯数字",
            "正文写得太长了，压缩一下",
            "润色一下第二段",
            "/compact",
            "/采用吧",
            "采用更正式的表述修改一下",
            "应用公文的写法改一下提案",
            "",
            "把工作稿第二部分改得更正式一些，然后写进正文，并补上各单位联系人与联系电话，再核对一遍日期",
        ] {
            assert!(!is_commit_request(text), "不该认：{text}");
        }
    }
}
