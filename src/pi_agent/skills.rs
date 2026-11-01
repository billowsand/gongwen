//! Pi 智能写作的标准技能：随应用资源安装到工作区只读目录，host 只做发现与按需读取。
//!
//! 技能是标准 Markdown（YAML frontmatter + 自然语言步骤），**不是流程 DSL**：
//! 没有 `flow`、`step`、程序变量插值，也不在这里解释执行顺序。权限、事实来源与
//! 交付闸门都在 Rust 程序里判定。语法与文种规则仍复用 `skills/gongwen-markdown/`，
//! 这里不复制第二套。

use std::path::Path;

/// (目录名, SKILL.md 全文)。目录名同时是 frontmatter 里的 `name`。
pub(crate) const SKILLS: &[(&str, &str)] = &[
    (
        "gongwen-draft",
        include_str!("../../assets/pi-skills/gongwen-draft/SKILL.md"),
    ),
    (
        "gongwen-revise",
        include_str!("../../assets/pi-skills/gongwen-revise/SKILL.md"),
    ),
    (
        "gongwen-review",
        include_str!("../../assets/pi-skills/gongwen-review/SKILL.md"),
    ),
    (
        "gongwen-imitate",
        include_str!("../../assets/pi-skills/gongwen-imitate/SKILL.md"),
    ),
    (
        "gongwen-research",
        include_str!("../../assets/pi-skills/gongwen-research/SKILL.md"),
    ),
];

/// 技能索引：写进 `input/request.md` 与 host 的用户消息，模型据此按需读取。
pub(crate) fn index_text() -> String {
    let mut lines = vec!["可用技能（只读，位于 skills/pi-skills/<目录>/SKILL.md）：".to_string()];
    for (name, content) in SKILLS {
        lines.push(format!(
            "- {name}（{}）：skills/pi-skills/{name}/SKILL.md",
            describe(content)
        ));
    }
    lines.join("\n")
}

/// 从 frontmatter 读 description，只取第一行。
pub(crate) fn describe(content: &str) -> &str {
    frontmatter(content)
        .and_then(|meta| {
            meta.lines()
                .find_map(|line| line.trim().strip_prefix("description:"))
        })
        .map(str::trim)
        .unwrap_or("")
}

/// frontmatter 是否合规：`---` 包围、name 与目录同名、description 非空、
/// 正文有内容，且没有流程解释器关键字。
pub(crate) fn validate(name: &str, content: &str) -> Result<(), String> {
    let meta =
        frontmatter(content).ok_or_else(|| format!("{name}：缺少 --- 包围的 frontmatter"))?;
    let declared = meta
        .lines()
        .find_map(|line| line.trim().strip_prefix("name:"))
        .map(str::trim);
    if declared != Some(name) {
        return Err(format!("{name}：frontmatter 的 name 应是 {name}"));
    }
    if describe(content).is_empty() {
        return Err(format!("{name}：description 不能为空"));
    }
    let body = content
        .split_once("\n---")
        .map(|(_, rest)| rest.trim_start_matches('\n'))
        .unwrap_or("");
    if body.trim().is_empty() {
        return Err(format!("{name}：正文不能为空"));
    }
    for keyword in ["```flow", "flow:", "step:", "variables:"] {
        if content.contains(keyword) {
            return Err(format!(
                "{name}：技能是自然语言步骤，不能包含流程执行字段 {keyword}"
            ));
        }
    }
    Ok(())
}

fn frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// 安装到 `<workspace>/skills/pi-skills/`。只写 SKILL.md；host 负责列出索引。
pub(crate) fn install(skills_root: &Path) -> Result<(), String> {
    let root = skills_root.join("pi-skills");
    for (name, content) in SKILLS {
        validate(name, content)?;
        let dir = root.join(name);
        std::fs::create_dir_all(&dir)
            .map_err(|error| format!("建不了技能目录（{}）：{error}", dir.display()))?;
        let path = dir.join("SKILL.md");
        crate::storage::atomic_write(&path, |file| {
            use std::io::Write;
            file.write_all(content.as_bytes())?;
            Ok(())
        })
        .map_err(|error| format!("写不出技能文件（{}）：{error:#}", path.display()))?;
    }
    Ok(())
}

/// 技能名清单（界面下拉框与测试用）。
pub(crate) fn names() -> Vec<&'static str> {
    SKILLS.iter().map(|(name, _)| *name).collect()
}

/// 用户指定的技能必须在这份清单里；`None` 表示自动。
pub(crate) fn resolve(selected: Option<&str>) -> Result<Option<&'static str>, String> {
    let Some(selected) = selected else {
        return Ok(None);
    };
    SKILLS
        .iter()
        .find(|(name, _)| *name == selected)
        .map(|(name, _)| Some(*name))
        .ok_or_else(|| format!("没有名为 {selected} 的 Pi 技能"))
}

/// 界面显示名。
pub(crate) fn label(name: &str) -> &'static str {
    match name {
        "gongwen-draft" => "起草",
        "gongwen-revise" => "修改",
        "gongwen-review" => "审核",
        "gongwen-imitate" => "仿写",
        "gongwen-research" => "研究报告",
        _ => "自动",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_standard_skills_are_valid_and_installed() {
        assert_eq!(SKILLS.len(), 5);
        for (name, content) in SKILLS {
            validate(name, content).unwrap_or_else(|error| panic!("{error}"));
        }
        let temp = tempfile::tempdir().unwrap();
        install(temp.path()).unwrap();
        for (name, _) in SKILLS {
            let path = temp.path().join("pi-skills").join(name).join("SKILL.md");
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(text.contains(&format!("name: {name}")));
        }
    }

    #[test]
    fn skills_are_prose_not_a_flow_interpreter() {
        for (name, content) in SKILLS {
            assert!(!content.contains("```flow"), "{name}");
            assert!(!content.contains("{{"), "{name}：不做程序变量插值");
            assert!(content.contains("submit_document") || content.contains("finish_report"));
        }
        assert!(resolve(Some("gongwen-draft")).unwrap().is_some());
        assert!(resolve(Some("unknown")).is_err());
        assert!(resolve(None).unwrap().is_none());
        assert!(index_text().contains("skills/pi-skills/gongwen-review/SKILL.md"));
    }

    #[test]
    fn invalid_frontmatter_is_rejected() {
        assert!(
            validate(
                "gongwen-draft",
                "---\nname: other\ndescription: x\n---\n正文"
            )
            .is_err()
        );
        assert!(validate("gongwen-draft", "---\nname: gongwen-draft\n---\n正文").is_err());
        assert!(
            validate(
                "gongwen-draft",
                "---\nname: gongwen-draft\ndescription: x\n---\nflow: [a, b]"
            )
            .is_err()
        );
    }
}
