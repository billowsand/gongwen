//! Pi 标准技能：内置只读默认 + 配置目录里的用户技能，供管理页、侧栏与工作区快照共用。
//!
//! 技能是标准 Markdown（YAML frontmatter + 自然语言步骤），**不是流程 DSL**。
//! 权限、事实来源与交付闸门都在 Rust 程序里判定；这里只负责发现、解析、保存与
//! 复制文件，不执行脚本，也不扩大 host 权限。
//!
//! 代码分组：
//! - `parser`：frontmatter 与名称校验，识别旧流程字段；
//! - `catalog`：内置 / 用户覆盖 / 停用合并后的有效目录；
//! - `store`：`配置目录/pi/skills/<name>/` 的读写、导入导出；
//! - `snapshot`：每轮任务的技能快照与 `input/skills.json`。
//!
//! 旧 `配置目录/skills/` 仍保留旧流程技能数据，但 Pi 不读取、不迁移、不执行。

mod catalog;
mod parser;
mod snapshot;
pub(crate) mod store;

pub(crate) use catalog::{Catalog, Entry};
pub(crate) use parser::valid_name;
pub(crate) use snapshot::SkillSnapshot;
pub(crate) use snapshot::content_digest;

use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! skill_file {
    ($skill:literal, $path:literal) => {
        (
            $skill,
            $path,
            include_str!(concat!("../../../assets/pi-skills/", $skill, "/", $path)),
        )
    };
}

/// 内置技能的全部文件：（技能名，包内相对路径，内容）。
///
/// 整包编进二进制：新建工作区、复制为用户副本、导出技能包都从这里取，
/// 不依赖安装目录里恰好带着 `assets/pi-skills/`。新增文件必须登记在这里，
/// `every_builtin_file_on_disk_is_embedded` 会核对两边一致。
pub(crate) const FILES: &[(&str, &str, &str)] = &[
    skill_file!("gongwen-style-learn", "SKILL.md"),
    skill_file!("gongwen-style-learn", "references/style-profile.md"),
    skill_file!("gongwen-draft", "SKILL.md"),
    skill_file!("gongwen-draft", "references/material-drafting.md"),
    skill_file!("gongwen-draft", "references/phased-writing.md"),
    skill_file!("gongwen-revise", "SKILL.md"),
    skill_file!("gongwen-revise", "references/condense.md"),
    skill_file!("gongwen-revise", "references/normalize.md"),
    skill_file!("gongwen-revise", "references/polish.md"),
    skill_file!("gongwen-revise", "references/tone.md"),
    skill_file!("gongwen-review", "SKILL.md"),
    skill_file!("gongwen-review", "references/fact-check.md"),
    skill_file!("gongwen-imitate", "SKILL.md"),
    skill_file!("gongwen-imitate", "references/style-analysis.md"),
    skill_file!("gongwen-research", "SKILL.md"),
    skill_file!(
        "gongwen-research",
        "references/argumentation-and-sources.md"
    ),
    skill_file!("gongwen-reply", "SKILL.md"),
    skill_file!("gongwen-reply", "references/incoming-letter.md"),
    skill_file!("gongwen-extract", "SKILL.md"),
    skill_file!("gongwen-extract", "references/extraction-checklist.md"),
    skill_file!("gongwen-task-extract", "SKILL.md"),
    skill_file!("gongwen-task-extract", "references/candidate-kinds.md"),
    skill_file!("gongwen-task-fill", "SKILL.md"),
    skill_file!("gongwen-task-fill", "references/form-fields.md"),
    skill_file!("gongwen-policy-check", "SKILL.md"),
    skill_file!("gongwen-policy-check", "references/citation-check.md"),
];

/// 内置技能目录名，同时是 frontmatter 的 name。顺序就是管理页里的固定顺序。
pub(crate) const STYLE_SKILL: &str = "gongwen-style-learn";
/// 任务助手的两份作业技能：材料整理与单任务填写。
///
/// 它们不是写作技能，不进侧栏写作下拉；由任务助手页面直接指定，交付走
/// `submit_task`（JSON 回执，`kind = task`），不写公文正文。
pub(crate) const TASK_EXTRACT_SKILL: &str = "gongwen-task-extract";
pub(crate) const TASK_FILL_SKILL: &str = "gongwen-task-fill";

pub(crate) fn is_task_skill(name: &str) -> bool {
    matches!(name, TASK_EXTRACT_SKILL | TASK_FILL_SKILL)
}

pub(crate) const BUILTINS: &[&str] = &[
    "gongwen-draft",
    "gongwen-revise",
    "gongwen-imitate",
    "gongwen-research",
    "gongwen-reply",
    "gongwen-extract",
    TASK_EXTRACT_SKILL,
    TASK_FILL_SKILL,
    STYLE_SKILL,
];

/// 已退出新任务的审校技能。历史文件和用户副本保留，不再列入可执行目录。
pub(crate) const RETIRED: &[&str] = &["gongwen-review", "gongwen-policy-check"];

/// 内置技能的中文显示名。用户副本没有写 metadata 显示名时沿用。
pub(crate) fn builtin_label(name: &str) -> Option<&'static str> {
    match name {
        "gongwen-style-learn" => Some("风格学习"),
        "gongwen-draft" => Some("起草"),
        "gongwen-revise" => Some("修改"),
        "gongwen-review" => Some("审核"),
        "gongwen-imitate" => Some("仿写"),
        "gongwen-research" => Some("研究报告"),
        "gongwen-reply" => Some("复函"),
        "gongwen-extract" => Some("要点提炼"),
        TASK_EXTRACT_SKILL => Some("任务材料整理"),
        TASK_FILL_SKILL => Some("任务表单填写"),
        "gongwen-policy-check" => Some("政策依据核对"),
        _ => None,
    }
}

/// 内置技能包里的一个文件；没有登记就返回 `None`。
pub(crate) fn builtin_file(name: &str, relative: &str) -> Option<&'static str> {
    FILES
        .iter()
        .find(|(skill, path, _)| *skill == name && *path == relative)
        .map(|(_, _, text)| *text)
}

pub(crate) fn builtin_text(name: &str) -> Option<&'static str> {
    builtin_file(name, "SKILL.md")
}

/// 目录变化计数：管理页保存 / 启停 / 导入后加一，侧栏据此刷新缓存，不每帧读盘。
static REVISION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

fn bump_revision() {
    REVISION.fetch_add(1, Ordering::Relaxed);
}

/// 读有效目录。单技能损坏只记在条目与提示里，不影响整页。
pub(crate) fn load_catalog() -> Catalog {
    catalog::load()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn source_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("pi-skills")
    }

    fn files_under(dir: &std::path::Path, base: &std::path::Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).expect("技能目录可读") {
            let path = entry.expect("目录项").path();
            if path.is_dir() {
                files_under(&path, base, out);
            } else {
                let relative = path.strip_prefix(base).expect("在技能目录内");
                out.insert(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    #[test]
    fn active_builtin_skills_are_valid_standard_skills() {
        assert_eq!(BUILTINS.len(), 9);
        for name in BUILTINS {
            let text = builtin_text(name).unwrap_or_else(|| panic!("{name} 缺 SKILL.md"));
            let parsed = parser::parse(name, text).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(parsed.name, *name);
            assert!(!parsed.description.is_empty());
            assert!(!text.contains("```flow"));
            assert!(!text.contains("{{"));
            assert!(
                text.contains("submit_document")
                    || text.contains("finish_report")
                    || text.contains("submit_task")
                    || text.contains("submit_style")
            );
            assert!(builtin_label(name).is_some());
        }
    }

    #[test]
    fn every_builtin_file_on_disk_is_embedded() {
        let dir = source_dir();
        let mut on_disk = BTreeSet::new();
        files_under(&dir, &dir, &mut on_disk);
        let embedded = FILES
            .iter()
            .map(|(skill, path, _)| format!("{skill}/{path}"))
            .collect::<BTreeSet<_>>();
        assert_eq!(on_disk, embedded, "assets/pi-skills/ 与 FILES 登记不一致");
    }

    #[test]
    fn every_builtin_skill_has_its_skill_md_and_registered_references() {
        for name in BUILTINS {
            assert!(builtin_text(name).is_some(), "{name} 缺 SKILL.md");
            assert!(
                FILES
                    .iter()
                    .any(|(skill, path, _)| skill == name && *path != "SKILL.md"),
                "{name} 没有任何参考文件"
            );
        }
        // SKILL.md 里点名的 references/... 必须真的在包里，否则模型会读空气。
        for name in BUILTINS {
            let text = builtin_text(name).unwrap();
            for relative in referenced_files(text) {
                assert!(
                    builtin_file(name, &relative).is_some(),
                    "{name}/SKILL.md 引用了不存在的 {relative}"
                );
            }
        }
    }

    /// 从正文里扫出 `references/xxx.md` 形式的相对引用（只认代码标记里的，
    /// `skills/gongwen-markdown/references/...` 这类跨包路径不算本包文件）。
    fn referenced_files(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = text;
        while let Some(index) = rest.find("`references/") {
            rest = &rest[index + 1..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')))
                .unwrap_or(rest.len());
            let candidate = &rest[..end];
            if candidate.ends_with(".md") && !out.iter().any(|seen| seen == candidate) {
                out.push(candidate.to_string());
            }
            rest = &rest[end..];
        }
        out
    }

    #[test]
    fn revision_only_moves_when_a_skill_changes() {
        let before = revision();
        assert_eq!(revision(), before);
        crate::pi_agent::pi_skills::bump_revision();
        assert!(revision() > before);
    }
}
