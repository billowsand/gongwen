//! 维基作业真机评测（`#[ignore]`，默认不跑）：用真实模型服务商把
//! 「归纳框架 → 整编页面 → 更新页面」三个 Pi 作业整条链跑一遍，
//! 验证 host 交付、确定性复核与写库采纳全链路在真实模型下成立。
//!
//! 运行方式（密钥只从环境变量读，不落库、不回显）：
//!
//! ```bash
//! export DMXAPI_API_KEY=...        # 或任何 OpenAI 兼容服务商的密钥
//! source scripts/pi-dev.sh         # 准备 GONGWEN_PI_RUNTIME_DIR / GONGWEN_PI_NODE
//! # 可选：GONGWEN_WIKI_EVAL_BASE 覆盖服务商地址，GONGWEN_WIKI_EVAL_MODEL 覆盖模型名
//! cargo test --locked wiki_real_model -- --ignored --nocapture
//! ```
//!
//! 测试把配置目录重定向到独立临时目录（线程局部），资料库、维基库与
//! Pi 工作区全部落在里面，不碰本机真实数据；结束后保留目录供检查。

use super::wiki_jobs::{induce_frame, update_pages, write_pages};
use crate::knowledge::{KnowledgeImportItem, KnowledgeSource, KnowledgeStore, NewKnowledgeChunk};
use crate::models::{AppConfig, ModelRef, ProviderConfig, TemplateKind};
use crate::wiki::{PageInput, WikiStore};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 评测资料：安全生产域的五篇公文，主题相近但侧重不同，
/// 便于框架归纳切出「责任制度 / 监督检查 / 应急防汛」等分类。
const SEED_DOCS: [(&str, &str); 5] = [
    (
        "关于开展2025年安全生产大检查的通知",
        "# 关于开展2025年安全生产大检查的通知\n\n\
         ## 一、检查范围\n\n\
         各县（区）人民政府，市安委会各成员单位：定于2025年3月至6月在全市开展安全生产大检查，\
         覆盖危险化学品、矿山、建筑施工、交通运输、消防等重点行业领域。\n\n\
         ## 二、检查内容\n\n\
         一是企业安全生产主体责任落实情况，包括安全生产责任制、安全投入、教育培训；\
         二是重大事故隐患排查治理情况，建立隐患台账，实行闭环管理；\
         三是应急预案编制与演练情况。\n\n\
         ## 三、工作要求\n\n\
         各级各部门要高度重视，主要负责人亲自部署；对检查发现的重大隐患实行挂牌督办，\
         限期整改到位。市安委会办公室将于7月组织回头看。",
    ),
    (
        "安全生产责任制实施办法",
        "# 安全生产责任制实施办法\n\n\
         ## 第一章 总则\n\n\
         为落实安全生产责任制，坚持党政同责、一岗双责、齐抓共管、失职追责，制定本办法。\n\n\
         ## 第二章 责任体系\n\n\
         生产经营单位主要负责人是本单位安全生产第一责任人；分管安全生产的负责人协助\
         主要负责人履行安全生产管理职责；班组和岗位职工对本岗位安全生产负责。\n\n\
         ## 第三章 考核与问责\n\n\
         安全生产责任制落实情况纳入年度考核；对失职渎职导致生产安全事故的，\
         依规依纪依法追究责任。",
    ),
    (
        "2025年防汛抗旱工作总结",
        "# 2025年防汛抗旱工作总结\n\n\
         ## 一、雨情水情\n\n\
         2025年汛期全市平均降雨量较常年偏多两成，主要江河发生3次编号洪水。\n\n\
         ## 二、主要做法\n\n\
         一是汛前完成水库、堤防隐患排查，整改隐患47处；二是落实以行政首长负责制\
         为核心的防汛责任制，明确包保责任人；三是组织防汛抢险演练120场次，\
         参演人数1.2万人次。\n\n\
         ## 三、下一步安排\n\n\
         抓紧修复水毁工程，完善山洪灾害监测预警系统，做好冬春雨季蓄水保水工作。",
    ),
    (
        "消防安全专项整治方案",
        "# 消防安全专项整治方案\n\n\
         ## 一、整治目标\n\n\
         用一年时间集中整治高层建筑、地下空间、人员密集场所消防安全隐患，\
         坚决遏制重特大火灾事故。\n\n\
         ## 二、整治重点\n\n\
         一是疏散通道、安全出口畅通情况；二是消防设施器材完好有效情况；\
         三是电动自行车违规充电、违规住人问题；四是消防安全责任制和微型消防站建设情况。\n\n\
         ## 三、进度安排\n\n\
         2025年4月动员部署，5月至11月集中整治，12月检查验收。",
    ),
    (
        "突发事件应急预案管理办法",
        "# 突发事件应急预案管理办法\n\n\
         ## 一、预案体系\n\n\
         应急预案体系由总体应急预案、专项应急预案、部门应急预案和单位应急预案构成。\n\n\
         ## 二、编制要求\n\n\
         编制预案应当开展风险评估和应急资源调查，明确组织指挥体系、响应分级、\
         处置措施和保障措施。\n\n\
         ## 三、演练与修订\n\n\
         专项应急预案每三年至少演练一次；有下列情形之一的，应当及时修订：\
         有关法律、法规、规章发生变化的，应急指挥机构及其职责发生重大调整的。",
    ),
];

/// 整编完成后再入库的「新资料」：口径明确取代种子文档里的旧数字，
/// 供更新作业发现「旧提法被新资料取代」的改动点。
const UPDATE_DOC: (&str, &str) = (
    "关于开展2026年安全生产大检查的通知",
    "# 关于开展2026年安全生产大检查的通知\n\n\
     ## 一、检查安排\n\n\
     定于2026年3月至7月在全市开展安全生产大检查。与2025年相比，\
     本轮检查新增燃气、经营性自建房两个重点领域，检查时长由三个月延长为四个月。\n\n\
     ## 二、检查内容\n\n\
     继续检查企业安全生产主体责任落实、重大事故隐患排查治理、应急预案编制与演练情况；\
     新增城镇燃气管道老化更新改造、经营性自建房结构安全两项内容。\n\n\
     ## 三、工作要求\n\n\
     重大隐患实行清单化管理、销号式整改，2026年8月底前全部整改到位；\
     回头看安排在2026年9月。",
);

fn eval_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// 把一篇 Markdown 资料入库并补上 FTS 切块（不走嵌入，与索引流水线的
/// 关键词路一致：tokens 为 jieba 分词后的复合检索文本）。
fn seed_doc(store: &mut KnowledgeStore, title: &str, content: &str) -> i64 {
    let id = store
        .import_document(&KnowledgeImportItem {
            source: KnowledgeSource::Markdown,
            source_manuscript_id: None,
            source_path: format!("eval/{title}.md"),
            kind: TemplateKind::PlainDocument,
            title: title.to_string(),
            content_markdown: content.to_string(),
        })
        .expect("资料入库");
    let params = crate::rag::ChunkParams::from(&crate::models::RagConfig::default());
    let chunks: Vec<NewKnowledgeChunk> = crate::rag::chunk_markdown(content, &params)
        .into_iter()
        .map(|chunk| {
            let mut text = title.to_string();
            let section = chunk.section.trim();
            if !section.is_empty() && section != title {
                text.push('\n');
                text.push_str(section);
            }
            text.push('\n');
            text.push_str(&chunk.text);
            NewKnowledgeChunk {
                ord: chunk.ord,
                section: chunk.section,
                text: chunk.text,
                tokens: crate::rag::tokenize(&text),
                embedding: None,
                dims: 0,
            }
        })
        .collect();
    store.replace_chunks(id, &chunks, "").expect("FTS 切块");
    id
}

fn eval_config() -> Arc<AppConfig> {
    let api_key = eval_env("DMXAPI_API_KEY").expect("缺少环境变量 DMXAPI_API_KEY");
    let base = eval_env("GONGWEN_WIKI_EVAL_BASE")
        .unwrap_or_else(|| "https://www.dmxapi.cn/v1".to_string());
    let model =
        eval_env("GONGWEN_WIKI_EVAL_MODEL").unwrap_or_else(|| "deepseek-v4-flash".to_string());
    let mut config = AppConfig::default();
    config.providers.push(ProviderConfig {
        id: "eval".into(),
        name: "评测服务商".into(),
        base_url: base,
        api_key,
        enabled: true,
        ..Default::default()
    });
    config.pi_model = ModelRef {
        provider_id: "eval".into(),
        model,
    };
    // 知识库检索开关：作业靠 kb_search / kb_read 找资料。
    config.rag.enabled = true;
    Arc::new(config)
}

/// 真机全链：归纳框架 → 采纳框架 → 整编分类页面 → 采纳页面 →
/// 补一篇新资料 → 更新页面 → 采纳更新。每一步都过确定性复核，
/// 任何一步被复核拦下或模型没交回交付，测试直接失败并保留现场。
#[test]
#[ignore = "真机评测：需要 DMXAPI_API_KEY 与 pi-dev 运行时，默认不跑"]
fn wiki_real_model_end_to_end() {
    for var in ["GONGWEN_PI_RUNTIME_DIR", "GONGWEN_PI_NODE"] {
        if eval_env(var).is_none() {
            panic!("缺少环境变量 {var}；先 source scripts/pi-dev.sh");
        }
    }
    let dir = std::env::temp_dir().join(format!("gongwen_wiki_eval_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    crate::storage::set_test_config_dir_in_current_thread(dir.clone());
    // Pi 工具在 process 派生的线程上执行，线程局部覆盖到不了那里；
    // 必须再挂进程级覆盖，否则工具会去开本机真实库（加密库按明文读直接报
    // 「file is not a database」——第二轮评测就是这样失败的）。
    let _global = crate::storage::push_test_config_dir_global(dir.clone());
    eprintln!("评测目录：{}", dir.display());

    let store_path = crate::storage::manuscript_db_path().unwrap();
    let mut kb = KnowledgeStore::open(&store_path).unwrap();
    let mut doc_ids = Vec::new();
    for (title, content) in SEED_DOCS {
        doc_ids.push(seed_doc(&mut kb, title, content));
    }
    eprintln!("已入库 {} 篇评测资料：{doc_ids:?}", doc_ids.len());
    drop(kb);

    let config = eval_config();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = |line: String| eprintln!("[pi] {line}");

    // ① 归纳框架。
    let frame = induce_frame(
        Arc::clone(&config),
        store_path.clone(),
        Arc::clone(&cancel),
        progress,
    )
    .expect("框架归纳未通过复核");
    assert!(!frame.themes.is_empty(), "框架提案没有主题");
    let mut categories = 0usize;
    for theme in &frame.themes {
        assert!(!theme.name.trim().is_empty());
        for category in &theme.categories {
            categories += 1;
            assert!(!category.name.trim().is_empty());
            assert!(
                !category.doc_ids.is_empty(),
                "分类「{}」没有依据资料",
                category.name
            );
            for id in &category.doc_ids {
                assert!(doc_ids.contains(id), "分类依据了未入库的文档 {id}");
            }
        }
    }
    assert!(categories > 0, "框架提案没有分类");
    eprintln!(
        "① 框架：{} 个主题 / {categories} 个分类",
        frame.themes.len()
    );

    // 采纳框架（与界面「确认入库」同一条路径）。
    let wiki = WikiStore::open(&store_path).unwrap();
    let (created, reused) = wiki.apply_frame(&frame.themes).unwrap();
    eprintln!("① 框架入库：新建 {created} 个节点，复用 {reused} 个节点");
    let tree = wiki.list_tree().unwrap();
    let (node_id, node_name) = tree
        .iter()
        .flat_map(|t| t.categories.iter().map(move |c| (c.id, c.name.as_str())))
        .next()
        .expect("框架里没有分类节点");
    eprintln!("② 整编目标分类：「{node_name}」(node {node_id})");

    // ② 整编该分类的页面（含导览页）。
    let proposal = write_pages(
        Arc::clone(&config),
        store_path.clone(),
        node_id,
        Vec::new(),
        Arc::clone(&cancel),
        progress,
    )
    .expect("页面整编未通过复核");
    assert!(!proposal.pages.is_empty(), "整编提案没有页面");
    let seeded: std::collections::HashSet<i64> = doc_ids.iter().copied().collect();
    let mut adopted = Vec::new();
    for page in &proposal.pages {
        assert!(page.uuid.is_none() && page.base_version.is_none());
        for (doc_id, _) in &page.doc_refs {
            assert!(seeded.contains(doc_id), "页面依据了未入库的文档 {doc_id}");
        }
        let kind = if page.kind == "guide" {
            "guide"
        } else {
            "normal"
        };
        let uuid = wiki
            .create_page(&PageInput {
                title: page.title.clone(),
                node_id,
                kind,
                body: page.body.clone(),
                source_run_id: proposal.run_id.clone(),
                review_edited: false,
                doc_refs: page.doc_refs.clone(),
                base_version: None,
            })
            .unwrap_or_else(|error| panic!("采纳页面「{}」失败：{error:#}", page.title));
        adopted.push((uuid, page.title.clone(), page.kind.clone()));
        eprintln!(
            "② 采纳页面「{}」（{}，依据 {} 篇）",
            page.title,
            page.kind,
            page.doc_refs.len()
        );
    }
    assert!(
        adopted.iter().any(|(_, _, kind)| kind == "guide"),
        "整编提案里没有导览页"
    );

    // ③ 补一篇口径更新的新资料，跑更新作业。
    let mut kb = KnowledgeStore::open(&store_path).unwrap();
    let new_doc = seed_doc(&mut kb, UPDATE_DOC.0, UPDATE_DOC.1);
    drop(kb);
    eprintln!("③ 新资料入库：E{new_doc}「{}」", UPDATE_DOC.0);
    let (uuid, title, _) = adopted
        .iter()
        .find(|(_, _, kind)| kind == "normal")
        .cloned()
        .unwrap_or_else(|| adopted[0].clone());
    let updates = update_pages(
        Arc::clone(&config),
        store_path.clone(),
        vec![(uuid.clone(), title.clone())],
        Some(new_doc),
        Arc::clone(&cancel),
        progress,
    )
    .expect("页面更新未通过复核");
    assert!(!updates.pages.is_empty(), "更新提案没有页面");
    let update = &updates.pages[0];
    assert_eq!(update.uuid.as_deref(), Some(uuid.as_str()));
    assert!(update.base_version.is_some(), "更新提案缺 base_version");
    assert_ne!(update.body, "", "更新正文为空");
    wiki.update_page(
        &uuid,
        &PageInput {
            title: update.title.clone(),
            node_id: updates.node_id,
            kind: if update.kind == "guide" {
                "guide"
            } else {
                "normal"
            },
            body: update.body.clone(),
            source_run_id: updates.run_id.clone(),
            review_edited: false,
            doc_refs: update.doc_refs.clone(),
            base_version: update.base_version,
        },
    )
    .expect("采纳更新失败");
    let page = wiki.get_page(&uuid).unwrap().expect("页面不见了");
    let versions = wiki.list_versions(&uuid).unwrap();
    assert_eq!(versions.len(), 2, "采纳更新后应有两个版本");
    eprintln!(
        "③ 采纳更新「{title}」：v1 → v{}，变更说明：{}",
        page.meta.current_version, update.change_note
    );
    assert!(!cancel.load(Ordering::Relaxed));
    eprintln!("真机全链通过；现场保留在 {}", dir.display());
}
