//! 接入循环的夹具测试：假服务按请求回应，假桌面按问题作答，脚本模型按提示调工具。

use super::*;
use crate::agent::api::{Prepared, RawResponse, trial_via};
use crate::agent::api_import::{Material, analyze};
use crate::agent::testkit::ScriptedModel;
use serde_json::json;

type Respond = Box<dyn Fn(&Prepared) -> Result<RawResponse, String>>;
type Reply = Box<dyn FnMut(&[Question]) -> Option<Answers>>;

struct FakeDesk {
    respond: Respond,
    reply: Reply,
    steps: Vec<String>,
    asked: Vec<Vec<Question>>,
    requests: Vec<String>,
}

impl FakeDesk {
    fn new(
        respond: impl Fn(&Prepared) -> Result<RawResponse, String> + 'static,
        reply: impl FnMut(&[Question]) -> Option<Answers> + 'static,
    ) -> Self {
        Self {
            respond: Box::new(respond),
            reply: Box::new(reply),
            steps: Vec::new(),
            asked: Vec::new(),
            requests: Vec::new(),
        }
    }

    fn said(&self, needle: &str) -> bool {
        self.steps.iter().any(|s| s.contains(needle))
    }
}

impl Desk for FakeDesk {
    fn emit(&mut self, event: Event) {
        if let Event::Step(line) = event {
            self.steps.push(line);
        }
    }

    fn ask(&mut self, questions: &[Question]) -> Option<Answers> {
        self.asked.push(questions.to_vec());
        (self.reply)(questions)
    }

    fn cancelled(&self) -> bool {
        false
    }

    fn trial(
        &mut self,
        endpoint: &ApiEndpoint,
        args: &Map<String, Value>,
        secrets: &ApiSecrets,
    ) -> Trial {
        let requests = &mut self.requests;
        let respond = &self.respond;
        trial_via(endpoint, args, secrets, |prepared, _| {
            requests.push(format!(
                "{} {} {:?}",
                prepared.method.label(),
                prepared.url,
                prepared.headers
            ));
            respond(prepared)
        })
    }
}

fn ok(body: Value) -> Result<RawResponse, String> {
    Ok(RawResponse {
        status: 200,
        body: body.to_string(),
    })
}

fn status(code: u16, body: &str) -> Result<RawResponse, String> {
    Ok(RawResponse {
        status: code,
        body: body.into(),
    })
}

/// 鉴权写在开头一节、两个接口、其中一个必填参数没有样例。
const DOC: &str = "# 统计平台接口\n\n服务地址：http://10.0.0.9:8080\n\n\
## 鉴权\n\n所有请求在请求头中携带 X-API-Key。\n\n\
## 政策检索\n\n请求地址：/api/policy/search\n请求方式：GET\n\n\
| 参数名 | 类型 | 必填 | 说明 |\n|---|---|---|---|\n| keyword | string | 是 | 关键词，如：中小企业 |\n\n\
## 年度统计\n\n请求地址：/api/stat/yearly\n请求方式：GET\n\n\
| 参数名 | 类型 | 必填 | 说明 |\n|---|---|---|---|\n| year | int | 是 | 年份 |\n";

fn items_of(doc: &str) -> (Material, Vec<Item>) {
    let material = Material::new(doc);
    let analysis = analyze(&material, None, &[]);
    let items = analysis
        .drafts
        .into_iter()
        .map(|draft| Item::new(draft, true, true, None))
        .collect();
    (material, items)
}

fn key_guarded(prepared: &Prepared, key: &str) -> Result<RawResponse, String> {
    let sent = prepared
        .headers
        .iter()
        .find(|(name, _)| name == "X-API-Key")
        .map(|(_, v)| v.as_str());
    if sent != Some(key) {
        return status(401, "{\"code\": 401, \"msg\": \"invalid api key\"}");
    }
    ok(json!({"code": 0, "data": {"list": [{"id": 1, "title": "结果"}]}}))
}

#[test]
fn keys_and_sample_values_are_asked_once_then_everything_is_retried() {
    let (material, items) = items_of(DOC);
    let mut desk = FakeDesk::new(
        |p| key_guarded(p, "k1"),
        |questions| {
            Some(
                questions
                    .iter()
                    .map(|q| {
                        let answer = match q.ask {
                            Ask::Secret { .. } => Answer::Text("k1".into()),
                            _ => Answer::Text("2025".into()),
                        };
                        (q.id.clone(), answer)
                    })
                    .collect(),
            )
        },
    );
    let mut run = Run::new(items, ApiSecrets::default(), &material, None);
    let report = run.run(&mut desk);
    assert_eq!(report.done, 2, "{:?} {:?}", report, desk.steps);
    assert!(report.open.is_empty());
    let first: Vec<&str> = desk.asked[0].iter().map(|q| q.id.as_str()).collect();
    assert_eq!(
        first,
        ["secret:x_api_key", "example:1:year"],
        "同一个 Key 只问一次"
    );
    assert_eq!(desk.asked.len(), 1);
    assert_eq!(run.secrets.secrets["x_api_key"], "k1");
    assert_eq!(run.items[1].draft.endpoint.inputs[0].example, "2025");
    assert_eq!(
        run.items[0].draft.endpoint.mapping.list, "/data/list",
        "按真实返回补了映射"
    );
    assert!(desk.said("调通了"));
}

#[test]
fn a_rejected_key_is_asked_again_and_a_skip_hands_it_to_the_model() {
    let (material, items) = items_of(DOC);
    let mut round = 0;
    let mut desk = FakeDesk::new(
        |p| key_guarded(p, "good"),
        move |questions| {
            round += 1;
            Some(
                questions
                    .iter()
                    .map(|q| {
                        let answer = match (&q.ask, round) {
                            (Ask::Secret { .. }, 1) => Answer::Text("bad".into()),
                            (Ask::Secret { .. }, _) => Answer::Text("good".into()),
                            _ => Answer::Text("2025".into()),
                        };
                        (q.id.clone(), answer)
                    })
                    .collect(),
            )
        },
    );
    let mut run = Run::new(items, ApiSecrets::default(), &material, None);
    let report = run.run(&mut desk);
    assert_eq!(report.done, 2, "{:?}", desk.steps);
    assert!(
        desk.asked[1]
            .iter()
            .any(|q| q.id == "rekey:x_api_key" && q.text.contains("可能不对或已过期"))
    );
    assert!(desk.said("鉴权没过"));
}

#[test]
fn without_a_model_fixable_failures_stop_with_a_reason() {
    let (material, items) = items_of(DOC);
    let mut round = 0;
    let mut desk = FakeDesk::new(
        |_| status(404, "not found"),
        move |questions| {
            round += 1;
            // 第一回把年份填成了字，程序该再问一次。
            let year = if round == 1 { "今年" } else { "2025" };
            Some(
                questions
                    .iter()
                    .map(|q| {
                        let answer = match q.ask {
                            Ask::Secret { .. } => "k",
                            _ => year,
                        };
                        (q.id.clone(), Answer::Text(answer.into()))
                    })
                    .collect(),
            )
        },
    );
    let mut run = Run::new(items, ApiSecrets::default(), &material, None);
    let report = run.run(&mut desk);
    assert_eq!(report.done, 0);
    assert!(
        report
            .open
            .iter()
            .all(|o| o.contains("卡住") && o.contains("404") && o.contains("没配起草模型")),
        "{:?}",
        report.open
    );
    assert!(
        desk.asked[1]
            .iter()
            .any(|q| q.id == "example:1:year" && q.basis.contains("要是数字")),
        "试调值不合用要再问：{:?}",
        desk.asked
    );
}

#[test]
fn unreachable_servers_are_asked_about_and_can_be_skipped() {
    let (material, items) = items_of(DOC);
    let mut desk = FakeDesk::new(
        |_| Err("连接超时".into()),
        |questions| {
            Some(
                questions
                    .iter()
                    .map(|q| {
                        let answer = match (&q.ask, q.id.starts_with("host:")) {
                            (_, true) => Answer::Choice(1),
                            (Ask::Secret { .. }, _) => Answer::Text("k".into()),
                            _ => Answer::Text("2025".into()),
                        };
                        (q.id.clone(), answer)
                    })
                    .collect(),
            )
        },
    );
    let mut run = Run::new(items, ApiSecrets::default(), &material, None);
    let report = run.run(&mut desk);
    let host_question = desk.asked[1]
        .iter()
        .find(|q| q.id == "host:http://10.0.0.9:8080")
        .expect("问了连不上的服务器");
    assert!(host_question.text.contains("内网"));
    assert_eq!(desk.asked[1].len(), 1, "两个接口同一台服务器，只问一次");
    assert!(
        report.open.iter().all(|o| o.contains("跳过")),
        "{:?}",
        report.open
    );
}

#[test]
fn a_document_without_auth_asks_how_the_key_is_carried() {
    let doc = "## 政策检索\n\n请求地址：http://10.0.0.9/api/policy/search\n请求方式：GET\n";
    let (material, items) = items_of(doc);
    let mut desk = FakeDesk::new(
        |p| key_guarded(p, "k1"),
        |questions| {
            Some(
                questions
                    .iter()
                    .map(|q| {
                        let answer = match &q.ask {
                            Ask::Choice { .. } => Answer::Choice(1),
                            Ask::Secret { .. } => Answer::Text("k1".into()),
                            Ask::Text { .. } => Answer::Skip,
                        };
                        (q.id.clone(), answer)
                    })
                    .collect(),
            )
        },
    );
    let mut run = Run::new(items, ApiSecrets::default(), &material, None);
    let report = run.run(&mut desk);
    assert_eq!(report.done, 1, "{:?}", desk.steps);
    assert_eq!(desk.asked[0][0].id, "auth:http://10.0.0.9");
    assert_eq!(desk.asked[1][0].id, "secret:x_api_key");
    assert_eq!(run.items[0].draft.endpoint.headers[0].name, "X-API-Key");
}

#[test]
fn the_model_fixes_a_wrong_path_from_the_material() {
    let doc = "## 政策检索\n\n所有接口都经网关转发，路径前面要加 /gateway。\n\n\
请求地址：http://10.0.0.9/api/policy/search\n请求方式：GET\n";
    let (material, items) = items_of(doc);
    let model = ScriptedModel::new(|_, prompt| {
        if prompt.contains("已改：") {
            "<tool_call>{\"name\": \"test\", \"arguments\": {}}</tool_call>".into()
        } else if prompt.contains("没采用") {
            "<tool_call>{\"name\": \"update_config\", \"arguments\": {\"url\": \"http://10.0.0.9/gateway/api/policy/search\"}}</tool_call>".into()
        } else {
            // 先编一个资料里没有的路径，程序该拒绝。
            "<tool_call>{\"name\": \"update_config\", \"arguments\": {\"url\": \"http://10.0.0.9/v9/policy/find\"}}</tool_call>".into()
        }
    });
    let mut desk = FakeDesk::new(
        |p| {
            if p.url.contains("/gateway/") {
                ok(json!({"code": 0, "data": [{"title": "意见"}]}))
            } else {
                status(404, "not found")
            }
        },
        |questions| {
            Some(
                questions
                    .iter()
                    .map(|q| (q.id.clone(), Answer::Choice(0)))
                    .collect(),
            )
        },
    );
    let mut run = Run::new(items, ApiSecrets::default(), &material, Some(&model));
    let report = run.run(&mut desk);
    assert_eq!(report.done, 1, "{:?}", desk.steps);
    assert_eq!(
        run.items[0].draft.endpoint.url,
        "http://10.0.0.9/gateway/api/policy/search"
    );
    assert!(desk.said("模型改了「政策检索」的地址"));
    assert!(
        run.items[0]
            .history
            .iter()
            .any(|h| h.contains("没采用") && h.contains("/v9/policy/find")),
        "{:?}",
        run.items[0].history
    );
    // 资料摘录、试调结果都交给了模型。
    assert!(model.asked("网关转发") > 0 && model.asked("返回 404") > 0);
}

#[test]
fn patches_are_checked_against_the_material() {
    let endpoint = ApiEndpoint {
        id: "s".into(),
        name: "检索".into(),
        url: "http://h/api/search?q={q}".into(),
        inputs: vec![crate::agent::api::ApiInput {
            name: "q".into(),
            ..Default::default()
        }],
        headers: vec![crate::agent::api::ApiHeader {
            name: "Authorization".into(),
            value: "Bearer {secret:token}".into(),
        }],
        ..ApiEndpoint::default()
    };
    let known = "资料：GET /api/search，参数 q、page；请求头 X-Tenant 写租户编号。返回 data.rows。";
    let patch = |args: Value| fix::apply_patch(&endpoint, &args, known, None);
    assert!(
        patch(json!({"method": "DELETE"}))
            .unwrap_err()
            .contains("GET 或 POST")
    );
    assert!(
        patch(json!({"url": "http://other/api/search"}))
            .unwrap_err()
            .contains("服务器")
    );
    assert!(
        patch(json!({"url": "http://h/api/search?q={q}&size={size}"}))
            .unwrap_err()
            .contains("size")
    );
    assert!(
        patch(json!({"headers": {"Authorization": "Bearer abcdef1234567890abcd"}}))
            .unwrap_err()
            .contains("令牌")
    );
    assert!(
        patch(json!({"headers": {"X-Tenant": "{secret:other}"}}))
            .unwrap_err()
            .contains("不存在")
    );
    assert!(
        patch(json!({"url": "http://h/api/search?q={q}&page={page}"}))
            .unwrap_err()
            .contains("没有对应的输入变量"),
        "改完配置要过静态检查"
    );
    let (fixed, changed) = patch(json!({
        "url": "http://h/api/search?q={q}&page={page}",
        "inputs": [{"name": "page", "kind": "number", "example": "1"}],
        "headers": {"X-Tenant": "gov01"},
        "list": "/data/rows",
    }))
    .unwrap();
    assert_eq!(changed, ["地址", "请求头", "参数", "列表位置"]);
    assert_eq!(fixed.inputs[1].kind, crate::agent::api::InputKind::Number);
    let reply = json!({"data": {"items": []}});
    assert!(
        fix::apply_patch(
            &endpoint,
            &json!({"list": "/data/rows"}),
            known,
            Some(&reply)
        )
        .unwrap_err()
        .contains("真实返回里没有")
    );
}
