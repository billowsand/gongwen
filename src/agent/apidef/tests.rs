//! 标准接口文件的测试：迁移、各格式导入、写回不走样、导出再导入。

use super::import::import_files;
use super::*;
use crate::agent::api::{
    self, ApiDestination, ApiEndpoint, ApiExample, ApiHeader, ApiInput, ApiMapping, ApiSecrets,
    ApiSuccess, BodyKind, InputKind,
};
use serde_json::{Map, Value, json};

fn secrets() -> ApiSecrets {
    ApiSecrets {
        secrets: [
            ("stat_token", "s3cr3t"),
            ("k", "key-1"),
            ("sig", "sig-1"),
            ("api_key", "pet-key"),
            ("token", "tok"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
    }
}

/// 实际发出去的请求：方法、地址（查询参数排好序）、请求头（排好序）、请求体、是不是表单。
fn request(
    endpoint: &ApiEndpoint,
    args: Value,
) -> (String, String, Vec<(String, String)>, Option<Value>, bool) {
    let args: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
    let prepared = api::prepare(endpoint, &args, &secrets())
        .unwrap_or_else(|e| panic!("{}：{e}", endpoint.id));
    let (base, query) = prepared
        .url
        .split_once('?')
        .map_or((prepared.url.clone(), String::new()), |(b, q)| {
            (b.to_string(), q.to_string())
        });
    let mut pairs: Vec<&str> = query.split('&').filter(|p| !p.is_empty()).collect();
    pairs.sort();
    let mut headers = prepared.headers.clone();
    headers.sort();
    (
        prepared.method.label().to_string(),
        format!("{base}?{}", pairs.join("&")),
        headers,
        prepared.body.clone(),
        prepared.form,
    )
}

fn input(name: &str, kind: InputKind, required: bool, example: &str) -> ApiInput {
    ApiInput {
        name: name.into(),
        kind,
        required,
        description: format!("{name} 的说明"),
        example: example.into(),
        ..ApiInput::default()
    }
}

/// 旧版 `apis.json` 里会出现的各种写法。
fn legacy_endpoints() -> Vec<ApiEndpoint> {
    vec![
        ApiEndpoint {
            id: "fire_stat".into(),
            name: "森林火灾统计".into(),
            description: "按地区与年份查".into(),
            url: "http://10.0.0.8:8080/api/stat?region={region}&year={year}".into(),
            inputs: vec![
                input("region", InputKind::Text, true, "全省"),
                input("year", InputKind::Number, false, "2025"),
            ],
            headers: vec![ApiHeader {
                name: "Authorization".into(),
                value: "Bearer {secret:stat_token}".into(),
            }],
            mapping: ApiMapping {
                list: "/data/items".into(),
                title: "{region}{year}年".into(),
                text: "{region}共{count}起".into(),
                source: "省统计系统".into(),
                id: "id".into(),
            },
            ..ApiEndpoint::default()
        },
        ApiEndpoint {
            id: "search_policy".into(),
            name: "政策检索".into(),
            method: api::ApiMethod::Post,
            url: "http://10.0.0.8:8080/api/search".into(),
            inputs: vec![
                input("keyword", InputKind::Text, true, "森林防火"),
                input("page", InputKind::Number, false, "1"),
                input("filters", InputKind::Json, false, r#"{"year":2025}"#),
            ],
            headers: vec![
                ApiHeader {
                    name: "X-Api-Key".into(),
                    value: "{secret:k}".into(),
                },
                ApiHeader {
                    name: "X-Client".into(),
                    value: "gongwen".into(),
                },
            ],
            body:
                r#"{"q": "{keyword}", "page": "{page}", "type": "policy", "filters": "{filters}"}"#
                    .into(),
            success: ApiSuccess {
                pointer: "/code".into(),
                equals: "0|200".into(),
            },
            destination: ApiDestination::Variable,
            timeout_seconds: 60,
            examples: vec![ApiExample {
                name: "例一".into(),
                note: "查第一页".into(),
                args: [("keyword", "森林防火"), ("page", "1")]
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                ..Default::default()
            }],
            ..ApiEndpoint::default()
        },
        // 同一地址同一方法、只是请求体里写死的类型不同：OpenAPI 放不下两个，进副本服务。
        ApiEndpoint {
            id: "search_law".into(),
            name: "法规检索".into(),
            method: api::ApiMethod::Post,
            url: "http://10.0.0.8:8080/api/search".into(),
            inputs: vec![input("keyword", InputKind::Text, true, "防火")],
            body: r#"{"q": "{keyword}", "type": "law"}"#.into(),
            ..ApiEndpoint::default()
        },
        // 改过名的查询参数、写死的参数、混着密钥的请求头、没用上的变量、路径变量。
        ApiEndpoint {
            id: "doc_detail".into(),
            name: "文件详情".into(),
            url: "http://10.0.0.8:8080/api/doc/{doc_id}?fmt=json&q={kw}".into(),
            inputs: vec![
                input("doc_id", InputKind::Text, true, "A-1"),
                input("kw", InputKind::Text, false, "防火"),
                input("extra", InputKind::Text, false, ""),
            ],
            headers: vec![ApiHeader {
                name: "X-Sign".into(),
                value: "abc-{secret:sig}".into(),
            }],
            ..ApiEndpoint::default()
        },
        // 变量嵌在请求体深处：拆不开，原样保留模板。
        ApiEndpoint {
            id: "deep_query".into(),
            name: "深层查询".into(),
            method: api::ApiMethod::Post,
            url: "http://other.host/api/q".into(),
            inputs: vec![input("cond", InputKind::Json, true, r#"{"a":1}"#)],
            body: r#"{"query": {"bool": "{cond}"}}"#.into(),
            ..ApiEndpoint::default()
        },
        // 还没填服务器地址。
        ApiEndpoint {
            id: "no_host".into(),
            name: "没地址".into(),
            url: "/api/nohost".into(),
            ..ApiEndpoint::default()
        },
    ]
}

fn full_args(id: &str) -> Value {
    match id {
        "fire_stat" => json!({"region": "全省", "year": 2025}),
        "search_policy" => json!({"keyword": "森林", "page": 2, "filters": {"y": 1}}),
        "search_law" => json!({"keyword": "法"}),
        "doc_detail" => json!({"doc_id": "A 1", "kw": "火"}),
        "deep_query" => json!({"cond": {"a": 1}}),
        _ => json!({}),
    }
}

/// 不影响请求的字段与输入（不含编译补上的 schema、OpenAPI 名字）。
fn meta(endpoint: &ApiEndpoint) -> Value {
    let inputs: Vec<Value> = endpoint
        .inputs
        .iter()
        .map(|i| {
            json!([
                i.name,
                i.kind.label(),
                i.required,
                i.description,
                i.example,
                i.comma
            ])
        })
        .collect();
    json!({
        "id": endpoint.id,
        "name": endpoint.name,
        "description": endpoint.description,
        "inputs": inputs,
        "mapping": serde_json::to_value(&endpoint.mapping).unwrap(),
        "success": [endpoint.success.pointer, endpoint.success.equals],
        "destination": endpoint.destination.label(),
        "timeout": endpoint.timeout_seconds,
        "examples": serde_json::to_value(&endpoint.examples).unwrap(),
        "readonly": endpoint.readonly,
        "ai": endpoint.ai,
        "body_kind": format!("{:?}", endpoint.body_kind),
    })
}

#[test]
fn legacy_endpoints_migrate_without_changing_requests() {
    let legacy = legacy_endpoints();
    let mut store = ApiStore {
        endpoints: legacy.clone(),
        ..ApiStore::default()
    };
    let notes = absorb(&mut store);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(store.endpoints.len(), legacy.len());
    for (before, after) in legacy.iter().zip(&store.endpoints) {
        assert_eq!(meta(before), meta(after), "{}", before.id);
        if before.url.starts_with("http") {
            assert_eq!(
                request(before, full_args(&before.id)),
                request(after, full_args(&before.id)),
                "{}",
                before.id
            );
        }
        assert!(after.origin.is_some());
    }
    let ids: Vec<&str> = store.services.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        4,
        "{ids:?}：主服务、副本、另一台服务器、未填地址"
    );
    // 再存一次：文件一个字都不变。
    let before = store.services.clone();
    absorb(&mut store);
    assert_eq!(store.services, before);

    let main = &store.services[0];
    let search = main.doc.pointer("/paths/~1api~1search/post").unwrap();
    assert_eq!(search["x-gongwen-id"], "search_policy");
    assert_eq!(search["x-gongwen-readonly"], true, "旧接口都当只查询");
    assert_eq!(search["x-gongwen-success"]["equals"], json!([0, 200]));
    assert!(
        search
            .pointer("/requestBody/content/application~1json/schema/properties/type/enum")
            .is_some(),
        "写死的字段是单值枚举"
    );
    let detail = main
        .doc
        .pointer("/paths/~1api~1doc~1{doc_id}/get")
        .unwrap_or_else(|| panic!("{:#}", main.doc["paths"]));
    let q = detail["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "q")
        .unwrap();
    assert_eq!(q["x-gongwen-name"], "kw", "参数名 q、变量名 kw");
    assert!(
        detail["x-gongwen-template"]["headers"].is_array(),
        "混着密钥的请求头原样留着"
    );
    assert_eq!(detail["x-gongwen-template"]["inputs"][0]["name"], "extra");
    assert!(
        main.doc
            .pointer("/components/securitySchemes/stat_token/scheme")
            .is_some(),
        "Bearer 写成 http bearer"
    );
}

#[test]
fn optional_inputs_are_left_out_and_arrays_expanded() {
    let endpoint = ApiEndpoint {
        id: "x".into(),
        name: "x".into(),
        method: api::ApiMethod::Post,
        url: "http://h/x?a={a}&ids={ids}&tags={tags}".into(),
        inputs: vec![
            input("a", InputKind::Text, false, ""),
            input("ids", InputKind::Json, false, ""),
            ApiInput {
                comma: true,
                ..input("tags", InputKind::Json, false, "")
            },
            input("b", InputKind::Text, false, ""),
        ],
        headers: vec![ApiHeader {
            name: "X-B".into(),
            value: "{b}".into(),
        }],
        body: r#"{"b": "{b}", "k": 1}"#.into(),
        ..ApiEndpoint::default()
    };
    let prepared = api::prepare(
        &endpoint,
        json!({"ids": [1, 2], "tags": ["x", "y"]})
            .as_object()
            .unwrap(),
        &secrets(),
    )
    .unwrap();
    assert_eq!(prepared.url, "http://h/x?ids=1&ids=2&tags=x,y");
    assert!(prepared.headers.is_empty(), "没给的请求头不发");
    assert_eq!(prepared.body, Some(json!({"k": 1})), "没给的字段不发");
    let form = ApiEndpoint {
        method: api::ApiMethod::Put,
        body_kind: BodyKind::Form,
        ..endpoint.clone()
    };
    let prepared = api::prepare(
        &form,
        json!({"b": "甲 乙"}).as_object().unwrap(),
        &secrets(),
    )
    .unwrap();
    assert!(prepared.form);
    assert_eq!(
        api::form_encode(prepared.body.as_ref().unwrap()),
        "b=%E7%94%B2%20%E4%B9%99&k=1"
    );
}

fn import(files: &[(&str, &str)]) -> (ApiStore, import::ImportOutcome) {
    let mut store = ApiStore::default();
    let files: Vec<(String, String)> = files
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect();
    let outcome = import_files(&mut store, &files);
    (store, outcome)
}

fn endpoint<'a>(store: &'a ApiStore, op: &str) -> &'a ApiEndpoint {
    store
        .endpoints
        .iter()
        .find(|e| e.id.ends_with(&format!(".{op}")))
        .unwrap_or_else(|| {
            panic!(
                "没有 {op}：{:?}",
                store.endpoints.iter().map(|e| &e.id).collect::<Vec<_>>()
            )
        })
}

const PETSTORE: &str = include_str!("fixtures/petstore30.json");

#[test]
fn openapi30_compiles_parameters_bodies_and_security() {
    let (store, outcome) = import(&[("petstore.json", PETSTORE)]);
    assert_eq!(outcome.added, 7, "{:?}", outcome.report);
    assert!(
        outcome.report.iter().any(|l| l.contains("暂时发不了")),
        "{:?}",
        outcome.report
    );
    let list = endpoint(&store, "listPets");
    assert_eq!(list.id, "petstore.listPets");
    assert_eq!(
        list.url,
        "http://10.1.1.1:8080/v1/pets?limit={limit}&tags={tags}"
    );
    assert_eq!(list.inputs[0].description, "最多几条");
    assert!(list.inputs[1].comma);
    assert!(list.readonly);
    assert_eq!(list.success.pointer, "/code");
    let (_, url, headers, _, _) = request(list, json!({"tags": ["a", "b"]}));
    assert_eq!(url, "http://10.1.1.1:8080/v1/pets?tags=a,b");
    assert_eq!(
        headers,
        [
            ("X-API-Key".to_string(), "pet-key".to_string()),
            ("X-Client".to_string(), "gongwen".to_string())
        ]
    );

    let search = endpoint(&store, "searchPets");
    assert!(!search.readonly, "POST 默认按会改数据");
    assert_eq!(
        search
            .inputs
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>(),
        ["kind", "age"]
    );
    assert!(search.inputs[0].required);
    assert_eq!(search.examples[0].name, "查狗");
    assert_eq!(search.examples[0].args["age"], "2");

    let show = endpoint(&store, "showPet");
    assert_eq!(show.inputs[0].kind, InputKind::Number);
    assert_eq!(show.inputs[0].example, "7");
    assert!(show.headers.is_empty(), "security: [] 表示不鉴权");
    assert!(!endpoint(&store, "deletePet").readonly);
    assert!(
        endpoint(&store, "uploadPhoto")
            .unsupported
            .contains("multipart")
    );
    let stat = endpoint(&store, "get_stat");
    assert_eq!(stat.name, "统计", "中文操作 id 挪去当名称");
    assert_eq!(
        stat.url,
        "http://10.1.1.1:8080/v1/stat?%E5%9C%B0%E5%8C%BA={p1}"
    );
}

#[test]
fn untouched_and_meta_edits_keep_the_document() {
    let (mut store, _) = import(&[("petstore.json", PETSTORE)]);
    let original = store.services[0].doc.clone();
    absorb(&mut store);
    assert_eq!(store.services[0].doc, original, "没改的不动");

    let index = store
        .endpoints
        .iter()
        .position(|e| e.id.ends_with("searchPets"))
        .unwrap();
    store.endpoints[index].description = "按种类、年龄查".into();
    store.endpoints[index].readonly = true;
    absorb(&mut store);
    let op = store.services[0].doc.pointer("/paths/~1pets/post").unwrap();
    assert_eq!(op["description"], "按种类、年龄查");
    assert_eq!(op["x-gongwen-readonly"], true);
    assert_eq!(
        op["requestBody"]["$ref"], "#/components/requestBodies/PetQuery",
        "引用留着"
    );
    assert!(store.endpoints[index].readonly);

    // 改了参数说明：请求部分重写，鉴权仍用文件里的方案，请求不变。
    let index = store
        .endpoints
        .iter()
        .position(|e| e.id.ends_with("listPets"))
        .unwrap();
    let before = store.endpoints[index].clone();
    store.endpoints[index].inputs[0].description = "每页条数".into();
    absorb(&mut store);
    let after = &store.endpoints[index];
    assert_eq!(after.inputs[0].description, "每页条数");
    assert_eq!(
        request(&before, json!({"limit": 5, "tags": ["a"]})),
        request(after, json!({"limit": 5, "tags": ["a"]}))
    );
    let op = store.services[0].doc.pointer("/paths/~1pets/get").unwrap();
    assert!(op.get("security").is_none(), "和根上的鉴权一样就不写");
    assert_eq!(op["tags"], json!(["pets"]), "没建模的字段留着");
    assert_eq!(
        op["parameters"][0]["schema"]["maximum"], 100,
        "原 schema 留着"
    );
    let schemes = store.services[0]
        .doc
        .pointer("/components/securitySchemes")
        .unwrap();
    assert_eq!(schemes.as_object().unwrap().len(), 1, "没有多出鉴权方案");

    // 删掉一个接口：文件里也没了。
    store.endpoints.retain(|e| !e.id.ends_with("deletePet"));
    absorb(&mut store);
    assert!(
        store.services[0]
            .doc
            .pointer("/paths/~1pets~1{petId}/delete")
            .is_none()
    );
    assert!(
        store.services[0]
            .doc
            .pointer("/paths/~1pets~1{petId}/get")
            .is_some()
    );
}

#[test]
fn changing_the_host_moves_the_service_address() {
    let (mut store, _) = import(&[("petstore.json", PETSTORE)]);
    let index = store
        .endpoints
        .iter()
        .position(|e| e.id.ends_with("listPets"))
        .unwrap();
    store.endpoints[index].url = store.endpoints[index]
        .url
        .replace("http://10.1.1.1:8080", "http://192.168.0.5");
    let notes = absorb(&mut store);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(store.services.len(), 1);
    assert!(
        store
            .endpoints
            .iter()
            .all(|e| e.url.starts_with("http://192.168.0.5/v1/")),
        "同一服务的接口都跟着换"
    );
    assert_eq!(
        store.services[0].doc["servers"][0]["url"], "http://{host}:8080/v1",
        "文件里的 servers 不动，地址记在环境里"
    );
}

#[test]
fn swagger2_is_converted() {
    let (store, outcome) = import(&[("s.json", include_str!("fixtures/swagger2.json"))]);
    assert_eq!(outcome.added, 3, "{:?}", outcome.report);
    let stat = endpoint(&store, "getStat");
    assert_eq!(
        stat.url,
        "http://10.2.2.2:9000/api/stat?regions={regions}&year={year}&access_token={secret:token}"
    );
    assert!(stat.inputs[0].comma, "2.0 的数组默认逗号连");
    assert_eq!(stat.inputs[1].example, "2025");
    let (_, url, ..) = request(stat, json!({"regions": ["甲", "乙"]}));
    assert_eq!(
        url,
        "http://10.2.2.2:9000/api/stat?access_token=tok&regions=%E7%94%B2,%E4%B9%99"
    );
    let query = endpoint(&store, "query");
    assert_eq!(query.inputs[0].name, "keyword");
    let form = endpoint(&store, "formQuery");
    assert_eq!(form.body_kind, BodyKind::Form);
    assert!(form.inputs[0].required);
    assert!(
        store.services[0]
            .doc
            .pointer("/paths/~1stat/get/responses/200/content/application~1json/schema/$ref")
            .is_some_and(|r| r == "#/components/schemas/Stat")
    );
}

#[test]
fn openapi31_yaml_is_downgraded() {
    let (store, outcome) = import(&[("s.yaml", include_str!("fixtures/stat31.yaml"))]);
    assert_eq!(outcome.added, 1, "{:?}", outcome.report);
    let items = endpoint(&store, "items");
    assert_eq!(items.url, "http://10.3.3.3/items?q={q}&mode=brief");
    assert_eq!(items.inputs[0].example, "森林");
    let doc = &store.services[0].doc;
    assert_eq!(doc["openapi"], "3.0.3");
    assert_eq!(
        doc.pointer("/paths/~1items/get/parameters/0/schema/nullable"),
        Some(&json!(true))
    );
}

#[test]
fn postman_collection_with_environment() {
    let (store, outcome) = import(&[
        ("c.json", include_str!("fixtures/postman.json")),
        ("e.json", include_str!("fixtures/postman_env.json")),
    ]);
    assert_eq!(outcome.added, 3, "{:?}", outcome.report);
    assert!(
        outcome.report.iter().any(|l| l.contains("脚本")),
        "{:?}",
        outcome.report
    );
    assert!(
        outcome
            .secrets
            .contains(&("token".to_string(), "abc123".to_string()))
    );
    let stat = store
        .endpoints
        .iter()
        .find(|e| e.name == "按地区查")
        .expect("按地区查");
    assert_eq!(
        stat.url,
        "http://10.4.4.4:8000/stat/{region}?year={year}&page={page}"
    );
    assert_eq!(stat.inputs[0].example, "全省");
    assert!(stat.inputs[1].required && !stat.inputs[2].required);
    assert!(
        stat.headers
            .iter()
            .any(|h| h.value == "Bearer {secret:token}")
    );
    assert!(
        stat.headers
            .iter()
            .any(|h| h.name == "X-App" && h.value == "gw")
    );
    let origin = stat.origin.as_ref().unwrap();
    assert_eq!(origin.op["tags"], json!(["统计"]));
    let search = store.endpoints.iter().find(|e| e.name == "检索").unwrap();
    assert_eq!(
        search
            .inputs
            .iter()
            .map(|i| (i.name.as_str(), i.kind))
            .collect::<Vec<_>>(),
        [("keyword", InputKind::Text), ("size", InputKind::Number)]
    );
    let form = store
        .endpoints
        .iter()
        .find(|e| e.name == "表单查询")
        .unwrap();
    assert_eq!(form.body_kind, BodyKind::Form);
    assert!(form.headers.is_empty(), "noauth 不带鉴权");
}

#[test]
fn export_then_import_compiles_the_same() {
    let (store, _) = import(&[
        ("petstore.json", PETSTORE),
        ("c.json", include_str!("fixtures/postman.json")),
        ("e.json", include_str!("fixtures/postman_env.json")),
    ]);
    let mut legacy = ApiStore {
        endpoints: legacy_endpoints(),
        ..ApiStore::default()
    };
    absorb(&mut legacy);
    for source in [store, legacy] {
        let files: Vec<(String, String)> = export(&source)
            .into_iter()
            .map(|(name, doc)| (name, to_text(&doc, true).unwrap()))
            .collect();
        let mut again = ApiStore::default();
        import_files(&mut again, &files);
        let shape = |s: &ApiStore| -> Vec<Value> {
            let mut all: Vec<Value> = s
                .endpoints
                .iter()
                .map(|e| {
                    let mut m = meta(e);
                    m["id"] = json!(e.id.rsplit('.').next());
                    m["url"] = json!(e.url);
                    m["headers"] = serde_json::to_value(&e.headers).unwrap();
                    m["body"] = json!(e.body);
                    m
                })
                .collect();
            all.sort_by_key(|v| v.to_string());
            all
        };
        assert_eq!(shape(&source), shape(&again));
    }
}

#[test]
fn write_operations_are_not_tried_or_called() {
    let (store, _) = import(&[("petstore.json", PETSTORE)]);
    let delete = endpoint(&store, "deletePet");
    let trial = api::trial(delete, &Map::new(), &secrets());
    assert!(trial.error.unwrap().contains("会改数据"));
    assert!(trial.request.is_none(), "没发出去");
}

#[test]
fn write_operations_send_only_by_hand() {
    let server = api::test_server::TestServer::start(vec![(200, r#"{"code": 0}"#.into())]);
    let (mut store, _) = import(&[("petstore.json", PETSTORE)]);
    let index = store
        .endpoints
        .iter()
        .position(|e| e.id.ends_with("deletePet"))
        .unwrap();
    store.endpoints[index].url = store.endpoints[index]
        .url
        .replace("http://10.1.1.1:8080/v1", &server.url);
    let delete = &store.endpoints[index];
    let args = json!({"petId": 7}).as_object().cloned().unwrap();
    let refused = api::trial(delete, &args, &secrets());
    assert!(refused.request.is_none(), "自动试调不发");
    let sent = api::trial_by_hand(delete, &args, &secrets());
    assert!(sent.ok(), "{:?}", sent.error);
    assert!(
        server.request(0).starts_with("DELETE /pets/7"),
        "{}",
        server.request(0)
    );
}

#[test]
fn case_expectations_round_trip_through_openapi() {
    use super::cases::Expect;
    let (mut store, _) = import(&[("petstore.json", PETSTORE)]);
    let index = store
        .endpoints
        .iter()
        .position(|e| e.id.ends_with("listPets"))
        .unwrap();
    store.endpoints[index].examples.push(ApiExample {
        name: "查标签".into(),
        args: [("tags".to_string(), r#"["dog"]"#.to_string())].into(),
        expect: vec![
            Expect::MinItems {
                pointer: String::new(),
                value: 1,
            },
            Expect::Equals {
                pointer: "/0/kind".into(),
                value: json!("dog"),
            },
        ],
        ..Default::default()
    });
    absorb(&mut store);
    let op = store.services[0].doc.pointer("/paths/~1pets/get").unwrap();
    assert_eq!(
        op["x-gongwen-tests"][0]["expect"],
        json!([
            {"kind": "min_items", "pointer": "", "value": 1},
            {"kind": "equals", "pointer": "/0/kind", "value": "dog"}
        ])
    );
    assert_eq!(
        store.endpoints[index].examples[0].expect.len(),
        2,
        "读回来不变"
    );
    // 导出再导入，期望跟着文件走。
    let files: Vec<(String, String)> = export(&store)
        .into_iter()
        .map(|(name, doc)| (name, to_text(&doc, false).unwrap()))
        .collect();
    let mut again = ApiStore::default();
    import_files(&mut again, &files);
    let list = endpoint(&again, "listPets");
    assert_eq!(
        list.examples[0].expect,
        store.endpoints[index].examples[0].expect
    );
}

#[test]
fn ai_cases_round_trip_through_openapi() {
    let (mut store, _) = import(&[("petstore.json", PETSTORE)]);
    let index = store
        .endpoints
        .iter()
        .position(|e| e.id.ends_with("listPets"))
        .unwrap();
    store.endpoints[index].ai_cases.push(api::ApiAiCase {
        question: "最多列 5 只宠物".into(),
        expect_args: [("limit".to_string(), json!(5))].into(),
    });
    let before = store.endpoints[index].fingerprint();
    absorb(&mut store);
    let op = store.services[0].doc.pointer("/paths/~1pets/get").unwrap();
    assert_eq!(
        op["x-gongwen-ai-tests"],
        json!([{"question": "最多列 5 只宠物", "expect": {"args": {"limit": 5}}}])
    );
    assert_eq!(store.endpoints[index].ai_cases.len(), 1);
    assert_eq!(
        store.endpoints[index].fingerprint(),
        before,
        "加 AI 用例不让接口测试记录过期"
    );
    let files: Vec<(String, String)> = export(&store)
        .into_iter()
        .map(|(name, doc)| (name, to_text(&doc, true).unwrap()))
        .collect();
    let mut again = ApiStore::default();
    import_files(&mut again, &files);
    assert_eq!(
        endpoint(&again, "listPets").ai_cases,
        store.endpoints[index].ai_cases
    );
}

#[test]
fn status_prefers_a_fresh_case_suite() {
    let endpoint = legacy_endpoints().remove(0);
    let mut log = api::ApiTestLog::default();
    assert!(matches!(log.status(&endpoint), api::TestStatus::Untested));
    let ok = api::Trial {
        raw: Some(api::RawResponse {
            status: 200,
            body: "{}".into(),
        }),
        ..api::Trial::default()
    };
    log.record(&endpoint, &ok);
    log.record_suite(
        &endpoint,
        &[
            ("甲".into(), ok.clone(), Ok(())),
            (
                "乙".into(),
                ok.clone(),
                Err("期望 /a 不为空，实际为空".into()),
            ),
        ],
    );
    match log.status(&endpoint) {
        api::TestStatus::Failed(record) => {
            assert_eq!(record.case_counts(), (1, 2));
            assert!(record.summary.contains("「乙」"), "{}", record.summary);
        }
        _ => panic!("整组用例优先"),
    }
    // 配置改了：整组结论过期；再单次试调一次，显示单次的。
    let mut changed = endpoint.clone();
    changed.url.push_str("&x=1");
    assert!(matches!(log.status(&changed), api::TestStatus::Stale(_)));
    log.record(&changed, &ok);
    assert!(matches!(log.status(&changed), api::TestStatus::Passed(r) if r.cases.is_empty()));
}

#[test]
fn curl_and_unknown_files() {
    let (store, outcome) = import(&[
        (
            "a.sh",
            "curl 'http://10.9.9.9/api/list?page=1' -H 'X-Token: abc'",
        ),
        ("b.txt", "随便一段话"),
    ]);
    assert_eq!(outcome.added, 1, "{:?}", outcome.report);
    assert!(
        store.endpoints[0]
            .url
            .starts_with("http://10.9.9.9/api/list")
    );
    assert!(outcome.report.iter().any(|l| l.contains("认不出格式")));
}

#[test]
fn store_loads_migrates_and_saves_files() {
    let dir = std::env::temp_dir().join(format!("gongwen-apidef-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    crate::storage::set_test_config_dir(Some(dir.clone()));
    let legacy = ApiStore {
        endpoints: legacy_endpoints(),
        ..ApiStore::default()
    };
    std::fs::write(
        dir.join("apis.json"),
        serde_json::to_string(&legacy).unwrap(),
    )
    .unwrap();
    let mut store = ApiStore::load().unwrap();
    assert!(dir.join("apis.json.bak").exists() && !dir.join("apis.json").exists());
    assert_eq!(
        store
            .endpoints
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>()
            .len(),
        legacy.endpoints.len()
    );
    let files = std::fs::read_dir(dir.join("apis")).unwrap().count();
    assert_eq!(files, 4);
    // 删到只剩一个服务的接口：别的服务文件跟着删。
    store.endpoints.retain(|e| e.id == "fire_stat");
    store.save().unwrap();
    assert_eq!(std::fs::read_dir(dir.join("apis")).unwrap().count(), 1);
    assert_eq!(ApiStore::load().unwrap(), store);
    crate::storage::set_test_config_dir(None);
    let _ = std::fs::remove_dir_all(dir);
}
