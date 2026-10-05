//! 从接口文档自动填报数据接口（`docs/http-skill-design.md` 第一层）。
//!
//! 流程：
//! 1. 程序扫描文档（[`scan`]）：cURL、`GET /路径` 请求行、JSON 示例、服务器地址；
//! 2. 程序把认出的请求转成模板（[`build`]），凭据提成本机密钥（[`redact`]）；
//! 3. 配了起草模型时，把**脱敏后**的文档交给模型整理名称、用途、参数说明与只读性质（[`model`]）；
//! 4. 合并：程序认出的结构优先，模型补文字；模型给的地址、参数名必须在资料里找得到，
//!    找不到的不收，并说明；
//! 5. 有返回示例就推断返回映射（[`infer`]）；实测拿到真实返回后再补一次（[`refine_with_reply`]）。
//!
//! 产物只是候选接口，用户核对、勾选后才进 `apis.json`。这里不发任何外部请求。

mod build;
pub(crate) mod infer;
mod model;
pub(crate) mod redact;
mod scan;

use crate::agent::api::{ApiEndpoint, ApiInput, ApiMethod, ApiSecrets, ApiSuccess, InputKind};
use crate::agent::backend::{ModelBackend, ModelRole};
use model::ModelEndpoint;
use redact::Secrets;
use regex::Regex;
use scan::{BlockRole, JsonBlock, RawRequest};
use serde_json::Value;
use std::sync::LazyLock;

static SECRET_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{secret:([A-Za-z_][A-Za-z0-9_]*)\}").expect("密钥占位正则"));

/// 地址里出现这些词，多半是改数据的接口。
const WRITE_WORDS: &[&str] = &[
    "add", "create", "insert", "save", "update", "modify", "edit", "delete", "remove", "del",
    "submit", "approve", "audit", "upload", "import", "send", "push", "reset",
];

/// 字段从哪来，界面上标给人看。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// 程序从文档里直接认出来的。
    Document,
    /// 模型按文档整理的。
    Model,
    /// 实测拿到返回后程序推断的。
    Reply,
}

impl Origin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Document => "文档原文",
            Self::Model => "AI 整理",
            Self::Reply => "实测推断",
        }
    }
}

/// 接口性质。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// 只查询：可以加入，可以自动实测。
    Query,
    /// 会改数据：不允许加入。
    Write,
    /// 判断不了：人确认只查询后才能加入、才实测。
    Unknown,
}

/// 一个候选接口。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Draft {
    pub(crate) endpoint: ApiEndpoint,
    /// (字段, 来源)，按界面展示顺序。
    pub(crate) origins: Vec<(&'static str, Origin)>,
    pub(crate) access: Access,
    pub(crate) access_basis: String,
    /// 不支持的写法、模型指出的缺项、被舍弃的内容。
    pub(crate) notes: Vec<String>,
    /// 文档里的返回示例。
    pub(crate) sample: Option<Value>,
}

impl Draft {
    fn set_origin(&mut self, field: &'static str, origin: Origin) {
        match self.origins.iter_mut().find(|(f, _)| *f == field) {
            Some(entry) => entry.1 = origin,
            None => self.origins.push((field, origin)),
        }
    }

    #[cfg(test)]
    pub(crate) fn origin(&self, field: &str) -> Option<Origin> {
        self.origins
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, origin)| *origin)
    }

    /// 还缺什么才能用。密钥按本机已填的算。
    pub(crate) fn missing(&self, secrets: &ApiSecrets) -> Vec<String> {
        let endpoint = &self.endpoint;
        let mut missing = Vec::new();
        let url = endpoint.url.trim();
        if url.is_empty() {
            missing.push("接口地址".to_string());
        } else if !(url.starts_with("http://") || url.starts_with("https://")) {
            missing.push(format!(
                "服务器地址：资料只写了路径 {url}，要补上 http://主机:端口"
            ));
        }
        for name in secret_refs(endpoint) {
            if secrets
                .secrets
                .get(&name)
                .is_none_or(|value| value.trim().is_empty())
            {
                missing.push(format!("密钥「{name}」的值"));
            }
        }
        if endpoint.description.trim().is_empty() {
            missing.push("说明：这个接口能查什么".into());
        }
        if self.access == Access::Unknown {
            missing.push("确认这个接口只查询、不改数据".into());
        }
        for input in &endpoint.inputs {
            if input.required && input.example.trim().is_empty() {
                missing.push(format!("参数「{}」的样例值（测试要用）", input.name));
            }
        }
        missing
    }
}

/// 接口里引用的密钥名。
pub(crate) fn secret_refs(endpoint: &ApiEndpoint) -> Vec<String> {
    let mut names = Vec::new();
    let mut templates: Vec<&str> = vec![&endpoint.url, &endpoint.body];
    templates.extend(endpoint.headers.iter().map(|h| h.value.as_str()));
    for template in templates {
        for caps in SECRET_REF.captures_iter(template) {
            if !names.contains(&caps[1].to_string()) {
                names.push(caps[1].to_string());
            }
        }
    }
    names
}

/// 读进来的资料：程序扫描结果 + 脱敏后的文字。构造时就把凭据提走了。
#[derive(Debug, Clone)]
pub(crate) struct Material {
    /// 原文（含凭据），只在本机用来核对，不发出去。
    text: String,
    /// 发给模型的文字（已脱敏、已截断）。
    pub(crate) redacted: String,
    /// 原文太长被截了。
    pub(crate) clipped: bool,
    drafts: Vec<Draft>,
    blocks: Vec<JsonBlock>,
    origins: Vec<String>,
    secrets: Secrets,
}

impl Material {
    pub(crate) fn new(text: &str) -> Self {
        let text = text.replace("\r\n", "\n");
        let requests = scan::requests(&text);
        let blocks = scan::json_blocks(&text);
        let origins = scan::origins(&text);
        let mut secrets = Secrets::default();
        let mut drafts: Vec<Draft> = requests
            .iter()
            .enumerate()
            .map(|(index, request)| {
                let next = requests.get(index + 1).map_or(usize::MAX, |r| r.offset);
                draft_from_request(request, next, &text, &blocks, &origins, &mut secrets)
            })
            .collect();
        // 只有一个请求时，文档里任何位置的返回示例都算它的。
        if drafts.len() == 1 && drafts[0].sample.is_none() {
            let sample = blocks
                .iter()
                .find(|b| b.role == BlockRole::Response)
                .map(|b| b.value.clone());
            if let Some(sample) = sample {
                apply_sample(&mut drafts[0], sample);
            }
        }
        // 文档正文里明写的令牌（不在 cURL 里的）也遮掉。
        let redacted = secrets.redact(&text);
        let clipped = redacted.chars().count() > model::MAX_MATERIAL_CHARS;
        let redacted = if clipped {
            redacted.chars().take(model::MAX_MATERIAL_CHARS).collect()
        } else {
            redacted
        };
        Self {
            text,
            redacted,
            clipped,
            drafts,
            blocks,
            origins,
            secrets,
        }
    }

    /// 遮蔽了几处。
    pub(crate) fn masked(&self) -> usize {
        Secrets::count_masks(&self.redacted)
    }

    /// 程序认出的请求，写给模型参考（模板已脱敏）。
    fn findings(&self) -> String {
        if self.drafts.is_empty() {
            return "（没有认出 cURL 或请求行，请从文字说明里整理）".into();
        }
        self.drafts
            .iter()
            .map(|d| {
                let mut line = format!("- {} {}", d.endpoint.method.label(), d.endpoint.url);
                if !d.endpoint.body.is_empty() {
                    line.push_str(&format!("  请求体：{}", compact(&d.endpoint.body)));
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn compact(json: &str) -> String {
    serde_json::from_str::<Value>(json)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| json.to_string())
}

/// 识别结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct Analysis {
    pub(crate) drafts: Vec<Draft>,
    /// 提出来的密钥：名字 → 值（文档里只有占位写法的值为空）。
    pub(crate) secrets: Vec<(String, String)>,
    /// 整体说明：模型没配、模型输出不合格、资料被截断……
    pub(crate) notes: Vec<String>,
    pub(crate) model_used: bool,
}

/// 识别。`model` 为 None 时只做程序解析。`taken_ids` 是已有接口的 id，新 id 避开它们。
pub(crate) fn analyze(
    material: &Material,
    model: Option<&dyn ModelBackend>,
    taken_ids: &[String],
) -> Analysis {
    let mut analysis = Analysis {
        drafts: material.drafts.clone(),
        ..Analysis::default()
    };
    let mut secrets = material.secrets.clone();
    match model {
        None => analysis
            .notes
            .push("没有配置起草模型，只做了程序解析：名称、说明等需要手填。".into()),
        Some(model) => {
            if material.clipped {
                analysis.notes.push(format!(
                    "资料太长，只把前 {} 字交给了模型。",
                    model::MAX_MATERIAL_CHARS
                ));
            }
            let prompt = model::prompt(&material.redacted, &material.findings());
            match model.complete(ModelRole::Draft, model::SYSTEM, &prompt, &mut |_| {}) {
                Ok(reply) => match model::parse(&reply.content) {
                    Some(endpoints) => {
                        analysis.model_used = true;
                        analysis.drafts = merge(
                            analysis.drafts,
                            endpoints,
                            material,
                            &mut secrets,
                            &mut analysis.notes,
                        );
                    }
                    None => analysis.notes.push(
                        "模型没有给出可用的整理结果，只用了程序解析；可以再识别一次。".into(),
                    ),
                },
                Err(error) => analysis
                    .notes
                    .push(format!("调用模型失败（{error:#}），只用了程序解析。")),
            }
        }
    }
    if analysis.drafts.is_empty() {
        analysis.notes.push(
            "没有认出接口。资料里最好有 cURL 命令、「GET /路径」这样的请求行或完整的接口地址。"
                .into(),
        );
    }
    // 只有文字说明、由模型整理出的单个接口：文档里的返回示例也算它的。
    if let [draft] = analysis.drafts.as_mut_slice()
        && draft.sample.is_none()
        && let Some(block) = material
            .blocks
            .iter()
            .find(|b| b.role == BlockRole::Response)
    {
        apply_sample(draft, block.value.clone());
    }
    let mut taken: Vec<String> = taken_ids.to_vec();
    for draft in &mut analysis.drafts {
        finish(draft);
        draft.endpoint.id = unique_id(&draft.endpoint.id, &taken);
        taken.push(draft.endpoint.id.clone());
    }
    analysis.secrets = secrets.found;
    analysis
}

/// 程序认出的一个请求 → 候选接口。
fn draft_from_request(
    request: &RawRequest,
    next_offset: usize,
    text: &str,
    blocks: &[JsonBlock],
    origins: &[String],
    secrets: &mut Secrets,
) -> Draft {
    let mut inputs = Vec::new();
    let mut notes = request.notes.clone();
    let url = join_origin(&request.url, origins);
    let url = build::template_url(&url, &mut inputs, secrets);
    let headers = build::template_headers(&request.headers, secrets);
    let in_range = |b: &&JsonBlock| b.offset > request.offset && b.offset < next_offset;
    let body_source = request.body.clone().or_else(|| {
        blocks
            .iter()
            .filter(in_range)
            .find(|b| b.role == BlockRole::Request)
            .map(|b| b.value.to_string())
    });
    let mut body = String::new();
    if let Some(source) = body_source {
        match serde_json::from_str::<Value>(&source) {
            Ok(value @ Value::Object(_)) => {
                let templated = build::template_body(&value, &mut inputs, secrets, &mut notes);
                body = serde_json::to_string_pretty(&templated).unwrap_or_default();
            }
            _ if build::looks_like_form(&source) => notes.push(
                "请求体是表单格式（a=1&b=2），暂只支持 JSON 请求体，需要接口方确认能否收 JSON"
                    .into(),
            ),
            _ => notes.push("请求体不是 JSON 对象，没有采用".into()),
        }
    }
    let write = request.method.is_none() || scan::write_method_near(text, request.offset);
    let mut draft = Draft {
        endpoint: ApiEndpoint {
            id: build::id_from_path(&url),
            name: String::new(),
            method: request.method.unwrap_or(ApiMethod::Get),
            url,
            inputs,
            headers,
            body,
            ..ApiEndpoint::default()
        },
        origins: vec![("地址", Origin::Document), ("方法", Origin::Document)],
        access: if write {
            Access::Write
        } else {
            Access::Unknown
        },
        access_basis: if write {
            "文档里写的是 PUT / DELETE / PATCH 一类改数据的方法".into()
        } else {
            String::new()
        },
        notes,
        sample: None,
    };
    if !draft.endpoint.inputs.is_empty() {
        draft.set_origin("参数", Origin::Document);
    }
    if !draft.endpoint.body.is_empty() {
        draft.set_origin("请求体", Origin::Document);
    }
    let sample = blocks
        .iter()
        .filter(in_range)
        .find(|b| b.role != BlockRole::Request)
        .map(|b| b.value.clone());
    if let Some(sample) = sample {
        apply_sample(&mut draft, sample);
    }
    draft
}

/// 用文档里的返回示例补返回映射。
fn apply_sample(draft: &mut Draft, sample: Value) {
    let inferred = infer::infer(&sample);
    let endpoint = &mut draft.endpoint;
    if !infer::fill_mapping(&mut endpoint.mapping, &mut endpoint.success, &inferred).is_empty() {
        draft.set_origin("返回映射", Origin::Document);
    }
    draft.sample = Some(sample);
}

/// 相对路径配上文档里唯一的服务器地址；有好几个地址时不猜。
fn join_origin(url: &str, origins: &[String]) -> String {
    if url.starts_with('/') && origins.len() == 1 {
        format!("{}{url}", origins[0])
    } else {
        url.to_string()
    }
}

/// 比较用的路径：去掉服务器、查询串，变量统一写成 `{}`，小写。
fn path_key(url: &str) -> String {
    let path = url
        .split_once("://")
        .map_or(url, |(_, rest)| rest.find('/').map_or("", |i| &rest[i..]));
    let path = path.split('?').next().unwrap_or_default();
    static VAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{[^}]*\}|:[A-Za-z_]\w*").expect("变量正则"));
    VAR.replace_all(path, "{}")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// 资料里提到过这个词没有（不分大小写）。
fn mentioned(text: &str, word: &str) -> bool {
    let word = word.trim();
    !word.is_empty() && text.to_lowercase().contains(&word.to_lowercase())
}

/// 合并程序与模型的结果。
fn merge(
    mut drafts: Vec<Draft>,
    endpoints: Vec<ModelEndpoint>,
    material: &Material,
    secrets: &mut Secrets,
    notes: &mut Vec<String>,
) -> Vec<Draft> {
    for found in endpoints {
        let key = path_key(&found.url);
        match drafts
            .iter_mut()
            .find(|d| !key.is_empty() && path_key(&d.endpoint.url) == key)
        {
            Some(draft) => enrich(draft, &found, &material.text),
            None => match draft_from_model(&found, material, secrets) {
                Ok(draft) => drafts.push(draft),
                Err(reason) => notes.push(reason),
            },
        }
    }
    drafts
}

/// 程序认出的接口，用模型的整理补文字说明与性质。
fn enrich(draft: &mut Draft, found: &ModelEndpoint, text: &str) {
    if !found.name.trim().is_empty() {
        draft.endpoint.name = found.name.trim().to_string();
        draft.set_origin("名称", Origin::Model);
    }
    if !found.description.trim().is_empty() {
        draft.endpoint.description = found.description.trim().to_string();
        draft.set_origin("说明", Origin::Model);
    }
    let mut described = false;
    for input in &mut draft.endpoint.inputs {
        if let Some(theirs) = found
            .inputs
            .iter()
            .find(|i| build::ident(&i.name) == input.name)
        {
            if !theirs.description.trim().is_empty() {
                input.description = theirs.description.trim().to_string();
                described = true;
            }
            input.required = theirs.required;
            // 类型只在模型说是数字 / 真假、且样例对得上时改。
            match kind_of(&theirs.kind) {
                InputKind::Number
                    if input.example.trim().parse::<f64>().is_ok() || input.example.is_empty() =>
                {
                    input.kind = InputKind::Number;
                }
                InputKind::Bool if matches!(input.example.trim(), "" | "true" | "false") => {
                    input.kind = InputKind::Bool;
                }
                _ => {}
            }
            if input.example.trim().is_empty() && mentioned(text, &theirs.example) {
                input.example = theirs.example.trim().to_string();
            }
        }
    }
    if described {
        draft.set_origin("参数说明", Origin::Model);
    }
    apply_model_response(draft, found, text);
    apply_access(draft, found);
    draft.notes.extend(
        found
            .missing
            .iter()
            .filter(|m| !m.trim().is_empty())
            .map(|m| format!("AI 指出缺：{}", m.trim())),
    );
}

/// 返回列表与成功判据：模型说的字段名要在资料里出现过，有返回示例时还要在示例里取得到。
fn apply_model_response(draft: &mut Draft, found: &ModelEndpoint, text: &str) {
    let list = found.list.trim();
    if draft.endpoint.mapping.list.is_empty() && list.starts_with('/') && list.len() > 1 {
        let ok = match &draft.sample {
            Some(sample) => sample.pointer(list).is_some_and(Value::is_array),
            None => list
                .split('/')
                .filter(|s| !s.is_empty())
                .all(|s| mentioned(text, s)),
        };
        if ok {
            draft.endpoint.mapping.list = list.to_string();
            draft.set_origin("返回映射", Origin::Model);
        }
    }
    let pointer = found.success_pointer.trim();
    if !draft.endpoint.success.is_set()
        && pointer.starts_with('/')
        && !found.success_equals.trim().is_empty()
    {
        let ok = match &draft.sample {
            Some(sample) => sample.pointer(pointer).is_some(),
            None => mentioned(text, pointer.trim_start_matches('/')),
        };
        if ok {
            draft.endpoint.success = ApiSuccess {
                pointer: pointer.to_string(),
                equals: found.success_equals.trim().to_string(),
            };
            draft.set_origin("成功判据", Origin::Model);
        }
    }
}

/// 性质：程序认定改数据的不翻案；模型说只查询，但地址里有改数据的字样，要人确认。
fn apply_access(draft: &mut Draft, found: &ModelEndpoint) {
    if draft.access == Access::Write {
        return;
    }
    let basis = found.access_basis.trim().to_string();
    let path = path_key(&draft.endpoint.url);
    let write_word = path
        .split(['/', '_', '-', '.'])
        .find(|seg| WRITE_WORDS.iter().any(|w| seg.starts_with(w)))
        .map(str::to_string);
    let method = found.method.trim().to_ascii_uppercase();
    (draft.access, draft.access_basis) = match found.access.trim() {
        "write" => (Access::Write, basis),
        _ if matches!(method.as_str(), "PUT" | "DELETE" | "PATCH") => {
            (Access::Write, format!("资料里的请求方法是 {method}"))
        }
        "query" => match write_word {
            Some(word) => (
                Access::Unknown,
                format!("AI 判断只查询，但地址里有「{word}」字样，请确认"),
            ),
            None => (Access::Query, basis),
        },
        _ => (Access::Unknown, basis),
    };
}

fn kind_of(kind: &str) -> InputKind {
    match kind.trim().to_ascii_lowercase().as_str() {
        "number" | "int" | "integer" | "float" | "数字" => InputKind::Number,
        "bool" | "boolean" | "是否" => InputKind::Bool,
        _ => InputKind::Text,
    }
}

/// 只有文字说明、程序没认出请求的接口：按模型的整理建，逐项核对资料。
fn draft_from_model(
    found: &ModelEndpoint,
    material: &Material,
    secrets: &mut Secrets,
) -> Result<Draft, String> {
    let text = &material.text;
    let label = if found.name.trim().is_empty() {
        found.url.trim().to_string()
    } else {
        found.name.trim().to_string()
    };
    // 地址：路径（去掉变量以后的前缀）必须在资料里出现过。
    let raw_url = found.url.trim();
    let path = path_key(raw_url);
    let anchor = path
        .split("{}")
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    if raw_url.is_empty() || anchor.is_empty() || !mentioned(text, anchor) {
        return Err(format!(
            "AI 整理出的接口「{label}」在资料里找不到对应的地址，没有采用。"
        ));
    }
    let mut notes = Vec::new();
    let mut inputs: Vec<ApiInput> = Vec::new();
    let url = join_origin(raw_url, &material.origins);
    let mut url = build::template_url(&url, &mut inputs, secrets);
    let method = match found.method.trim().to_ascii_uppercase().as_str() {
        "POST" => ApiMethod::Post,
        _ => ApiMethod::Get,
    };
    let mut body = String::new();
    if method == ApiMethod::Post {
        let object = found.body_object().unwrap_or_else(|| {
            Value::Object(
                found
                    .inputs
                    .iter()
                    .map(|i| {
                        (
                            i.name.clone(),
                            Value::String(format!("{{{}}}", build::ident(&i.name))),
                        )
                    })
                    .collect(),
            )
        });
        let templated = build::template_body(&object, &mut inputs, secrets, &mut notes);
        body = serde_json::to_string_pretty(&templated).unwrap_or_default();
    }
    // 参数：名字要在资料里出现过；补上说明、类型、样例。
    let mut dropped = Vec::new();
    for theirs in &found.inputs {
        let name = build::ident(&theirs.name);
        if !mentioned(text, &theirs.name) {
            dropped.push(theirs.name.clone());
            continue;
        }
        let input = match inputs.iter_mut().find(|i| i.name == name) {
            Some(input) => input,
            None if method == ApiMethod::Get => {
                // 查询参数没写进地址：补到查询串上。
                let joiner = if url.contains('?') { '&' } else { '?' };
                url.push_str(&format!("{joiner}{}={{{name}}}", theirs.name.trim()));
                inputs.push(ApiInput {
                    name: name.clone(),
                    ..ApiInput::default()
                });
                inputs.last_mut().expect("刚加的")
            }
            None => continue,
        };
        input.description = theirs.description.trim().to_string();
        input.required = theirs.required;
        input.kind = kind_of(&theirs.kind);
        if mentioned(text, &theirs.example) {
            input.example = theirs.example.trim().to_string();
        }
    }
    // 模板里用了、但资料里没提过的变量也去掉，免得带着猜出来的参数名。
    inputs.retain(|input| {
        let keep = mentioned(text, &input.name);
        if !keep && !dropped.contains(&input.name) {
            dropped.push(input.name.clone());
        }
        keep
    });
    if !dropped.is_empty() {
        notes.push(format!(
            "AI 给的参数 {} 在资料里找不到，已去掉",
            dropped.join("、")
        ));
    }
    let mut draft = Draft {
        endpoint: ApiEndpoint {
            id: build::id_from_path(&url),
            name: found.name.trim().to_string(),
            description: found.description.trim().to_string(),
            method,
            url,
            inputs,
            body,
            ..ApiEndpoint::default()
        },
        origins: vec![
            ("地址", Origin::Model),
            ("方法", Origin::Model),
            ("名称", Origin::Model),
            ("说明", Origin::Model),
            ("参数", Origin::Model),
        ],
        access: Access::Unknown,
        access_basis: String::new(),
        notes,
        sample: None,
    };
    // 去掉了参数以后模板里可能还留着它的占位：程序检查会报出来，交给人改。
    apply_model_response(&mut draft, found, text);
    apply_access(&mut draft, found);
    draft.notes.extend(
        found
            .missing
            .iter()
            .filter(|m| !m.trim().is_empty())
            .map(|m| format!("AI 指出缺：{}", m.trim())),
    );
    Ok(draft)
}

/// 收尾：没名字的按路径起名，单个接口时把文档里唯一的返回示例挂上。
fn finish(draft: &mut Draft) {
    if draft.endpoint.name.trim().is_empty() {
        draft.endpoint.name = format!("接口 {}", draft.endpoint.id);
    }
    draft.notes.dedup();
}

/// 新 id 避开已有的：重名时加 `_2`、`_3`。
fn unique_id(id: &str, taken: &[String]) -> String {
    let base = if id.is_empty() { "api" } else { id };
    let mut candidate = base.to_string();
    let mut n = 2;
    while taken.contains(&candidate) {
        candidate = format!("{base}_{n}");
        n += 1;
    }
    candidate
}

/// 实测拿到真实返回后，用它补返回映射与成功判据（只补空着的项）。返回补了哪些项。
pub(crate) fn refine_with_reply(draft: &mut Draft, body: &str) -> Vec<&'static str> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let inferred = infer::infer(&root);
    let endpoint = &mut draft.endpoint;
    let filled = infer::fill_mapping(&mut endpoint.mapping, &mut endpoint.success, &inferred);
    if filled.iter().any(|f| *f != "成功判据") {
        draft.set_origin("返回映射", Origin::Reply);
    }
    if filled.contains(&"成功判据") {
        draft.set_origin("成功判据", Origin::Reply);
    }
    filled
}

/// 把识别出的密钥并进本机密钥表：同名不同值的改名，返回改名表 (旧名, 新名)。
pub(crate) fn merge_secrets(
    store: &mut ApiSecrets,
    found: &[(String, String)],
) -> Vec<(String, String)> {
    let mut renamed = Vec::new();
    for (name, value) in found {
        match store.secrets.get(name) {
            None => {
                store.secrets.insert(name.clone(), value.clone());
            }
            Some(existing) if existing == value || value.is_empty() => {}
            Some(existing) if existing.is_empty() => {
                store.secrets.insert(name.clone(), value.clone());
            }
            Some(_) => {
                let mut n = 2;
                let mut new_name = format!("{name}{n}");
                while store.secrets.contains_key(&new_name) {
                    n += 1;
                    new_name = format!("{name}{n}");
                }
                store.secrets.insert(new_name.clone(), value.clone());
                renamed.push((name.clone(), new_name));
            }
        }
    }
    renamed
}

/// 密钥改了名，接口模板里的引用跟着改。
pub(crate) fn rename_secret_refs(endpoint: &mut ApiEndpoint, renamed: &[(String, String)]) {
    for (old, new) in renamed {
        let from = format!("{{secret:{old}}}");
        let to = format!("{{secret:{new}}}");
        endpoint.url = endpoint.url.replace(&from, &to);
        endpoint.body = endpoint.body.replace(&from, &to);
        for header in &mut endpoint.headers {
            header.value = header.value.replace(&from, &to);
        }
    }
}

#[cfg(test)]
mod tests;
