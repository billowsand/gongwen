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
//! # 可选：GONGWEN_WIKI_EVAL_MATERIAL_DIR 指向一批真实资料所在目录（只读 .md），
//! #       设了就在内置种子资料之外再跑一条「真实资料」全链；
//! #       GONGWEN_WIKI_EVAL_MATERIAL_FILES 可用逗号指定文件名（默认常挑的几篇见下）。
//! cargo test --locked wiki_real_model -- --ignored --nocapture
//! ```
//!
//! 测试把配置目录重定向到独立临时目录（线程局部 + 进程级），资料库、维基库与
//! Pi 工作区全部落在里面，不碰本机真实数据；结束后保留目录供检查。

use super::wiki_jobs::{induce_frame, update_pages, write_pages};
use crate::knowledge::{KnowledgeImportItem, KnowledgeSource, KnowledgeStore, NewKnowledgeChunk};
use crate::models::{AppConfig, ModelRef, ProviderConfig, TemplateKind};
use crate::wiki::{PageInput, WikiStore};
use std::path::{Path, PathBuf};
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

/// 真实资料目录里默认优先挑的几篇（同属《Project Maven》主题，便于归纳出
/// 一条连贯的两级框架）；找不到时退回目录里按名字排序的前几篇 .md。
const DEFAULT_MATERIAL_FILES: [&str; 5] = [
    "maven-bureaucracy-analysis.md",
    "maven-china-narrative-analysis.md",
    "maven-data-governance-analysis.md",
    "maven-intelligence-analysis-insights.md",
    "maven-narrative-to-action.md",
];

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

/// 从外部目录读评测资料：优先 `GONGWEN_WIKI_EVAL_MATERIAL_FILES` 指定文件，
/// 其次默认常挑的几篇，最后退回目录内按名字排序的 .md（至少两篇）。
fn load_material_docs(dir: &Path) -> Vec<(String, String)> {
    const MAX_BYTES: u64 = 512 * 1024;
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(list) = eval_env("GONGWEN_WIKI_EVAL_MATERIAL_FILES") {
        for name in list.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let path = dir.join(name);
            assert!(path.is_file(), "评测资料文件不存在：{}", path.display());
            paths.push(path);
        }
    }
    if paths.is_empty() {
        for name in DEFAULT_MATERIAL_FILES {
            let path = dir.join(name);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    if paths.len() < 2 {
        let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("读不到评测资料目录 {}：{error}", dir.display()))
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            })
            .collect();
        found.sort();
        paths = found;
    }
    assert!(
        paths.len() >= 2,
        "真实资料评测至少需要两篇 .md：{} 下只找到 {} 篇",
        dir.display(),
        paths.len()
    );
    paths
        .iter()
        .map(|path| {
            let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            assert!(
                bytes <= MAX_BYTES,
                "评测资料过大（{bytes} 字节）：{}",
                path.display()
            );
            let content = std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()));
            let title = content
                .lines()
                .find_map(|line| line.strip_prefix("# ").map(str::trim))
                .filter(|title| !title.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    path.file_stem()
                        .map(|stem| stem.to_string_lossy().to_string())
                        .unwrap_or_else(|| "评测资料".to_string())
                });
            (title, content)
        })
        .collect()
}

/// 一条完整链的统计，供两个用例做各自的断言语境。
struct ChainReport {
    pages: usize,
    updated: bool,
    updated_refs: Vec<i64>,
}

/// 跑一条「种子资料 → 归纳框架 → 整编并采纳 → （可选）补新资料并更新采纳」
/// 的真机链，最后做一轮确定性治理检查（依据删除后页面进需复核、退出自动取证）。
fn run_chain(
    tag: &str,
    seed_docs: &[(String, String)],
    update_doc: Option<&(String, String)>,
) -> ChainReport {
    let dir =
        std::env::temp_dir().join(format!("gongwen_wiki_eval_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    crate::storage::set_test_config_dir_in_current_thread(dir.clone());
    // Pi 工具在 process 派生的线程上执行，线程局部覆盖到不了那里；
    // 必须再挂进程级覆盖，否则工具会去开本机真实库（加密库按明文读直接报
    // 「file is not a database」——第二轮评测就是这样失败的）。
    let _global = crate::storage::push_test_config_dir_global(dir.clone());
    eprintln!("[{tag}] 评测目录：{}", dir.display());
    eprintln!("[{tag}] 种子资料 {} 篇", seed_docs.len());

    let store_path = crate::storage::manuscript_db_path().unwrap();
    let mut kb = KnowledgeStore::open(&store_path).unwrap();
    let mut doc_ids = Vec::new();
    for (title, content) in seed_docs {
        doc_ids.push(seed_doc(&mut kb, title, content));
    }
    drop(kb);

    let config = eval_config();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = |line: String| eprintln!("[{tag}] {line}");

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
        "[{tag}] ① 框架：{} 个主题 / {categories} 个分类",
        frame.themes.len()
    );

    // 采纳框架（与界面「确认入库」同一条路径）。
    let wiki = WikiStore::open(&store_path).unwrap();
    let (created, reused) = wiki.apply_frame(&frame.themes).unwrap();
    eprintln!("[{tag}] ① 框架入库：新建 {created} 个节点，复用 {reused} 个节点");
    let tree = wiki.list_tree().unwrap();
    let (node_id, node_name) = tree
        .iter()
        .flat_map(|t| t.categories.iter().map(move |c| (c.id, c.name.as_str())))
        .next()
        .expect("框架里没有分类节点");
    eprintln!("[{tag}] ② 整编目标分类：「{node_name}」(node {node_id})");

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
            "[{tag}] ② 采纳页面「{}」（{}，依据 {} 篇）",
            page.title,
            page.kind,
            page.doc_refs.len()
        );
    }
    assert!(
        adopted.iter().any(|(_, _, kind)| kind == "guide"),
        "整编提案里没有导览页"
    );

    // ③ 可选：补一篇口径更新的新资料，跑更新作业并采纳。
    let mut updated = false;
    let mut updated_refs: Vec<i64> = Vec::new();
    let mut update_uuid = None;
    if let Some((title, content)) = update_doc {
        let mut kb = KnowledgeStore::open(&store_path).unwrap();
        let new_doc = seed_doc(&mut kb, title, content);
        drop(kb);
        eprintln!("[{tag}] ③ 新资料入库：E{new_doc}「{title}」");
        let (uuid, page_title, _) = adopted
            .iter()
            .find(|(_, _, kind)| kind == "normal")
            .cloned()
            .unwrap_or_else(|| adopted[0].clone());
        let before = wiki.get_page(&uuid).unwrap().expect("更新前页面不见了");
        let updates = update_pages(
            Arc::clone(&config),
            store_path.clone(),
            vec![(uuid.clone(), page_title.clone())],
            Some(new_doc),
            Arc::clone(&cancel),
            progress,
        )
        .expect("页面更新未通过复核");
        assert!(!updates.pages.is_empty(), "更新提案没有页面");
        let update = &updates.pages[0];
        assert_eq!(update.uuid.as_deref(), Some(uuid.as_str()));
        // 基线必须是发起时读到的当前版本，采纳时拿到的是这一版，防止陈旧覆盖。
        assert_eq!(
            update.base_version,
            Some(before.meta.current_version),
            "更新提案的 base_version 不是发起时的当前版本"
        );
        assert_ne!(update.body, before.body, "更新正文与更新前逐字相同");
        assert!(
            update.doc_refs.iter().any(|(id, _)| *id == new_doc),
            "更新提案没有把新资料 E{new_doc} 写进依据清单"
        );
        updated_refs = update.doc_refs.iter().map(|(id, _)| *id).collect();
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
            "[{tag}] ③ 采纳更新「{page_title}」：v1 → v{}，变更说明：{}",
            page.meta.current_version, update.change_note
        );
        updated = true;
        update_uuid = Some(uuid);
    }

    // ④ 确定性治理检查（不依赖模型）：资料删除后，引用它的页面必须进
    // 「需复核」并退出问答 / Pi 自动取证（人工阅读仍可用）。
    let (check_uuid, _check_title) = adopted
        .iter()
        .find(|(uuid, _, kind)| *kind == "normal" && update_uuid.as_deref() != Some(uuid.as_str()))
        .map(|(uuid, title, _)| (uuid.clone(), title.clone()))
        .or_else(|| adopted.first().map(|(u, t, _)| (u.clone(), t.clone())))
        .expect("没有可检查的页面");
    let refs = wiki.page_doc_refs(&check_uuid).unwrap();
    assert!(!refs.is_empty(), "采纳页面没有依据");
    let victim = refs[0].0;
    let before_search = wiki.search(adopted[0].1.as_str(), 10).unwrap();
    assert!(!before_search.is_empty(), "采纳后的页面应能被检索到");
    let mut kb = KnowledgeStore::open(&store_path).unwrap();
    kb.delete_documents(&[victim]).expect("删除资料失败");
    drop(kb);
    assert!(
        wiki.pages_needing_review()
            .unwrap()
            .iter()
            .any(|(uuid, _)| *uuid == check_uuid),
        "依据被删除的页面没有进入需复核清单"
    );
    assert!(
        !wiki.page_evidence_ready(&check_uuid).unwrap(),
        "依据被删除的页面仍在自动取证范围"
    );
    let after_title = wiki
        .get_page(&check_uuid)
        .unwrap()
        .map(|page| page.meta.title)
        .unwrap_or_default();
    assert!(
        wiki.search_for_evidence(&after_title, 10)
            .unwrap()
            .iter()
            .all(|hit| hit.uuid != check_uuid),
        "需复核页面仍被 search_for_evidence 召回"
    );
    eprintln!("[{tag}] ④ 治理检查：删除 E{victim} 后页面进入需复核并退出自动取证");

    assert!(!cancel.load(Ordering::Relaxed));
    eprintln!("[{tag}] 真机全链通过；现场保留在 {}", dir.display());
    ChainReport {
        pages: adopted.len(),
        updated,
        updated_refs,
    }
}

/// 真机全链：归纳框架 → 采纳框架 → 整编分类页面 → 采纳页面 →
/// 补一篇新资料 → 更新页面 → 采纳更新。每一步都过确定性复核，
/// 任何一步被复核拦下或模型没交回交付，测试直接失败并保留现场。
///
/// 设了 `GONGWEN_WIKI_EVAL_MATERIAL_DIR` 时，再用该目录里的真实资料跑一条
/// 同样的链（只读 .md，不修改原文件），验证治理链在真实资料上同样成立。
#[test]
#[ignore = "真机评测：需要 DMXAPI_API_KEY 与 pi-dev 运行时，默认不跑"]
fn wiki_real_model_end_to_end() {
    for var in ["GONGWEN_PI_RUNTIME_DIR", "GONGWEN_PI_NODE"] {
        if eval_env(var).is_none() {
            panic!("缺少环境变量 {var}；先 source scripts/pi-dev.sh");
        }
    }
    let seed: Vec<(String, String)> = SEED_DOCS
        .iter()
        .map(|(title, body)| ((*title).to_string(), (*body).to_string()))
        .collect();
    let update_doc = (UPDATE_DOC.0.to_string(), UPDATE_DOC.1.to_string());
    let builtin = run_chain("builtin", &seed, Some(&update_doc));
    assert!(builtin.pages >= 2, "整编只采纳了 {} 页", builtin.pages);
    assert!(builtin.updated, "内置链没有跑更新环节");
    assert!(!builtin.updated_refs.is_empty(), "更新后的依据清单是空的");

    if let Some(dir) = eval_env("GONGWEN_WIKI_EVAL_MATERIAL_DIR") {
        let docs = load_material_docs(Path::new(&dir));
        eprintln!(
            "真实资料链：{} 篇，前 {} 篇作种子、最后一篇作新资料",
            docs.len(),
            docs.len() - 1
        );
        let (seed, update) = docs.split_at(docs.len() - 1);
        let report = run_chain("materials", seed, Some(&update[0]));
        assert!(
            report.pages >= 2,
            "真实资料整编只采纳了 {} 页",
            report.pages
        );
        assert!(report.updated, "真实资料链没有跑更新环节");
    } else {
        eprintln!(
            "未设置 GONGWEN_WIKI_EVAL_MATERIAL_DIR，跳过真实资料链；\
             可用它指向一批 .md 资料目录（如 /Users/ours/Documents/req/11-material）"
        );
    }
}
