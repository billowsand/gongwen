//! 自动填报的夹具测试：程序解析、模型整理的核对与合并、实测补全、密钥合并。

use super::*;
use crate::agent::api::{ApiSecrets, InputKind};
use crate::agent::testkit::ScriptedModel;
use serde_json::json;

const CURL_DOC: &str = "## 政策检索\n\n按关键词检索政策原文。\n\n\
```bash\n\
curl -X POST 'http://10.0.0.9:8080/api/policy/search' \\\n  \
-H 'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc' \\\n  \
-H 'Content-Type: application/json' \\\n  \
-d '{\"keyword\": \"中小企业\", \"size\": 10}'\n\
```\n\n\
参数：keyword 关键词（必填）；size 返回条数。\n\n\
返回示例：\n\
```json\n\
{\"code\": 0, \"msg\": \"成功\", \"data\": {\"records\": [{\"id\": \"p1\", \"title\": \"关于支持中小企业发展的意见\", \"content\": \"……\"}]}}\n\
```\n";

#[test]
fn a_curl_document_is_parsed_without_a_model() {
    let material = Material::new(CURL_DOC);
    assert!(
        !material.redacted.contains("eyJhbGci"),
        "发给模型的资料不带令牌"
    );
    assert_eq!(material.masked(), 1);
    let analysis = analyze(&material, None, &["policy_search".into()]);
    assert!(!analysis.model_used);
    assert!(analysis.notes[0].contains("没有配置起草模型"));
    assert_eq!(analysis.drafts.len(), 1);
    let draft = &analysis.drafts[0];
    let endpoint = &draft.endpoint;
    assert_eq!(endpoint.id, "policy_search_2", "避开已有的 id");
    assert_eq!(endpoint.method, ApiMethod::Post);
    assert_eq!(endpoint.url, "http://10.0.0.9:8080/api/policy/search");
    assert_eq!(endpoint.headers.len(), 1, "Content-Type 不写进配置");
    assert_eq!(endpoint.headers[0].value, "Bearer {secret:token}");
    assert_eq!(
        serde_json::from_str::<Value>(&endpoint.body).unwrap(),
        json!({"keyword": "{keyword}", "size": "{size}"})
    );
    assert_eq!(endpoint.inputs[1].kind, InputKind::Number);
    assert_eq!(endpoint.mapping.list, "/data/records", "返回示例推断出列表");
    assert_eq!(endpoint.success.pointer, "/code");
    assert_eq!(draft.origin("返回映射"), Some(Origin::Document));
    assert_eq!(draft.access, Access::Unknown, "没有模型时性质要人确认");
    assert_eq!(
        analysis.secrets,
        [("token".to_string(), "eyJhbGciOiJIUzI1NiJ9.abc".to_string())]
    );
    assert!(endpoint.problems().is_empty(), "{:?}", endpoint.problems());

    let mut secrets = ApiSecrets::default();
    merge_secrets(&mut secrets, &analysis.secrets);
    let missing = draft.missing(&secrets);
    assert!(missing.iter().any(|m| m.contains("只查询")), "{missing:?}");
    assert!(missing.iter().any(|m| m.contains("说明")), "{missing:?}");
    assert!(!missing.iter().any(|m| m.contains("密钥")), "{missing:?}");
    assert!(draft.endpoint.problems().is_empty());
}

#[test]
fn the_model_fills_words_but_cannot_invent_addresses_or_parameters() {
    let reply = json!({"endpoints": [
        {
            "name": "政策检索", "description": "按关键词检索政策原文", "method": "POST",
            "url": "/api/policy/search",
            "inputs": [
                {"name": "keyword", "kind": "text", "required": true, "description": "政策主题关键词"},
                {"name": "size", "kind": "number", "required": false, "description": "返回条数"}
            ],
            "access": "query", "access_basis": "按关键词检索政策原文", "missing": ["接口的分页规则"]
        },
        {
            "name": "政策全文", "method": "GET", "url": "/api/policy/detail/{id}",
            "inputs": [{"name": "id", "required": true}], "access": "query"
        }
    ]})
    .to_string();
    let model = ScriptedModel::new(move |_, _| format!("整理好了：\n```json\n{reply}\n```"));
    let material = Material::new(CURL_DOC);
    let analysis = analyze(&material, Some(&model), &[]);
    assert!(analysis.model_used);
    assert_eq!(model.asked("eyJhbGci"), 0, "令牌没发给模型");
    assert_eq!(model.asked("******"), 1);
    assert_eq!(analysis.drafts.len(), 1, "资料里没有的「政策全文」不收");
    assert!(
        analysis
            .notes
            .iter()
            .any(|n| n.contains("政策全文") && n.contains("找不到")),
        "{:?}",
        analysis.notes
    );
    let draft = &analysis.drafts[0];
    assert_eq!(draft.endpoint.name, "政策检索");
    assert_eq!(draft.origin("名称"), Some(Origin::Model));
    assert_eq!(
        draft.origin("地址"),
        Some(Origin::Document),
        "结构仍以文档为准"
    );
    assert_eq!(draft.endpoint.inputs[0].description, "政策主题关键词");
    assert!(draft.endpoint.inputs[0].required);
    assert_eq!(draft.access, Access::Query);
    assert!(draft.notes.iter().any(|n| n.contains("分页规则")));

    let mut secrets = ApiSecrets::default();
    merge_secrets(&mut secrets, &analysis.secrets);
    assert!(
        draft.missing(&secrets).is_empty(),
        "{:?}",
        draft.missing(&secrets)
    );
}

#[test]
fn prose_only_documents_are_built_from_the_model_and_checked() {
    let doc = "月度统计查询接口\n服务地址：http://10.1.2.3:9000\n\
               路径 /stat/monthly ，GET 方式，参数 month 表示月份，格式 2026-09，必填；\
               region 表示地区，可不填。\n返回 data.items 列表，code 为 0 表示成功。";
    let reply = json!({"endpoints": [{
        "name": "月度统计", "description": "按月份查统计数据", "method": "GET",
        "url": "/stat/monthly",
        "inputs": [
            {"name": "month", "kind": "text", "required": true, "description": "月份", "example": "2026-09"},
            {"name": "region", "required": false, "description": "地区"},
            {"name": "unit_code", "required": false, "description": "单位编码"}
        ],
        "list": "/data/items", "success_pointer": "/code", "success_equals": 0,
        "access": "query"
    }]})
    .to_string();
    let model = ScriptedModel::new(move |_, _| reply.clone());
    let analysis = analyze(&Material::new(doc), Some(&model), &[]);
    assert_eq!(analysis.drafts.len(), 1, "{:?}", analysis.notes);
    let draft = &analysis.drafts[0];
    let endpoint = &draft.endpoint;
    assert_eq!(
        endpoint.url,
        "http://10.1.2.3:9000/stat/monthly?month={month}&region={region}"
    );
    assert_eq!(endpoint.inputs[0].example, "2026-09");
    assert!(endpoint.inputs[0].required && !endpoint.inputs[1].required);
    assert!(
        draft.notes.iter().any(|n| n.contains("unit_code")),
        "猜出来的参数去掉了：{:?}",
        draft.notes
    );
    assert_eq!(endpoint.mapping.list, "/data/items");
    assert_eq!(
        (
            endpoint.success.pointer.as_str(),
            endpoint.success.equals.as_str()
        ),
        ("/code", "0")
    );
    assert_eq!(draft.access, Access::Query);
    assert!(endpoint.problems().is_empty(), "{:?}", endpoint.problems());
}

#[test]
fn write_endpoints_are_marked_and_bad_model_output_falls_back() {
    let doc = "删除记录：DELETE http://x/api/record/1\n\n查询：GET http://x/api/record/list?type=a";
    let model = ScriptedModel::new(|_, _| "我不太确定。".into());
    let analysis = analyze(&Material::new(doc), Some(&model), &[]);
    assert!(!analysis.model_used);
    assert!(
        analysis
            .notes
            .iter()
            .any(|n| n.contains("没有给出可用的整理结果"))
    );
    assert_eq!(analysis.drafts.len(), 2);
    assert_eq!(analysis.drafts[0].access, Access::Write);
    assert_eq!(analysis.drafts[1].access, Access::Unknown);

    let reply = json!({"endpoints": [{"url": "http://x/api/record/list", "access": "query", "method": "GET"}]}).to_string();
    let doc = "GET http://x/api/record/updateList?type=a";
    let model = ScriptedModel::new(move |_, _| reply.replace("record/list", "record/updateList"));
    let analysis = analyze(&Material::new(doc), Some(&model), &[]);
    assert_eq!(
        analysis.drafts[0].access,
        Access::Unknown,
        "地址里有 update 字样要人确认"
    );
    assert!(analysis.drafts[0].access_basis.contains("update"));
}

#[test]
fn a_real_reply_fills_the_mapping_left_empty() {
    let material = Material::new("GET http://x/api/stat?region=全省");
    let mut draft = analyze(&material, None, &[]).drafts.remove(0);
    assert!(draft.endpoint.mapping.list.is_empty());
    let filled = refine_with_reply(
        &mut draft,
        &json!({"success": true, "result": [{"region": "全省", "count": 12}]}).to_string(),
    );
    assert_eq!(filled, ["列表位置", "正文", "成功判据"]);
    assert_eq!(draft.endpoint.mapping.list, "/result");
    assert_eq!(draft.origin("返回映射"), Some(Origin::Reply));
    assert!(refine_with_reply(&mut draft, "<html>").is_empty());
}

#[test]
fn found_secrets_merge_without_overwriting() {
    let mut store = ApiSecrets::default();
    store.secrets.insert("token".into(), "old".into());
    store.secrets.insert("cookie".into(), String::new());
    let renamed = merge_secrets(
        &mut store,
        &[
            ("token".into(), "new".into()),
            ("cookie".into(), "c=1".into()),
            ("key".into(), String::new()),
        ],
    );
    assert_eq!(renamed, [("token".to_string(), "token2".to_string())]);
    assert_eq!(store.secrets["token"], "old");
    assert_eq!(store.secrets["token2"], "new");
    assert_eq!(store.secrets["cookie"], "c=1", "空值的补上");
    assert_eq!(store.secrets["key"], "");

    let mut endpoint = ApiEndpoint {
        headers: vec![crate::agent::api::ApiHeader {
            name: "Authorization".into(),
            value: "Bearer {secret:token}".into(),
        }],
        ..ApiEndpoint::default()
    };
    rename_secret_refs(&mut endpoint, &renamed);
    assert_eq!(endpoint.headers[0].value, "Bearer {secret:token2}");
    assert_eq!(secret_refs(&endpoint), ["token2"]);
}
