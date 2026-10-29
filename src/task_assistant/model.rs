use chrono::NaiveDate;
use uuid::Uuid;

pub const MAX_TEXT: usize = 4096;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Todo,
    InProgress,
    Waiting,
    Blocked,
    Done,
    Cancelled,
}
impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Todo => "未开始",
            Self::InProgress => "进行中",
            Self::Waiting => "等待",
            Self::Blocked => "阻塞",
            Self::Done => "已完成",
            Self::Cancelled => "已取消",
        }
    }
    pub fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub category: String,
    pub owner_id: Option<u64>,
    pub owner_name: String,
    pub progress: String,
    pub risks: String,
    pub next_step: String,
    pub deadline: Option<String>,
    pub status: Status,
    pub parent_id: Option<String>,
    pub prerequisites: Vec<String>,
    pub needs_review: bool,
    pub archived: bool,
    pub deleted: bool,
    pub revision: i64,
    pub updated_at: String,
}
impl Task {
    pub fn blank() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            category: "未分类".into(),
            owner_id: None,
            owner_name: String::new(),
            progress: String::new(),
            risks: String::new(),
            next_step: String::new(),
            deadline: None,
            status: Status::Todo,
            parent_id: None,
            prerequisites: vec![],
            needs_review: false,
            archived: false,
            deleted: false,
            revision: 0,
            updated_at: String::new(),
        }
    }
}
pub fn valid_date(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(|s| {
        s.len() == 10
            && NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .is_ok_and(|d| d.format("%Y-%m-%d").to_string() == s)
    })
}
pub fn valid_task(t: &Task) -> Result<(), String> {
    if t.name.trim().is_empty() {
        return Err("任务名称不能为空".into());
    }
    for (label, value) in [
        ("任务名称", &t.name),
        ("当前进展", &t.progress),
        ("风险问题及应对措施", &t.risks),
        ("下一步", &t.next_step),
    ] {
        if value.chars().count() > MAX_TEXT {
            return Err(format!("{label}不能超过 {MAX_TEXT} 字"));
        }
    }
    if !valid_date(&t.deadline) {
        return Err("日期须使用真实日历日期 YYYY-MM-DD".into());
    }
    Ok(())
}
