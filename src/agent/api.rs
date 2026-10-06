//! 数据接口：配置式的内网 HTTP 接口，只查询（`docs/ai-agent-workbench.md` 16.8）。
//!
//! 一个接口 = 方法与地址 + 输入变量 + 请求头 + 请求体模板 + 返回映射。技能经 `http.call`
//! 工具调用：程序按模板组请求、发出去、按映射把返回整理成「标题 / 正文 / 出处」的条目，
//! 资料类结果并入证据包，可被引用、可核验。
//!
//! - 接口定义存在 `配置目录/apis.json`，**不写进技能文件**（技能可以分享，地址不能）；
//! - 密钥存在 `配置目录/api-secrets.json`，模板里写 `{secret:名字}` 引用，导出接口时不带；
//! - 接口的「真身」是 `apis/<服务>.openapi.json`（见 [`crate::agent::apidef`]）；这里的
//!   [`ApiEndpoint`] 是从 OpenAPI 编译出来的运行时结构，界面改完再写回 OpenAPI；
//! - 方法不限于 GET / POST，但只有标了只查询（`readonly`）的接口给 AI 调；会改数据的接口
//!   只在调试台由人发。

use crate::agent::board::value_to_text;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::LazyLock;

/// 模板占位：`{region}`、`{secret:token}`。
pub(crate) static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{(secret:)?([A-Za-z_][A-Za-z0-9_]*)\}").expect("占位正则"));

/// 返回体最多读这么多字节，防止接口回一个超大文件把内存吃满。
const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
/// 映射后最多保留几条、每条正文最多几个字。
const MAX_ITEMS: usize = 50;
const MAX_ITEM_CHARS: usize = 3000;
/// 默认超时（秒）。
pub(crate) const DEFAULT_TIMEOUT: u64 = 30;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub(crate) enum ApiMethod {
    #[default]
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
    #[serde(rename = "PUT")]
    Put,
    #[serde(rename = "PATCH")]
    Patch,
    #[serde(rename = "DELETE")]
    Delete,
    #[serde(rename = "HEAD")]
    Head,
}

impl ApiMethod {
    pub(crate) const ALL: [ApiMethod; 6] = [
        Self::Get,
        Self::Post,
        Self::Put,
        Self::Patch,
        Self::Delete,
        Self::Head,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
        }
    }

    /// OpenAPI 里的写法：`get`、`post`……
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
            Self::Head => "head",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|method| method.key().eq_ignore_ascii_case(key.trim()))
    }

    /// 能带请求体。
    pub(crate) fn has_body(self) -> bool {
        matches!(self, Self::Post | Self::Put | Self::Patch | Self::Delete)
    }

    /// 按方法默认算不算只查询（地址里的字样另看）。
    pub(crate) fn reads(self) -> bool {
        matches!(self, Self::Get | Self::Head)
    }
}

/// 请求体的编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BodyKind {
    #[default]
    Json,
    /// `application/x-www-form-urlencoded`：请求体模板是一个对象，逐项编成 `键=值`。
    Form,
}

/// 输入变量的类型。决定请求体里单独一个 `{变量}` 替换成字符串、数字还是真假。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputKind {
    #[default]
    Text,
    Number,
    Bool,
    /// 一整段 JSON（对象、数组）：键由调用方定的「问题表」、条件列表之类，没法拆成一个个变量。
    Json,
}

impl InputKind {
    pub(crate) const ALL: [InputKind; 4] = [Self::Text, Self::Number, Self::Bool, Self::Json];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Text => "文字",
            Self::Number => "数字",
            Self::Bool => "是否",
            Self::Json => "JSON",
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
    /// 地址参数是数组时用逗号连成一个值（`ids=1,2`），不是逐个重复（`ids=1&ids=2`）。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) comma: bool,
    /// OpenAPI 里这个参数的 schema（枚举、格式、嵌套结构），写回时沿用。
    #[serde(skip)]
    pub(crate) schema: Option<Value>,
    /// 路径参数在 OpenAPI 里的名字（变量名为合法标识符改过时与 `name` 不同）。
    #[serde(skip)]
    pub(crate) wire: String,
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

/// 业务成功判据：HTTP 200 不等于查询成功，很多接口把失败写在返回体里（`{"code": 500}`）。
/// `pointer` 空表示没配；`equals` 可以写几个，用 `|` 分开（`0|200`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiSuccess {
    pub(crate) pointer: String,
    pub(crate) equals: String,
}

impl ApiSuccess {
    pub(crate) fn is_set(&self) -> bool {
        !self.pointer.trim().is_empty()
    }
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

/// 一组试调用的输入（「试一下」里一键填入、「全部试一遍」逐个发）。
/// 只是测试数据，不影响请求与返回的配置，不算进配置指纹。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiExample {
    pub(crate) name: String,
    /// 这个例子演示什么。
    pub(crate) note: String,
    /// 参数名 → 值（JSON 参数写 JSON 文本）。
    pub(crate) args: BTreeMap<String, String>,
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
    pub(crate) success: ApiSuccess,
    pub(crate) destination: ApiDestination,
    pub(crate) timeout_seconds: u64,
    /// 试调样例。
    pub(crate) examples: Vec<ApiExample>,
    #[serde(skip_serializing_if = "is_json_body")]
    pub(crate) body_kind: BodyKind,
    /// 只查询、不改变外部系统状态。只有只查询的接口给 AI 调，也只有它们自动试调。
    pub(crate) readonly: bool,
    /// 开放给 AI（前提是只查询）。
    pub(crate) ai: bool,
    /// 现在还发不了的原因（multipart 表单之类）；非空时 [`prepare`] 拒绝。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub(crate) unsupported: String,
    /// 从哪份 OpenAPI 的哪个操作编译来的（写回时用）；手填的新接口为空。
    #[serde(skip)]
    pub(crate) origin: Option<OpOrigin>,
}

fn is_json_body(kind: &BodyKind) -> bool {
    *kind == BodyKind::Json
}

/// 接口在 OpenAPI 里的位置与原文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OpOrigin {
    pub(crate) service: String,
    pub(crate) path: String,
    pub(crate) method: ApiMethod,
    /// 操作对象原文：没建模的字段（标签、返回结构、外部文档……）写回时原样保留。
    pub(crate) op: Value,
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
            success: ApiSuccess::default(),
            destination: ApiDestination::Evidence,
            timeout_seconds: DEFAULT_TIMEOUT,
            examples: Vec::new(),
            body_kind: BodyKind::Json,
            readonly: true,
            ai: true,
            unsupported: String::new(),
            origin: None,
        }
    }
}

/// 全部接口：OpenAPI 服务文件 + 编译出来的接口清单。
///
/// `endpoints` 是界面与技能用的平铺视图；`services`、`envs` 是落盘的真身。读的时候从
/// OpenAPI 编译出 `endpoints`，存的时候把改过的接口写回 OpenAPI（[`crate::agent::apidef::absorb`]）。
/// 序列化只带 `endpoints`，是旧版 `apis.json` 与「导入配置」的格式。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiStore {
    pub(crate) endpoints: Vec<ApiEndpoint>,
    #[serde(skip)]
    pub(crate) services: Vec<crate::agent::apidef::Service>,
    #[serde(skip)]
    pub(crate) envs: crate::agent::apidef::EnvStore,
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
    /// 读 `apis/` 下的 OpenAPI 并编译；只有旧版 `apis.json` 时先迁移。
    pub(crate) fn load() -> anyhow::Result<Self> {
        crate::agent::apidef::load_store()
    }

    /// 把改过的接口写回 OpenAPI 并落盘；`endpoints` 随之换成重新编译的结果（顺序不变）。
    pub(crate) fn save(&mut self) -> anyhow::Result<()> {
        crate::agent::apidef::save_store(self)
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

/// 一个接口最近一次实测的结论。只记结论，不存返回内容与参数。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TestRecord {
    pub(crate) ok: bool,
    /// 本地时间「MM-DD HH:MM」。
    pub(crate) at: String,
    pub(crate) summary: String,
    /// 测试时的配置指纹，配置改过就对不上。
    pub(crate) fingerprint: String,
    /// 失败像是鉴权没过（密钥不对、过期、没带）。
    pub(crate) auth: bool,
}

/// 实测记录（`api-tests.json`）：接口 id → 最近一次结论。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApiTestLog {
    pub(crate) records: BTreeMap<String, TestRecord>,
}

/// 列表上显示的测试状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestStatus<'a> {
    Untested,
    Passed(&'a TestRecord),
    Failed(&'a TestRecord),
    /// 测过，但之后改了会影响请求或返回的配置。
    Stale(&'a TestRecord),
}

impl ApiTestLog {
    pub(crate) fn load() -> anyhow::Result<Self> {
        read_json("api-tests.json")
    }

    pub(crate) fn save(&self) -> anyhow::Result<()> {
        write_json("api-tests.json", self)
    }

    pub(crate) fn record(&mut self, endpoint: &ApiEndpoint, trial: &Trial) {
        self.records.insert(
            endpoint.id.clone(),
            TestRecord {
                ok: trial.ok(),
                at: chrono::Local::now().format("%m-%d %H:%M").to_string(),
                summary: trial.summary(),
                fingerprint: endpoint.fingerprint(),
                auth: trial.auth_rejected(),
            },
        );
    }

    pub(crate) fn status(&self, endpoint: &ApiEndpoint) -> TestStatus<'_> {
        match self.records.get(&endpoint.id) {
            None => TestStatus::Untested,
            Some(record) if record.fingerprint != endpoint.fingerprint() => {
                TestStatus::Stale(record)
            }
            Some(record) if record.ok => TestStatus::Passed(record),
            Some(record) => TestStatus::Failed(record),
        }
    }
}

impl ApiSecrets {
    pub(crate) fn load() -> anyhow::Result<Self> {
        read_json("api-secrets.json")
    }

    pub(crate) fn save(&self) -> anyhow::Result<()> {
        write_json("api-secrets.json", self)
    }

    /// 填了值没有（空白不算）。
    pub(crate) fn filled(&self, name: &str) -> bool {
        self.secrets.get(name).is_some_and(|v| !v.trim().is_empty())
    }

    /// 接口引用了、但还没填值的密钥。
    pub(crate) fn missing(&self, endpoint: &ApiEndpoint) -> Vec<String> {
        endpoint
            .secret_names()
            .into_iter()
            .filter(|name| !self.filled(name))
            .collect()
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
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            problems.push("id 只能用英文字母、数字、下划线、短横线和点".into());
        }
        if !self.unsupported.trim().is_empty() {
            problems.push(self.unsupported.trim().to_string());
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
        if self.method.has_body() && !self.body.trim().is_empty() {
            match serde_json::from_str::<Value>(&self.body) {
                Err(_) => problems.push("请求体模板不是合法的 JSON".into()),
                Ok(body) if self.body_kind == BodyKind::Form && !body.is_object() => {
                    problems.push("表单请求体的模板要是一个对象：{\"键\": \"{变量}\"}".into())
                }
                Ok(_) => {}
            }
        }
        if !self.mapping.list.is_empty() && !self.mapping.list.starts_with('/') {
            problems.push("列表位置要写成 JSON 指针，以 / 开头，例如 /data/items".into());
        }
        if self.success.is_set() {
            if !self.success.pointer.trim().starts_with('/') {
                problems.push("成功判据的位置要写成 JSON 指针，以 / 开头，例如 /code".into());
            }
            if self.success.equals.trim().is_empty() {
                problems.push("成功判据要写成功时的取值，例如 0".into());
            }
        }
        if !(1..=300).contains(&self.timeout_seconds) {
            problems.push("超时要在 1 到 300 秒之间".into());
        }
        problems.dedup();
        problems
    }

    /// 配置指纹：只算影响请求与返回的部分（名称、说明、参数说明、样例改了不算）。
    pub(crate) fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut core = self.clone();
        core.id.clear();
        core.name.clear();
        core.description.clear();
        core.examples.clear();
        core.readonly = true;
        core.ai = true;
        for input in &mut core.inputs {
            input.description.clear();
            input.example.clear();
        }
        let text = serde_json::to_string(&core).unwrap_or_default();
        Sha256::digest(text.as_bytes())[..8]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// 模板里引用的密钥名，按出现先后、去重。
    pub(crate) fn secret_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        let mut templates: Vec<&str> = vec![&self.url, &self.body];
        templates.extend(self.headers.iter().map(|h| h.value.as_str()));
        for template in templates {
            for caps in PLACEHOLDER.captures_iter(template) {
                if caps.get(1).is_some() && !names.iter().any(|n| n == &caps[2]) {
                    names.push(caps[2].to_string());
                }
            }
        }
        names
    }

    /// 测试用的样例输入。
    pub(crate) fn example_args(&self) -> Map<String, Value> {
        self.inputs
            .iter()
            .filter(|input| !input.example.is_empty())
            .map(|input| (input.name.clone(), Value::String(input.example.clone())))
            .collect()
    }

    /// 自动试调用的输入：有样例组用第一组（缺的参数拿参数自己的样例补），没有就用参数样例。
    pub(crate) fn trial_args(&self) -> Map<String, Value> {
        let mut args = self.example_args();
        if let Some(example) = self.examples.first() {
            for (name, value) in &example.args {
                if self.inputs.iter().any(|i| i.name == *name) && !value.trim().is_empty() {
                    args.insert(name.clone(), Value::String(value.clone()));
                }
            }
        }
        args
    }

    /// 一组样例的输入（缺的参数拿参数自己的样例补）。
    pub(crate) fn args_of(&self, example: &ApiExample) -> Map<String, Value> {
        let mut args = self.example_args();
        for (name, value) in &example.args {
            if self.inputs.iter().any(|i| i.name == *name) {
                args.insert(name.clone(), Value::String(value.clone()));
            }
        }
        args
    }
}

/// 组好的请求。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Prepared {
    pub(crate) method: ApiMethod,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Option<Value>,
    /// 请求体按表单（`application/x-www-form-urlencoded`）发，不是 JSON。
    pub(crate) form: bool,
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
            if self.form {
                text.push_str(&form_encode(body));
            } else {
                text.push_str(&serde_json::to_string_pretty(body).unwrap_or_default());
            }
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
            InputKind::Json => match raw {
                Value::String(text) => serde_json::from_str::<Value>(text.trim())
                    .map_err(|e| format!("输入 {} 要是 JSON：{e}", input.name))?,
                other => other.clone(),
            },
        };
        values.insert(input.name.clone(), value);
    }
    Ok(values)
}

/// 地址里的值按 RFC 3986 编码：只留字母数字与 `-._~`，其余按 UTF-8 字节转成 `%XX`。
pub(crate) fn percent_encode(text: &str) -> String {
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
            match secrets.secrets.get(name).filter(|s| !s.trim().is_empty()) {
                Some(secret) => {
                    used.push(secret.trim().to_string());
                    secret.trim().to_string()
                }
                None => {
                    error = Some(format!(
                        "密钥「{name}」还没有填，在接口详情的「鉴权」里粘贴"
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

/// 整段模板就是一个输入变量（`{region}`，不是密钥）时返回变量名。
pub(crate) fn sole_input(template: &str) -> Option<&str> {
    let caps = PLACEHOLDER.captures(template)?;
    (caps[0].len() == template.len() && caps.get(1).is_none())
        .then(|| caps.get(2).map(|m| m.as_str()))
        .flatten()
}

/// 整段模板就是一个密钥（`{secret:token}`）时返回密钥名。
pub(crate) fn sole_secret(template: &str) -> Option<&str> {
    let caps = PLACEHOLDER.captures(template)?;
    (caps[0].len() == template.len() && caps.get(1).is_some())
        .then(|| caps.get(2).map(|m| m.as_str()))
        .flatten()
}

/// 模板里有没有占位。
pub(crate) fn has_placeholder(template: &str) -> bool {
    PLACEHOLDER.is_match(template)
}

/// 没给的可选变量：整段只是这个变量的查询参数、请求头、请求体字段整个不发。
fn absent(values: &BTreeMap<String, Value>, template: &str) -> bool {
    sole_input(template).is_some_and(|name| matches!(values.get(name), None | Some(Value::Null)))
}

/// 值在地址、表单里的文字。
fn plain_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 组地址：`?` 后面逐个参数处理——没给的可选参数不发，数组按 OpenAPI 默认逐个重复
/// （`ids=1&ids=2`），输入标了 `comma` 的连成一个（`ids=1,2`）。
fn fill_url(
    endpoint: &ApiEndpoint,
    values: &BTreeMap<String, Value>,
    secrets: &ApiSecrets,
    used: &mut Vec<String>,
) -> Result<String, String> {
    let template = endpoint.url.trim();
    let (base, query) = match template.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (template, None),
    };
    let mut url = fill(base, values, secrets, true, used)?;
    let mut pairs = Vec::new();
    for pair in query.unwrap_or_default().split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if absent(values, value) {
            continue;
        }
        if let Some(name) = sole_input(value)
            && let Some(Value::Array(items)) = values.get(name)
        {
            let texts: Vec<String> = items
                .iter()
                .map(|item| percent_encode(&plain_text(item)))
                .collect();
            let comma = endpoint
                .inputs
                .iter()
                .any(|input| input.name == name && input.comma);
            if comma {
                pairs.push(format!("{key}={}", texts.join(",")));
            } else {
                pairs.extend(texts.iter().map(|text| format!("{key}={text}")));
            }
            continue;
        }
        pairs.push(fill(pair, values, secrets, true, used)?);
    }
    if !pairs.is_empty() {
        url.push('?');
        url.push_str(&pairs.join("&"));
    }
    Ok(url)
}

/// 请求体模板里逐个字符串替换：整串只有一个 `{变量}` 时换成带类型的值；对象里这样的
/// 字段变量没给时整个不发。
fn fill_body(
    template: &Value,
    values: &BTreeMap<String, Value>,
    secrets: &ApiSecrets,
    used: &mut Vec<String>,
) -> Result<Value, String> {
    Ok(match template {
        Value::String(text) => match sole_input(text) {
            Some(name) => values.get(name).cloned().unwrap_or(Value::Null),
            None => Value::String(fill(text, values, secrets, false, used)?),
        },
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| fill_body(item, values, secrets, used))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(_, item)| !item.as_str().is_some_and(|text| absent(values, text)))
                .map(|(key, item)| Ok((key.clone(), fill_body(item, values, secrets, used)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

/// 表单请求体：对象逐项编成 `键=值`，数组逐个重复，空值不发。
pub(crate) fn form_encode(body: &Value) -> String {
    let mut pairs = Vec::new();
    if let Value::Object(map) = body {
        for (key, value) in map {
            let key = percent_encode(key);
            match value {
                Value::Null => {}
                Value::Array(items) => pairs.extend(
                    items
                        .iter()
                        .map(|item| format!("{key}={}", percent_encode(&plain_text(item)))),
                ),
                other => pairs.push(format!("{key}={}", percent_encode(&plain_text(other)))),
            }
        }
    }
    pairs.join("&")
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
    let url = fill_url(endpoint, &values, secrets, &mut used)?;
    let headers = endpoint
        .headers
        .iter()
        .filter(|header| !header.name.trim().is_empty() && !absent(&values, &header.value))
        .map(|header| {
            Ok((
                header.name.trim().to_string(),
                fill(&header.value, &values, secrets, false, &mut used)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let body = if endpoint.method.has_body() && !endpoint.body.trim().is_empty() {
        let template: Value = serde_json::from_str(&endpoint.body)
            .map_err(|e| format!("请求体模板不是 JSON：{e}"))?;
        Some(fill_body(&template, &values, secrets, &mut used)?).filter(|body| !body.is_null())
    } else {
        None
    };
    Ok(Prepared {
        method: endpoint.method,
        url,
        headers,
        body,
        form: endpoint.body_kind == BodyKind::Form,
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
    let raw = send(prepared, timeout_seconds)?;
    check_status(&raw)?;
    Ok(raw)
}

fn check_status(raw: &RawResponse) -> Result<(), String> {
    if (200..300).contains(&raw.status) {
        Ok(())
    } else {
        Err(format!(
            "接口返回 {}：{}",
            raw.status,
            crate::agent::tools::short(raw.body.trim(), 200)
        ))
    }
}

/// 发出请求，读回返回体，不管状态码（实测要把出错时的返回也给人看）。
fn send(prepared: &Prepared, timeout_seconds: u64) -> Result<RawResponse, String> {
    let client =
        crate::net::client(&prepared.url, timeout_seconds).map_err(|e| format!("{e:#}"))?;
    let method = reqwest::Method::from_bytes(prepared.method.label().as_bytes())
        .map_err(|e| e.to_string())?;
    let mut request = client.request(method, &prepared.url);
    for (name, value) in &prepared.headers {
        request = request.header(name, value);
    }
    match &prepared.body {
        Some(body) if prepared.form => {
            request = request
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(form_encode(body));
        }
        Some(body) => request = request.json(body),
        None => {}
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
    let truncated = bytes.len() as u64 >= MAX_RESPONSE_BYTES;
    let body = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        return Err(format!(
            "返回超过 {} MB，读不完整，没有采用；请缩小查询条件",
            MAX_RESPONSE_BYTES / 1024 / 1024
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

/// 返回体像不像网页：没登录时很多内网系统回一张登录页，状态码照样是 200。
pub(crate) fn looks_like_html(body: &str) -> bool {
    let head: String = body
        .trim_start()
        .chars()
        .take(64)
        .collect::<String>()
        .to_ascii_lowercase();
    head.starts_with("<!doctype html") || head.starts_with("<html") || head.starts_with("<head")
}

/// 判据比较时的取值：真假写成 true / false，不用「是 / 否」，和文档里的写法对得上。
fn success_text(value: &Value) -> String {
    match value {
        Value::Bool(flag) => flag.to_string(),
        other => value_to_text(other).trim().to_string(),
    }
}

/// 按业务成功判据检查返回体。没配判据时不查。
pub(crate) fn check_success(endpoint: &ApiEndpoint, body: &str) -> Result<(), String> {
    let success = &endpoint.success;
    if !success.is_set() {
        return Ok(());
    }
    let root: Value = serde_json::from_str(body)
        .map_err(|_| "返回的不是 JSON，没法按成功判据检查".to_string())?;
    let pointer = success.pointer.trim();
    let actual = root
        .pointer(pointer)
        .map(success_text)
        .ok_or_else(|| format!("返回里没有成功判据的字段 {pointer}"))?;
    if success
        .equals
        .split('|')
        .map(str::trim)
        .any(|expected| expected == actual)
    {
        return Ok(());
    }
    let reason = [
        "/msg",
        "/message",
        "/error",
        "/errmsg",
        "/error_msg",
        "/detail",
    ]
    .iter()
    .find_map(|key| root.pointer(key).map(value_to_text))
    .filter(|text| !text.trim().is_empty())
    .map(|text| format!("：{}", crate::agent::tools::short(text.trim(), 120)))
    .unwrap_or_default();
    Err(format!(
        "接口报告查询失败（{pointer} 是 {actual}，成功应为 {}）{reason}",
        success.equals.trim()
    ))
}

/// 按映射把返回体整理成条目。
#[cfg(test)]
pub(crate) fn map_response(endpoint: &ApiEndpoint, body: &str) -> Result<Vec<MappedItem>, String> {
    map_counted(endpoint, body).map(|(items, _)| items)
}

/// 同 [`map_response`]，另返回列表原有多少条（超过 [`MAX_ITEMS`] 的部分没有收）。
pub(crate) fn map_counted(
    endpoint: &ApiEndpoint,
    body: &str,
) -> Result<(Vec<MappedItem>, usize), String> {
    if looks_like_html(body) {
        return Err("返回的是一张网页（多半是登录页或报错页），不是数据；检查地址与鉴权".into());
    }
    let mapping = &endpoint.mapping;
    let root: Value = match serde_json::from_str(body) {
        Ok(root) => root,
        Err(_) if mapping.list.is_empty() => {
            // 不是 JSON（纯文本、CSV……）又没要求取列表：整段当一条。
            let item = MappedItem {
                id: "1".into(),
                title: endpoint.name.clone(),
                text: truncate(body.trim(), MAX_ITEM_CHARS),
                source: endpoint.name.clone(),
            };
            return Ok((vec![item], 1));
        }
        Err(error) => return Err(format!("返回的不是 JSON：{error}")),
    };
    let target = if mapping.list.is_empty() {
        &root
    } else {
        root.pointer(&mapping.list)
            .ok_or_else(|| format!("返回里找不到列表位置 {}", mapping.list))?
    };
    // 列表位置指到一张「键 → 对象」的表（按调用方起的名字返回的答案之类）：每一项算一条，
    // 键当标题与编号。
    let keyed = !mapping.list.is_empty()
        && target
            .as_object()
            .is_some_and(|map| !map.is_empty() && map.values().all(Value::is_object));
    let items: Vec<(Option<&String>, &Value)> = match target {
        Value::Array(items) => items.iter().map(|item| (None, item)).collect(),
        Value::Object(map) if keyed => map.iter().map(|(key, item)| (Some(key), item)).collect(),
        Value::Null => Vec::new(),
        other => vec![(None, other)],
    };
    let total = items.len();
    let items = items
        .into_iter()
        .take(MAX_ITEMS)
        .enumerate()
        .map(|(index, (key, item))| {
            let title = pick(item, &mapping.title)
                .or_else(|| guess(item, &["title", "name", "标题", "名称"]))
                .or_else(|| key.cloned())
                .unwrap_or_default();
            let text = pick(item, &mapping.text)
                .or_else(|| guess(item, &["text", "content", "summary", "正文", "内容"]))
                .unwrap_or_else(|| value_to_text(item));
            let source = pick(item, &mapping.source)
                .or_else(|| guess(item, &["source", "url", "出处", "来源"]))
                .unwrap_or_else(|| endpoint.name.clone());
            let id = pick(item, &mapping.id)
                .or_else(|| guess(item, &["id", "ID", "编号"]))
                .or_else(|| key.cloned())
                .unwrap_or_else(|| (index + 1).to_string());
            MappedItem {
                id: truncate(id.trim(), 80),
                title: truncate(title.trim(), 200),
                text: truncate(text.trim(), MAX_ITEM_CHARS),
                source: truncate(source.trim(), 200),
            }
        })
        .collect();
    Ok((items, total))
}

/// 一次调用的结果。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Called {
    pub(crate) prepared: Prepared,
    pub(crate) raw: RawResponse,
    pub(crate) items: Vec<MappedItem>,
    /// 列表原有多少条；比 `items` 多说明截掉了。
    pub(crate) total: usize,
}

/// 组请求、发出去、查业务成功、映射，一步到位。
pub(crate) fn call(
    endpoint: &ApiEndpoint,
    args: &Map<String, Value>,
    secrets: &ApiSecrets,
) -> Result<Called, String> {
    let prepared = prepare(endpoint, args, secrets)?;
    let raw = execute(&prepared, endpoint.timeout_seconds)?;
    check_success(endpoint, &raw.body)?;
    let (items, total) = map_counted(endpoint, &raw.body)?;
    Ok(Called {
        prepared,
        raw,
        items,
        total,
    })
}

/// 实测的分阶段结果：走到哪一步、在哪一步出的错。测试面板与自动填报用，和正式调用
/// 走同一套「组请求 → 发出 → 状态 → 成功判据 → 映射」，只是出错时把已拿到的东西留着给人看。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Trial {
    /// 给人看的请求（密钥打码）。
    pub(crate) request: Option<String>,
    pub(crate) raw: Option<RawResponse>,
    pub(crate) items: Vec<MappedItem>,
    pub(crate) total: usize,
    pub(crate) error: Option<String>,
}

impl Trial {
    pub(crate) fn ok(&self) -> bool {
        self.error.is_none()
    }

    /// 一句话结论，列表卡片与测试记录用。
    /// 配置改了（实测补了映射之类）以后，用已拿到的返回重新检查、映射，不再发请求。
    pub(crate) fn remap(&mut self, endpoint: &ApiEndpoint) {
        let Some(raw) = &self.raw else {
            return;
        };
        match check_status(raw)
            .and_then(|_| check_success(endpoint, &raw.body))
            .and_then(|_| map_counted(endpoint, &raw.body))
        {
            Ok((items, total)) => {
                self.items = items;
                self.total = total;
                self.error = None;
            }
            Err(error) => {
                self.items.clear();
                self.total = 0;
                self.error = Some(error);
            }
        }
    }

    /// 失败像是鉴权没过：401 / 403、回了登录页、返回体说未授权或令牌无效。
    /// 只看发出去以后的失败；密钥没填（没发出去）不算。
    pub(crate) fn auth_rejected(&self) -> bool {
        let Some(raw) = &self.raw else {
            return false;
        };
        if self.error.is_none() {
            return false;
        }
        if matches!(raw.status, 401 | 403 | 407) {
            return true;
        }
        let head: String = raw
            .body
            .chars()
            .take(2000)
            .collect::<String>()
            .to_lowercase();
        if looks_like_html(&raw.body) {
            // 200 回一张网页多半是登录页；500 的报错页不算。
            return (200..400).contains(&raw.status)
                || head.contains("登录")
                || head.contains("login");
        }
        if matches!(raw.status, 404 | 405 | 415) {
            return false;
        }
        if let Ok(root) = serde_json::from_str::<Value>(&head) {
            let code = ["/code", "/status", "/errcode", "/error_code"]
                .iter()
                .find_map(|key| root.pointer(key))
                .map(|v| value_to_text(v).trim().to_string());
            if matches!(code.as_deref(), Some("401" | "403")) {
                return true;
            }
        }
        [
            "unauthorized",
            "unauthorised",
            "forbidden",
            "access denied",
            "invalid token",
            "token",
            "令牌",
            "未登录",
            "请登录",
            "登录失效",
            "未授权",
            "无权",
            "鉴权",
            "认证失败",
            "签名",
            "signature",
            "apikey",
            "api key",
            "api_key",
            "密钥",
        ]
        .iter()
        .any(|word| head.contains(word))
    }

    pub(crate) fn summary(&self) -> String {
        match &self.error {
            Some(error) => crate::agent::tools::short(error, 80),
            None if self.total > self.items.len() => {
                format!("取到 {} 条（共 {} 条）", self.items.len(), self.total)
            }
            None => format!("取到 {} 条", self.items.len()),
        }
    }
}

/// 实测一次。
pub(crate) fn trial(
    endpoint: &ApiEndpoint,
    args: &Map<String, Value>,
    secrets: &ApiSecrets,
) -> Trial {
    trial_via(endpoint, args, secrets, send)
}

/// 同 [`trial`]，发请求这一步可以换掉（测试里不走网络）。
pub(crate) fn trial_via(
    endpoint: &ApiEndpoint,
    args: &Map<String, Value>,
    secrets: &ApiSecrets,
    send: impl FnOnce(&Prepared, u64) -> Result<RawResponse, String>,
) -> Trial {
    let mut trial = Trial::default();
    if !endpoint.readonly {
        trial.error = Some(
            "这个接口会改数据，不自动试调。只是查询的，在「技术配置」里勾上「只查询」；             需要真的发出去的，等调试台（下一期）里二次确认后由人手动发"
                .into(),
        );
        return trial;
    }
    let prepared = match prepare(endpoint, args, secrets) {
        Ok(prepared) => prepared,
        Err(error) => {
            trial.error = Some(error);
            return trial;
        }
    };
    trial.request = Some(prepared.describe());
    let raw = match send(&prepared, endpoint.timeout_seconds) {
        Ok(raw) => raw,
        Err(error) => {
            trial.error = Some(error);
            return trial;
        }
    };
    let checked = check_status(&raw)
        .and_then(|_| check_success(endpoint, &raw.body))
        .and_then(|_| map_counted(endpoint, &raw.body));
    trial.raw = Some(raw);
    match checked {
        Ok((items, total)) => {
            trial.items = items;
            trial.total = total;
        }
        Err(error) => trial.error = Some(error),
    }
    trial
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
        let Called {
            prepared,
            raw,
            items,
            total,
        } = call(&endpoint, args.as_object().unwrap(), &secrets()).unwrap();
        assert_eq!((raw.status, total), (200, 1));
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
    fn business_failures_and_login_pages_are_not_success() {
        let mut endpoint = stat_endpoint("http://x");
        endpoint.success = ApiSuccess {
            pointer: "/code".into(),
            equals: "0|200".into(),
        };
        assert!(endpoint.problems().is_empty());
        assert!(check_success(&endpoint, r#"{"code": 0}"#).is_ok());
        assert!(check_success(&endpoint, r#"{"code": "200"}"#).is_ok());
        let error = check_success(&endpoint, r#"{"code": 401, "msg": "令牌过期"}"#).unwrap_err();
        assert!(
            error.contains("/code 是 401") && error.contains("令牌过期"),
            "{error}"
        );
        let error = check_success(&endpoint, r#"{"data": []}"#).unwrap_err();
        assert!(error.contains("没有成功判据的字段 /code"), "{error}");
        endpoint.success.equals = "true".into();
        endpoint.success.pointer = "/ok".into();
        assert!(
            check_success(&endpoint, r#"{"ok": true}"#).is_ok(),
            "真假按 true 比"
        );

        let plain = ApiEndpoint::default();
        let error = map_response(&plain, "<!DOCTYPE html><html>请登录</html>").unwrap_err();
        assert!(error.contains("网页"), "{error}");

        endpoint.success.pointer = "code".into();
        assert!(endpoint.problems().iter().any(|p| p.contains("成功判据")));
    }

    #[test]
    fn test_records_go_stale_when_the_request_changes() {
        let mut endpoint = stat_endpoint("http://x");
        let mut log = ApiTestLog::default();
        assert_eq!(log.status(&endpoint), TestStatus::Untested);
        let passed = Trial {
            items: vec![],
            ..Trial::default()
        };
        log.record(&endpoint, &passed);
        assert!(matches!(log.status(&endpoint), TestStatus::Passed(r) if r.summary == "取到 0 条"));
        endpoint.description = "改说明不影响".into();
        endpoint.inputs[0].example = "甲市".into();
        assert!(matches!(log.status(&endpoint), TestStatus::Passed(_)));
        endpoint.mapping.list = "/rows".into();
        assert!(matches!(log.status(&endpoint), TestStatus::Stale(_)));
        let failed = Trial {
            error: Some("接口返回 500".into()),
            ..Trial::default()
        };
        log.record(&endpoint, &failed);
        assert!(
            matches!(log.status(&endpoint), TestStatus::Failed(r) if r.summary == "接口返回 500")
        );

        let mut trial = Trial {
            raw: Some(RawResponse {
                status: 200,
                body: json!({"rows": [{"id": 1}, {"id": 2}]}).to_string(),
            }),
            error: Some("旧错误".into()),
            ..Trial::default()
        };
        trial.remap(&endpoint);
        assert!(trial.ok());
        assert_eq!(trial.items.len(), 2);
    }

    #[test]
    fn long_lists_report_how_many_were_dropped() {
        let items: Vec<Value> = (0..80).map(|i| json!({"id": i, "title": "条"})).collect();
        let body = json!({"items": items}).to_string();
        let endpoint = ApiEndpoint {
            mapping: ApiMapping {
                list: "/items".into(),
                ..ApiMapping::default()
            },
            ..ApiEndpoint::default()
        };
        let (items, total) = map_counted(&endpoint, &body).unwrap();
        assert_eq!((items.len(), total), (MAX_ITEMS, 80));
    }

    #[test]
    fn a_trial_keeps_the_reply_when_a_later_stage_fails() {
        let server = TestServer::start(vec![
            (200, json!({"code": 500, "msg": "无权限"}).to_string()),
            (403, "禁止访问".into()),
            (
                200,
                json!({"code": 0, "data": {"items": [{"id": 1}]}}).to_string(),
            ),
        ]);
        let mut endpoint = stat_endpoint(&server.url);
        endpoint.success = ApiSuccess {
            pointer: "/code".into(),
            equals: "0".into(),
        };
        let args = json!({"region": "全省"});
        let args = args.as_object().unwrap();
        let first = trial(&endpoint, args, &secrets());
        assert!(!first.ok());
        assert!(first.error.as_deref().unwrap().contains("无权限"));
        assert!(
            first.raw.as_ref().unwrap().body.contains("无权限"),
            "返回留着给人看"
        );
        assert!(first.request.as_deref().unwrap().contains("******"));
        let second = trial(&endpoint, args, &secrets());
        assert_eq!(second.raw.as_ref().unwrap().status, 403);
        assert!(second.error.as_deref().unwrap().contains("接口返回 403"));
        let third = trial(&endpoint, args, &secrets());
        assert!(third.ok(), "{:?}", third.error);
        assert_eq!(third.summary(), "取到 1 条");

        let missing = trial(&endpoint, &Map::new(), &secrets());
        assert!(missing.request.is_none() && missing.error.unwrap().contains("缺少输入"));
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
            ..Default::default()
        };
        store.save().unwrap();
        secrets().save().unwrap();
        assert_eq!(ApiStore::load().unwrap(), store);
        assert_eq!(ApiSecrets::load().unwrap(), secrets());
        assert!(!dir.join("apis.json").exists(), "不再写旧版接口文件");
        let files: Vec<_> = std::fs::read_dir(dir.join("apis"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(files.len(), 1, "一台服务器一份 OpenAPI");
        let exported = std::fs::read_to_string(files[0].path()).unwrap();
        assert!(exported.contains("\"openapi\": \"3.0.3\""));
        assert!(!exported.contains("s3cr3t"), "接口文件里不带密钥");
        crate::storage::set_test_config_dir(None);

        let mut renamed = stat_endpoint("http://y");
        renamed.name = "改过的".into();
        let mut other = stat_endpoint("http://z");
        other.id = "other".into();
        assert_eq!(
            store.merge(ApiStore {
                endpoints: vec![renamed, other],
                ..Default::default()
            }),
            (1, 1)
        );
        assert_eq!(store.get("fire_stat").unwrap().name, "改过的");
        let _ = std::fs::remove_dir_all(dir);
    }
}
