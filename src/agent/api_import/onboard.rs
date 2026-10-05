//! 接入循环：识别之后不止一轮，一直试到候选接口调通，或者多轮之后确实不行、说清卡在哪。
//!
//! 每一轮：
//! 1. **试调**：配置齐了、改过或没试过的接口用样例值试一次，程序给结论（[`diagnose`]）；
//! 2. **问人**：只有人知道的一次问完——密钥（粘贴，值不给模型看）、服务器地址、样例值、
//!    判断不了的「只查询吗」、连不上时接没接内网、文档没写的鉴权带法；
//! 3. **交模型**：路径、方法、参数、请求体、返回映射这类要对照资料改的，交起草模型用工具
//!    查资料、改配置、再试（[`fix`]），改动由程序逐项核对；
//! 4. 都调通、都有结论，或者一轮下来毫无进展，就停。
//!
//! 只改候选接口的工作副本；加入数据接口仍要用户点。改数据的接口不试、不加入。

mod diagnose;
mod fix;
#[cfg(test)]
mod tests;

use super::auth::{self, AuthPlace, AuthSpec};
use super::redact::secret_name;
use super::{Access, Draft, Material, refine_with_reply};
use crate::agent::api::{self, ApiEndpoint, ApiSecrets, Trial};
use crate::agent::backend::ModelBackend;
pub(crate) use diagnose::Verdict;
use diagnose::verdict;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// 最多几轮。
const MAX_ROUNDS: usize = 6;
/// 每个接口最多试调几次（含模型修配置时的试调）。
const MAX_TRIALS: usize = 8;
/// 每个接口最多交模型修几回。
const MAX_FIXES: usize = 2;
/// 同一个密钥被拒后最多再问几次。
const MAX_REKEY: usize = 2;

/// 一个接口在循环里的状态。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum State {
    /// 还在处理。
    Working,
    /// 调通了。
    Done,
    /// 试过了，卡住：原因。
    Stuck(String),
    /// 用户跳过、缺东西没法试：原因。
    Skipped(String),
    /// 不参与：没勾选、会改数据。
    Excluded(String),
}

impl State {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Working => "处理中".into(),
            Self::Done => "调通了".into(),
            Self::Stuck(why) => format!("卡住：{why}"),
            Self::Skipped(why) => format!("跳过：{why}"),
            Self::Excluded(why) => format!("不处理：{why}"),
        }
    }
}

/// 循环里的一个候选接口。
#[derive(Debug, Clone)]
pub(crate) struct Item {
    pub(crate) draft: Draft,
    /// 人确认过只查询。
    pub(crate) confirmed: bool,
    pub(crate) state: State,
    pub(crate) trial: Option<Trial>,
    pub(crate) verdict: Option<Verdict>,
    /// 配置改过（或从没试过），要重测。
    dirty: bool,
    trials: usize,
    fixes: usize,
    /// 用户说不是鉴权问题 / Key 没错：鉴权类失败也交模型按文档核对带法。
    auth_doubted: bool,
    /// 做过的事，交给模型参考。
    history: Vec<String>,
}

impl Item {
    /// `chosen`：界面上勾选了要加入；`trial`：界面上已经试过的结果，有就不重测。
    pub(crate) fn new(draft: Draft, chosen: bool, confirmed: bool, trial: Option<Trial>) -> Self {
        let state = if draft.access == Access::Write {
            State::Excluded("会改数据".into())
        } else if !chosen {
            State::Excluded("没勾选".into())
        } else {
            State::Working
        };
        let verdict = trial.as_ref().map(|t| verdict(t, &draft.endpoint));
        let state = match (&state, &verdict) {
            (State::Working, Some(Verdict::Ok)) => State::Done,
            _ => state,
        };
        Self {
            dirty: trial.is_none(),
            draft,
            confirmed,
            state,
            trial,
            verdict,
            trials: 0,
            fixes: 0,
            auth_doubted: false,
            history: Vec::new(),
        }
    }

    fn working(&self) -> bool {
        self.state == State::Working
    }

    fn name(&self) -> &str {
        &self.draft.endpoint.name
    }

    fn query(&self) -> bool {
        self.draft.access == Access::Query || self.confirmed
    }

    fn host(&self) -> String {
        host_of(&self.draft.endpoint.url)
    }

    /// 配置齐了、能发请求（只看程序能判断的：地址完整、密钥有值、必填参数有样例、确认了只查询）。
    fn ready(&self, secrets: &ApiSecrets) -> bool {
        let endpoint = &self.draft.endpoint;
        self.query()
            && endpoint.url.starts_with("http")
            && secrets.missing(endpoint).is_empty()
            && endpoint
                .inputs
                .iter()
                .all(|i| !i.required || !i.example.trim().is_empty())
            && endpoint.problems().is_empty()
    }
}

/// 要问用户的一件事。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Question {
    pub(crate) id: String,
    pub(crate) text: String,
    /// 为什么问、依据。
    pub(crate) basis: String,
    pub(crate) ask: Ask,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ask {
    /// 粘贴密钥：值只进本机密钥表，不给模型看。
    Secret {
        name: String,
    },
    Text {
        hint: String,
    },
    /// 选一个；`other` 有值时还可以自己写（提示语）。
    Choice {
        options: Vec<String>,
        other: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Answer {
    Text(String),
    Choice(usize),
    Skip,
}

pub(crate) type Answers = BTreeMap<String, Answer>;

/// 循环往界面报的事。
#[derive(Debug, Clone)]
pub(crate) enum Event {
    /// 过程记录一行。
    Step(String),
    /// 第几个候选接口有了新状态（配置、试调结果）。
    Item(usize, Box<Item>),
}

/// 循环与界面之间：报进度、问用户、试调。
pub(crate) trait Desk {
    fn emit(&mut self, event: Event);
    /// 问用户一批问题。返回 None 表示用户停止了。
    fn ask(&mut self, questions: &[Question]) -> Option<Answers>;
    fn cancelled(&self) -> bool;
    /// 试调一次。测试里换成假的。
    fn trial(
        &mut self,
        endpoint: &ApiEndpoint,
        args: &Map<String, Value>,
        secrets: &ApiSecrets,
    ) -> Trial {
        api::trial(endpoint, args, secrets)
    }
}

/// 循环结束时的结论。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Report {
    pub(crate) done: usize,
    /// 没调通的：「名称：原因」。
    pub(crate) open: Vec<String>,
    pub(crate) stopped: bool,
}

impl Report {
    pub(crate) fn summary(&self) -> String {
        let head = if self.stopped {
            "已停止".to_string()
        } else {
            "接入助手做完了".to_string()
        };
        if self.open.is_empty() {
            format!("{head}：调通 {} 个接口。", self.done)
        } else {
            format!(
                "{head}：调通 {} 个，{} 个没调通。",
                self.done,
                self.open.len()
            )
        }
    }
}

/// 循环里共用的东西。
pub(crate) struct Run<'a> {
    pub(crate) items: Vec<Item>,
    pub(crate) secrets: ApiSecrets,
    material: &'a Material,
    model: Option<&'a dyn ModelBackend>,
    /// 用户回答过的文字（服务器地址、参数取值）：核对模型的改动时也算「资料里有」。
    known: String,
    /// 密钥被拒后问过几次。
    rekeyed: BTreeMap<String, usize>,
}

impl<'a> Run<'a> {
    pub(crate) fn new(
        items: Vec<Item>,
        secrets: ApiSecrets,
        material: &'a Material,
        model: Option<&'a dyn ModelBackend>,
    ) -> Self {
        Self {
            items,
            secrets,
            material,
            model,
            known: String::new(),
            rekeyed: BTreeMap::new(),
        }
    }

    /// 跑到底。
    pub(crate) fn run(&mut self, desk: &mut dyn Desk) -> Report {
        let mut stopped = false;
        'rounds: for round in 0..MAX_ROUNDS {
            if desk.cancelled() {
                stopped = true;
                break;
            }
            if round > 0 {
                desk.emit(Event::Step(format!("第 {} 轮", round + 1)));
            }
            let mut progress = false;
            // 1. 试调。
            for index in 0..self.items.len() {
                if desk.cancelled() {
                    stopped = true;
                    break 'rounds;
                }
                let item = &self.items[index];
                if item.working() && item.dirty && item.ready(&self.secrets) {
                    self.test(index, desk);
                    progress = true;
                }
            }
            // 2. 问人。
            let questions = self.questions();
            if !questions.is_empty() {
                desk.emit(Event::Step(format!(
                    "有 {} 件事要你补，补完接着试",
                    questions.len()
                )));
                match desk.ask(&questions) {
                    None => {
                        stopped = true;
                        break;
                    }
                    Some(answers) => progress |= self.apply(&questions, &answers, desk),
                }
            }
            // 3. 交模型：要对照资料改配置的。
            for index in 0..self.items.len() {
                if desk.cancelled() {
                    stopped = true;
                    break 'rounds;
                }
                if self.needs_fix(index) {
                    progress |= self.fix(index, desk);
                }
            }
            if self.items.iter().all(|i| !i.working()) {
                break;
            }
            if !progress {
                break;
            }
        }
        // 还在处理的：给结论。
        for index in 0..self.items.len() {
            let item = &mut self.items[index];
            if item.working() {
                item.state = State::Stuck(if stopped {
                    "停止时还没调通".into()
                } else {
                    item.verdict
                        .as_ref()
                        .map_or("没能试调".into(), Verdict::label)
                });
                desk.emit(Event::Item(index, Box::new(item.clone())));
            }
        }
        let report = Report {
            done: self.items.iter().filter(|i| i.state == State::Done).count(),
            open: self
                .items
                .iter()
                .filter(|i| matches!(i.state, State::Stuck(_) | State::Skipped(_)))
                .map(|i| format!("{}：{}", i.name(), i.state.label()))
                .collect(),
            stopped,
        };
        desk.emit(Event::Step(report.summary()));
        report
    }

    /// 试调一次，下结论，报给界面。
    fn test(&mut self, index: usize, desk: &mut dyn Desk) -> Verdict {
        let item = &mut self.items[index];
        let args = item.draft.endpoint.example_args();
        let mut trial = desk.trial(&item.draft.endpoint, &args, &self.secrets);
        item.trials += 1;
        item.dirty = false;
        if let Some(raw) = &trial.raw
            && (200..300).contains(&raw.status)
        {
            let body = raw.body.clone();
            let refined = refine_with_reply(&mut item.draft, &body);
            if !refined.is_empty() {
                trial.remap(&item.draft.endpoint);
                item.history
                    .push(format!("按真实返回补了：{}", refined.join("、")));
            }
        }
        let result = verdict(&trial, &item.draft.endpoint);
        let line = match &result {
            Verdict::Ok => format!("试调「{}」：调通了，{}", item.name(), trial.summary()),
            other => format!("试调「{}」：{}", item.name(), other.label()),
        };
        item.history.push(line.clone());
        if result == Verdict::Ok {
            item.state = State::Done;
        } else if item.trials >= MAX_TRIALS {
            item.state = State::Stuck(format!("试了 {MAX_TRIALS} 次没调通：{}", result.label()));
        }
        item.trial = Some(trial);
        item.verdict = Some(result.clone());
        desk.emit(Event::Step(line));
        desk.emit(Event::Item(index, Box::new(item.clone())));
        result
    }

    /// 这一轮要问用户的事。按服务器、按密钥合并，同一件事只问一次。
    fn questions(&self) -> Vec<Question> {
        let mut questions: Vec<Question> = Vec::new();
        let mut push = |question: Question| {
            if !questions.iter().any(|q| q.id == question.id) {
                questions.push(question);
            }
        };
        self.collect(&mut push);
        questions
    }

    fn collect(&self, push: &mut dyn FnMut(Question)) {
        let names_of = |pick: &dyn Fn(&Item) -> bool| -> Vec<String> {
            self.items
                .iter()
                .filter(|i| i.working() && pick(i))
                .map(|i| i.name().to_string())
                .collect()
        };
        for (index, item) in self.items.iter().enumerate() {
            if !item.working() {
                continue;
            }
            let endpoint = &item.draft.endpoint;
            if !item.query() {
                push(Question {
                    id: format!("access:{index}"),
                    text: format!("「{}」只查询、不改数据吗？", item.name()),
                    basis: if item.draft.access_basis.is_empty() {
                        format!("程序判断不了。{} {}", endpoint.method.label(), endpoint.url)
                    } else {
                        item.draft.access_basis.clone()
                    },
                    ask: Ask::Choice {
                        options: vec!["只查询，可以试调".into(), "会改数据，不要加入".into()],
                        other: None,
                    },
                });
                continue;
            }
            if endpoint.url.starts_with('/') {
                let names = names_of(&|i| i.draft.endpoint.url.starts_with('/'));
                push(Question {
                    id: "origin".into(),
                    text: format!("这些接口只写了路径，服务器地址是？（{}）", names.join("、")),
                    basis: "资料里没写服务器地址，或者写了好几个、判断不了用哪个。".into(),
                    ask: Ask::Text {
                        hint: "http://10.0.0.9:8080".into(),
                    },
                });
                continue;
            }
            for name in self.secrets.missing(endpoint) {
                let users = names_of(&|i| i.draft.endpoint.secret_names().contains(&name));
                let how = auth::from_endpoint(endpoint)
                    .filter(|(_, secret)| *secret == name)
                    .map(|(spec, _)| format!("带法：{}。", spec.describe()))
                    .unwrap_or_default();
                push(Question {
                    id: format!("secret:{name}"),
                    text: format!("粘贴密钥「{name}」"),
                    basis: format!(
                        "{how}{} 个接口要用：{}。Key 只存在本机，不发给模型。",
                        users.len(),
                        users.join("、")
                    ),
                    ask: Ask::Secret { name },
                });
            }
            for input in &endpoint.inputs {
                if input.required && input.example.trim().is_empty() {
                    let label = if input.description.trim().is_empty() {
                        input.name.clone()
                    } else {
                        format!("{}（{}）", input.description.trim(), input.name)
                    };
                    push(Question {
                        id: format!("example:{index}:{}", input.name),
                        text: format!("「{}」的「{label}」试调时填什么？", item.name()),
                        basis: "必填参数，资料里没有示例值；填一个能查到数据的真实取值。".into(),
                        ask: Ask::Text {
                            hint: "例如 2025".into(),
                        },
                    });
                }
            }
            match &item.verdict {
                // 试调值不合用（「输入 year 要是数字」）：再问一次。
                Some(Verdict::Blocked(why)) if !item.dirty => {
                    for input in &endpoint.inputs {
                        if why.contains(&format!("输入 {} ", input.name)) {
                            push(Question {
                                id: format!("example:{index}:{}", input.name),
                                text: format!(
                                    "「{}」的「{}」试调时填什么？",
                                    item.name(),
                                    input.name
                                ),
                                basis: format!("上次填的值不合用：{why}"),
                                ask: Ask::Text {
                                    hint: input.kind.label().into(),
                                },
                            });
                        }
                    }
                }
                Some(Verdict::Unreachable(why)) if !item.dirty => {
                    let host = item.host();
                    let names = names_of(&|i| {
                        i.host() == host && matches!(i.verdict, Some(Verdict::Unreachable(_)))
                    });
                    push(Question {
                        id: format!("host:{host}"),
                        text: format!("连不上 {host}：电脑接上内网了吗？地址对不对？"),
                        basis: format!("{why}。涉及：{}", names.join("、")),
                        ask: Ask::Choice {
                            options: vec![
                                "已经连上了，再试一次".into(),
                                "先跳过这个服务器上的接口".into(),
                            ],
                            other: Some("服务器地址不对，改成（如 http://10.0.0.9:8080）".into()),
                        },
                    });
                }
                Some(Verdict::Auth { has_secret: false }) if !item.dirty && !item.auth_doubted => {
                    let host = item.host();
                    let names = names_of(&|i| {
                        i.host() == host
                            && matches!(i.verdict, Some(Verdict::Auth { has_secret: false }))
                    });
                    push(Question {
                        id: format!("auth:{host}"),
                        text: format!(
                            "「{}」拒绝了请求，要鉴权，但文档里没认出带法：密钥放在哪？",
                            names.join("」「")
                        ),
                        basis: "看接口文档的「鉴权 / 认证」一节，或者问接口方。选好后再粘贴 Key。"
                            .into(),
                        ask: Ask::Choice {
                            options: vec![
                                "请求头 Authorization: Bearer 令牌".into(),
                                "请求头 X-API-Key".into(),
                                "地址参数 access_token".into(),
                                "不是鉴权的问题，交给助手按文档再查".into(),
                            ],
                            other: Some("别的请求头名（如 token、appKey）".into()),
                        },
                    });
                }
                Some(Verdict::Auth { has_secret: true }) if !item.dirty && !item.auth_doubted => {
                    for name in endpoint.secret_names() {
                        if self.rekeyed.get(&name).copied().unwrap_or(0) >= MAX_REKEY {
                            continue;
                        }
                        let users = names_of(&|i| {
                            i.draft.endpoint.secret_names().contains(&name)
                                && matches!(i.verdict, Some(Verdict::Auth { .. }))
                        });
                        push(Question {
                            id: format!("rekey:{name}"),
                            text: format!(
                                "「{}」拒绝了请求：密钥「{name}」可能不对或已过期，重新粘贴",
                                users.join("」「")
                            ),
                            basis: "Key 确定没错的话跳过这题，助手会按文档再核对带法（请求头名、要不要 Bearer）。"
                                .into(),
                            ask: Ask::Secret { name },
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// 用回答更新接口；返回有没有进展。
    fn apply(&mut self, questions: &[Question], answers: &Answers, desk: &mut dyn Desk) -> bool {
        let mut progress = false;
        let mut touched: Vec<usize> = Vec::new();
        for question in questions {
            let answer = answers.get(&question.id).unwrap_or(&Answer::Skip);
            let (kind, key) = question.id.split_once(':').unwrap_or((&question.id, ""));
            progress |= *answer != Answer::Skip;
            match kind {
                "access" => {
                    let Ok(index) = key.parse::<usize>() else {
                        continue;
                    };
                    let item = &mut self.items[index];
                    match answer {
                        Answer::Choice(0) => {
                            item.confirmed = true;
                            item.dirty = true;
                        }
                        Answer::Choice(_) => item.state = State::Excluded("你说会改数据".into()),
                        _ => item.state = State::Skipped("没确认只查询".into()),
                    }
                    touched.push(index);
                }
                "origin" => {
                    let origin = answer_text(answer).and_then(|t| origin_of(&t));
                    for (index, item) in self.items.iter_mut().enumerate() {
                        if !item.working() || !item.draft.endpoint.url.starts_with('/') {
                            continue;
                        }
                        match &origin {
                            Some(origin) => {
                                item.draft.endpoint.url =
                                    format!("{origin}{}", item.draft.endpoint.url);
                                item.dirty = true;
                            }
                            None => item.state = State::Skipped("缺服务器地址".into()),
                        }
                        touched.push(index);
                    }
                    if let Some(origin) = origin {
                        self.known.push_str(&format!("\n{origin}"));
                    }
                }
                "secret" | "rekey" => {
                    let name = key.to_string();
                    let value = answer_text(answer);
                    if kind == "rekey" {
                        *self.rekeyed.entry(name.clone()).or_default() += 1;
                    }
                    if let Some(value) = &value {
                        self.secrets.secrets.insert(name.clone(), value.clone());
                    }
                    for (index, item) in self.items.iter_mut().enumerate() {
                        if !item.working() || !item.draft.endpoint.secret_names().contains(&name) {
                            continue;
                        }
                        match (&value, kind) {
                            (Some(_), _) => item.dirty = true,
                            (None, "secret") => item.state = State::Skipped("没给密钥".into()),
                            // Key 没错：交模型按文档核对带法。
                            (None, _) => item.auth_doubted = true,
                        }
                        touched.push(index);
                    }
                }
                "example" => {
                    let Some((index, input)) = key.split_once(':') else {
                        continue;
                    };
                    let Ok(index) = index.parse::<usize>() else {
                        continue;
                    };
                    let item = &mut self.items[index];
                    match answer_text(answer) {
                        Some(value) => {
                            if let Some(slot) = item
                                .draft
                                .endpoint
                                .inputs
                                .iter_mut()
                                .find(|i| i.name == input)
                            {
                                slot.example = value.clone();
                            }
                            self.known.push_str(&format!("\n{value}"));
                            item.dirty = true;
                        }
                        None => item.state = State::Skipped(format!("缺参数「{input}」的试调值")),
                    }
                    touched.push(index);
                }
                "host" => {
                    let host = key.to_string();
                    let new_origin = match answer {
                        Answer::Text(text) => origin_of(text),
                        _ => None,
                    };
                    for (index, item) in self.items.iter_mut().enumerate() {
                        if !item.working() || item.host() != host {
                            continue;
                        }
                        match (answer, &new_origin) {
                            (Answer::Choice(0), _) => item.dirty = true,
                            (Answer::Text(_), Some(origin)) => {
                                let endpoint = &mut item.draft.endpoint;
                                endpoint.url = endpoint.url.replacen(&host, origin, 1);
                                item.dirty = true;
                            }
                            _ => item.state = State::Skipped(format!("连不上 {host}")),
                        }
                        touched.push(index);
                    }
                    if let Some(origin) = new_origin {
                        self.known.push_str(&format!("\n{origin}"));
                    }
                }
                "auth" => {
                    let host = key.to_string();
                    let spec = match answer {
                        Answer::Choice(0) => Some(preset("Authorization", AuthPlace::Header, true)),
                        Answer::Choice(1) => Some(preset("X-API-Key", AuthPlace::Header, false)),
                        Answer::Choice(2) => Some(preset("access_token", AuthPlace::Query, false)),
                        Answer::Text(name) => Some(preset(name.trim(), AuthPlace::Header, false))
                            .filter(AuthSpec::valid_name),
                        _ => None,
                    };
                    for (index, item) in self.items.iter_mut().enumerate() {
                        if !item.working()
                            || item.host() != host
                            || auth::has_auth(&item.draft.endpoint)
                        {
                            continue;
                        }
                        match (&spec, answer) {
                            (Some(spec), _) => {
                                let secret = secret_name(&spec.name);
                                self.secrets.secrets.entry(secret.clone()).or_default();
                                auth::apply(&mut item.draft.endpoint, spec, &secret);
                                item.draft.set_origin("鉴权", super::Origin::User);
                                item.history.push(format!("你说鉴权：{}", spec.describe()));
                                item.dirty = true;
                            }
                            (None, Answer::Choice(3)) => item.auth_doubted = true,
                            _ => item.state = State::Skipped("不知道鉴权怎么带".into()),
                        }
                        touched.push(index);
                    }
                }
                _ => {}
            }
        }
        touched.sort_unstable();
        touched.dedup();
        for index in touched {
            desk.emit(Event::Item(index, Box::new(self.items[index].clone())));
        }
        progress
    }

    /// 要不要交模型修。
    fn needs_fix(&self, index: usize) -> bool {
        let item = &self.items[index];
        if !item.working() || item.dirty || item.fixes >= MAX_FIXES || item.trials >= MAX_TRIALS {
            return false;
        }
        match &item.verdict {
            Some(v) if v.fixable() => true,
            Some(Verdict::Auth { .. }) => item.auth_doubted,
            // 配置有问题、没法组请求的（模板里的变量对不上之类）。
            Some(Verdict::Blocked(_)) => !item.draft.endpoint.problems().is_empty(),
            _ => false,
        }
    }

    /// 交模型修一回；没配模型就给结论。返回配置有没有变。
    fn fix(&mut self, index: usize, desk: &mut dyn Desk) -> bool {
        let Some(model) = self.model else {
            let item = &mut self.items[index];
            let why = item.verdict.as_ref().map_or(String::new(), Verdict::label);
            item.state = State::Stuck(format!("{why}（没配起草模型，要按提示手动改配置）"));
            desk.emit(Event::Item(index, Box::new(item.clone())));
            return false;
        };
        self.items[index].fixes += 1;
        let before = self.items[index].draft.endpoint.clone();
        desk.emit(Event::Step(format!(
            "交给模型对照资料修「{}」",
            self.items[index].name()
        )));
        fix::session(self, index, model, desk);
        let item = &self.items[index];
        desk.emit(Event::Item(index, Box::new(item.clone())));
        item.draft.endpoint != before || item.state != State::Working
    }
}

fn preset(name: &str, place: AuthPlace, bearer: bool) -> AuthSpec {
    AuthSpec {
        place,
        name: name.to_string(),
        bearer,
        basis: "你在接入助手里选的".into(),
        value: String::new(),
    }
}

fn answer_text(answer: &Answer) -> Option<String> {
    match answer {
        Answer::Text(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        _ => None,
    }
}

/// 用户填的服务器地址：要 http(s):// 开头，去掉结尾的斜杠与路径。
fn origin_of(text: &str) -> Option<String> {
    let text = text.trim();
    (text.starts_with("http://") || text.starts_with("https://"))
        .then(|| host_of(text))
        .filter(|origin| origin.len() > "http://".len())
}

/// `http://10.0.0.9:8080/api/x` → `http://10.0.0.9:8080`。
pub(crate) fn host_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host = rest.split(['/', '?']).next().unwrap_or_default();
            format!("{scheme}://{host}")
        }
        None => String::new(),
    }
}
