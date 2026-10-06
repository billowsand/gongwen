//! 导入标准接口文件：认格式、转成 OpenAPI 3.0、编译后并进接口清单，出一份导入报告。
//!
//! 支持 OpenAPI 3.0 / 3.1（JSON、YAML）、Swagger 2.0、Postman 集合（可同时选环境文件）、
//! cURL 命令、旧版 `apis.json`。全部是确定性转换，不用模型，不发请求。

use super::normalize::{self, Detected};
use super::{Service, ServiceEnv, base_url, lower, postman, service_id};
use crate::agent::api::{ApiEndpoint, ApiStore};
use crate::agent::api_import::{self, Access, Material};
use serde_json::Value;
use std::collections::BTreeMap;

/// 一次导入的结果。
#[derive(Debug, Default)]
pub(crate) struct ImportOutcome {
    /// 新加的接口数。
    pub(crate) added: usize,
    /// 提出来的凭据：密钥名 → 值（值可能为空，等人粘贴）。
    pub(crate) secrets: Vec<(String, String)>,
    /// 给人看的报告，一行一条。
    pub(crate) report: Vec<String>,
}

/// 导入几个文件：(文件名, 内容)。同时选的 Postman 环境文件给集合里的变量取值。
pub(crate) fn import_files(store: &mut ApiStore, files: &[(String, String)]) -> ImportOutcome {
    let mut outcome = ImportOutcome::default();
    let detected: Vec<(String, Detected)> = files
        .iter()
        .map(|(name, text)| (name.clone(), normalize::detect(text)))
        .collect();
    let mut env_vars = BTreeMap::new();
    for (name, found) in &detected {
        if let Detected::PostmanEnv(env) = found {
            let (env_name, vars) = postman::env_vars(env);
            outcome
                .report
                .push(format!("{name}：环境「{env_name}」，{} 个变量", vars.len()));
            env_vars.extend(vars);
        }
    }
    for ((name, found), (_, text)) in detected.into_iter().zip(files) {
        match found {
            Detected::OpenApi(doc) => {
                let mut notes = Vec::new();
                match normalize::to_openapi30(doc, &mut notes) {
                    Ok(doc) => add_service(store, doc, None, &name, notes, &mut outcome),
                    Err(error) => outcome.report.push(format!("{name}：{error}")),
                }
            }
            Detected::Postman(collection) => {
                let mut notes = Vec::new();
                let converted = postman::convert(&collection, &env_vars, &mut notes);
                outcome.secrets.extend(converted.secrets);
                let mut first = true;
                for service in converted.services {
                    let mut doc = service.doc;
                    let mut more = if first { std::mem::take(&mut notes) } else { Vec::new() };
                    first = false;
                    let mut normalize_notes = Vec::new();
                    doc = match normalize::to_openapi30(doc, &mut normalize_notes) {
                        Ok(doc) => doc,
                        Err(error) => {
                            outcome.report.push(format!("{name}：{error}"));
                            continue;
                        }
                    };
                    more.extend(normalize_notes);
                    add_service(store, doc, None, &name, more, &mut outcome);
                }
            }
            Detected::PostmanEnv(_) => {}
            Detected::Legacy(incoming) => {
                let count = incoming.endpoints.len();
                let (added, replaced) = store.merge(incoming);
                outcome.added += added;
                outcome.report.push(format!(
                    "{name}：旧版接口配置 {count} 个（新增 {added}、覆盖同 id 的 {replaced}），保存时转成 OpenAPI"
                ));
            }
            Detected::Curl => import_curl(store, &name, text, &mut outcome),
            Detected::Unknown => outcome.report.push(format!(
                "{name}：认不出格式。支持 OpenAPI / Swagger（JSON、YAML）、Postman 集合与环境、cURL；Word、PDF 这类文档请用「从文档识别」"
            )),
        }
    }
    outcome
}

/// 加一个服务：起不重复的 id、编译、核对接口 id 不和已有的冲突，写报告。
fn add_service(
    store: &mut ApiStore,
    mut doc: Value,
    env: Option<ServiceEnv>,
    file: &str,
    notes: Vec<String>,
    outcome: &mut ImportOutcome,
) {
    let title = doc
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("接口")
        .to_string();
    let probe = Service {
        id: String::new(),
        doc: doc.clone(),
    };
    let base = base_url(&probe, None);
    let id = service_id(&title, &base, store);
    // 文件里写了本机 id（别处导出的）、又和这里已有的接口重名的，改用默认 id。
    let taken: Vec<String> = store.endpoints.iter().map(|e| e.id.clone()).collect();
    let mut renamed = 0;
    if let Some(paths) = doc.get_mut("paths").and_then(Value::as_object_mut) {
        for item in paths.values_mut() {
            for method in crate::agent::api::ApiMethod::ALL {
                if let Some(op) = item.get_mut(method.key()).and_then(Value::as_object_mut)
                    && op
                        .get(lower::X_ID)
                        .and_then(Value::as_str)
                        .is_some_and(|x| taken.iter().any(|t| t == x))
                {
                    op.remove(lower::X_ID);
                    renamed += 1;
                }
            }
        }
    }
    let service = Service {
        id: id.clone(),
        doc,
    };
    if let Some(env) = env {
        store.envs.services.insert(id.clone(), env);
    }
    let endpoints = lower::lower_service(&service, store.envs.get(&id));
    store.services.push(service);
    outcome.added += endpoints.len();
    outcome.report.push(summary_line(file, &title, &endpoints));
    if base.is_empty() {
        outcome.report.push(format!(
            "「{title}」没有服务器地址：保存后在接口地址里补上 http://主机:端口"
        ));
    }
    if renamed > 0 {
        outcome.report.push(format!(
            "「{title}」有 {renamed} 个接口的 id 和已有的重名，改用 {id}.<操作 id>"
        ));
    }
    outcome
        .report
        .extend(notes.into_iter().map(|note| format!("「{title}」：{note}")));
    store.endpoints.extend(endpoints);
}

/// 「文件：服务名，N 个接口；M 个会改数据……」
fn summary_line(file: &str, title: &str, endpoints: &[ApiEndpoint]) -> String {
    let writes: Vec<&ApiEndpoint> = endpoints.iter().filter(|e| !e.readonly).collect();
    let posts = writes
        .iter()
        .filter(|e| e.method == crate::agent::api::ApiMethod::Post)
        .count();
    let blocked: Vec<String> = endpoints
        .iter()
        .filter(|e| !e.unsupported.is_empty())
        .map(|e| format!("{}（{}）", e.name, e.unsupported))
        .collect();
    let mut line = format!("{file}：「{title}」{} 个接口", endpoints.len());
    if !writes.is_empty() {
        line.push_str(&format!(
            "；{} 个按会改数据处理，不给 AI 调、不自动试调",
            writes.len()
        ));
        if posts > 0 {
            line.push_str(&format!(
                "（其中 {posts} 个 POST，确认只是查询的，在「技术配置」里勾上「只查询」）"
            ));
        }
    }
    if !blocked.is_empty() {
        line.push_str(&format!(
            "；{} 个暂时发不了：{}",
            blocked.len(),
            blocked
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join("、")
        ));
    }
    line
}

/// cURL：走「从文档识别」的程序解析（不用模型），每条命令一个接口。
fn import_curl(store: &mut ApiStore, file: &str, text: &str, outcome: &mut ImportOutcome) {
    let taken: Vec<String> = store.endpoints.iter().map(|e| e.id.clone()).collect();
    let analysis = api_import::analyze(&Material::new(text), None, &taken);
    if analysis.drafts.is_empty() {
        outcome.report.push(format!("{file}：没有认出 cURL 命令"));
        return;
    }
    let count = analysis.drafts.len();
    for draft in analysis.drafts {
        let mut endpoint = draft.endpoint;
        endpoint.readonly = draft.access == Access::Query;
        store.endpoints.push(endpoint);
    }
    outcome.added += count;
    outcome.secrets.extend(analysis.secrets);
    outcome.report.push(format!(
        "{file}：cURL {count} 个接口（名称、说明要手填；保存时按服务器归进服务）"
    ));
}
