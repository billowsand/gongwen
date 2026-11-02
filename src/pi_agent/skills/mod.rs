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

use std::sync::atomic::{AtomicU64, Ordering};

/// 内置五份技能：目录名同时是 frontmatter 的 name。
pub(crate) const BUILTINS: &[(&str, &str)] = &[
    (
        "gongwen-draft",
        include_str!("../../../assets/pi-skills/gongwen-draft/SKILL.md"),
    ),
    (
        "gongwen-revise",
        include_str!("../../../assets/pi-skills/gongwen-revise/SKILL.md"),
    ),
    (
        "gongwen-review",
        include_str!("../../../assets/pi-skills/gongwen-review/SKILL.md"),
    ),
    (
        "gongwen-imitate",
        include_str!("../../../assets/pi-skills/gongwen-imitate/SKILL.md"),
    ),
    (
        "gongwen-research",
        include_str!("../../../assets/pi-skills/gongwen-research/SKILL.md"),
    ),
];

/// 内置技能的中文显示名。用户副本没有写 metadata 显示名时沿用。
pub(crate) fn builtin_label(name: &str) -> Option<&'static str> {
    match name {
        "gongwen-draft" => Some("起草"),
        "gongwen-revise" => Some("修改"),
        "gongwen-review" => Some("审核"),
        "gongwen-imitate" => Some("仿写"),
        "gongwen-research" => Some("研究报告"),
        _ => None,
    }
}

pub(crate) fn builtin_text(name: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|(builtin, _)| *builtin == name)
        .map(|(_, text)| *text)
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

    #[test]
    fn five_builtin_skills_are_valid_standard_skills() {
        assert_eq!(BUILTINS.len(), 5);
        for (name, text) in BUILTINS {
            let parsed = parser::parse(name, text).unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(parsed.name, *name);
            assert!(!parsed.description.is_empty());
            assert!(!text.contains("```flow"));
            assert!(!text.contains("{{"));
            assert!(text.contains("submit_document") || text.contains("finish_report"));
            assert!(builtin_label(name).is_some());
        }
    }

    #[test]
    fn revision_only_moves_when_a_skill_changes() {
        let before = revision();
        assert_eq!(revision(), before);
        crate::pi_agent::pi_skills::bump_revision();
        assert!(revision() > before);
    }
}
