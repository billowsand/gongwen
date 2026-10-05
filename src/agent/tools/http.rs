//! F 组：外部系统接口（只查询）。
//!
//! 只能调AI 管理页「数据接口」里配好的接口，不能临时拼地址；技能用 `tools: [http.call:<接口 id>]`
//! 声明具体能调哪几个（`tools::call` 按「工具:接口」查白名单）。

use super::{Input, Permission, Tool, ToolCtx, ToolOutput, arg_str, required};
use crate::agent::api::{self, ApiDestination};
use crate::agent::evidence::EvidenceDoc;
use serde_json::{Map, Value};

pub(super) const TOOLS: [&dyn Tool; 1] = [&HttpCall];

struct HttpCall;

impl Tool for HttpCall {
    fn id(&self) -> &'static str {
        "http.call"
    }
    fn permission(&self) -> Permission {
        Permission::External
    }
    fn description(&self) -> &'static str {
        "调用设置里配好的数据接口（只查询），按返回映射整理成条目；资料类结果并入证据包"
    }
    fn inputs(&self) -> &'static [Input] {
        const INPUTS: &[Input] = &[required(
            "api",
            "接口 id（AI 管理页「数据接口」里配的；写成 http.call:<id> 时自动填上）；其余参数按接口定义的输入变量给",
        )];
        INPUTS
    }
    fn run(
        &self,
        ctx: &mut ToolCtx<'_, '_>,
        args: &Map<String, Value>,
    ) -> Result<ToolOutput, String> {
        let id = arg_str(args, "api").unwrap_or_default();
        let endpoint = ctx.env.apis.get(&id).ok_or_else(|| {
            format!("没有配置 id 为「{id}」的数据接口，先在AI 管理页「数据接口」里添加")
        })?;
        let mut inputs = args.clone();
        inputs.remove("api");
        let api::Called { items, total, .. } = api::call(endpoint, &inputs, ctx.env.secrets)?;
        let mut summary = format!("调用接口「{}」→ {} 条", endpoint.name, items.len());
        if total > items.len() {
            // 截掉的部分要让模型知道，免得把前几十条当成全部。
            summary.push_str(&format!(
                "（共 {total} 条，只给了前 {} 条；要更全的结果请缩小查询条件）",
                items.len()
            ));
        }
        let mut output = ToolOutput::new(
            serde_json::to_value(&items).map_err(|e| e.to_string())?,
            summary,
        );
        output.evidence = items
            .iter()
            .map(|item| EvidenceDoc {
                key: format!("http:{id}:{}", item.id),
                title: if item.title.is_empty() {
                    endpoint.name.clone()
                } else {
                    item.title.clone()
                },
                section: item.source.clone(),
                kind_label: "数据接口".into(),
                text: item.text.clone(),
            })
            .collect();
        output.evidence_by_default = endpoint.destination == ApiDestination::Evidence;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::Fixture;
    use crate::agent::api::test_server::TestServer;
    use crate::agent::api::{ApiEndpoint, ApiInput, ApiMapping};
    use serde_json::json;

    fn endpoint(url: &str) -> ApiEndpoint {
        ApiEndpoint {
            id: "stat".into(),
            name: "火灾统计".into(),
            url: format!("{url}/stat?region={{region}}"),
            inputs: vec![ApiInput {
                name: "region".into(),
                required: true,
                ..ApiInput::default()
            }],
            mapping: ApiMapping {
                list: "/items".into(),
                text: "{region}共发生森林火灾{count}起".into(),
                ..ApiMapping::default()
            },
            ..ApiEndpoint::default()
        }
    }

    #[test]
    fn calls_a_configured_api_and_returns_evidence() {
        let server = TestServer::start(vec![(
            200,
            json!({"items": [{"id": 3, "title": "年度统计", "region": "全省", "count": 12}]})
                .to_string(),
        )]);
        let mut fixture = Fixture::new("");
        fixture.apis.endpoints.push(endpoint(&server.url));
        let out = fixture
            .call("http.call:stat", json!({"region": "全省"}))
            .unwrap();
        assert_eq!(out.value[0]["text"], "全省共发生森林火灾12起");
        assert_eq!(out.summary, "调用接口「火灾统计」→ 1 条");
        assert!(out.evidence_by_default);
        assert_eq!(out.evidence[0].key, "http:stat:3");
        assert_eq!(out.evidence[0].title, "年度统计");
        assert!(
            server
                .request(0)
                .starts_with("GET /stat?region=%E5%85%A8%E7%9C%81 ")
        );
    }

    #[test]
    fn the_whitelist_is_checked_per_api() {
        let mut fixture = Fixture::new("");
        fixture.apis.endpoints.push(endpoint("http://127.0.0.1:9"));
        fixture.skill.tools = vec!["http.call:other".into()];
        let error = fixture
            .call("http.call", json!({"api": "stat", "region": "全省"}))
            .unwrap_err();
        assert!(error.contains("没有声明工具「http.call:stat」"), "{error}");
        let error = fixture
            .call("http.call:other", json!({"api": "stat"}))
            .unwrap_err();
        assert!(error.contains("与限定名不一致"), "{error}");
        fixture.skill.tools = vec!["http.call".into()];
        let error = fixture.call("http.call:nope", json!({})).unwrap_err();
        assert!(
            error.contains("没有配置 id 为「nope」的数据接口"),
            "{error}"
        );
        let error = fixture.call("http.call:stat", json!({})).unwrap_err();
        assert!(error.contains("缺少输入 region"), "{error}");
    }
}
