//! 数据接口：配置式的内网 HTTP 接口，只查询（`docs/ai-agent-workbench.md` 16.8）。
//!
//! 一个接口 = 方法与地址 + 输入变量 + 请求头 + 请求体模板 + 返回映射。技能经 `http.call`
//! 工具调用：程序按模板组请求、发出去、按映射把返回整理成「标题 / 正文 / 出处」的条目，
//! 资料类结果并入证据包，可被引用、可核验。
//!
//! - 接口定义存在 `配置目录/apis.json`，**不写进技能文件**（技能可以分享，地址不能）；
//! - 密钥存在 `配置目录/api-secrets.json`，模板里写 `{secret:名字}` 引用，导出接口时不带；
//! - 只有 `GET` 与 `POST` 两种方法，`POST` 只用于带请求体的查询：接口配置里没有「提交」一类。

use crate::agent::board::value_to_text;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::LazyLock;

/// 模板占位：`{region}`、`{secret:token}`。
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{(secret:)?([A-Za-z_][A-Za-z0-9_]*)\}").expect("占位正则"));

/// 返回体最多读这么多字节，防止接口回一个超大文件把内存吃满。
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
/// 映射后最多保留几条、每条正文最多几个字。
const MAX_ITEMS: usize = 50;
const MAX_ITEM_CHARS: usize = 3000;
/// 默认超时（秒）。
pub(crate) const DEFAULT_TIMEOUT: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) enum ApiMethod {
    #[default]
    #[serde(rename = "GET")]
    Get,
    /// 只用于带请求体的查询。
    #[serde(rename = "POST")]
    Post,
}

impl ApiMethod {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// 输入变量的类型。决定请求体里单独一个 `{变量}` 替换成字符串、数字还是真假。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputKind {
    #[default]
    Text,
    Number,
    Bool,
}

impl InputKind {
    pub(crate) const ALL: [InputKind; 3] = [Self::Text, Self::Number, Self::Bool];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Text => "文字",
            Self::Number => "数字",
            Self::Bool => "是否",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiInput {
    pub(crate) name: String,
    pub(crate) kind: InputKind,
    pub(crate) description: String,
    pub(crate) required: bool,
    /// 测试时的样例值。
    pub(crate) example: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiHeader {
    pub(crate) name: String,
    pub(crate) value: String,
}

/// 返回映射。`list` 是 JSON 指针（`/data/items`），空表示整个返回；其余字段写英文字段名
/// （`title`）、JSON 指针（`/meta/name`）、模板（`{region}{year}年：{value}{unit}`）或固定文字
/// （`省统计系统`），空表示按常见字段名猜。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiMapping {
    pub(crate) list: String,
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) source: String,
    pub(crate) id: String,
}

/// 返回去处。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ApiDestination {
    /// 证据包：可溯源、可被引用、可核验。
    #[default]
    Evidence,
    /// 只存进变量，给后面的步骤用（例如候选列表）。
    Variable,
}

impl ApiDestination {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Evidence => "证据包",
            Self::Variable => "变量",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiEndpoint {
    pub(crate) id: String,
    pub(crate) name: String,
    /// 写清这个接口能回答什么，给技能作者与模型看。
    pub(crate) description: String,
    pub(crate) method: ApiMethod,
    pub(crate) url: String,
    pub(crate) inputs: Vec<ApiInput>,
    pub(crate) headers: Vec<ApiHeader>,
    /// 请求体模板（JSON），只对 POST 有意义。
    pub(crate) body: String,
    pub(crate) mapping: ApiMapping,
    pub(crate) destination: ApiDestination,
    pub(crate) timeout_seconds: u64,
}

impl Default for ApiEndpoint {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            method: ApiMethod::Get,
            url: String::new(),
            inputs: Vec::new(),
            headers: Vec::new(),
            body: String::new(),
            mapping: ApiMapping::default(),
            destination: ApiDestination::Evidence,
            timeout_seconds: DEFAULT_TIMEOUT,
        }
    }
}

/// 全部接口定义（`apis.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiStore {
    pub(crate) endpoints: Vec<ApiEndpoint>,
}

/// 密钥（`api-secrets.json`）：名字 → 值。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiSecrets {
    pub(crate) secrets: BTreeMap<String, String>,
}

fn store_path(name: &str) -> anyhow::Result<PathBuf> {
    Ok(crate::storage::config_dir()?.join(name))
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned + Default>(name: &str) -> anyhow::Result<T> {
    let path = store_path(name)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} 解析失败：{e}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(anyhow::anyhow!("读取 {} 失败：{error}", path.display())),
    }
}

pub(crate) fn write_json<T: Serialize>(name: &str, value: &T) -> anyhow::Result<()> {
    let path = store_path(name)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

impl ApiStore {
    pub(crate) fn load() -> anyhow::Result<Self> {
        read_json("apis.json")
    }

    pub(crate) fn save(&self) -> anyhow::Result<()> {
        write_json("apis.json", self)
    }

    pub(crate) fn get(&self, id: &str) -> Option<&ApiEndpoint> {
        self.endpoints.iter().find(|endpoint| endpoint.id == id)
    }

    /// 导入：同 id 的覆盖，新的追加。返回 (新增, 覆盖)。
    pub(crate) fn merge(&mut self, incoming: ApiStore) -> (usize, usize) {
        let (mut added, mut replaced) = (0, 0);
        for endpoint in incoming.endpoints {
            match self.endpoints.iter_mut().find(|e| e.id == endpoint.id) {
                Some(existing) => {
                    *existing = endpoint;
                    replaced += 1;
                }
                None => {
                    self.endpoints.push(endpoint);
                    added += 1;
                }
            }
        }
        (added, replaced)
    }
}

impl ApiSecrets {
    pub(crate) fn load() -> anyhow::Result<Self> {
        read_json("api-secrets.json")
    }

    pub(crate) fn save(&self) -> anyhow::Result<()> {
        write_json("api-secrets.json", self)
    }
}

impl ApiEndpoint {
    /// 静态检查：id、地址、模板里的变量、请求体、超时。返回问题清单，空表示通过。
    pub(crate) fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.id.trim().is_empty() {
            problems.push("缺少 id".into());
        } else if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            problems.push("id 只能用英文字母、数字、下划线和短横线".into());
        }
        if self.name.trim().is_empty() {
            problems.push("缺少名称".into());
        }
        let url = self.url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            problems.push("地址要以 http:// 或 https:// 开头".into());
        }
        let mut seen = Vec::new();
        for input in &self.inputs {
            let name = input.name.trim();
            if name.is_empty() {
                problems.push("有输入变量没写名字".into());
            } else if !PLACEHOLDER.is_match(&format!("{{{name}}}")) {
                problems.push(format!("输入变量名「{name}」只能用英文字母、数字和下划线"));
            } else if seen.contains(&name) {
                problems.push(format!("输入变量「{name}」重复"));
            }
            seen.push(name);
        }
        let mut templates: Vec<&str> = vec![&self.url, &self.body];
        templates.extend(self.headers.iter().map(|h| h.value.as_str()));
        for template in templates {
            for caps in PLACEHOLDER.captures_iter(template) {
                let name = &caps[2];
                if caps.get(1).is_none() && !seen.contains(&name) {
                    problems.push(format!("模板里的 {{{name}}} 没有对应的输入变量"));
                }
            }
        }
        if self.method == ApiMethod::Post
            && !self.body.trim().is_empty()
            && serde_json::from_str::<Value>(&self.body).is_err()
        {
            problems.push("请求体模板不是合法的 JSON".into());
        }
        if !self.mapping.list.is_empty() && !self.mapping.list.starts_with('/') {
            problems.push("列表位置要写成 JSON 指针，以 / 开头，例如 /data/items".into());
        }
        if !(1..=300).contains(&self.timeout_seconds) {
            problems.push("超时要在 1 到 300 秒之间".into());
        }
        problems.dedup();
        problems
    }

    /// 测试用的样例输入。
    pub(crate) fn example_args(&self) -> Map<String, Value> {
        self.inputs
            .iter()
            .filter(|input| !input.example.is_empty())
            .map(|input| (input.name.clone(), Value::String(input.example.clone())))
            .collect()
    }
}

/// 组好的请求。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prepared {
    pub(crate) method: ApiMethod,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Option<Value>,
    /// 用到的密钥值，展示请求时打码用。
    secrets_used: Vec<String>,
}

impl Prepared {
    /// 给人看的请求（密钥打码），测试面板里显示。
    pub(crate) fn describe(&self) -> String {
        let mut text = format!("{} {}", self.method.label(), self.url);
        for (name, value) in &self.headers {
            text.push_str(&format!("\n{name}: {value}"));
        }
        if let Some(body) = &self.body {
            text.push_str("\n\n");
            text.push_str(&serde_json::to_string_pretty(body).unwrap_or_default());
        }
        for secret in self.secrets_used.iter().filter(|s| !s.is_empty()) {
            text = text.replace(secret.as_str(), "******");
        }
        text
    }
}

/// 输入值 → 按类型规整好的 JSON。
fn input_values(
    endpoint: &ApiEndpoint,
    args: &Map<String, Value>,
) -> Result<BTreeMap<String, Value>, String> {
    let mut values = BTreeMap::new();
    for input in &endpoint.inputs {
        let raw = match args.get(&input.name) {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) if text.trim().is_empty() => None,
            Some(value) => Some(value),
        };
        let Some(raw) = raw else {
            if input.required {
                let doc = if input.description.is_empty() {
                    String::new()
                } else {
                    format!("（{}）", input.description)
                };
                return Err(format!(
                    "接口「{}」缺少输入 {}{doc}",
                    endpoint.name, input.name
                ));
            }
            values.insert(input.name.clone(), Value::Null);
            continue;
        };
        let value = match input.kind {
            InputKind::Text => Value::String(match raw {
                Value::String(text) => text.clone(),
                other => value_to_text(other),
            }),
            InputKind::Number => match raw {
                Value::Number(n) => Value::Number(n.clone()),
                Value::String(text) => crate::agent::tools::parse_number(text)
                    .and_then(|n| {
                        if n.fract() == 0.0 && n.abs() < 9e15 {
                            Some(Value::from(n as i64))
                        } else {
                            serde_json::Number::from_f64(n).map(Value::Number)
                        }
                    })
                    .ok_or_else(|| format!("输入 {} 要是数字，收到「{text}」", input.name))?,
                other => return Err(format!("输入 {} 要是数字，收到 {other}", input.name)),
            },
            InputKind::Bool => match raw {
                Value::Bool(flag) => Value::Bool(*flag),
                Value::String(text) => match text.trim() {
                    "true" | "是" | "1" => Value::Bool(true),
                    "false" | "否" | "0" => Value::Bool(false),
                    other => {
                        return Err(format!(
                            "输入 {} 要是「是 / 否」，收到「{other}」",
                            input.name
                        ));
                    }
                },
                other => return Err(format!("输入 {} 要是「是 / 否」，收到 {other}", input.name)),
            },
        };
        values.insert(input.name.clone(), value);
    }
    Ok(values)
}

/// 地址里的值按 RFC 3986 编码：只留字母数字与 `-._~`，其余按 UTF-8 字节转成 `%XX`。
fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 替换模板里的占位。`encode` 为真时值做地址编码。
fn fill(
    template: &str,
    values: &BTreeMap<String, Value>,
    secrets: &ApiSecrets,
    encode: bool,
    used: &mut Vec<String>,
) -> Result<String, String> {
    let mut error = None;
    let text = PLACEHOLDER.replace_all(template, |caps: &regex::Captures<'_>| {
        let name = &caps[2];
        let raw = if caps.get(1).is_some() {
            match secrets.secrets.get(name) {
                Some(secret) => {
                    used.push(secret.clone());
                    secret.clone()
                }
                None => {
                    error = Some(format!(
                        "密钥「{name}」还没有填，在「数据接口」页的密钥里补上"
                    ));
                    String::new()
                }
            }
        } else {
            match values.get(name) {
                Some(Value::Null) | None => String::new(),
                Some(Value::String(text)) => text.clone(),
                Some(other) => other.to_string(),
            }
        };
        if encode { percent_encode(&raw) } else { raw }
    });
    match error {
        Some(error) => Err(error),
        None => Ok(text.into_owned()),
    }
}

/// 请求体模板里逐个字符串替换：整串只有一个 `{变量}` 时换成带类型的值。
fn fill_body(
    template: &Value,
    values: &BTreeMap<String, Value>,
    secrets: &ApiSecrets,
    used: &mut Vec<String>,
) -> Result<Value, String> {
    Ok(match template {
        Value::String(text) => {
            if let Some(caps) = PLACEHOLDER.captures(text)
                && caps[0].len() == text.len()
                && caps.get(1).is_none()
            {
                values.get(&caps[2]).cloned().unwrap_or(Value::Null)
            } else {
                Value::String(fill(text, values, secrets, false, used)?)
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| fill_body(item, values, secrets, used))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| Ok((key.clone(), fill_body(item, values, secrets, used)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

/// 按模板组请求。
pub(crate) fn prepare(
    endpoint: &ApiEndpoint,
    args: &Map<String, Value>,
    secrets: &ApiSecrets,
) -> Result<Prepared, String> {
    let problems = endpoint.problems();
    if !problems.is_empty() {
        return Err(format!(
            "接口「{}」配置有问题：{}",
            endpoint.name,
            problems.join("；")
        ));
    }
    let values = input_values(endpoint, args)?;
    let mut used = Vec::new();
    let url = fill(endpoint.url.trim(), &values, secrets, true, &mut used)?;
    let headers = endpoint
        .headers
        .iter()
        .filter(|header| !header.name.trim().is_empty())
        .map(|header| {
            Ok((
                header.name.trim().to_string(),
                fill(&header.value, &values, secrets, false, &mut used)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let body = if endpoint.method == ApiMethod::Post && !endpoint.body.trim().is_empty() {
        let template: Value = serde_json::from_str(&endpoint.body)
            .map_err(|e| format!("请求体模板不是 JSON：{e}"))?;
        Some(fill_body(&template, &values, secrets, &mut used)?)
    } else {
        None
    };
    Ok(Prepared {
        method: endpoint.method,
        url,
        headers,
        body,
        secrets_used: used,
    })
}

/// 接口的原始返回。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawResponse {
    pub(crate) status: u16,
    pub(crate) body: String,
}

/// 发出请求，读回返回体。非 2xx 算失败。
pub(crate) fn execute(prepared: &Prepared, timeout_seconds: u64) -> Result<RawResponse, String> {
    let client =
        crate::net::client(&prepared.url, timeout_seconds).map_err(|e| format!("{e:#}"))?;
    let mut request = match prepared.method {
        ApiMethod::Get => client.get(&prepared.url),
        ApiMethod::Post => client.post(&prepared.url),
    };
    for (name, value) in &prepared.headers {
        request = request.header(name, value);
    }
    if let Some(body) = &prepared.body {
        request = request.json(body);
    }
    let response = request.send().map_err(|e| {
        let error = e.without_url();
        if error.is_timeout() {
            format!("接口超时（{timeout_seconds} 秒）")
        } else {
            format!("接口连不上：{error}")
        }
    })?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读取返回失败：{e}"))?;
    let body = String::from_utf8_lossy(&bytes).into_owned();
    if !(200..300).contains(&status) {
        return Err(format!(
            "接口返回 {status}：{}",
            crate::agent::tools::short(body.trim(), 200)
        ));
    }
    Ok(RawResponse { status, body })
}

/// 映射后的一条。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct MappedItem {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) source: String,
}

/// 取一个字段：`/JSON 指针`、`{模板}`、英文字段名，其余当固定文字（如出处写「省统计系统」）。
fn pick(item: &Value, spec: &str) -> Option<String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    if spec.starts_with('/') {
        return item.pointer(spec).map(value_to_text);
    }
    if spec.contains('{') {
        let text = PLACEHOLDER.replace_all(spec, |caps: &regex::Captures<'_>| {
            item.get(&caps[2]).map(value_to_text).unwrap_or_default()
        });
        return Some(text.into_owned());
    }
    let identifier = spec
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && spec.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if identifier {
        item.get(spec).map(value_to_text)
    } else {
        Some(spec.to_string())
    }
}

fn guess(item: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| item.get(*key))
        .map(value_to_text)
        .filter(|text| !text.trim().is_empty())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect::<String>() + "…"
    }
}

/// 按映射把返回体整理成条目。
pub(crate) fn map_response(endpoint: &ApiEndpoint, body: &str) -> Result<Vec<MappedItem>, String> {
    let mapping = &endpoint.mapping;
    let root: Value = match serde_json::from_str(body) {
        Ok(root) => root,
        Err(_) if mapping.list.is_empty() => {
            // 不是 JSON（纯文本、CSV……）又没要求取列表：整段当一条。
            return Ok(vec![MappedItem {
                id: "1".into(),
                title: endpoint.name.clone(),
                text: truncate(body.trim(), MAX_ITEM_CHARS),
                source: endpoint.name.clone(),
            }]);
        }
        Err(error) => return Err(format!("返回的不是 JSON：{error}")),
    };
    let target = if mapping.list.is_empty() {
        &root
    } else {
        root.pointer(&mapping.list)
            .ok_or_else(|| format!("返回里找不到列表位置 {}", mapping.list))?
    };
    let items: Vec<&Value> = match target {
        Value::Array(items) => items.iter().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    };
    Ok(items
        .into_iter()
        .take(MAX_ITEMS)
        .enumerate()
        .map(|(index, item)| {
            let title = pick(item, &mapping.title)
                .or_else(|| guess(item, &["title", "name", "标题", "名称"]))
                .unwrap_or_default();
            let text = pick(item, &mapping.text)
                .or_else(|| guess(item, &["text", "content", "summary", "正文", "内容"]))
                .unwrap_or_else(|| value_to_text(item));
            let source = pick(item, &mapping.source)
                .or_else(|| guess(item, &["source", "url", "出处", "来源"]))
                .unwrap_or_else(|| endpoint.name.clone());
            let id = pick(item, &mapping.id)
                .or_else(|| guess(item, &["id", "ID", "编号"]))
                .unwrap_or_else(|| (index + 1).to_string());
            MappedItem {
                id: truncate(id.trim(), 80),
                title: truncate(title.trim(), 200),
                text: truncate(text.trim(), MAX_ITEM_CHARS),
                source: truncate(source.trim(), 200),
            }
        })
        .collect())
}

/// 组请求、发出去、映射，一步到位。返回 (请求描述, 原始返回, 条目)。
pub(crate) fn call(
    endpoint: &ApiEndpoint,
    args: &Map<String, Value>,
    secrets: &ApiSecrets,
) -> Result<(Prepared, RawResponse, Vec<MappedItem>), String> {
    let prepared = prepare(endpoint, args, secrets)?;
    let raw = execute(&prepared, endpoint.timeout_seconds)?;
    let items = map_response(endpoint, &raw.body)?;
    Ok((prepared, raw, items))
}

/// 本机假 HTTP 服务：按顺序回预设的响应，记下收到的请求。测试 `http.call` 与整条
/// 「调用 → 映射 → 证据包 → 引用 → 核验」链路用。
#[cfg(test)]
pub(crate) mod test_server {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub(crate) struct TestServer {
        pub(crate) url: String,
        pub(crate) requests: Arc<Mutex<Vec<String>>>,
    }

    impl TestServer {
        /// 依次回 `responses` 里的 (状态码, 返回体)，回完就退出。
        pub(crate) fn start(responses: Vec<(u16, String)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("绑定本机端口");
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = requests.clone();
            std::thread::spawn(move || {
                for (status, body) in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    // 读到请求头结束，再按 Content-Length 读完请求体。
                    loop {
                        let n = stream.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buffer.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buffer).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let length = text[..end]
                                .lines()
                                .find_map(|line| {
                                    let lower = line.to_ascii_lowercase();
                                    lower
                                        .strip_prefix("content-length:")
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            if buffer.len() >= end + 4 + length {
                                break;
                            }
                        }
                    }
                    log.lock()
                        .unwrap()
                        .push(String::from_utf8_lossy(&buffer).to_string());
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(reply.as_bytes());
                }
            });
            Self { url, requests }
        }

        pub(crate) fn request(&self, index: usize) -> String {
            self.requests.lock().unwrap()[index].clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_server::TestServer;
    use super::*;
    use serde_json::json;

    fn stat_endpoint(url: &str) -> ApiEndpoint {
        ApiEndpoint {
            id: "fire_stat".into(),
            name: "森林火灾统计".into(),
            description: "按地区与年份查森林火灾起数".into(),
            method: ApiMethod::Get,
            url: format!("{url}/api/stat?region={{region}}&year={{year}}"),
            inputs: vec![
                ApiInput {
                    name: "region".into(),
                    required: true,
                    description: "地区名称".into(),
                    example: "全省".into(),
                    ..ApiInput::default()
                },
                ApiInput {
                    name: "year".into(),
                    kind: InputKind::Number,
                    ..ApiInput::default()
                },
            ],
            headers: vec![ApiHeader {
                name: "Authorization".into(),
                value: "Bearer {secret:stat_token}".into(),
            }],
            mapping: ApiMapping {
                list: "/data/items".into(),
                title: "{region}{year}年森林火灾统计".into(),
                text: "{region}{year}年共发生森林火灾{count}起".into(),
                source: "/meta/source".into(),
                id: "id".into(),
            },
            ..ApiEndpoint::default()
        }
    }

    fn secrets() -> ApiSecrets {
        ApiSecrets {
            secrets: [("stat_token".to_string(), "s3cr3t".to_string())].into(),
        }
    }

    #[test]
    fn requests_are_filled_from_typed_inputs_and_secrets() {
        let endpoint = stat_endpoint("http://10.0.0.8");
        let args = json!({"region": "甲 区", "year": "2025"});
        let prepared = prepare(&endpoint, args.as_object().unwrap(), &secrets()).unwrap();
        assert_eq!(
            prepared.url,
            "http://10.0.0.8/api/stat?region=%E7%94%B2%20%E5%8C%BA&year=2025"
        );
        assert_eq!(
            prepared.headers,
            [("Authorization".into(), "Bearer s3cr3t".into())]
        );
        assert!(prepared.body.is_none());
        let shown = prepared.describe();
        assert!(
            shown.contains("Bearer ******") && !shown.contains("s3cr3t"),
            "{shown}"
        );

        let missing = prepare(&endpoint, &Map::new(), &secrets()).unwrap_err();
        assert!(missing.contains("缺少输入 region（地区名称）"), "{missing}");
        let bad = prepare(
            &endpoint,
            json!({"region": "甲", "year": "去年"}).as_object().unwrap(),
            &secrets(),
        )
        .unwrap_err();
        assert!(bad.contains("要是数字"), "{bad}");
        let no_secret = prepare(
            &endpoint,
            json!({"region": "甲"}).as_object().unwrap(),
            &ApiSecrets::default(),
        )
        .unwrap_err();
        assert!(
            no_secret.contains("密钥「stat_token」还没有填"),
            "{no_secret}"
        );
    }

    #[test]
    fn post_bodies_keep_the_type_of_a_lone_variable() {
        let endpoint = ApiEndpoint {
            id: "policy".into(),
            name: "政策库".into(),
            method: ApiMethod::Post,
            url: "https://10.0.0.9/search".into(),
            inputs: vec![
                ApiInput {
                    name: "keyword".into(),
                    required: true,
                    ..ApiInput::default()
                },
                ApiInput {
                    name: "top".into(),
                    kind: InputKind::Number,
                    ..ApiInput::default()
                },
                ApiInput {
                    name: "exact".into(),
                    kind: InputKind::Bool,
                    ..ApiInput::default()
                },
            ],
            body:
                r#"{"q": "{keyword}", "size": "{top}", "exact": "{exact}", "note": "查{keyword}"}"#
                    .into(),
            ..ApiEndpoint::default()
        };
        let args = json!({"keyword": "森林防火", "top": "5", "exact": "是"});
        let prepared =
            prepare(&endpoint, args.as_object().unwrap(), &ApiSecrets::default()).unwrap();
        assert_eq!(
            prepared.body.unwrap(),
            json!({"q": "森林防火", "size": 5, "exact": true, "note": "查森林防火"})
        );
    }

    #[test]
    fn configuration_problems_are_listed() {
        let endpoint = ApiEndpoint {
            id: "坏 id".into(),
            url: "ftp://x/{who}".into(),
            method: ApiMethod::Post,
            body: "{不是 json".into(),
            inputs: vec![
                ApiInput {
                    name: "a".into(),
                    ..ApiInput::default()
                },
                ApiInput {
                    name: "a".into(),
                    ..ApiInput::default()
                },
            ],
            mapping: ApiMapping {
                list: "data".into(),
                ..ApiMapping::default()
            },
            timeout_seconds: 0,
            ..ApiEndpoint::default()
        };
        let problems = endpoint.problems().join("\n");
        for expected in [
            "id 只能用",
            "缺少名称",
            "http:// 或 https://",
            "「a」重复",
            "{who} 没有对应的输入变量",
            "不是合法的 JSON",
            "JSON 指针",
            "超时",
        ] {
            assert!(
                problems.contains(expected),
                "缺少「{expected}」：\n{problems}"
            );
        }
        assert!(stat_endpoint("http://x").problems().is_empty());
    }

    #[test]
    fn responses_are_mapped_by_pointer_field_and_template() {
        let endpoint = stat_endpoint("http://x");
        let body = json!({
            "meta": {"source": "省应急厅统计系统"},
            "data": {"items": [
                {"id": 11, "region": "全省", "year": 2025, "count": 12},
                {"id": 12, "region": "甲市", "year": 2025, "count": 3}
            ]}
        })
        .to_string();
        let items = map_response(&endpoint, &body).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "全省2025年森林火灾统计");
        assert_eq!(items[0].text, "全省2025年共发生森林火灾12起");
        assert_eq!(items[0].id, "11");
        // 出处写的是根上的指针，每条里没有：退回接口名。
        assert_eq!(items[0].source, "森林火灾统计");

        let plain = ApiEndpoint {
            name: "天气".into(),
            ..ApiEndpoint::default()
        };
        let fixed = ApiEndpoint {
            mapping: ApiMapping {
                source: "省气象台".into(),
                text: "content".into(),
                ..ApiMapping::default()
            },
            ..plain.clone()
        };
        let item = &map_response(&fixed, r#"{"content": "明日晴"}"#).unwrap()[0];
        assert_eq!(
            (item.source.as_str(), item.text.as_str()),
            ("省气象台", "明日晴"),
            "非英文字段名当固定文字"
        );
        let guessed = map_response(&plain, r#"{"title": "预报", "content": "明日晴"}"#).unwrap();
        assert_eq!(
            (guessed[0].title.as_str(), guessed[0].text.as_str()),
            ("预报", "明日晴")
        );
        let text = map_response(&plain, "不是 JSON 的纯文本").unwrap();
        assert_eq!(text[0].text, "不是 JSON 的纯文本");
        assert!(
            map_response(&endpoint, "{}")
                .unwrap_err()
                .contains("/data/items")
        );
    }

    #[test]
    fn a_local_server_round_trip_sends_the_request_and_maps_the_reply() {
        let server = TestServer::start(vec![
            (
                200,
                json!({"data": {"items": [{"id": 1, "region": "全省", "year": 2025, "count": 12}]}})
                    .to_string(),
            ),
            (500, "内部错误".into()),
        ]);
        let endpoint = stat_endpoint(&server.url);
        let args = json!({"region": "全省", "year": 2025});
        let (prepared, raw, items) =
            call(&endpoint, args.as_object().unwrap(), &secrets()).unwrap();
        assert_eq!(raw.status, 200);
        assert!(
            prepared
                .url
                .ends_with("region=%E5%85%A8%E7%9C%81&year=2025")
        );
        assert_eq!(items[0].text, "全省2025年共发生森林火灾12起");
        let request = server.request(0);
        assert!(
            request.starts_with("GET /api/stat?region=%E5%85%A8%E7%9C%81&year=2025 "),
            "{request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer s3cr3t"),
            "{request}"
        );

        let error = call(&endpoint, args.as_object().unwrap(), &secrets()).unwrap_err();
        assert!(error.contains("接口返回 500"), "{error}");
    }

    #[test]
    fn stores_round_trip_and_merge_by_id() {
        let dir = std::env::temp_dir().join(format!("gongwen-apis-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        crate::storage::set_test_config_dir(Some(dir.clone()));
        assert_eq!(
            ApiStore::load().unwrap(),
            ApiStore::default(),
            "没有文件就是空的"
        );
        let mut store = ApiStore {
            endpoints: vec![stat_endpoint("http://x")],
        };
        store.save().unwrap();
        secrets().save().unwrap();
        assert_eq!(ApiStore::load().unwrap(), store);
        assert_eq!(ApiSecrets::load().unwrap(), secrets());
        let exported = std::fs::read_to_string(dir.join("apis.json")).unwrap();
        assert!(!exported.contains("s3cr3t"), "接口文件里不带密钥");
        crate::storage::set_test_config_dir(None);

        let mut renamed = stat_endpoint("http://y");
        renamed.name = "改过的".into();
        let mut other = stat_endpoint("http://z");
        other.id = "other".into();
        assert_eq!(
            store.merge(ApiStore {
                endpoints: vec![renamed, other]
            }),
            (1, 1)
        );
        assert_eq!(store.get("fire_stat").unwrap().name, "改过的");
        let _ = std::fs::remove_dir_all(dir);
    }
}
