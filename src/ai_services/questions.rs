//! 自主步骤向用户提问时的通用校验；只包含中立题面与选项 DTO。

/// 一道模型候选题，不带旧流程的目标、黑板或 UI 状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QuestionCandidate {
    pub(crate) question: String,
    pub(crate) options: Vec<String>,
}

/// 通过通用安全闸门的题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidatedQuestion {
    pub(crate) question: String,
    pub(crate) options: Vec<String>,
}

pub(crate) const MAX_QUESTION_CHARS: usize = 40;

const AUTHORIZED_WORDS: &[&str] = &[
    "密级",
    "份号",
    "签发人",
    "成文日期",
    "印发机关",
    "印发日期",
    "发文字号",
    "发文机关",
];

fn touches_authorized(text: &str) -> bool {
    AUTHORIZED_WORDS.iter().any(|word| text.contains(word))
        || (text.contains("文号") && !text.contains('《'))
}

/// 模型把提示词里的格式示例「问题｜选项1｜选项2」原样抄了出来。
pub(crate) fn echoes_format(question: &str, options: &[String]) -> bool {
    let is_placeholder = |text: &str| {
        text.strip_prefix("选项")
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
    };
    question.trim_end_matches('？') == "问题" || options.iter().any(|o| is_placeholder(o))
}

/// 校验自主步骤的题目，维持旧 `clarify::agent_questions` 的闸门和中文报错。
pub(crate) fn validate_agent_questions(
    candidates: &[QuestionCandidate],
) -> Result<Vec<ValidatedQuestion>, String> {
    const MAX_QUESTIONS: usize = 3;
    if candidates.is_empty() {
        return Err("没有题目：questions 里至少写一道".into());
    }
    if candidates.len() > MAX_QUESTIONS {
        return Err(format!(
            "一次最多问 {MAX_QUESTIONS} 道，只问最要紧的、只有用户知道的"
        ));
    }
    let mut out = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let question = candidate
            .question
            .trim()
            .trim_end_matches(['?', '？'])
            .to_string()
            + "？";
        let options: Vec<String> = candidate
            .options
            .iter()
            .map(|option| option.trim().to_string())
            .collect();
        let all = std::iter::once(question.as_str())
            .chain(options.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        if touches_authorized(&all) {
            return Err(format!(
                "「{question}」问的是密级、文号、签发人、成文日期这类授权要素：它们由起草人在要素区填，\
                 你不能问也不能建议，不要写进稿子"
            ));
        }
        if question.contains("文种") {
            return Err("文种在要素区定，不在这里问；按当前文种做".into());
        }
        let unique = options
            .iter()
            .enumerate()
            .all(|(i, option)| !options[..i].contains(option));
        if question.chars().count() > MAX_QUESTION_CHARS + 1 {
            return Err(format!(
                "「{question}」太长：问题不超过 {MAX_QUESTION_CHARS} 字，解释放到 progress 里"
            ));
        }
        if options.len() == 1
            || options.len() > 4
            || options.iter().any(String::is_empty)
            || !unique
            || echoes_format(&question, &options)
        {
            return Err(format!(
                "「{question}」的选项不对：要么不给选项（让用户自己填），要么给 2 到 4 个不重复的选项"
            ));
        }
        out.push(ValidatedQuestion { question, options });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(question: &str, options: &[&str]) -> QuestionCandidate {
        QuestionCandidate {
            question: question.into(),
            options: options.iter().map(|value| (*value).into()).collect(),
        }
    }

    #[test]
    fn accepts_free_text_and_two_to_four_unique_options() {
        let actual = validate_agent_questions(&[
            candidate(" 联系人是谁? ", &[]),
            candidate("优先级", &["高", "中"]),
            candidate("范围", &["甲", "乙", "丙", "丁"]),
        ])
        .unwrap();
        assert_eq!(actual[0].question, "联系人是谁？");
        assert!(actual[0].options.is_empty());
        assert_eq!(actual[2].options.len(), 4);
    }

    #[test]
    fn rejects_authorized_kind_format_and_malformed_options() {
        for candidate in [
            candidate("文号是多少", &[]),
            candidate("文种怎么选", &["函", "通知"]),
            candidate("进度如何", &["选项1", "选项2"]),
            candidate("进度如何", &["快", "快"]),
            candidate("进度如何", &["快"]),
            candidate("进度如何", &["一", "二", "三", "四", "五"]),
        ] {
            assert!(validate_agent_questions(&[candidate]).is_err());
        }
        assert!(validate_agent_questions(&[]).is_err());
        assert!(
            validate_agent_questions(&[
                candidate("一", &[]),
                candidate("二", &[]),
                candidate("三", &[]),
                candidate("四", &[]),
            ])
            .is_err()
        );
    }
}
