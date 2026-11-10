//! 维基知识库的 Pi 作业：框架归纳（页面整编在切片 5 接入同一作业链）。
//!
//! 与任务作业同一套链：不占写作门禁，host 用技能文件里的自然语言步骤工作，
//! 用 `submit_task` 交一份 JSON；Rust 在这里做确定性复核——主题/分类名合规、
//! 无重名、依据文档编号必须真实存在——通过后也只是一份**提案**，由用户在
//! 知识库页确认后才写框架。
//!
//! 工具面：只读全套（资料库检索/读取、词库、日期），不挂维基工具（框架还没有），
//! 不挂任务工具，不是任务作业（没有 task_only 收窄）。

use super::{
    PiBridge, PiEvent, PiJob, PiMaterialInput, PiOutcome, PiRunLog, PiRunRecord, PiRuntime, PiTask,
    PiToolContext, PiWorkspace, pi_skills, run,
};
use crate::models::{AppConfig, DraftInput};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// 框架作业使用保留的目录键，避开稿件标签键、风格学习的 0 与任务作业的两个保留键。
const FRAME_DOC_KEY: u64 = u64::MAX - 2;
/// 与写作一致的单轮执行上限。
const TIMEOUT: Duration = Duration::from_secs(1800);
/// 主题与分类名的字数上限（与导入大纲同一口径）。
const MAX_NODE_NAME_CHARS: usize = 60;
/// 框架规模上限：防止模型交回失控的大框架撑爆审核界面。
const MAX_THEMES: usize = 30;
const MAX_CATEGORIES_PER_THEME: usize = 20;
/// 单个分类挂靠依据文档的上限。
const MAX_DOC_REFS: usize = 50;

/// 归纳出的框架提案：两级 + 每个分类挂靠的资料文档 id。只表示「模型建议」，
/// 只有用户确认后才写库。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameProposal {
    pub(crate) themes: Vec<FrameTheme>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameTheme {
    pub(crate) name: String,
    pub(crate) categories: Vec<FrameCategory>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameCategory {
    pub(crate) name: String,
    pub(crate) doc_ids: Vec<i64>,
}

/// 交付 JSON 的原始形状（校验后转换成 [`FrameProposal`]）。
#[derive(serde::Deserialize)]
struct RawFrame {
    themes: Vec<RawTheme>,
}

#[derive(serde::Deserialize)]
struct RawTheme {
    name: String,
    categories: Vec<RawCategory>,
}

#[derive(serde::Deserialize)]
struct RawCategory {
    name: String,
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
            if category.doc_ids.len() > MAX_DOC_REFS {
                return Err(format!("分类「{cname}」依据文档超过 {MAX_DOC_REFS} 篇"));
            }
            let mut doc_ids = Vec::new();
            for id in category.doc_ids {
                if !known.contains(&id) {
                    return Err(format!(
                        "分类「{cname}」依据的文档 {id} 不在资料库里；只填 kb_search / kb_read 真实返回的编号"
                    ));
                }
                if !doc_ids.contains(&id) {
                    doc_ids.push(id);
                }
            }
            categories.push(FrameCategory {
                name: cname,
                doc_ids,
            });
        }
        themes.push(FrameTheme { name, categories });
    }
    Ok(FrameProposal { themes })
}

/// 资料库里现存的文档编号，供依据校验。
fn known_doc_ids(store_path: &Path) -> Result<Vec<i64>, String> {
    let mut store = crate::knowledge::KnowledgeStore::open(store_path)
        .map_err(|error| format!("打开资料库失败：{error:#}"))?;
    store
        .list_docs(None)
        .map(|rows| rows.into_iter().map(|row| row.id).collect())
        .map_err(|error| format!("读取资料库文档失败：{error:#}"))
}

/// 浏览资料库归纳两级框架：返回通过确定性复核的提案，由用户确认后写库。
pub(crate) fn induce_frame(
    config: Arc<AppConfig>,
    store_path: PathBuf,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(String),
) -> Result<FrameProposal, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止归纳框架。".into());
    }
    let catalog = pi_skills::load_catalog();
    catalog.resolve(Some(pi_skills::WIKI_FRAME_SKILL))?;
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let request = format!(
        "按技能 {} 浏览资料库，归纳两级知识框架（主题 → 分类），每个分类挂靠它实际依据的\
         资料文档，最后用 submit_task 交 work/frame.json。今天日期：{today}。",
        pi_skills::WIKI_FRAME_SKILL,
    );
    // 框架归纳不注入材料：模型用 kb_search / kb_read 自己遍历资料库。
    let materials: Vec<PiMaterialInput> = Vec::new();
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
    let tools = Arc::new(PiToolContext::new(
        Arc::clone(&config),
        DraftInput::default(),
        String::new(),
        request.clone(),
        Vec::new(),
    ));
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
            tools,
            bridge: Arc::new(PiBridge::default()),
            record,
            resume: false,
            request_id,
            run_id,
            timeout: TIMEOUT,
            proxy_env: crate::net::proxy_env(&config.proxy),
        },
        Arc::clone(&cancel),
        |event| match event {
            PiEvent::Progress(message) | PiEvent::Tool(message) => on_progress(message),
            PiEvent::Activity(activity) if activity.outcome.is_none() => {
                on_progress(activity.action)
            }
            PiEvent::Strategy(info) => on_progress(info.line()),
            _ => {}
        },
    )
    .map_err(|error| format!("{error}；工作文件保留在 {label}"))?;
    if cancel.load(Ordering::Relaxed) {
        return Err("已停止归纳框架，未采纳本轮结果。".into());
    }
    let PiOutcome::Task(delivery) = outcome else {
        return Err("Pi 没有交回框架提案，未采纳。".into());
    };
    parse_frame_delivery(&delivery.content, &known_doc_ids(&store_path)?)
        .map_err(|error| format!("框架提案未通过复核：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<i64> {
        vec![3, 7, 12]
    }

    #[test]
    fn valid_frame_delivery_parses_into_proposal() {
        let proposal = parse_frame_delivery(
            r#"{"themes":[
                {"name":"安全生产","categories":[
                    {"name":"监督检查","doc_ids":[3,7,3]},
                    {"name":"应急管理","doc_ids":[]}
                ]},
                {"name":"政务服务","categories":[{"name":"一网通办","doc_ids":[12]}]}
            ]}"#,
            &known(),
        )
        .unwrap();
        assert_eq!(proposal.themes.len(), 2);
        assert_eq!(proposal.themes[0].categories[0].doc_ids, vec![3, 7], "去重");
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
                "{\"themes\":[{\"name\":\"甲\",\"categories\":[{\"name\":\"x\"},{\"name\":\"x\"}]}]}",
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
}
