use super::model::{Status, Task};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
/// 单份材料的字节上限。Pi 每份材料文件 512 KiB、整轮合计 4 MiB：整份材料按这个
/// 上限收，超过就明确拒绝；交给模型前由程序按块拆分，不静默截断原文。
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
/// 任务交付 JSON 的上限，与 host 的 `submit_task` 一致。
pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
/// 一次材料整理最多收多少条候选。
pub const MAX_CANDIDATES: usize = 100;
/// 批量整理里正文字段顶替摘录的最短字数。
const EVIDENCE_FALLBACK_MIN: usize = 6;

/// 模型常把 JSON 包在 Markdown 代码块里，或前后带一两句说明：取第一个 `{` 到最后
/// 一个 `}` 之间的部分再解析。取不到就原样交给解析器，由它报格式错误。
fn json_body(raw: &str) -> &str {
    let raw = raw.trim();
    match (raw.find('{'), raw.rfind('}')) {
        (Some(start), Some(end)) if start < end => &raw[start..=end],
        _ => raw,
    }
}

/// 日期要有原文支撑。原文写了完整日期（`2026-11-15` 或 `2026年11月15日`）就照它；
/// 只写了月日（`11月15日` / `11月15号`）时，必须是今天起最近的那一个，防止模型自己
/// 编年份（小模型常把「12月15日」写成训练语料里的某一年）；原文里找不到这个月日就不认。
pub fn date_supported(source: &str, date: &str, today: &str) -> bool {
    use chrono::Datelike;
    let Ok(day) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
        return false;
    };
    let Ok(now) = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") else {
        return false;
    };
    let text = source.split_whitespace().collect::<String>();
    let (y, m, d) = (day.year(), day.month(), day.day());
    let month_days = [
        format!("{m}月{d}日"),
        format!("{m}月{d}号"),
        format!("{m:02}月{d:02}日"),
    ];
    if text.contains(date)
        || month_days
            .iter()
            .any(|md| text.contains(&format!("{y}年{md}")))
    {
        return true;
    }
    if !month_days.iter().any(|md| text.contains(md.as_str())) {
        return false;
    }
    let nearest = [now.year(), now.year() + 1]
        .into_iter()
        .filter_map(|year| chrono::NaiveDate::from_ymd_opt(year, m, d))
        .find(|candidate| *candidate >= now);
    nearest == Some(day)
}

/// 批量候选的期限逐条核对原文（见 [`date_supported`]）：对不上的期限清空；
/// 本身就是「期限调整」的候选，期限没了也就没有内容，整条拿掉。只在模型刚返回时
/// 调用——`parse_response` 还用于编辑候选时的复核，那时的「今天」已经变了。
pub fn check_proposal_dates(items: Vec<Proposal>, source: &str, today: &str) -> Vec<Proposal> {
    items
        .into_iter()
        .filter_map(|mut p| {
            if p.deadline
                .as_deref()
                .is_some_and(|d| !date_supported(source, d, today))
            {
                p.deadline = None;
                if p.kind == "deadline" {
                    return None;
                }
            }
            Some(p)
        })
        .collect()
}

/// 摘录允许复制过程产生的空白与全半角标点差异，内容仍须连续匹配。
pub fn evidence_in(source: &str, evidence: &str) -> bool {
    if evidence.trim().is_empty() {
        return false;
    }
    if source.contains(evidence) {
        return true;
    }
    fn normalize(s: &str) -> String {
        s.chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| match c {
                '，' => ',',
                '。' => '.',
                '；' => ';',
                '：' => ':',
                '！' => '!',
                '？' => '?',
                '（' => '(',
                '）' => ')',
                '“' | '”' => '"',
                '‘' | '’' => '\'',
                x => x,
            })
            .collect()
    }
    let source = normalize(source);
    let evidence = normalize(evidence);
    !evidence.is_empty() && source.contains(&evidence)
}
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Proposal {
    pub kind: String,
    pub task_id: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    pub next_step: Option<String>,
    pub deadline: Option<String>,
    #[serde(default)]
    pub collaborators: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub owner_id: Option<u64>,
    #[serde(default)]
    pub owner_identity: String,
    #[serde(default)]
    pub progress: Option<String>,
    #[serde(default)]
    pub risks: Option<String>,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default)]
    pub about: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// 模型偶尔漏掉摘录；缺了按空处理，由下面的正文字段顶替规则核对，不让整批作废。
    #[serde(default)]
    pub evidence: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    /// 任务作业交付的 JSON 带 `mode` 与 `notes`；只有 `items` 数组的旧格式照常接受。
    #[serde(default)]
    #[allow(dead_code)]
    mode: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    notes: Vec<String>,
    items: Vec<Proposal>,
}
/// `known` 是现有任务：模型只能引用其中未删除的 id，不能凭记忆造 ID。
pub fn parse_response(raw: &str, source: &str, known: &[Task]) -> Result<Vec<Proposal>> {
    if raw.len() > MAX_OUTPUT_BYTES {
        bail!("模型输出超过上限")
    }
    let data = match serde_json::from_str::<Envelope>(json_body(raw)) {
        Ok(data) => data,
        // 小模型常省掉外层 `{"items":…}`，直接给数组（没东西时就是 `[]`）。
        Err(e) => {
            let raw = raw.trim();
            let array = match (raw.find('['), raw.rfind(']')) {
                (Some(start), Some(end)) if start < end => &raw[start..=end],
                _ => raw,
            };
            match serde_json::from_str::<Vec<Proposal>>(array) {
                Ok(items) => Envelope {
                    mode: None,
                    notes: Vec::new(),
                    items,
                },
                Err(_) => return Err(e).context("模型结果不是约定的 JSON 格式"),
            }
        }
    };
    if data.items.len() > MAX_CANDIDATES {
        bail!("模型建议数量超过上限")
    }
    let ids: std::collections::HashSet<&str> = known
        .iter()
        .filter(|task| !task.deleted)
        .map(|task| task.id.as_str())
        .collect();
    let mut out = Vec::new();
    for mut p in data.items {
        if ![
            "new_task",
            "progress",
            "decision",
            "opinion",
            "owner",
            "responsibility",
            "deadline",
            "risk",
            "scope",
            "completion_evidence",
        ]
        .contains(&p.kind.as_str())
        {
            bail!("模型输出了不支持的候选类型");
        }
        p.owner_identity.clear();
        p.owner_id = None; // 模型给出的词条 ID 不可信，只能由界面匹配标准词库。
        if !evidence_in(source, &p.evidence) || p.evidence.chars().count() > 500 {
            // 摘录对不上时，只允许一段足够长、原样出现在原文里的正文字段顶替摘录。
            // 人名、意见主体、日期这类短值在原文里随处可见，拿来当出处等于没有出处。
            let supported = [&p.name, &p.progress, &p.next_step, &p.risks, &p.note]
                .into_iter()
                .flatten()
                .find(|value| {
                    (EVIDENCE_FALLBACK_MIN..=500).contains(&value.chars().count())
                        && evidence_in(source, value)
                });
            if let Some(value) = supported {
                p.evidence = value.clone();
            } else {
                bail!("拒绝不在原文中的证据摘录");
            }
        }
        if p.evidence.chars().count() > 500 {
            bail!("证据摘录不能超过 500 字")
        }
        if let Some(id) = &p.task_id
            && !ids.contains(id.as_str())
        {
            bail!("模型引用了上下文外任务，已拒绝结果")
        }
        if p.kind == "new_task" && p.task_id.is_some() {
            bail!("新任务候选不能同时指定已有任务");
        }
        if p.deadline
            .as_ref()
            .is_some_and(|d| !super::model::valid_date(&Some(d.clone())))
        {
            bail!("候选日期不是有效的 YYYY-MM-DD");
        }
        for field in [
            &p.name,
            &p.next_step,
            &p.owner,
            &p.progress,
            &p.risks,
            &p.speaker,
            &p.about,
            &p.note,
        ] {
            if field
                .as_ref()
                .is_some_and(|x| x.chars().count() > super::model::MAX_TEXT)
            {
                bail!("候选文本超过长度上限");
            }
        }
        // 拟议调整只能记录待决，不能借“采纳进展”悄悄改变正式安排。
        if p.kind != "new_task"
            && p.kind != "opinion"
            && ["建议", "拟于", "拟将", "待批准", "待确认", "尚待", "拟定"]
                .iter()
                .any(|x| p.evidence.contains(x))
        {
            p.kind = "decision".into();
            if p.note.is_none() {
                p.note = Some(p.evidence.clone());
            }
        }
        if p.kind == "responsibility" {
            p.kind = "owner".into();
        }
        // 一项任务都没有时，「进展 / 风险 / 期限 / 负责人 / 完成证据」没有可挂的目标，
        // 人也选不出来；内容照留，改成新任务候选，由人补名称后建成任务。
        if known.is_empty() && p.task_id.is_none() && converts_to_new_task(&p.kind) {
            p.kind = "new_task".into();
        }
        // 同类型、同目标、同摘录的只留第一条（小模型常对同一段再给一条空壳）。
        if out.iter().any(|q: &Proposal| {
            q.kind == p.kind && q.task_id == p.task_id && q.evidence == p.evidence
        }) {
            continue;
        }
        out.push(p)
    }
    Ok(out)
}
/// 讲某项工作本身内容的候选：没有目标任务时可以改作新任务建成。意见与待决事项
/// 只是记进任务历史的一句话，不在此列。
pub fn converts_to_new_task(kind: &str) -> bool {
    matches!(
        kind,
        "progress" | "risk" | "deadline" | "owner" | "completion_evidence"
    )
}

pub fn proposal_task(p: &Proposal) -> Task {
    let mut t = Task::blank();
    t.name = p.name.clone().unwrap_or_default();
    t.next_step = p.next_step.clone().unwrap_or_default();
    t.progress = p.progress.clone().unwrap_or_default();
    // 完成证据的内容在说明里；没挂目标而改建新任务时，拿它当当前进展。
    if t.progress.is_empty() && p.kind == "completion_evidence" {
        t.progress = p.note.clone().unwrap_or_default();
    }
    t.risks = p.risks.clone().unwrap_or_default();
    t.owner_id = p.owner_id;
    t.owner_identity = p.owner_identity.clone();
    t.owner_name = p.owner_id.and_then(|_| p.owner.clone()).unwrap_or_default();
    t.deadline = p.deadline.clone();
    t.needs_review = true;
    t.status = Status::Todo;
    t
}

/// 单任务填表：值与连续原文摘录一起返回，逐栏失败不会丢掉其余合格值。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FilledValue {
    pub value: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Default)]
pub struct Fill {
    pub name: Option<FilledValue>,
    pub owner: Option<FilledValue>,
    pub progress: Option<FilledValue>,
    pub next_step: Option<FilledValue>,
    pub deadline: Option<FilledValue>,
    pub risks: Option<FilledValue>,
    pub others: usize,
    pub dropped: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillFields {
    name: Option<serde_json::Value>,
    owner: Option<serde_json::Value>,
    progress: Option<serde_json::Value>,
    next_step: Option<serde_json::Value>,
    deadline: Option<serde_json::Value>,
    risks: Option<serde_json::Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillEnvelope {
    /// 任务作业交付的 JSON 带 `mode` 与 `notes`；旧格式照常接受。
    #[serde(default)]
    #[allow(dead_code)]
    mode: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    notes: Vec<String>,
    fields: FillFields,
    /// 模型常省略这个计数；省略按 0 算，不为此整体作废。
    #[serde(default)]
    others: usize,
}

/// `today` 用来核对只写了月日的期限（见 [`date_supported`]）。
pub fn parse_fill(raw: &str, source: &str, today: &str) -> Result<Fill> {
    if raw.len() > MAX_OUTPUT_BYTES {
        bail!("填表结果超过长度上限");
    }
    let data: FillEnvelope =
        serde_json::from_str(json_body(raw)).context("填表结果不是约定的 JSON 格式")?;
    if data.others > MAX_CANDIDATES {
        bail!("其它事项数量超过上限");
    }
    let mut fill = Fill {
        others: data.others,
        ..Default::default()
    };
    let fields = data.fields;
    for (slot, raw, kind) in [
        (&mut fill.name, fields.name, "text"),
        (&mut fill.owner, fields.owner, "text"),
        (&mut fill.progress, fields.progress, "text"),
        (&mut fill.next_step, fields.next_step, "text"),
        (&mut fill.deadline, fields.deadline, "date"),
        (&mut fill.risks, fields.risks, "text"),
    ] {
        let Some(raw) = raw else {
            continue;
        };
        let checked = (|| -> Option<FilledValue> {
            // 小模型常只给值不给摘录（`"name":"…"`）：当作摘录为空，按下面的规则用值本身核对。
            let mut v: FilledValue = match raw {
                serde_json::Value::String(value) => FilledValue {
                    value,
                    evidence: String::new(),
                },
                other => serde_json::from_value(other).ok()?,
            };
            v.value = v.value.trim().to_owned();
            if v.value.is_empty() || v.value.chars().count() > super::model::MAX_TEXT {
                return None;
            }
            if kind == "date" {
                // 期限的出处就是原文里的那个日期，摘录有没有、对不对都以原文为准。
                if !date_supported(source, &v.value, today) {
                    return None;
                }
                if !evidence_in(source, &v.evidence) {
                    v.evidence = v.value.clone();
                }
                return Some(v);
            }
            if !evidence_in(source, &v.evidence) || v.evidence.chars().count() > 500 {
                if v.value.chars().count() > 500 || !evidence_in(source, &v.value) {
                    return None;
                }
                v.evidence = v.value.clone();
            }
            Some(v)
        })();
        if checked.is_none() {
            fill.dropped += 1;
        }
        *slot = checked;
    }
    Ok(fill)
}

/// 任务作业交付的两种结构：材料整理候选，或单任务六栏填写。
#[derive(Debug)]
pub enum TaskDelivery {
    Extract(Vec<Proposal>),
    Fill(Box<Fill>),
}

/// 解析任务作业交付的 JSON：`mode` 决定走哪条校验，语义校验与旧路径完全相同。
///
/// 证据仍必须原样出现在材料原文里，期限仍须有原文支撑，候选类型与目标 ID 照旧受限；
/// 交付 JSON 本身不能绕过任何一条。
pub fn parse_task_delivery(
    raw: &str,
    source: &str,
    today: &str,
    known: &[Task],
) -> Result<TaskDelivery> {
    #[derive(Deserialize)]
    struct ModeTag {
        mode: Option<String>,
    }
    let tag: ModeTag =
        serde_json::from_str(json_body(raw)).context("任务交付不是约定的 JSON 格式")?;
    match tag.mode.as_deref() {
        Some("extract") => Ok(TaskDelivery::Extract(parse_response(raw, source, known)?)),
        Some("fill") => Ok(TaskDelivery::Fill(Box::new(parse_fill(
            raw, source, today,
        )?))),
        _ => bail!("任务交付缺少合法的 mode（extract / fill）"),
    }
}

pub fn auto_title(source: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let first = source
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let title = first
        .trim_matches(|c: char| {
            c.is_whitespace()
                || "，。；：、！？,.;:!?\"'‘’“”「」『』《》〈〉【】〔〕（）()[]{}<>［］｛｝"
                    .contains(c)
        })
        .chars()
        .take(30)
        .collect::<String>();
    if title.chars().count() < 4 {
        format!("粘贴材料 {}", now.format("%m-%d %H:%M"))
    } else {
        title
    }
}
