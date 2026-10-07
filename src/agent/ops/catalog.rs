//! 自主步骤的工具目录：哪些工具直接给模型，哪些先搜再调（`docs/agent-kernel-hardening.md` 3.3）。
//!
//! 工具不多时全部直给，与以前一样。一步能用的工具（内置 + 数据接口）超过 [`ALL_AT`] 个、或数据接口
//! 超过 [`tooling::TWO_LEVEL_AT`] 个时分两级：
//! - **直给**：读正文、读写工作稿、留言这几个常用的（步骤可用 `direct: [...]` 改），模型一眼能看到；
//! - **先搜再调**：其余内置工具与全部数据接口，经 `tool_search`（按关键词找，返回名字、用途、参数
//!   签名）与 `tool_call`（`tool` 填找到的名字、`args` 填参数）两个元工具使用。
//!
//! 数据接口原来自带的 `api_search` / `api_call` 在自主步骤里并进这两个元工具，不出现两套。
//! **权限不变**：搜只搜这一步能用的工具，`tool_call` 调的工具照样经 `tools::call` 查技能白名单。

use crate::agent::api::ApiStore;
use crate::agent::apidef::tooling::{self, ApiTools, Resolved};
use crate::agent::argcheck;
use crate::agent::board::value_to_text;
use crate::agent::toolcall::{ToolCall, ToolSpec, wire_name};
use crate::agent::tools;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// 一步能用的工具（内置 + 接口）超过这么多就分两级。
pub(super) const ALL_AT: usize = 24;
/// 「搜工具」一次最多列几个。
const SEARCH_TOP: usize = 6;
pub(super) const SEARCH_TOOL: &str = "tool_search";
pub(super) const CALL_TOOL: &str = "tool_call";
/// 分两级时默认直给的内置工具（还要在这一步能用的工具里）。
const CORE: &[&str] = &["doc.read", "ws.read", "ws.write", "ws.replace", "note"];

/// 模型的一次调用落到哪。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Route {
    /// 调内置工具（工具 id，参数）。
    Tool(String, Value),
    /// 调数据接口（`http.call:<id>`，参数）。
    Api(String, Value),
    /// 直接回给模型的文字：搜索结果、拒绝原因。
    Reply(String),
    /// 不认识的名字。
    Unknown,
}

pub(super) struct Catalog {
    /// 直给的内置工具：发给模型的名字 → 工具 id。
    direct: BTreeMap<String, String>,
    /// 直给的数据接口（逐个编成工具）。
    direct_apis: ApiTools,
    /// 先搜再调的内置工具 id。
    hidden: Vec<String>,
    /// 先搜再调的接口 id。
    hidden_apis: Vec<String>,
}

impl Catalog {
    /// `ids` 是这一步能用的内置工具 id，`api_ids` 是能用的接口 id，`core` 是分两级时直给的工具
    /// （步骤写了 `direct:` 用它，否则 [`CORE`]）。
    pub(super) fn new(ids: Vec<String>, api_ids: Vec<String>, core: Option<&[String]>) -> Self {
        let total = ids.len() + api_ids.len();
        let apis_hidden = total > ALL_AT || api_ids.len() > tooling::TWO_LEVEL_AT;
        let (direct, hidden): (Vec<String>, Vec<String>) = if total > ALL_AT {
            ids.into_iter().partition(|id| match core {
                Some(core) => core.contains(id),
                None => CORE.contains(&id.as_str()),
            })
        } else {
            (ids, Vec::new())
        };
        let (direct_apis, hidden_apis) = if apis_hidden {
            (ApiTools::Direct(Vec::new()), api_ids)
        } else {
            (ApiTools::new(api_ids), Vec::new())
        };
        Self {
            direct: direct.into_iter().map(|id| (wire_name(&id), id)).collect(),
            direct_apis,
            hidden,
            hidden_apis,
        }
    }

    /// 分了两级没有。
    pub(super) fn two_level(&self) -> bool {
        !self.hidden.is_empty() || !self.hidden_apis.is_empty()
    }

    /// 发给模型的工具说明（不含 `finish`）。
    pub(super) fn specs(&self, apis: &ApiStore) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self
            .direct
            .values()
            .filter_map(|id| super::agent::spec_of(id))
            .collect();
        specs.extend(self.direct_apis.specs(apis));
        if self.two_level() {
            let count = self.hidden.len() + self.hidden_apis.len();
            specs.push(ToolSpec {
                name: SEARCH_TOOL.into(),
                description: format!(
                    "上面没列出的还有 {count} 个工具（计算、检索、核对、词库、数据接口等），按关键词找，\
                     返回名字、用途与参数。要用它们先搜"
                ),
                params: vec![("query".into(), true, "要做什么，例如「算工作日」「森林火灾 起数」".into())],
                schema: Some(json!({
                    "type": "object",
                    "properties": {"query": {
                        "type": "string",
                        "description": "要做什么，例如「算工作日」「森林火灾 起数」",
                    }},
                    "required": ["query"],
                })),
            });
            specs.push(ToolSpec {
                name: CALL_TOOL.into(),
                description: format!(
                    "调用 {SEARCH_TOOL} 找到的工具：tool 填它列出的名字，args 按它列出的参数填"
                ),
                params: vec![
                    ("tool".into(), true, "工具名字（tool_search 列出的）".into()),
                    ("args".into(), true, "参数，JSON 对象".into()),
                ],
                schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "tool": {"type": "string", "description": "工具名字（tool_search 列出的）"},
                        "args": {"type": "object", "description": "参数，按 tool_search 列出的参数填"},
                    },
                    "required": ["tool", "args"],
                })),
            });
        }
        specs
    }

    /// 模型的一次调用落到哪。
    pub(super) fn route(&self, call: &ToolCall, apis: &ApiStore) -> Route {
        if let Some(id) = self.direct.get(&call.name) {
            return Route::Tool(id.clone(), call.arguments.clone());
        }
        match self.direct_apis.resolve(call, apis) {
            Resolved::Call { api, args } => return Route::Api(api, Value::Object(args)),
            Resolved::Reply(text) => return Route::Reply(text),
            Resolved::Other => {}
        }
        if !self.two_level() {
            return Route::Unknown;
        }
        let args = match &call.arguments {
            Value::Object(map) => map.clone(),
            _ => Map::new(),
        };
        match call.name.as_str() {
            SEARCH_TOOL => {
                let query = args.get("query").map(value_to_text).unwrap_or_default();
                Route::Reply(self.search(&query, apis))
            }
            CALL_TOOL => {
                let name = args.get("tool").map(value_to_text).unwrap_or_default();
                let inner = match args.get("args") {
                    Some(Value::Object(map)) => Value::Object(map.clone()),
                    Some(Value::String(text)) => {
                        serde_json::from_str(text).unwrap_or_else(|_| json!({}))
                    }
                    _ => json!({}),
                };
                self.call_target(name.trim(), inner)
            }
            _ => Route::Unknown,
        }
    }

    /// `tool_call` 的 `tool` 落到哪：内置工具 id（或它的模型名）、`http.call:<接口 id>`。
    fn call_target(&self, name: &str, args: Value) -> Route {
        if let Some(api) = name.strip_prefix("http.call:") {
            if self.hidden_apis.iter().any(|id| id == api) {
                return Route::Api(api.to_string(), args);
            }
            return Route::Reply(format!(
                "没有能用的接口「{api}」。先用 {SEARCH_TOOL} 找，tool 照它列出的名字填。"
            ));
        }
        let found = self
            .hidden
            .iter()
            .chain(self.direct.values())
            .find(|id| *id == name || wire_name(id) == name);
        match found {
            Some(id) => Route::Tool(id.clone(), args),
            None => Route::Reply(format!(
                "没有能用的工具「{name}」。先用 {SEARCH_TOOL} 找，tool 照它列出的名字填。"
            )),
        }
    }

    /// 搜工具：内置工具按 id、说明、参数说明，接口按名称、用途、参数说明打分。
    fn search(&self, query: &str, apis: &ApiStore) -> String {
        let words = tooling::query_words(query);
        let mut scored: Vec<(usize, String)> = Vec::new();
        for id in &self.hidden {
            let Some(tool) = tools::find(id) else {
                continue;
            };
            let name = id.to_lowercase();
            let rest = format!(
                "{} {}",
                tool.description(),
                tool.inputs()
                    .iter()
                    .map(|input| input.doc)
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .to_lowercase();
            let score = tooling::match_score(&words, &name, &rest);
            if score > 0 {
                let line = format!(
                    "- 工具 {id}：{}；参数：{}",
                    tools::short(tool.description(), 120),
                    argcheck::signatures(&tools::schema(tool))
                );
                scored.push((score, line));
            }
        }
        for id in &self.hidden_apis {
            let Some(endpoint) = apis.get(id) else {
                continue;
            };
            let (name, rest) = tooling::search_text(endpoint);
            let score = tooling::match_score(&words, &name, &rest);
            if score > 0 {
                let line = format!(
                    "- 接口 http.call:{id}：{}（只查询）：{}；参数：{}",
                    endpoint.name,
                    tools::short(endpoint.description.trim(), 120),
                    tooling::signatures(endpoint)
                );
                scored.push((score, line));
            }
        }
        // 分数相同按先后：内置工具在前、技能白名单的顺序。
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        if scored.is_empty() {
            return format!("没找到和「{query}」相关的工具，换个说法再搜。");
        }
        let lines: Vec<&str> = scored
            .iter()
            .take(SEARCH_TOP)
            .map(|(_, line)| line.as_str())
            .collect();
        format!(
            "找到 {} 个相关工具（列出前 {}）：\n{}\n用 {CALL_TOOL} 调用：tool 填「工具 」或「接口 」后面的名字，args 按参数填。",
            scored.len(),
            lines.len(),
            lines.join("\n")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::api::{ApiEndpoint, ApiInput};

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| id.to_string()).collect()
    }

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments,
        }
    }

    fn apis() -> ApiStore {
        ApiStore {
            endpoints: vec![ApiEndpoint {
                id: "stat.fire".into(),
                name: "森林火灾统计".into(),
                description: "按地区查森林火灾起数".into(),
                inputs: vec![ApiInput {
                    name: "region".into(),
                    required: true,
                    description: "地区".into(),
                    ..ApiInput::default()
                }],
                ..ApiEndpoint::default()
            }],
            ..Default::default()
        }
    }

    /// 「自由任务」声明的内置工具（去掉自主步骤不能用的）。
    fn free_task_tools() -> Vec<String> {
        let skill = crate::agent::skill::builtin("free-task").unwrap();
        skill
            .tools
            .iter()
            .filter(|id| !id.starts_with("http.call"))
            .cloned()
            .collect()
    }

    #[test]
    fn few_tools_are_all_given_directly() {
        let catalog = Catalog::new(ids(&["doc.read", "calc.date"]), ids(&["stat.fire"]), None);
        assert!(!catalog.two_level());
        let names: Vec<String> = catalog.specs(&apis()).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["calc_date", "doc_read", "http_call__stat_fire"]);
    }

    #[test]
    fn many_tools_keep_the_core_direct_and_put_the_rest_behind_search() {
        let all = free_task_tools();
        assert!(all.len() > ALL_AT, "自由任务的工具数要超过阈值才测得到");
        let catalog = Catalog::new(all, ids(&["stat.fire"]), None);
        assert!(catalog.two_level());
        let specs = catalog.specs(&apis());
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "doc_read",
                "note",
                "ws_read",
                "ws_replace",
                "ws_write",
                SEARCH_TOOL,
                CALL_TOOL
            ]
        );

        let found = catalog.route(&call(SEARCH_TOOL, json!({"query": "工作日"})), &apis());
        let Route::Reply(text) = found else {
            panic!("{found:?}")
        };
        assert!(text.contains("- 工具 calc.workday："), "{text}");
        assert!(text.contains("days: 整数（必填）"), "带参数签名：{text}");
        let found = catalog.route(&call(SEARCH_TOOL, json!({"query": "森林火灾"})), &apis());
        let Route::Reply(text) = found else {
            panic!("{found:?}")
        };
        assert!(
            text.contains("- 接口 http.call:stat.fire：森林火灾统计"),
            "{text}"
        );

        let routed = catalog.route(
            &call(
                CALL_TOOL,
                json!({"tool": "calc.workday", "args": {"days": 5}}),
            ),
            &apis(),
        );
        assert_eq!(
            routed,
            Route::Tool("calc.workday".into(), json!({"days": 5}))
        );
        // 模型名、参数写成 JSON 字符串也认。
        let routed = catalog.route(
            &call(
                CALL_TOOL,
                json!({"tool": "calc_workday", "args": "{\"days\": 5}"}),
            ),
            &apis(),
        );
        assert_eq!(
            routed,
            Route::Tool("calc.workday".into(), json!({"days": 5}))
        );
        let routed = catalog.route(
            &call(
                CALL_TOOL,
                json!({"tool": "http.call:stat.fire", "args": {"region": "全省"}}),
            ),
            &apis(),
        );
        assert_eq!(
            routed,
            Route::Api("stat.fire".into(), json!({"region": "全省"}))
        );
        // 这一步不能用的工具搜不到、也调不到。
        let routed = catalog.route(
            &call(CALL_TOOL, json!({"tool": "llm.generate", "args": {}})),
            &apis(),
        );
        assert!(matches!(routed, Route::Reply(text) if text.contains("没有能用的工具")));
        assert_eq!(
            catalog.route(&call("calc_workday", json!({})), &apis()),
            Route::Unknown
        );
    }

    #[test]
    fn a_step_can_choose_what_stays_direct() {
        let core = ids(&["calc.date"]);
        let catalog = Catalog::new(free_task_tools(), Vec::new(), Some(&core));
        let names: Vec<String> = catalog.specs(&apis()).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["calc_date", SEARCH_TOOL, CALL_TOOL]);
    }
}
