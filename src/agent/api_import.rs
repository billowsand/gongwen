//! 从接口文档自动填报数据接口（`docs/http-skill-design.md` 第一层）。
//!
//! 流程：
//! 1. 程序扫描文档：cURL、`GET /路径` 请求行、JSON 示例、服务器地址（[`scan`]）；按节写的
//!    文档里「请求地址：」「请求方式：」字段行、键值表、参数表与接口总览表（[`sections`]）；
//!    同一个地址在几处出现的合成一个；
//! 2. 程序把认出的请求转成模板（[`build`]），凭据提成本机密钥（[`redact`]）；文档里单独写的
//!    鉴权方式套到没带鉴权的接口上（[`auth`]）；
//! 3. 配了起草模型时，把**脱敏后**的文档交给模型整理名称、用途、参数说明与只读性质（[`model`]）；
//! 4. 合并：程序认出的结构优先，模型补文字；模型给的地址、参数名必须在资料里找得到，
//!    找不到的不收，并说明；
//! 5. 有返回示例就推断返回映射（[`infer`]）；实测拿到真实返回后再补一次（[`refine_with_reply`]）。
//!
//! 产物只是候选接口，用户核对、勾选后才进 `apis.json`。这里不发任何外部请求。

pub(crate) mod auth;
mod build;
mod chunk;
pub(crate) mod examples;
pub(crate) mod infer;
mod model;
pub(crate) mod onboard;
pub(crate) mod redact;
mod scan;
mod sections;

use crate::agent::api::{ApiEndpoint, ApiInput, ApiMethod, ApiSecrets, ApiSuccess, InputKind};
use crate::agent::backend::{ModelBackend, ModelRole};
use auth::AuthSpec;
use model::ModelEndpoint;
use redact::Secrets;
use regex::Regex;
use scan::{BlockRole, DocParam, JsonBlock, ParamPlace, RawRequest};
use serde_json::{Value, json};
use std::sync::LazyLock;

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
    /// 用户在接入助手里补的。
    User,
}

impl Origin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Document => "文档原文",
            Self::Model => "AI 整理",
            Self::Reply => "实测推断",
            Self::User => "你补充的",
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
                missing.push(format!("密钥「{name}」（在上方「鉴权」里粘贴）"));
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
    endpoint.secret_names()
}

/// 读进来的资料：程序扫描结果 + 脱敏后的文字。构造时就把凭据提走了。
#[derive(Debug, Clone)]
pub(crate) struct Material {
    /// 原文（含凭据），只在本机用来核对，不发出去。
    text: String,
    /// 发给模型的文字（已脱敏）。太长的分段发。
    pub(crate) redacted: String,
    drafts: Vec<Draft>,
    blocks: Vec<JsonBlock>,
    origins: Vec<String>,
    secrets: Secrets,
    /// 文档里单独写的鉴权方式。
    auth: Option<AuthSpec>,
    /// 程序做不了的鉴权（签名、先登录换令牌）。
    auth_notes: Vec<String>,
}

impl Material {
    pub(crate) fn new(text: &str) -> Self {
        let text = text.replace("\r\n", "\n");
        let mut found = scan::requests(&text);
        let (in_sections, loose) = sections::parse(&text);
        found.extend(in_sections);
        found.sort_by_key(|r| r.offset);
        let requests = merge_requests(found);
        let blocks = scan::json_blocks(&text);
        let origins = scan::origins(&text);
        let mut secrets = Secrets::default();
        let mut drafts: Vec<Draft> = requests
            .iter()
            .enumerate()
            .map(|(index, request)| {
                let next = requests.get(index + 1).map_or(usize::MAX, |r| r.offset);
                draft_from_request(
                    request,
                    next,
                    &text,
                    &blocks,
                    &loose,
                    &origins,
                    &mut secrets,
                )
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
        let (auth, auth_notes) = auth::detect(&text);
        if let Some(spec) = &auth
            && !spec.value.is_empty()
        {
            // 文档里写了真值：先登记，好遮掉。
            secrets.add(&spec.name, &spec.value);
        }
        // 文档正文里明写的令牌（不在 cURL 里的）也遮掉。
        let redacted = secrets.redact(&text);
        Self {
            text,
            redacted,
            drafts,
            blocks,
            origins,
            secrets,
            auth,
            auth_notes,
        }
    }

    /// 遮蔽了几处。
    pub(crate) fn masked(&self) -> usize {
        Secrets::count_masks(&self.redacted)
    }

    /// 程序认出的请求里、路径在 `piece` 里出现过的，写给模型参考（模板已脱敏）。
    fn findings_in(drafts: &[Draft], piece: &str) -> String {
        let piece = piece.to_lowercase();
        let lines: Vec<String> = drafts
            .iter()
            .filter(|d| {
                let path = path_key(&d.endpoint.url);
                let anchor = path.split("{}").next().unwrap_or_default();
                !anchor.trim_matches('/').is_empty() && piece.contains(anchor)
            })
            .map(|d| {
                let mut line = format!("- {} {}", d.endpoint.method.label(), d.endpoint.url);
                if !d.endpoint.name.is_empty() {
                    line.push_str(&format!("（{}）", d.endpoint.name));
                }
                if !d.endpoint.body.is_empty() {
                    line.push_str(&format!("  请求体：{}", compact(&d.endpoint.body)));
                }
                line
            })
            .collect();
        if lines.is_empty() {
            "（这一段程序没认出 cURL 或请求行，请从文字说明里整理）".into()
        } else {
            lines.join("\n")
        }
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
    let mut model_auth = None;
    match model {
        None => analysis
            .notes
            .push("没有配置起草模型，只做了程序解析：名称、说明等需要手填。".into()),
        Some(model) => {
            let chunks = chunk::split(&material.redacted, model::MAX_MATERIAL_CHARS);
            if chunks.len() > 1 {
                analysis
                    .notes
                    .push(format!("资料较长，分 {} 段交给模型整理。", chunks.len()));
            }
            let preamble = chunk::preamble(&material.redacted, 1500);
            for (index, piece) in chunks.iter().enumerate() {
                if model.cancelled() {
                    break;
                }
                let text = if index == 0 {
                    piece.clone()
                } else {
                    format!("【文档开头（公共说明）】\n{preamble}\n\n【本段】\n{piece}")
                };
                let findings = Material::findings_in(&analysis.drafts, piece);
                let label = if chunks.len() > 1 {
                    format!("第 {} 段：", index + 1)
                } else {
                    String::new()
                };
                ask_model(
                    model,
                    &text,
                    &findings,
                    &label,
                    material,
                    &mut analysis,
                    &mut secrets,
                    &mut model_auth,
                );
            }
            // 查漏：文档里出现过、没整理成接口的路径，交模型再看一遍。
            let missing = chunk::uncovered(&material.text, &analysis.drafts);
            if !missing.is_empty() && !model.cancelled() {
                let excerpts: Vec<String> = missing
                    .iter()
                    .filter_map(|path| chunk::around(&material.redacted, path, 15, 4000, 1))
                    .collect();
                let mut text = excerpts.join("\n……\n");
                if text.chars().count() > model::MAX_MATERIAL_CHARS {
                    text = text.chars().take(model::MAX_MATERIAL_CHARS).collect();
                }
                let findings = format!(
                    "（程序在资料里还看到这些地址，没整理成接口：{}。请判断哪些是接口并按格式输出；只是说明里顺带提到、不是接口的不要输出）",
                    missing.join("、")
                );
                ask_model(
                    model,
                    &format!("【文档开头（公共说明）】\n{preamble}\n\n【相关摘录】\n{text}"),
                    &findings,
                    "查漏：",
                    material,
                    &mut analysis,
                    &mut secrets,
                    &mut model_auth,
                );
            }
        }
    }
    let missing = chunk::uncovered(&material.text, &analysis.drafts);
    if !missing.is_empty() {
        let shown: Vec<&str> = missing.iter().take(12).map(String::as_str).collect();
        analysis.notes.push(format!(
            "文档里还有 {} 个地址没整理成接口：{}{}。可能只是说明里提到的地址；是接口的话可以手动添加。",
            missing.len(),
            shown.join("、"),
            if missing.len() > shown.len() { "……" } else { "" }
        ));
    }
    if analysis.drafts.is_empty() {
        analysis.notes.push(
            "没有认出接口。资料里最好有 cURL 命令、「GET /路径」这样的请求行、「请求地址：」字段或接口地址表格。"
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
    if let Some(note) = finish_auth(
        &mut analysis.drafts,
        material.auth.as_ref(),
        model_auth,
        &mut secrets,
    ) {
        analysis.notes.push(note);
    }
    analysis.notes.extend(material.auth_notes.iter().cloned());
    let mut taken: Vec<String> = taken_ids.to_vec();
    for draft in &mut analysis.drafts {
        finish(draft);
        draft.endpoint.id = unique_id(&draft.endpoint.id, &taken);
        taken.push(draft.endpoint.id.clone());
    }
    analysis.secrets = secrets.found;
    analysis
}

/// 交模型整理一段资料，结果并进 `analysis`。
#[allow(clippy::too_many_arguments)]
fn ask_model(
    model: &dyn ModelBackend,
    text: &str,
    findings: &str,
    label: &str,
    material: &Material,
    analysis: &mut Analysis,
    secrets: &mut Secrets,
    model_auth: &mut Option<AuthSpec>,
) {
    let prompt = model::prompt(text, findings);
    match model.complete(ModelRole::Draft, model::SYSTEM, &prompt, &mut |_| {}) {
        Ok(reply) => match model::parse(&reply.content) {
            Some(parsed) => {
                analysis.model_used = true;
                analysis.drafts = merge(
                    std::mem::take(&mut analysis.drafts),
                    parsed.endpoints,
                    material,
                    secrets,
                    &mut analysis.notes,
                );
                if model_auth.is_none() {
                    *model_auth = parsed
                        .auth
                        .and_then(|auth| model_auth_spec(auth, &material.text));
                }
            }
            None if reply.truncated => analysis.notes.push(format!(
                "{label}模型的整理太长被截断了，这部分只用了程序解析。"
            )),
            None => analysis.notes.push(format!(
                "{label}模型没有给出可用的整理结果，这部分只用了程序解析；可以再识别一次。"
            )),
        },
        Err(error) => analysis.notes.push(format!(
            "{label}调用模型失败（{error:#}），这部分只用了程序解析。"
        )),
    }
}

/// 模型说的鉴权方式：字段名要像样、要在资料里出现过。值一律不收。
fn model_auth_spec(auth: model::ModelAuth, text: &str) -> Option<AuthSpec> {
    let place = match auth.place.trim().to_ascii_lowercase().as_str() {
        "query" | "url" | "参数" => auth::AuthPlace::Query,
        _ => auth::AuthPlace::Header,
    };
    let spec = AuthSpec {
        place,
        name: auth.name.trim().to_string(),
        bearer: place == auth::AuthPlace::Header && auth.scheme.to_lowercase().contains("bearer"),
        basis: format!("AI 按资料整理：{}", auth.basis.trim()),
        value: String::new(),
    };
    (spec.valid_name() && mentioned(text, &spec.name)).then_some(spec)
}

/// 鉴权收尾：文档写了鉴权方式的（程序认出的优先，其次模型整理的），没有就照同一份文档里
/// 已经带了鉴权的接口，给还没带的只查询接口补上。返回一句整体说明。
fn finish_auth(
    drafts: &mut [Draft],
    program: Option<&AuthSpec>,
    model: Option<AuthSpec>,
    secrets: &mut Secrets,
) -> Option<String> {
    let existing = drafts.iter().find_map(|d| auth::from_endpoint(&d.endpoint));
    let (spec, origin) = match (program, model) {
        (Some(spec), _) => (spec.clone(), Origin::Document),
        (None, Some(spec)) => (spec, Origin::Model),
        (None, None) => {
            let (spec, _) = existing.clone()?;
            (spec, Origin::Document)
        }
    };
    // 同名的鉴权已经有接口带了：用同一个密钥，免得同一个 Key 要填两遍。
    let secret = match &existing {
        Some((theirs, secret))
            if theirs.place == spec.place && theirs.name.eq_ignore_ascii_case(&spec.name) =>
        {
            secret.clone()
        }
        _ => secrets.add(&spec.name, &spec.value),
    };
    let mut count = 0;
    for draft in drafts.iter_mut() {
        if draft.access == Access::Write || auth::has_auth(&draft.endpoint) {
            continue;
        }
        auth::apply(&mut draft.endpoint, &spec, &secret);
        draft.set_origin("鉴权", origin);
        count += 1;
    }
    (count > 0).then(|| {
        format!(
            "鉴权：{}（依据：{}），已给 {count} 个接口补上。",
            spec.describe(),
            spec.basis
        )
    })
}

/// 同一个地址在文档里出现几次（总览表一行、详细一节、一条 cURL）：合成一个。
/// 结构以 cURL 或详细的一节为准，名称、参数说明互相补齐。方法明写了又不一样的不合。
fn merge_requests(found: Vec<RawRequest>) -> Vec<RawRequest> {
    let mut out: Vec<RawRequest> = Vec::new();
    for request in found {
        let key = path_key(&request.url);
        let same = out.iter().position(|r| {
            !key.is_empty()
                && path_key(&r.url) == key
                && (r.method == request.method || r.method_guessed || request.method_guessed)
        });
        match same {
            Some(index) => {
                let existing = std::mem::take(&mut out[index]);
                out[index] = absorb(existing, request);
            }
            None => out.push(request),
        }
    }
    out.sort_by_key(|r| r.offset);
    out
}

fn absorb(a: RawRequest, b: RawRequest) -> RawRequest {
    let rich = |r: &RawRequest| !r.headers.is_empty() || r.body.is_some();
    let b_first = (a.overview && !b.overview) || (!rich(&a) && rich(&b));
    let (mut primary, other) = if b_first { (b, a) } else { (a, b) };
    // 名称：详细一节的标题优先于总览表里的写法。
    if primary.name.is_none() || (primary.overview && !other.overview && other.name.is_some()) {
        primary.name = other.name.clone().or(primary.name.take());
    }
    if !primary.url.starts_with("http")
        && other.url.starts_with("http")
        && let Some(origin) = scan::origins(&other.url).first()
    {
        primary.url = format!("{origin}{}", primary.url);
    }
    if primary.method_guessed && !other.method_guessed {
        primary.method = other.method;
        primary.method_guessed = false;
        primary.notes.retain(|n| !n.starts_with("文档没写请求方式"));
    } else if other.method.is_none() && !other.method_guessed {
        // 别处明写了改数据的方法：从严。
        primary.method = None;
    }
    for param in other.params {
        match primary.params.iter_mut().find(|p| p.name == param.name) {
            Some(mine) => {
                if mine.description.is_empty() {
                    mine.description = param.description;
                }
                if mine.example.is_empty() {
                    mine.example = param.example;
                }
            }
            None => primary.params.push(param),
        }
    }
    for header in other.headers {
        if !primary
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(&header.0))
        {
            primary.headers.push(header);
        }
    }
    if primary.body.is_none() {
        primary.body = other.body;
    }
    if !other.overview {
        primary.offset = if primary.overview {
            other.offset
        } else {
            primary.offset.min(other.offset)
        };
    }
    primary.overview &= other.overview;
    for note in other.notes {
        if !primary.notes.contains(&note) {
            primary.notes.push(note);
        }
    }
    primary
}

/// 程序认出的一个请求 → 候选接口。
#[allow(clippy::too_many_arguments)]
fn draft_from_request(
    request: &RawRequest,
    next_offset: usize,
    text: &str,
    blocks: &[JsonBlock],
    loose: &[(usize, DocParam)],
    origins: &[String],
    secrets: &mut Secrets,
) -> Draft {
    let mut inputs = Vec::new();
    let mut notes = request.notes.clone();
    let method = request.method.unwrap_or(ApiMethod::Get);
    // 总览表里的一行后面跟的是下一行，不是它的示例。
    let in_range =
        |b: &&JsonBlock| !request.overview && b.offset > request.offset && b.offset < next_offset;
    let requests: Vec<&JsonBlock> = blocks
        .iter()
        .filter(in_range)
        .filter(|b| b.role == BlockRole::Request)
        .collect();
    // 「请求体」单独成节的文档：那一节的参数挂到这个接口上。有请求体示例时只收示例的顶层字段
    // （下面「问题类型」一类小节里的是对象里面的字段，不是请求体的）。
    let example_keys: Option<Vec<String>> = requests
        .first()
        .and_then(|b| b.value.as_object())
        .map(|map| map.keys().cloned().collect());
    let mut params = request.params.clone();
    for (at, param) in loose {
        let in_scope = *at > request.offset && *at < next_offset && !request.overview;
        let wanted = match &example_keys {
            Some(keys) => keys.contains(&param.name),
            None => request.params.is_empty(),
        };
        if in_scope && wanted && !params.iter().any(|p| p.name == param.name) {
            params.push(param.clone());
        }
    }
    let params = &params;
    // 参数表：放地址上的补进查询串（模板化时登记成输入），POST 没有请求体示例时按参数表拼一个。
    let in_query = |p: &&DocParam| {
        p.place == ParamPlace::Query || (p.place == ParamPlace::Unknown && method == ApiMethod::Get)
    };
    let json_keys: Vec<String> = params
        .iter()
        .filter(|p| p.kind == InputKind::Json)
        .map(|p| p.name.clone())
        .collect();
    let mut url = join_origin(&request.url, origins);
    for param in params.iter().filter(in_query) {
        if !query_has(&url, &param.name) {
            let example = if param.example.contains(['&', '=', '#', ' ']) {
                ""
            } else {
                param.example.as_str()
            };
            let joiner = if url.contains('?') { '&' } else { '?' };
            url.push_str(&format!("{joiner}{}={example}", param.name));
        }
    }
    let url = build::template_url(&url, &mut inputs, secrets);
    let headers = build::template_headers(&request.headers, secrets);
    let body_source = request
        .body
        .clone()
        .or_else(|| requests.first().map(|b| b.value.to_string()))
        .or_else(|| {
            let fields: serde_json::Map<String, Value> = params
                .iter()
                .filter(|p| {
                    method == ApiMethod::Post
                        && matches!(p.place, ParamPlace::Body | ParamPlace::Unknown)
                })
                .map(|p| (p.name.clone(), typed_example(p)))
                .collect();
            (!fields.is_empty()).then(|| Value::Object(fields).to_string())
        });
    let mut body = String::new();
    let mut doc_examples = Vec::new();
    if let Some(source) = body_source {
        match serde_json::from_str::<Value>(&source) {
            Ok(value @ Value::Object(_)) => {
                let templated = build::template_body(&value, &mut inputs, secrets, &json_keys);
                body = serde_json::to_string_pretty(&templated).unwrap_or_default();
                // 文档里的几个请求示例，按模板取成几组试调样例（只有一个的就是参数样例，不另列）。
                if requests.len() > 1 {
                    doc_examples = examples::from_document(&templated, &requests, text);
                }
            }
            _ if build::looks_like_form(&source) => notes.push(
                "请求体是表单格式（a=1&b=2），暂只支持 JSON 请求体，需要接口方确认能否收 JSON"
                    .into(),
            ),
            _ => notes.push("请求体不是 JSON 对象，没有采用".into()),
        }
    }
    let described = describe_inputs(&mut inputs, params);
    let write = request.method.is_none() || scan::write_method_near(text, request.offset);
    let suspicious = write_word(&url);
    let mut draft = Draft {
        endpoint: ApiEndpoint {
            id: build::id_from_path(&url),
            name: request.name.clone().unwrap_or_default(),
            method,
            url,
            inputs,
            headers,
            body,
            examples: doc_examples,
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
            suspicious
                .map(|word| format!("地址里有「{word}」字样，可能会改数据，请确认"))
                .unwrap_or_default()
        },
        notes,
        sample: None,
    };
    if request.name.is_some() {
        draft.set_origin("名称", Origin::Document);
    }
    if !draft.endpoint.inputs.is_empty() {
        draft.set_origin("参数", Origin::Document);
    }
    if described {
        draft.set_origin("参数说明", Origin::Document);
    }
    if !draft.endpoint.body.is_empty() {
        draft.set_origin("请求体", Origin::Document);
    }
    if auth::has_auth(&draft.endpoint) {
        draft.set_origin("鉴权", Origin::Document);
    }
    if !draft.endpoint.examples.is_empty() {
        draft.set_origin("样例", Origin::Document);
    }
    // 返回示例：明说是返回的优先，其次说不清的（文档里讲字段写法的 JSON 片段也是说不清的）。
    let sample = blocks
        .iter()
        .filter(in_range)
        .find(|b| b.role == BlockRole::Response)
        .or_else(|| {
            blocks
                .iter()
                .filter(in_range)
                .find(|b| b.role == BlockRole::Unknown)
        })
        .map(|b| b.value.clone());
    if let Some(sample) = sample {
        apply_sample(&mut draft, sample);
    }
    draft
}

/// 地址的查询串里有没有这个参数。
fn query_has(url: &str, name: &str) -> bool {
    url.split_once('?').is_some_and(|(_, query)| {
        query
            .split('&')
            .any(|pair| pair.split('=').next() == Some(name))
    })
}

/// 参数表里的样例按类型写进拼出来的请求体。
fn typed_example(param: &DocParam) -> Value {
    let example = param.example.trim();
    match param.kind {
        InputKind::Number => example
            .parse::<i64>()
            .map(Value::from)
            .or_else(|_| example.parse::<f64>().map(Value::from))
            .unwrap_or_else(|_| Value::String(example.to_string())),
        InputKind::Bool if matches!(example, "true" | "false") => Value::Bool(example == "true"),
        // JSON 参数：整段放进去，模板化时因为点了名整段做成一个参数。
        InputKind::Json => serde_json::from_str(example).unwrap_or_else(|_| json!({})),
        _ => Value::String(example.to_string()),
    }
}

/// 用参数表补输入变量的说明、必填、类型与样例。返回是否补了说明。
fn describe_inputs(inputs: &mut [ApiInput], params: &[DocParam]) -> bool {
    let mut described = false;
    for param in params {
        let Some(input) = inputs
            .iter_mut()
            .find(|i| i.name == build::ident(&param.name))
        else {
            continue;
        };
        if input.description.is_empty() && !param.description.is_empty() {
            input.description = param.description.clone();
            described = true;
        }
        input.required = param.required;
        let example = if input.example.is_empty() {
            &param.example
        } else {
            &input.example
        };
        match param.kind {
            InputKind::Number if example.is_empty() || example.trim().parse::<f64>().is_ok() => {
                input.kind = InputKind::Number;
            }
            InputKind::Bool if matches!(example.trim(), "" | "true" | "false") => {
                input.kind = InputKind::Bool;
            }
            InputKind::Json
                if example.is_empty() || serde_json::from_str::<Value>(example).is_ok() =>
            {
                input.kind = InputKind::Json;
            }
            _ => {}
        }
        if input.example.is_empty() {
            input.example = param.example.clone();
        }
    }
    described
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

/// 地址里像改数据的字样（`/fav/delete`、`/updateInfo`）。
pub(crate) fn write_word(url: &str) -> Option<String> {
    path_key(url)
        .split(['/', '_', '-', '.'])
        .find(|seg| WRITE_WORDS.iter().any(|w| seg.starts_with(w)))
        .map(str::to_string)
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
    // 文档里这一节的标题优先，模型只给没名字的起名。
    if draft.endpoint.name.trim().is_empty() && !found.name.trim().is_empty() {
        draft.endpoint.name = found.name.trim().to_string();
        draft.set_origin("名称", Origin::Model);
    }
    let from_table = draft
        .origins
        .iter()
        .any(|(f, o)| *f == "参数说明" && *o == Origin::Document);
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
            if !theirs.description.trim().is_empty()
                && (input.description.is_empty() || !from_table)
            {
                input.description = theirs.description.trim().to_string();
                described = true;
            }
            // 参数表写了必填与否的，以参数表为准。
            if !from_table {
                input.required = theirs.required;
            }
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
    apply_model_examples(draft, found);
    apply_access(draft, found);
    draft.notes.extend(
        found
            .missing
            .iter()
            .filter(|m| !m.trim().is_empty())
            .map(|m| format!("AI 指出缺：{}", m.trim())),
    );
}

/// 模型编的试调样例：核对后排在文档示例前面。
fn apply_model_examples(draft: &mut Draft, found: &ModelEndpoint) {
    if found.examples.is_empty() {
        return;
    }
    let (taken, notes) = examples::check(&draft.endpoint, &found.examples);
    draft.notes.extend(notes);
    if !taken.is_empty() {
        draft.endpoint.examples = examples::combine(taken, &draft.endpoint.examples);
        draft.set_origin("样例", Origin::Model);
    }
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
    let write_word = write_word(&draft.endpoint.url);
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
        "json" | "object" | "array" | "map" | "list" | "对象" | "数组" => InputKind::Json,
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
        let json_keys: Vec<String> = found
            .inputs
            .iter()
            .filter(|i| kind_of(&i.kind) == InputKind::Json)
            .map(|i| i.name.trim().to_string())
            .collect();
        let templated = build::template_body(&object, &mut inputs, secrets, &json_keys);
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
    apply_model_examples(&mut draft, found);
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
