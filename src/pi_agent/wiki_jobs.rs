//! 维基知识库的 Pi 作业：框架归纳、页面整编与页面更新。
//!
//! 与任务作业同一套链：不占写作门禁，host 用技能文件里的自然语言步骤工作，
//! 用 `submit_task` 交一份 JSON；Rust 在这里做确定性复核——主题/分类名合规、
//! 无重名、依据文档编号必须真实存在——通过后也只是一份**提案**，由用户在
//! 知识库页确认后才写库。
//!
//! 工具面：只读全套（资料库检索/读取、日期、维基四工具）；不挂任务工具、
//! 词库与数据接口，不是任务作业（没有 task_only 收窄）。

use super::{
    PiBridge, PiEvent, PiJob, PiMaterialInput, PiOutcome, PiRunLog, PiRunRecord, PiRuntime, PiTask,
    PiToolContext, PiWorkspace, pi_skills, run,
};
use crate::models::{AppConfig, DraftInput};
use crate::pi_agent::materials::MaterialRole;
use crate::wiki::{FrameCategory, FrameTheme};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// 框架作业使用保留的目录键，避开稿件标签键、风格学习的 0 与任务作业的两个保留键。
const FRAME_DOC_KEY: u64 = u64::MAX - 2;
/// 页面整编作业再占一个保留键。
const WRITE_DOC_KEY: u64 = u64::MAX - 3;
/// 页面更新作业再占一个保留键。
const UPDATE_DOC_KEY: u64 = u64::MAX - 4;
/// 与写作一致的单轮执行上限。
const TIMEOUT: Duration = Duration::from_secs(1800);
/// 主题与分类名的字数上限（与导入大纲同一口径）。
const MAX_NODE_NAME_CHARS: usize = 60;
/// 框架规模上限：防止模型交回失控的大框架撑爆审核界面。
const MAX_THEMES: usize = 30;
const MAX_CATEGORIES_PER_THEME: usize = 20;
/// 单个分类挂靠依据文档的上限。
const MAX_DOC_REFS: usize = 50;

/// 维基作业全程靠 kb_search / kb_read 取料；资料库检索没开时跑到闸门才报
/// 「文档 N 未检索到」既误导又浪费一整轮，发起时直接拦下。
fn ensure_rag_enabled(config: &AppConfig) -> Result<(), String> {
    if config.rag.enabled {
        Ok(())
    } else {
        Err("资料库检索未启用：请先在「资料库」页启用检索，再发起维基作业。".into())
    }
}

/// 归纳出的框架提案：两级 + 每个分类挂靠的资料文档 id。只表示「模型建议」，
/// 只有用户确认后才写库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameProposal {
    pub(crate) themes: Vec<FrameTheme>,
}

/// 交付 JSON 的原始形状（校验后转换成 [`FrameProposal`]）。
#[derive(serde::Deserialize)]
struct RawFrame {
    themes: Vec<RawTheme>,
}

#[derive(serde::Deserialize)]
struct RawTheme {
    name: String,
    #[serde(default)]
    guidance: String,
    categories: Vec<RawCategory>,
}

#[derive(serde::Deserialize)]
struct RawCategory {
    name: String,
    #[serde(default)]
    guidance: String,
    #[serde(default)]
    doc_ids: Vec<i64>,
}

/// 确定性复核交付 JSON：结构、名称合规、无重名、依据文档真实存在。
/// 任何一条不过都明确报错，绝不带病进审核界面。
pub(crate) fn parse_frame_delivery(
    content: &str,
    known_docs: &[i64],
) -> Result<FrameProposal, String> {
    let raw: RawFrame = serde_json::from_str(content)
        .map_err(|error| format!("框架交付不是合法 JSON（{error}）；应为 {{\"themes\": [...]}}"))?;
    if raw.themes.is_empty() {
        return Err("框架交付里没有主题".into());
    }
    if raw.themes.len() > MAX_THEMES {
        return Err(format!("主题超过 {MAX_THEMES} 个，请收窄归纳范围"));
    }
    let known: std::collections::HashSet<i64> = known_docs.iter().copied().collect();
    let mut theme_names = std::collections::HashSet::new();
    let mut themes = Vec::new();
    for theme in raw.themes {
        let name = theme.name.trim().to_string();
        if name.is_empty() {
            return Err("主题名不能为空".into());
        }
        if name.chars().count() > MAX_NODE_NAME_CHARS {
            return Err(format!("主题名超过 {MAX_NODE_NAME_CHARS} 字：{name}"));
        }
        if !theme_names.insert(name.clone()) {
            return Err(format!("主题「{name}」重复"));
        }
        if theme.categories.is_empty() {
            return Err(format!("主题「{name}」下没有分类；至少给一个分类"));
        }
        if theme.categories.len() > MAX_CATEGORIES_PER_THEME {
            return Err(format!(
                "主题「{name}」下分类超过 {MAX_CATEGORIES_PER_THEME} 个，请合并同类"
            ));
        }
        let guidance = parse_guidance(&name, &theme.guidance)?;
        let mut category_names = std::collections::HashSet::new();
        let mut categories = Vec::new();
        for category in theme.categories {
            let cname = category.name.trim().to_string();
            if cname.is_empty() {
                return Err(format!("主题「{name}」下有空分类名"));
            }
            if cname.chars().count() > MAX_NODE_NAME_CHARS {
                return Err(format!("分类名超过 {MAX_NODE_NAME_CHARS} 字：{cname}"));
            }
            if !category_names.insert(cname.clone()) {
                return Err(format!("主题「{name}」下分类「{cname}」重复"));
            }
            // 空依据的分类是「待定」占位壳：技能要求每个分类挂靠实际读到的资料，
            // 真机评测里模型交过零依据的「（待定）」分类，必须在闸门上拦下。
            if category.doc_ids.is_empty() {
                return Err(format!(
                    "分类「{cname}」没有依据文档；每个分类至少挂靠一篇本次检索到的资料，\
                     归不进分类的资料不要硬凑分类"
                ));
            }
            if category.doc_ids.len() > MAX_DOC_REFS {
                return Err(format!("分类「{cname}」依据文档超过 {MAX_DOC_REFS} 篇"));
            }
            let mut doc_ids = Vec::new();
            for id in category.doc_ids {
                if !known.contains(&id) {
                    return Err(format!(
                        "分类「{cname}」依据的文档 {id} 已不存在或本轮未检索到；只填工具实际返回的资料编号"
                    ));
                }
                if !doc_ids.contains(&id) {
                    doc_ids.push(id);
                }
            }
            categories.push(FrameCategory {
                guidance: parse_guidance(&cname, &category.guidance)?,
                name: cname,
                doc_ids,
            });
        }
        themes.push(FrameTheme {
            name,
            guidance,
            categories,
        });
    }
    Ok(FrameProposal { themes })
}

/// 资料库里现存的文档编号，供依据校验。
fn consulted_docs(
    store_path: &Path,
    tools: &PiToolContext,
    previous: impl IntoIterator<Item = i64>,
) -> Result<Vec<i64>, String> {
    let mut allowed = tools.wiki_doc_hits();
    allowed.extend(previous);
    let mut store = crate::knowledge::KnowledgeStore::open(store_path)
        .map_err(|error| format!("打开资料库失败：{error:#}"))?;
    store
        .all_doc_ids()
        .map(|mut ids| {
            ids.retain(|id| allowed.contains(id));
            ids
        })
        .map_err(|error| format!("读取资料库文档失败：{error:#}"))
}

fn parse_guidance(name: &str, guidance: &str) -> Result<String, String> {
    let guidance = guidance.trim();
    crate::wiki::validate_frame_guidance(guidance)
        .map_err(|error| format!("「{name}」：{error}"))?;
    Ok(guidance.to_owned())
}

fn scope_guidance(tree: &[crate::wiki::WikiTree], id: i64) -> String {
    for branch in tree {
        let category = branch.categories.iter().find(|node| node.id == id);
        if branch.theme.id != id && category.is_none() {
            continue;
        }
        let mut lines = Vec::new();
        for (kind, node) in
            std::iter::once(("主题", &branch.theme)).chain(category.map(|node| ("分类", node)))
        {
            if !node.guidance.is_empty() {
                lines.push(format!(
                    "{kind}「{}」的分类口径：{}",
                    node.name, node.guidance
                ));
            }
        }
        return lines.join("\n");
    }
    String::new()
}

fn frame_guidance_materials(tree: &[crate::wiki::WikiTree]) -> Vec<PiMaterialInput> {
    let rules = tree
        .iter()
        .flat_map(|branch| {
            std::iter::once((
                format!("主题「{}」", branch.theme.name),
                &branch.theme.guidance,
            ))
            .chain(branch.categories.iter().map(|node| {
                (
                    format!("主题「{}」/ 分类「{}」", branch.theme.name, node.name),
                    &node.guidance,
                )
            }))
        })
        .map(|(name, guidance)| {
            let rule = if guidance.is_empty() {
                "尚未填写分类口径"
            } else {
                guidance
            };
            format!("## {name}\n\n{rule}\n\n")
        })
        .collect::<Vec<_>>();
    rules
        .chunks(100)
        .enumerate()
        .map(|(index, chunk)| PiMaterialInput {
            title: format!("现有分类口径 {}", index + 1),
            source: "已确认的维基框架".into(),
            role: MaterialRole::StructureSample,
            text: chunk.concat(),
        })
        .collect()
}

/// 浏览资料库归纳两级框架：返回通过确定性复核的提案，由用户确认后写库。
pub(crate) fn induce_frame(
    config: Arc<AppConfig>,
    store_path: PathBuf,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(PiEvent),
) -> Result<FrameProposal, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止归纳框架。".into());
    }
    ensure_rag_enabled(&config)?;
    let catalog = pi_skills::load_catalog();
    catalog.resolve(Some(pi_skills::WIKI_FRAME_SKILL))?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let store = crate::wiki::WikiStore::open(&store_path)
        .map_err(|error| format!("打开维基库失败：{error:#}"))?;
    let tree = store
        .list_tree()
        .map_err(|error| format!("读取框架失败：{error:#}"))?;
    let materials = frame_guidance_materials(&tree);
    let existing = if materials.is_empty() {
        ""
    } else {
        " 先读材料中的现有框架与分类口径；可覆盖的资料优先归入对应节点，保留其层级与名称。为相关节点补拟缺失的口径。依据仍用 kb_search / kb_read 核实。"
    };
    let request = format!(
        "按技能 {} 浏览资料库，归纳两级知识框架（主题 → 分类），为每个主题和分类写出可执行的归类口径，         每个分类挂靠它实际依据的资料文档，最后用 submit_task 交 work/frame.json。今天日期：{today}。{existing}",
        pi_skills::WIKI_FRAME_SKILL,
    );
    let runtime = PiRuntime::locate()?;
    let chat = config.pi_chat().map_err(|error| error.to_string())?;
    let model = crate::ai_panel::pi_job::pi_model(&chat)?;
    let (workspace, metas) = PiWorkspace::prepare(
        &super::base_dir()?,
        FRAME_DOC_KEY,
        "",
        &request,
        &materials,
        &catalog,
        Some(pi_skills::WIKI_FRAME_SKILL),
    )?;
    // 只读全套：要读资料库；不挂维基工具（还没有框架可读）、不是任务作业。
    let tools = Arc::new(
        PiToolContext::new(
            Arc::clone(&config),
            DraftInput::default(),
            String::new(),
            request.clone(),
            Vec::new(),
        )
        .wiki_job(),
    );
    let run_id = uuid::Uuid::new_v4().to_string();
    let request_id = format!("wiki-frame-{run_id}");
    let record = PiRunRecord::start(
        run_id.clone(),
        request_id.clone(),
        &workspace,
        "",
        "",
        "",
        &workspace.read_request()?,
        None,
        &request,
        "维基框架",
        &today,
        Some(pi_skills::WIKI_FRAME_SKILL),
        None,
        metas.clone(),
        &config.pi_model.provider_id,
        &model.model_id,
    );
    let record = Arc::new(PiRunLog::open(&workspace, record)?);
    let label = workspace.label.clone();
    let outcome = run(
        PiJob {
            runtime,
            workspace,
            model,
            task: PiTask {
                request,
                kind: "维基框架".to_string(),
                system_date: today,
                skill: Some(pi_skills::WIKI_FRAME_SKILL.to_string()),
                strategy: Default::default(),
                capability: Default::default(),
                selection: None,
                materials: metas,
            },
            before: String::new(),
            base: String::new(),
            tools: Arc::clone(&tools),
            bridge: Arc::new(PiBridge::default()),
            record,
            resume: false,
            request_id,
            run_id,
            timeout: TIMEOUT,
            proxy_env: crate::net::proxy_env(&config.proxy),
        },
        Arc::clone(&cancel),
        &mut on_event,
    )
    .map_err(|error| format!("{error}；工作文件保留在 {label}"))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止归纳框架，未采纳本轮结果。".into());
    }
    let PiOutcome::Task(delivery) = outcome else {
        return Err("Pi 没有交回框架提案，未采纳。".into());
    };
    if !delivery.summary.trim().is_empty() {
        on_event(PiEvent::Progress(format!(
            "交付说明：{}",
            delivery.summary.trim()
        )));
    }
    parse_frame_delivery(&delivery.content, &consulted_docs(&store_path, &tools, [])?)
        .map_err(|error| format!("框架提案未通过复核：{error}"))
}

/// 页面整编提案：一个分类一次作业产出的待审页面集合。只是「模型建议」，
/// 由用户在审核台逐页把关（可直接改正文）后才写库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageProposalSet {
    pub(crate) run_id: String,
    /// 目标分类名（展示用；写库直接用 node_id）。
    pub(crate) category: String,
    /// 目标分类节点：作业交付时已解析并复核过存在，审核台采纳时不再按名字猜。
    pub(crate) node_id: i64,
    pub(crate) pages: Vec<PageProposal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageProposal {
    /// 更新提案的目标页 UUID；新建页提案为 None。
    pub uuid: Option<String>,
    pub base_version: Option<i64>,
    pub title: String,
    /// normal 或 guide。
    pub kind: String,
    pub body: String,
    /// 变更说明（更新提案用，审核台展示）。
    pub change_note: String,
    /// 依据清单：资料库 doc_id + 支撑说明。
    pub doc_refs: Vec<(i64, String)>,
}

/// 一次整编作业最多交回几页（与技能红线一致，超限退回重拆）。
const MAX_PAGES_PER_RUN: usize = 10;
/// 一次更新作业最多交回几页。
const MAX_UPDATE_PAGES_PER_RUN: usize = 5;
/// 单页正文字符上限。
const MAX_PAGE_BODY_CHARS: usize = 20_000;
/// 页面标题字数上限。
const MAX_PAGE_TITLE_CHARS: usize = 100;
/// 一条依据说明的字数上限。
const MAX_DOC_NOTE_CHARS: usize = 200;
/// 变更说明字数上限。
const MAX_CHANGE_NOTE_CHARS: usize = 200;

/// 交付 JSON 的原始形状（校验后转换成 [`PageProposalSet`]）。
#[derive(serde::Deserialize)]
struct RawPages {
    pages: Vec<RawPage>,
}

#[derive(serde::Deserialize)]
struct RawPage {
    /// 更新提案必须原样带回目标页 UUID；新建页交付带 UUID 会被拒。
    #[serde(default)]
    uuid: Option<String>,
    title: String,
    kind: String,
    body: String,
    #[serde(default)]
    change_note: String,
    #[serde(default)]
    doc_refs: Vec<RawDocRef>,
}

#[derive(serde::Deserialize)]
struct RawDocRef {
    doc_id: i64,
    #[serde(default)]
    note: String,
}

/// 结构校验也用于人工编辑后的采纳。来源标记使用资料编号，不使用本轮工具台账号。
pub(crate) fn validate_review_body(body: &str, refs: &[(i64, String)]) -> Result<(), String> {
    if body.trim().is_empty() || body.chars().count() > MAX_PAGE_BODY_CHARS {
        return Err(format!("正文不能为空或超过 {MAX_PAGE_BODY_CHARS} 字"));
    }
    if refs.is_empty()
        || refs.len() > MAX_DOC_REFS
        || refs.iter().any(|(_, note)| note.trim().is_empty())
    {
        return Err(format!(
            "每页需有 1 至 {MAX_DOC_REFS} 篇依据，并逐条说明支撑要点"
        ));
    }
    for mark in body.split("〔E").skip(1) {
        if let Some((id, _)) = mark.split_once('〕')
            && let Ok(id) = id.parse::<i64>()
            && !refs.iter().any(|(doc_id, _)| *doc_id == id)
        {
            return Err(format!("正文标记 〔E{id}〕 没有对应的资料依据"));
        }
    }
    Ok(())
}

/// 页面整编交付的确定性复核：结构、标题合规、kind 合法、
/// 依据文档真实存在、与现有页面不重名。任何一条不过都明确报错。
/// 目标节点在作业期间是否还在由调用方在作业结束后重读框架确认。
pub(crate) fn parse_page_delivery(
    content: &str,
    category: &str,
    node_id: i64,
    run_id: String,
    existing_titles: &[String],
    known_docs: &[i64],
) -> Result<PageProposalSet, String> {
    let raw: RawPages = serde_json::from_str(content)
        .map_err(|error| format!("页面交付不是合法 JSON（{error}）；应为 {{\"pages\": [...]}}"))?;
    if raw.pages.is_empty() {
        return Err("页面交付里没有页面".into());
    }
    if raw.pages.len() > MAX_PAGES_PER_RUN {
        return Err(format!(
            "一次整编最多 {MAX_PAGES_PER_RUN} 页，本轮交了 {} 页；请收窄范围重试",
            raw.pages.len()
        ));
    }
    let known: std::collections::HashSet<i64> = known_docs.iter().copied().collect();
    let existing: std::collections::HashSet<&str> =
        existing_titles.iter().map(String::as_str).collect();
    let mut seen_titles = std::collections::HashSet::new();
    let mut guide_seen = false;
    let mut pages = Vec::new();
    for page in raw.pages {
        if page.uuid.is_some() {
            return Err("新建页交付里不应带 uuid；要改已有页请走更新作业".into());
        }
        let title = page.title.trim().to_string();
        if title.is_empty() {
            return Err("页面标题不能为空".into());
        }
        if title.chars().count() > MAX_PAGE_TITLE_CHARS {
            return Err(format!("页面标题超过 {MAX_PAGE_TITLE_CHARS} 字：{title}"));
        }
        if existing.contains(title.as_str()) {
            return Err(format!(
                "已存在同名页面「{title}」；用更新流程改已有页，不要新建同名页"
            ));
        }
        if !seen_titles.insert(title.clone()) {
            return Err(format!("本轮交付里页面「{title}」重复"));
        }
        let kind = page.kind.trim().to_string();
        if kind != "normal" && kind != "guide" {
            return Err(format!(
                "页面「{title}」的 kind 必须是 normal 或 guide，不是「{kind}」"
            ));
        }
        if kind == "guide" {
            if guide_seen {
                return Err("一次整编只能有一篇导览页".into());
            }
            guide_seen = true;
        }
        if page.body.trim().is_empty() {
            return Err(format!("页面「{title}」正文是空的"));
        }
        if page.body.chars().count() > MAX_PAGE_BODY_CHARS {
            return Err(format!(
                "页面「{title}」正文超过 {MAX_PAGE_BODY_CHARS} 字，请拆成多页"
            ));
        }
        let mut doc_refs = Vec::new();
        for reference in page.doc_refs {
            if !known.contains(&reference.doc_id) {
                return Err(format!(
                    "页面「{title}」依据的文档 {} 已不存在或本轮未检索到；只填工具实际返回或本页保留的资料编号",
                    reference.doc_id
                ));
            }
            if reference.note.chars().count() > MAX_DOC_NOTE_CHARS {
                return Err(format!(
                    "页面「{title}」依据说明超过 {MAX_DOC_NOTE_CHARS} 字"
                ));
            }
            let note = reference.note.trim().to_string();
            if !doc_refs.iter().any(|(id, _)| *id == reference.doc_id) {
                doc_refs.push((reference.doc_id, note));
            }
        }
        validate_review_body(&page.body, &doc_refs)
            .map_err(|error| format!("页面「{title}」：{error}"))?;
        pages.push(PageProposal {
            uuid: None,
            base_version: None,
            title,
            kind,
            body: page.body,
            change_note: String::new(),
            doc_refs,
        });
    }
    Ok(PageProposalSet {
        run_id,
        category: category.to_string(),
        node_id,
        pages,
    })
}

/// 为指定分类整编维基页面：读资料库与该分类相关的文档，拆题成页，
/// 交回通过确定性复核的页面提案集，由用户在审核台逐页确认后写库。
pub(crate) fn write_pages(
    config: Arc<AppConfig>,
    store_path: PathBuf,
    node_id: i64,
    // 圈选资料提示：从资料库反向发起整编时，优先读这些 doc_id（kb_read 按编号读）。
    doc_hints: Vec<i64>,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(PiEvent),
) -> Result<PageProposalSet, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止整编页面。".into());
    }
    ensure_rag_enabled(&config)?;
    let store = crate::wiki::WikiStore::open(&store_path)
        .map_err(|error| format!("打开维基库失败：{error:#}"))?;
    let tree = store
        .list_tree()
        .map_err(|error| format!("读取框架失败：{error:#}"))?;
    let (node, parent, has_guide) = tree
        .iter()
        .find_map(|t| {
            if t.theme.id == node_id {
                return Some((&t.theme, None, t.theme_guide.is_some()));
            }
            t.categories.iter().find(|c| c.id == node_id).map(|c| {
                (
                    c,
                    Some(t.theme.name.as_str()),
                    t.category_guides.contains_key(&c.id),
                )
            })
        })
        .ok_or("所选节点已不存在，请重新发起整编")?;
    let guide_only = parent.is_none();
    if guide_only && has_guide {
        return Err("主题已有导览页，请打开导览页后使用「更新」。".into());
    }
    let target = match parent {
        Some(theme) => format!("主题「{theme}」下的分类「{}」", node.name),
        None => format!("主题「{}」", node.name),
    };
    // 圈选的资料是「优先读物」，不是取料范围：与分类已确认的资料映射合并，
    // 避免圈两篇就把整个分类已确认的依据丢掉。
    let mut doc_hints = doc_hints;
    for id in store
        .frame_doc_ids(node_id)
        .map_err(|error| format!("读取框架资料映射失败：{error:#}"))?
    {
        if !doc_hints.contains(&id) {
            doc_hints.push(id);
        }
    }
    let catalog = pi_skills::load_catalog();
    catalog.resolve(Some(pi_skills::WIKI_WRITE_SKILL))?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let hint_line = if doc_hints.is_empty() {
        String::new()
    } else {
        format!(
            " 优先用 kb_read 按编号读这些确认或圈选的资料：{}。",
            doc_hints
                .iter()
                .map(|id| format!("E{id}"))
                .collect::<Vec<_>>()
                .join("、"),
        )
    };
    // 已有页面标题先告诉模型：整编会拒收与全库现有页面同名的标题，靠模型
    // 自己 wiki_search 碰运气命中率太低，一次白跑就是 30 分钟。
    let existing_here = store
        .list_pages(node_id)
        .map_err(|error| format!("读取已有页面失败：{error:#}"))?;
    let existing_line = if existing_here.is_empty() {
        "本节点下还没有页面。".to_string()
    } else {
        format!(
            "本节点下已有页面：{}；这些标题不要重复新建，相关内容用更新流程。",
            existing_here
                .iter()
                .map(|page| page.title.as_str())
                .collect::<Vec<_>>()
                .join("、")
        )
    };
    let guidance = scope_guidance(&tree, node_id);
    let guidance = if guidance.is_empty() {
        String::new()
    } else {
        format!("\n按已确认的分类口径选材；边界外资料不要纳入。\n{guidance}\n")
    };
    let scope = if guide_only { "主题" } else { "分类" };
    let request = format!(
        "按技能 {} 为{target}整编维基页面，\
         最后用 submit_task 交 work/pages.json。今天日期：{today}。\
         该{scope}{guide_hint}。{existing_line}\
         资料用 kb_search / kb_read 找与目标相关的文档；\
         已有维基页用 wiki_search / wiki_links 查，便于互相链接。{hint_line}{guidance}",
        pi_skills::WIKI_WRITE_SKILL,
        guide_hint = if has_guide {
            "已有导览页，本轮不要再交 guide 页；至少交一篇新的 kind=normal 条目页"
        } else if guide_only {
            "只交一篇 kind=guide 的主题导览页，链接可用的分类导览页，不生成普通页"
        } else {
            "还没有导览页，本轮要交一篇 kind=guide 的导览页，并至少交一篇 kind=normal 的条目页"
        },
    );
    let materials: Vec<PiMaterialInput> = Vec::new();
    let runtime = PiRuntime::locate()?;
    let chat = config.pi_chat().map_err(|error| error.to_string())?;
    let model = crate::ai_panel::pi_job::pi_model(&chat)?;
    let (workspace, metas) = PiWorkspace::prepare(
        &super::base_dir()?,
        WRITE_DOC_KEY,
        "",
        &request,
        &materials,
        &catalog,
        Some(pi_skills::WIKI_WRITE_SKILL),
    )?;
    // 只读全套 + 维基工具：整编要查已有页面以便互链。
    let tools = Arc::new(
        PiToolContext::new(
            Arc::clone(&config),
            DraftInput::default(),
            String::new(),
            request.clone(),
            Vec::new(),
        )
        .wiki_job()
        .with_wiki_autoload(),
    );
    let run_id = uuid::Uuid::new_v4().to_string();
    let request_id = format!("wiki-write-{run_id}");
    let record = PiRunRecord::start(
        run_id.clone(),
        request_id.clone(),
        &workspace,
        "",
        "",
        "",
        &workspace.read_request()?,
        None,
        &request,
        "维基整编",
        &today,
        Some(pi_skills::WIKI_WRITE_SKILL),
        None,
        metas.clone(),
        &config.pi_model.provider_id,
        &model.model_id,
    );
    let record = Arc::new(PiRunLog::open(&workspace, record)?);
    let label = workspace.label.clone();
    let outcome = run(
        PiJob {
            runtime,
            workspace,
            model,
            task: PiTask {
                request,
                kind: "维基整编".to_string(),
                system_date: today,
                skill: Some(pi_skills::WIKI_WRITE_SKILL.to_string()),
                strategy: Default::default(),
                capability: Default::default(),
                selection: None,
                materials: metas,
            },
            before: String::new(),
            base: String::new(),
            tools: Arc::clone(&tools),
            bridge: Arc::new(PiBridge::default()),
            record,
            resume: false,
            request_id,
            run_id: run_id.clone(),
            timeout: TIMEOUT,
            proxy_env: crate::net::proxy_env(&config.proxy),
        },
        Arc::clone(&cancel),
        &mut on_event,
    )
    .map_err(|error| format!("{error}；工作文件保留在 {label}"))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止整编页面，未采纳本轮结果。".into());
    }
    let PiOutcome::Task(delivery) = outcome else {
        return Err("Pi 没有交回页面提案，未采纳。".into());
    };
    if !delivery.summary.trim().is_empty() {
        on_event(PiEvent::Progress(format!(
            "交付说明：{}",
            delivery.summary.trim()
        )));
    }
    let known = consulted_docs(&store_path, &tools, [])?;
    let tree = store
        .list_tree()
        .map_err(|error| format!("读取当前框架失败：{error:#}"))?;
    // 作业期间用户可能删了/移了目标节点：重读框架按节点 ID 确认还在，
    // 不能等到逐页采纳才报「节点已不存在」。分类名以库里当前值为准。
    let Some((node_name, node_has_guide)) = tree.iter().find_map(|t| {
        if t.theme.id == node_id {
            return Some((t.theme.name.clone(), t.theme_guide.is_some()));
        }
        t.categories
            .iter()
            .find(|c| c.id == node_id)
            .map(|c| (c.name.clone(), t.category_guides.contains_key(&c.id)))
    }) else {
        return Err("目标节点在作业期间已被删除或移动，本批提案作废；请重新发起整编。".into());
    };
    let mut existing_titles = Vec::new();
    for id in tree
        .iter()
        .flat_map(|t| std::iter::once(t.theme.id).chain(t.categories.iter().map(|c| c.id)))
    {
        let pages = store
            .list_pages(id)
            .map_err(|error| format!("读取已有页面失败：{error:#}"))?;
        existing_titles.extend(pages.into_iter().map(|meta| meta.title));
    }
    let proposal = parse_page_delivery(
        &delivery.content,
        &node_name,
        node_id,
        run_id,
        &existing_titles,
        &known,
    )
    .map_err(|error| format!("页面提案未通过复核：{error}"))?;
    let guides = proposal
        .pages
        .iter()
        .filter(|page| page.kind == "guide")
        .count();
    if (guide_only && proposal.pages.len() != 1) || guides != usize::from(!node_has_guide) {
        return Err("交付的导览页不符合本次整编范围。".into());
    }
    // 分类整编不能只交一篇导览页：导览只是入口，分类没有条目页时既无法被问答
    // 逐条引用，也会一直被缺口清单判为「没有条目覆盖」。
    if !guide_only && !proposal.pages.iter().any(|page| page.kind == "normal") {
        return Err(
            "分类整编至少要交一篇条目页（kind=normal）：只交导览页等于该分类没有条目知识，\
             请按「一页一主题」拆出至少一篇条目页后重试。"
                .into(),
        );
    }
    Ok(proposal)
}

/// 更新作业的目标页快照（发起时从库里读出，交付复核时对照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateTarget {
    pub uuid: String,
    pub title: String,
    pub node_id: i64,
    pub kind: String,
    pub current_body: String,
    pub current_version: i64,
    pub doc_refs: Vec<(i64, String)>,
}

/// 页面更新交付的确定性复核：UUID 对应页面存在、标题与类型未改、
/// 正文非空且与现状确有不同、依据文档真实、更新页数不超上限。
/// 模型只交回部分目标（认为其余的无需改）是允许的。
pub(crate) fn parse_update_delivery(
    content: &str,
    run_id: String,
    known_docs: &[i64],
    targets: &[UpdateTarget],
) -> Result<PageProposalSet, String> {
    let raw: RawPages = serde_json::from_str(content)
        .map_err(|error| format!("页面交付不是合法 JSON（{error}）；应为 {{\"pages\": [...]}}"))?;
    if raw.pages.is_empty() {
        return Err("页面交付里没有页面".into());
    }
    if raw.pages.len() > MAX_UPDATE_PAGES_PER_RUN {
        return Err(format!(
            "一次更新最多 {MAX_UPDATE_PAGES_PER_RUN} 页，本轮交了 {} 页；请分批重试",
            raw.pages.len()
        ));
    }
    let known: std::collections::HashSet<i64> = known_docs.iter().copied().collect();
    let mut seen_uuids = std::collections::HashSet::new();
    let mut pages = Vec::new();
    let mut node_id = 0i64;
    for page in raw.pages {
        let Some(uuid) = page.uuid.map(|u| u.trim().to_string()) else {
            return Err("更新提案缺 uuid；每个 pages[] 都要带目标页 UUID".into());
        };
        if !seen_uuids.insert(uuid.clone()) {
            return Err(format!("本轮交付里页面 {uuid} 重复"));
        }
        let Some(target) = targets.iter().find(|t| t.uuid == uuid) else {
            return Err(format!("目标页 {uuid} 不在本次发起范围（或已被删除）"));
        };
        node_id = target.node_id;
        let title = page.title.trim().to_string();
        if title != target.title {
            return Err(format!(
                "更新不改标题：目标页是「{}」，交付写的是「{title}」",
                target.title
            ));
        }
        let kind = page.kind.trim().to_string();
        if kind != target.kind {
            return Err(format!(
                "更新不改页面类型：「{title}」应保持 kind={}",
                target.kind
            ));
        }
        if page.body.trim().is_empty() {
            return Err(format!("页面「{title}」正文是空的"));
        }
        if page.body.chars().count() > MAX_PAGE_BODY_CHARS {
            return Err(format!(
                "页面「{title}」正文超过 {MAX_PAGE_BODY_CHARS} 字，请拆成多页"
            ));
        }
        if page.body == target.current_body {
            return Err(format!("页面「{title}」的正文与现状逐字相同，没有变化"));
        }
        let change_note = page.change_note.trim().to_string();
        if change_note.is_empty() {
            return Err(format!(
                "页面「{title}」缺 change_note：更新交付每页都要用一两句话说清这版改了什么"
            ));
        }
        if change_note.chars().count() > MAX_CHANGE_NOTE_CHARS {
            return Err(format!(
                "页面「{title}」变更说明超过 {MAX_CHANGE_NOTE_CHARS} 字"
            ));
        }
        let mut doc_refs = Vec::new();
        for reference in page.doc_refs {
            if !known.contains(&reference.doc_id) {
                return Err(format!(
                    "页面「{title}」依据的文档 {} 已不存在或本轮未检索到；只填工具实际返回或本页保留的资料编号",
                    reference.doc_id
                ));
            }
            if reference.note.chars().count() > MAX_DOC_NOTE_CHARS {
                return Err(format!(
                    "页面「{title}」依据说明超过 {MAX_DOC_NOTE_CHARS} 字"
                ));
            }
            let note = reference.note.trim().to_string();
            if !doc_refs.iter().any(|(id, _)| *id == reference.doc_id) {
                doc_refs.push((reference.doc_id, note));
            }
        }
        validate_review_body(&page.body, &doc_refs)
            .map_err(|error| format!("页面「{title}」：{error}"))?;
        pages.push(PageProposal {
            uuid: Some(uuid),
            base_version: Some(target.current_version),
            title,
            kind,
            body: page.body,
            change_note,
            doc_refs,
        });
    }
    Ok(PageProposalSet {
        run_id,
        category: format!("更新 {} 页", pages.len()),
        node_id,
        pages,
    })
}

/// 按资料库新内容更新已有维基页面：目标页正文以材料附上，Pi 找新资料改写，
/// 交回通过确定性复核的更新提案集，由用户在审核台看 diff、可编辑后写库。
pub(crate) fn update_pages(
    config: Arc<AppConfig>,
    store_path: PathBuf,
    targets: Vec<(String, String)>,
    source: Option<i64>,
    cancel: Arc<AtomicBool>,
    mut on_event: impl FnMut(PiEvent),
) -> Result<PageProposalSet, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止更新页面。".into());
    }
    ensure_rag_enabled(&config)?;
    if targets.is_empty() {
        return Err("没有指定要更新的页面。".into());
    }
    if targets.len() > MAX_UPDATE_PAGES_PER_RUN {
        return Err(format!(
            "一次更新至多 {MAX_UPDATE_PAGES_PER_RUN} 页，本次选了 {} 页",
            targets.len()
        ));
    }
    let store = crate::wiki::WikiStore::open(&store_path)
        .map_err(|error| format!("打开维基库失败：{error:#}"))?;
    let new_source = source
        .map(|id| format!("用户指定的新资料编号为 {id}，请先用 kb_read({{doc_id: {id}}}) 读它。"))
        .unwrap_or_default();
    let mut snapshot = Vec::new();
    for (uuid, title) in &targets {
        if cancel.load(Ordering::Relaxed) {
            return Err("已停止更新页面。".into());
        }
        let page = store
            .get_page(uuid)
            .map_err(|error| format!("读取页面「{title}」失败：{error:#}"))?
            .ok_or_else(|| format!("页面「{title}」已不存在，无法更新"))?;
        snapshot.push(UpdateTarget {
            uuid: uuid.clone(),
            title: page.meta.title.clone(),
            node_id: page.meta.node_id,
            kind: page.meta.kind.to_string(),
            current_body: page.body,
            current_version: page.meta.current_version,
            doc_refs: store
                .page_doc_refs(uuid)
                .map_err(|error| format!("读取原页依据失败：{error:#}"))?,
        });
    }
    // 原页依据里可能有已被删除的资料（页面正在「需复核」）。这类编号绝不能
    // 再写进新版本：材料里明确标成「已删除，勿保留」，否则模型照材料抄就会
    // 被闸门拒收，而需复核页正是最需要更新的页面。
    let existing_docs: std::collections::HashSet<i64> = {
        let mut kb = crate::knowledge::KnowledgeStore::open(&store_path)
            .map_err(|error| format!("打开资料库失败：{error:#}"))?;
        kb.all_doc_ids()
            .map_err(|error| format!("读取资料库文档失败：{error:#}"))?
            .into_iter()
            .collect()
    };
    let tree = store
        .list_tree()
        .map_err(|error| format!("读取框架失败：{error:#}"))?;
    let catalog = pi_skills::load_catalog();
    catalog.resolve(Some(pi_skills::WIKI_UPDATE_SKILL))?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let listing = snapshot
        .iter()
        .map(|t| format!("- {}（uuid：{}，类型：{}）", t.title, t.uuid, t.kind))
        .collect::<Vec<_>>()
        .join("\n");
    let request = format!(
        "按技能 {} 更新 {} 个维基页面，最后用 submit_task 交 work/pages.json。\
         今天日期：{today}。目标页（正文已作为材料附上）：\n{listing}\n\
         用 kb_search / kb_read 找与这些页面相关、尤其是新入库的文档；\
         维基内现状用 wiki_search / wiki_read / wiki_links 自查互链。\
         按各目标页材料中的分类口径判断新资料是否归入；边界外资料不要纳入。\
         每个 pages[] 原样带回 uuid、title 与 kind（类型不可改），正文给整页新正文。\
         依据必须仍存在于资料库：材料里标了「已删除」的资料不要写进 doc_refs。\
         每页必须写 change_note（一两句话说清这版改了什么、依据哪些文档）。{new_source}",
        pi_skills::WIKI_UPDATE_SKILL,
        snapshot.len(),
    );
    // 目标页现状作材料：模型在副本上改写，交付整页新正文。
    let materials: Vec<PiMaterialInput> = snapshot
        .iter()
        .map(|t| {
            let refs = t
                .doc_refs
                .iter()
                .map(|(id, note)| {
                    if existing_docs.contains(id) {
                        format!("- E{id}：{note}")
                    } else {
                        format!("- E{id}（资料已删除，不要保留到本版依据）：{note}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let guidance = scope_guidance(&tree, t.node_id);
            let guidance = if guidance.is_empty() {
                String::new()
            } else {
                format!("分类口径（用于选材）：\n{guidance}\n\n")
            };
            let text = format!(
                "# {}\n\n（本页类型：{}，更新时 kind 保持不变）\n\n{guidance}{}\n\n\
                 当前版本的依据清单（保留仍支撑本页、且仍存在的依据）：\n{}",
                t.title, t.kind, t.current_body, refs
            );
            PiMaterialInput {
                title: format!("维基页：{}", t.title),
                source: "维基现状".into(),
                role: MaterialRole::Fact,
                text,
            }
        })
        .collect();
    let runtime = PiRuntime::locate()?;
    let chat = config.pi_chat().map_err(|error| error.to_string())?;
    let model = crate::ai_panel::pi_job::pi_model(&chat)?;
    let (workspace, metas) = PiWorkspace::prepare(
        &super::base_dir()?,
        UPDATE_DOC_KEY,
        "",
        &request,
        &materials,
        &catalog,
        Some(pi_skills::WIKI_UPDATE_SKILL),
    )?;
    // 只读全套 + 维基工具：更新要查已有页面与互链现状。
    let tools = Arc::new(
        PiToolContext::new(
            Arc::clone(&config),
            DraftInput::default(),
            String::new(),
            request.clone(),
            Vec::new(),
        )
        .wiki_job()
        .with_wiki_autoload(),
    );
    let run_id = uuid::Uuid::new_v4().to_string();
    let request_id = format!("wiki-update-{run_id}");
    let record = PiRunRecord::start(
        run_id.clone(),
        request_id.clone(),
        &workspace,
        "",
        "",
        "",
        &workspace.read_request()?,
        None,
        &request,
        "维基更新",
        &today,
        Some(pi_skills::WIKI_UPDATE_SKILL),
        None,
        metas.clone(),
        &config.pi_model.provider_id,
        &model.model_id,
    );
    let record = Arc::new(PiRunLog::open(&workspace, record)?);
    let label = workspace.label.clone();
    let outcome = run(
        PiJob {
            runtime,
            workspace,
            model,
            task: PiTask {
                request,
                kind: "维基更新".to_string(),
                system_date: today,
                skill: Some(pi_skills::WIKI_UPDATE_SKILL.to_string()),
                strategy: Default::default(),
                capability: Default::default(),
                selection: None,
                materials: metas,
            },
            before: String::new(),
            base: String::new(),
            tools: Arc::clone(&tools),
            bridge: Arc::new(PiBridge::default()),
            record,
            resume: false,
            request_id,
            run_id: run_id.clone(),
            timeout: TIMEOUT,
            proxy_env: crate::net::proxy_env(&config.proxy),
        },
        Arc::clone(&cancel),
        &mut on_event,
    )
    .map_err(|error| format!("{error}；工作文件保留在 {label}"))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止更新页面，未采纳本轮结果。".into());
    }
    let PiOutcome::Task(delivery) = outcome else {
        return Err("Pi 没有交回页面更新提案，未采纳。".into());
    };
    if !delivery.summary.trim().is_empty() {
        on_event(PiEvent::Progress(format!(
            "交付说明：{}",
            delivery.summary.trim()
        )));
    }
    let known = consulted_docs(
        &store_path,
        &tools,
        snapshot
            .iter()
            .flat_map(|target| target.doc_refs.iter().map(|(id, _)| *id)),
    )?;
    parse_update_delivery(&delivery.content, run_id, &known, &snapshot)
        .map_err(|error| format!("页面更新提案未通过复核：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<i64> {
        vec![3, 7, 12]
    }

    #[test]
    fn frame_guidance_flows_from_delivery_to_job_context() {
        let content = serde_json::json!({
            "themes": [{
                "name": "安全生产", "guidance": "  收录生产经营安全。  ",
                "categories": [{"name": "监督检查", "guidance": "排除事故处置。", "doc_ids": [3]}]
            }]
        });
        let proposal = parse_frame_delivery(&content.to_string(), &[3]).unwrap();
        assert_eq!(proposal.themes[0].guidance, "收录生产经营安全。");
        assert_eq!(proposal.themes[0].categories[0].guidance, "排除事故处置。");

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wiki.sqlite");
        let store = crate::wiki::WikiStore::open(&path).unwrap();
        let theme_id = store.create_theme("安全生产").unwrap();
        let node_id = store.create_category(theme_id, "监督检查").unwrap();
        store
            .save_node_definition(theme_id, "安全生产", &proposal.themes[0].guidance)
            .unwrap();
        store
            .save_node_definition(
                node_id,
                "监督检查",
                &proposal.themes[0].categories[0].guidance,
            )
            .unwrap();
        let tree = store.list_tree().unwrap();
        let scope = scope_guidance(&tree, node_id);
        assert!(scope.contains("主题「安全生产」的分类口径：收录生产经营安全。"));
        assert!(scope.contains("分类「监督检查」的分类口径：排除事故处置。"));
        let materials = frame_guidance_materials(&tree);
        assert_eq!(materials.len(), 1);
        assert!(!materials[0].role.is_fact_source());
        assert!(materials[0].text.contains("分类「监督检查」"));
        assert!(materials[0].text.contains("排除事故处置。"));
    }

    #[test]
    fn valid_frame_delivery_parses_into_proposal() {
        let proposal = parse_frame_delivery(
            r#"{"themes":[
                {"name":"安全生产","categories":[
                    {"name":"监督检查","doc_ids":[3,7,3]},
                    {"name":"应急管理","doc_ids":[12]}
                ]},
                {"name":"政务服务","categories":[{"name":"一网通办","doc_ids":[12]}]}
            ]}"#,
            &known(),
        )
        .unwrap();
        assert_eq!(proposal.themes.len(), 2);
        assert_eq!(proposal.themes[0].categories[0].doc_ids, vec![3, 7], "去重");
        assert_eq!(proposal.themes[0].categories[1].name, "应急管理");
        assert_eq!(proposal.themes[1].categories[0].name, "一网通办");
    }

    #[test]
    fn frame_delivery_rejects_structural_and_reference_errors() {
        let cases = [
            ("不是 JSON", "[]"),
            ("缺主题", "{\"themes\":[]}"),
            (
                "重主题",
                "{\"themes\":[{\"name\":\"甲\",\"categories\":[{\"name\":\"x\"}]},{\"name\":\"甲\",\"categories\":[{\"name\":\"y\"}]}]}",
            ),
            (
                "空主题名",
                "{\"themes\":[{\"name\":\"  \",\"categories\":[{\"name\":\"x\"}]}]}",
            ),
            (
                "主题无分类",
                "{\"themes\":[{\"name\":\"甲\",\"categories\":[]}]}",
            ),
            (
                "分类重名",
                "{\"themes\":[{\"name\":\"甲\",\"categories\":[{\"name\":\"x\",\"doc_ids\":[3]},{\"name\":\"x\",\"doc_ids\":[3]}]}]}",
            ),
            (
                "空依据分类",
                "{\"themes\":[{\"name\":\"甲\",\"categories\":[{\"name\":\"（待定）\",\"doc_ids\":[]}]}]}",
            ),
            (
                "幽灵文档",
                "{\"themes\":[{\"name\":\"甲\",\"categories\":[{\"name\":\"x\",\"doc_ids\":[999]}]}]}",
            ),
        ];
        for (label, content) in cases {
            assert!(
                parse_frame_delivery(content, &known()).is_err(),
                "{label} 应被拒绝：{content}"
            );
        }
    }

    fn existing() -> Vec<String> {
        vec!["责任体系".to_string()]
    }

    #[test]
    fn valid_page_delivery_parses_into_proposal_set() {
        let set = parse_page_delivery(
            r##"{"pages":[
                {"title":"监督检查","kind":"guide","body":"# 导览\n\n见 [[责任体系]]。",
                 "doc_refs":[{"doc_id":3,"note":"支撑要点"}]},
                {"title":"闭环整改","kind":"normal","body":"整改要闭环。","doc_refs":[{"doc_id":3,"note":"整改要求"}]}
            ]}"##,
            "监督检查",
            42,
            "run-1".into(),
            &existing(),
            &known(),
        )
        .unwrap();
        assert_eq!(set.category, "监督检查");
        assert_eq!(set.node_id, 42);
        assert_eq!(set.run_id, "run-1");
        assert_eq!(set.pages.len(), 2);
        assert_eq!(set.pages[0].doc_refs, vec![(3, "支撑要点".to_string())]);
    }

    #[test]
    fn page_delivery_rejects_invalid_proposals() {
        let base = r#"{"pages":[{"title":"新页","kind":"normal","body":"正文","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"#;
        let good = (
            "正常交付",
            parse_page_delivery(base, "监督检查", 42, "r".into(), &existing(), &known()),
        );
        assert!(good.1.is_ok(), "{:?}", good.0);

        let cases = [
            ("不是 JSON", "[]".to_string()),
            ("空集合", "{\"pages\":[]}".to_string()),
            (
                "与已有页重名",
                "{\"pages\":[{\"title\":\"责任体系\",\"kind\":\"normal\",\"body\":\"x\",\"doc_refs\":[{\"doc_id\":3,\"note\":\"依据\"}]}]}".to_string(),
            ),
            (
                "本轮重名",
                "{\"pages\":[{\"title\":\"甲\",\"kind\":\"normal\",\"body\":\"x\",\"doc_refs\":[{\"doc_id\":3,\"note\":\"依据\"}]},{\"title\":\"甲\",\"kind\":\"normal\",\"body\":\"y\",\"doc_refs\":[{\"doc_id\":3,\"note\":\"依据\"}]}]}".to_string(),
            ),
            (
                "kind 非法",
                "{\"pages\":[{\"title\":\"甲\",\"kind\":\"chapter\",\"body\":\"x\",\"doc_refs\":[{\"doc_id\":3,\"note\":\"依据\"}]}]}".to_string(),
            ),
            (
                "空正文",
                "{\"pages\":[{\"title\":\"甲\",\"kind\":\"normal\",\"body\":\"  \",\"doc_refs\":[{\"doc_id\":3,\"note\":\"依据\"}]}]}".to_string(),
            ),
            (
                "幽灵依据",
                "{\"pages\":[{\"title\":\"甲\",\"kind\":\"normal\",\"body\":\"x\",\"doc_refs\":[{\"doc_id\":999,\"note\":\"\"}]}]}".to_string(),
            ),
            (
                "双导览",
                "{\"pages\":[{\"title\":\"甲\",\"kind\":\"guide\",\"body\":\"x\",\"doc_refs\":[{\"doc_id\":3,\"note\":\"依据\"}]},{\"title\":\"乙\",\"kind\":\"guide\",\"body\":\"y\",\"doc_refs\":[{\"doc_id\":3,\"note\":\"依据\"}]}]}".to_string(),
            ),
        ];
        for (label, content) in cases {
            assert!(
                parse_page_delivery(&content, "监督检查", 42, "r".into(), &existing(), &known(),)
                    .is_err(),
                "{label} 应被拒绝：{content}"
            );
        }
    }

    fn targets() -> Vec<UpdateTarget> {
        vec![UpdateTarget {
            uuid: "u-1".into(),
            title: "监督检查".into(),
            node_id: 42,
            kind: "normal".into(),
            current_body: "旧正文".into(),
            current_version: 1,
            doc_refs: Vec::new(),
        }]
    }

    #[test]
    fn valid_update_delivery_parses_into_proposal_set() {
        let set = parse_update_delivery(
            r##"{"pages":[
                {"uuid":"u-1","title":"监督检查","kind":"normal","body":"新正文",
                 "change_note":"按 E12 补充闭环口径","doc_refs":[{"doc_id":12,"note":"新通知"}]}
            ]}"##,
            "run-u".into(),
            &known(),
            &targets(),
        )
        .unwrap();
        assert_eq!(set.pages.len(), 1);
        assert_eq!(set.pages[0].uuid.as_deref(), Some("u-1"));
        assert_eq!(set.pages[0].change_note, "按 E12 补充闭环口径");
        assert_eq!(set.pages[0].doc_refs, vec![(12, "新通知".to_string())]);
    }

    #[test]
    fn update_delivery_rejects_invalid_proposals() {
        let base = r##"{"pages":[{"uuid":"u-1","title":"监督检查","kind":"normal","body":"新正文","change_note":"按 E3 补充口径","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"##;
        let good = parse_update_delivery(base, "r".into(), &known(), &targets());
        assert!(good.is_ok(), "{good:?}");

        let cases = [
            ("不是 JSON", "[]".to_string()),
            ("空集合", "{\"pages\":[]}".to_string()),
            (
                "缺 uuid",
                r##"{"pages":[{"title":"监督检查","kind":"normal","body":"新正文","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"##.to_string(),
            ),
            (
                "uuid 不在发起范围",
                r##"{"pages":[{"uuid":"u-9","title":"监督检查","kind":"normal","body":"新正文","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"##.to_string(),
            ),
            (
                "缺变更说明",
                r##"{"pages":[{"uuid":"u-1","title":"监督检查","kind":"normal","body":"新正文","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"##.to_string(),
            ),
            (
                "改了标题",
                r##"{"pages":[{"uuid":"u-1","title":"监督监察","kind":"normal","body":"新正文","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"##.to_string(),
            ),
            (
                "改了类型",
                r##"{"pages":[{"uuid":"u-1","title":"监督检查","kind":"guide","body":"新正文","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"##.to_string(),
            ),
            (
                "正文没有变化",
                r##"{"pages":[{"uuid":"u-1","title":"监督检查","kind":"normal","body":"旧正文","doc_refs":[{"doc_id":3,"note":"依据"}]}]}"##.to_string(),
            ),
            (
                "幽灵依据",
                r##"{"pages":[{"uuid":"u-1","title":"监督检查","kind":"normal","body":"新正文","doc_refs":[{"doc_id":999,"note":""}]}]}"##.to_string(),
            ),
            (
                "本轮重复 uuid",
                r##"{"pages":[
                    {"uuid":"u-1","title":"监督检查","kind":"normal","body":"甲","doc_refs":[{"doc_id":3,"note":"依据"}]},
                    {"uuid":"u-1","title":"监督检查","kind":"normal","body":"乙","doc_refs":[{"doc_id":3,"note":"依据"}]}
                ]}"##.to_string(),
            ),
        ];
        for (label, content) in cases {
            assert!(
                parse_update_delivery(&content, "r".into(), &known(), &targets()).is_err(),
                "{label} 应被拒绝：{content}"
            );
        }
    }

    #[test]
    fn new_page_delivery_rejects_uuid_bearing_pages() {
        let content = r##"{"pages":[
            {"uuid":"u-1","title":"新页","kind":"normal","body":"正文","doc_refs":[{"doc_id":3,"note":"依据"}]}
        ]}"##;
        assert!(
            parse_page_delivery(content, "监督检查", 42, "r".into(), &existing(), &known(),)
                .is_err()
        );
    }
}
