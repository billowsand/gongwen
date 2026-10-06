//! 标准接口文件：以 OpenAPI 3.0 为中心的数据接口（`docs/api-workbench.md`）。
//!
//! - 一个服务（一个内网系统）一份 OpenAPI，存在 `配置目录/apis/<服务 id>.openapi.json`；
//!   我们自己要的东西（只查询、业务成功判据、证据映射、测试用例……）放 `x-gongwen-*` 扩展；
//! - 实际发请求的服务器地址来自环境（`api-envs.json`），不写进文件；密钥仍在
//!   `api-secrets.json`，文件里只由 `x-gongwen-secret` 指过去；
//! - 运行时把每个操作编译成 [`ApiEndpoint`]（[`lower`]），界面、技能、`http.call` 都用它；
//!   界面改过的接口存盘时再写回 OpenAPI（[`raise::absorb`]），没改的操作原样不动；
//! - 导入（[`import`]）：OpenAPI 3.0 / 3.1、Swagger 2.0、Postman 集合与环境、cURL、旧版
//!   `apis.json`，都是确定性转换，不用模型。
//!
//! 第一次读的时候，只有旧版 `apis.json` 就按服务器分组迁成 OpenAPI，原文件改名 `.bak` 留着。

pub(crate) mod import;
mod lower;
mod normalize;
mod postman;
mod raise;
mod schema;
#[cfg(test)]
mod tests;

use crate::agent::api::{ApiEndpoint, ApiMethod, ApiStore};
pub(crate) use lower::base_url;
pub(crate) use raise::absorb;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 写出的 OpenAPI 版本。
pub(crate) const OPENAPI_VERSION: &str = "3.0.3";
/// 服务文件的后缀。
const SERVICE_SUFFIX: &str = ".openapi.json";

/// 读写服务文件时串行：后台技能任务与界面可能同时读，第一次读还可能触发迁移。
static IO_LOCK: Mutex<()> = Mutex::new(());

/// 一个服务：一份 OpenAPI 文档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Service {
    /// 文件名用的 id：英文字母、数字、短横线。
    pub(crate) id: String,
    pub(crate) doc: Value,
}

impl Service {
    /// 显示用的名字：`info.title`，没有就用 id。
    pub(crate) fn title(&self) -> String {
        self.doc
            .pointer("/info/title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map_or_else(|| self.id.clone(), str::to_string)
    }

    /// 操作的 (路径, 方法)，按路径、方法排好。
    pub(crate) fn operations(&self) -> Vec<(String, ApiMethod)> {
        let mut found = Vec::new();
        if let Some(paths) = self.doc.get("paths").and_then(Value::as_object) {
            for (path, item) in paths {
                for method in ApiMethod::ALL {
                    if item.get(method.key()).is_some_and(Value::is_object) {
                        found.push((path.clone(), method));
                    }
                }
            }
        }
        found
    }
}

/// 一个环境：实际的服务器地址与变量。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Env {
    pub(crate) name: String,
    /// 空表示用文件里 `servers` 的第一个。
    pub(crate) base_url: String,
    /// 服务器地址里的变量（`{basePath}`）。
    pub(crate) variables: BTreeMap<String, String>,
}

/// 一个服务的几个环境与当前用哪个。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ServiceEnv {
    pub(crate) active: String,
    pub(crate) envs: Vec<Env>,
}

impl ServiceEnv {
    pub(crate) fn current(&self) -> Option<&Env> {
        self.envs
            .iter()
            .find(|env| env.name == self.active)
            .or_else(|| self.envs.first())
    }

    /// 把当前环境的服务器地址改成 `base`（没有环境就建一个「默认」）。
    pub(crate) fn set_base(&mut self, base: &str) {
        if self.envs.is_empty() {
            self.envs.push(Env {
                name: "默认".into(),
                ..Env::default()
            });
            self.active = "默认".into();
        }
        let active = self.active.clone();
        let env = match self.envs.iter_mut().position(|env| env.name == active) {
            Some(index) => &mut self.envs[index],
            None => &mut self.envs[0],
        };
        env.base_url = base.trim_end_matches('/').to_string();
    }
}

/// 全部服务的环境（`api-envs.json`）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct EnvStore {
    pub(crate) services: BTreeMap<String, ServiceEnv>,
}

impl EnvStore {
    pub(crate) fn get(&self, service: &str) -> Option<&ServiceEnv> {
        self.services.get(service)
    }
}

/// 一个接口的默认 id：`<服务>.<operationId>`。
pub(crate) fn default_id(service: &str, operation_id: &str) -> String {
    format!("{service}.{operation_id}")
}

/// 文件名、服务 id 用的写法：小写英文字母、数字、短横线。
pub(crate) fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').chars().take(48).collect()
}

/// 不和已有的重复：`x`、`x-2`、`x-3`……
pub(crate) fn unique(base: &str, taken: impl Fn(&str) -> bool, sep: &str) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}{sep}{n}"))
        .find(|candidate| !taken(candidate))
        .expect("总能找到不重复的名字")
}

/// 新服务的 id：优先用标题，标题没有英文就用服务器地址。
pub(crate) fn service_id(title: &str, base: &str, store: &ApiStore) -> String {
    let host = base.split_once("://").map_or(base, |(_, rest)| rest);
    let host = host.split('/').next().unwrap_or_default();
    let base_id = [slug(title), slug(host)]
        .into_iter()
        .find(|id| !id.is_empty())
        .unwrap_or_else(|| "api".into());
    unique(
        &base_id,
        |id| store.services.iter().any(|service| service.id == id),
        "-",
    )
}

/// 一份空的 OpenAPI 文档。
pub(crate) fn empty_doc(title: &str, base: &str) -> Value {
    let mut doc = json!({
        "openapi": OPENAPI_VERSION,
        "info": {"title": title, "version": "1.0"},
        "paths": {},
    });
    if !base.is_empty() {
        doc["servers"] = json!([{ "url": base }]);
    }
    doc
}

/// 编译全部服务。
pub(crate) fn lower_all(store: &ApiStore) -> Vec<ApiEndpoint> {
    store
        .services
        .iter()
        .flat_map(|service| lower::lower_service(service, store.envs.get(&service.id)))
        .collect()
}

fn services_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::storage::config_dir()?.join("apis"))
}

fn service_files(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let id = name.strip_suffix(SERVICE_SUFFIX)?.to_string();
            Some((id, path))
        })
        .collect();
    files.sort();
    files
}

/// 读全部接口。只有旧版 `apis.json` 时先迁移。
pub(crate) fn load_store() -> anyhow::Result<ApiStore> {
    let _guard = IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = services_dir()?;
    let files = service_files(&dir);
    let legacy = crate::storage::config_dir()?.join("apis.json");
    if files.is_empty() && legacy.exists() {
        return migrate(&legacy);
    }
    let mut store = ApiStore::default();
    for (id, path) in files {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("读取 {} 失败：{e}", path.display()))?;
        let doc: Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("{} 解析失败：{e}", path.display()))?;
        store.services.push(Service { id, doc });
    }
    store.envs = crate::agent::api::read_json("api-envs.json")?;
    store.endpoints = lower_all(&store);
    Ok(store)
}

/// 旧版 `apis.json` → OpenAPI：按服务器分组写成服务文件，原文件改名 `apis.json.bak`。
fn migrate(legacy: &Path) -> anyhow::Result<ApiStore> {
    let mut store: ApiStore = crate::agent::api::read_json("apis.json")?;
    absorb(&mut store);
    write_store(&store)?;
    let backup = legacy.with_extension("json.bak");
    std::fs::rename(legacy, &backup).map_err(|e| anyhow::anyhow!("旧接口文件改名失败：{e}"))?;
    Ok(store)
}

/// 写回改过的接口并落盘。
pub(crate) fn save_store(store: &mut ApiStore) -> anyhow::Result<()> {
    let _guard = IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    absorb(store);
    write_store(store)
}

fn write_store(store: &ApiStore) -> anyhow::Result<()> {
    let dir = services_dir()?;
    std::fs::create_dir_all(&dir)?;
    for service in &store.services {
        let path = dir.join(format!("{}{SERVICE_SUFFIX}", service.id));
        std::fs::write(&path, serde_json::to_string_pretty(&service.doc)?)?;
    }
    for (id, path) in service_files(&dir) {
        if !store.services.iter().any(|service| service.id == id) {
            std::fs::remove_file(&path)?;
        }
    }
    let mut envs = store.envs.clone();
    envs.services
        .retain(|id, _| store.services.iter().any(|service| service.id == *id));
    crate::agent::api::write_json("api-envs.json", &envs)
}

/// 导出：先把没存的修改写回一份副本，再按服务给出 (文件名, 文档)。环境与密钥不导出。
pub(crate) fn export(store: &ApiStore) -> Vec<(String, Value)> {
    let mut copy = store.clone();
    absorb(&mut copy);
    copy.services
        .into_iter()
        .map(|service| {
            let title = service.title();
            let name: String = title
                .chars()
                .map(|c| if r#"\/:*?"<>|"#.contains(c) { '_' } else { c })
                .collect();
            (format!("{name}{SERVICE_SUFFIX}"), service.doc)
        })
        .collect()
}

/// 文档写成文字：`yaml` 为真时写 YAML。
pub(crate) fn to_text(doc: &Value, yaml: bool) -> anyhow::Result<String> {
    Ok(if yaml {
        serde_yaml::to_string(doc)?
    } else {
        serde_json::to_string_pretty(doc)?
    })
}
