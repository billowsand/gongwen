//! SKILL.md 技能文件：一个流程的提示词与参数。
//!
//! 格式沿用 Agent Skills 的写法（与仓库里的 `skills/gongwen-markdown/` 同构）：
//!
//! ```text
//! ---
//! name: 研究式起草
//! description: ……
//! triggers: [起草, 写]
//! max_rounds: 3
//! ---
//! # 标题
//! ## 步骤名
//! 这一步的提示词，{变量} 由程序替换
//! ```
//!
//! 内置一份编进二进制；用户把同名目录放到 `配置目录/skills/<目录名>/SKILL.md` 就能覆盖。
//! 用户版缺了哪个步骤，那一步沿用内置写法——改一处提示词不必把整份抄全。

use std::collections::BTreeMap;

/// 内置的研究式起草技能。
pub(crate) const RESEARCH_DRAFT_DIR: &str = "research-draft";
const RESEARCH_DRAFT_BUILTIN: &str =
    include_str!("../../assets/agent-skills/research-draft/SKILL.md");

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Skill {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) triggers: Vec<String>,
    /// 其余 frontmatter 键值，流程参数从这里取。
    pub(crate) params: BTreeMap<String, String>,
    /// 二级标题 → 正文。
    pub(crate) sections: BTreeMap<String, String>,
    /// 从哪里读来的：内置，或用户文件的路径。
    pub(crate) origin: String,
}

impl Skill {
    pub(crate) fn section(&self, name: &str) -> Option<&str> {
        self.sections.get(name).map(String::as_str)
    }

    /// 整数参数；缺了或写错就用默认值，并夹在 `range` 里，防止手误写出 0 轮或 1000 轮。
    pub(crate) fn param_usize(
        &self,
        key: &str,
        default: usize,
        range: std::ops::RangeInclusive<usize>,
    ) -> usize {
        self.params
            .get(key)
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(default)
            .clamp(*range.start(), *range.end())
    }

    /// 取一个步骤的提示词并替换变量；该步骤缺失时返回 None。
    pub(crate) fn render(&self, section: &str, vars: &[(&str, &str)]) -> Option<String> {
        let mut text = self.section(section)?.to_string();
        for (key, value) in vars {
            text = text.replace(&format!("{{{key}}}"), value);
        }
        Some(text)
    }

    /// 用 `fallback` 补齐缺失的步骤与参数。
    fn merged_with(mut self, fallback: &Skill) -> Skill {
        for (key, value) in &fallback.sections {
            self.sections
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        for (key, value) in &fallback.params {
            self.params
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        if self.name.is_empty() {
            self.name = fallback.name.clone();
        }
        if self.description.is_empty() {
            self.description = fallback.description.clone();
        }
        if self.triggers.is_empty() {
            self.triggers = fallback.triggers.clone();
        }
        self
    }
}

/// 解析 SKILL.md。frontmatter 只认 `键: 值` 与 `键: [甲, 乙]` 两种写法，够用且不必引 YAML 库。
pub(crate) fn parse(text: &str, origin: &str) -> Result<Skill, String> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let rest = text
        .strip_prefix("---\n")
        .ok_or("SKILL.md 缺少开头的 --- frontmatter")?;
    let (front, body) = rest
        .split_once("\n---")
        .ok_or("SKILL.md 的 frontmatter 没有用 --- 收尾")?;
    let mut skill = Skill {
        origin: origin.to_string(),
        ..Skill::default()
    };
    for line in front.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("frontmatter 这一行看不懂：{line}"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "name" => skill.name = value.to_string(),
            "description" => skill.description = value.to_string(),
            "triggers" => skill.triggers = parse_list(value),
            _ => {
                skill.params.insert(key.to_string(), value.to_string());
            }
        }
    }
    let mut current: Option<(String, String)> = None;
    for line in body.lines() {
        if let Some(title) = line.strip_prefix("## ") {
            if let Some((name, text)) = current.take() {
                skill.sections.insert(name, text.trim().to_string());
            }
            current = Some((title.trim().to_string(), String::new()));
        } else if line.starts_with("# ") {
            if let Some((name, text)) = current.take() {
                skill.sections.insert(name, text.trim().to_string());
            }
        } else if let Some((_, text)) = current.as_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    if let Some((name, text)) = current {
        skill.sections.insert(name, text.trim().to_string());
    }
    Ok(skill)
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split([',', '，'])
        .map(|item| item.trim().trim_matches(['"', '\'']).to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// 内置的研究式起草技能。内置文件解析失败是编译期就该发现的错误，测试锁住。
pub(crate) fn builtin_research_draft() -> Skill {
    parse(RESEARCH_DRAFT_BUILTIN, "内置").expect("内置 SKILL.md 必须能解析")
}

/// 读研究式起草技能：用户目录里有就用用户的（缺的步骤用内置补齐），否则用内置。
/// 用户文件写坏了也不中断起草，退回内置并返回一条说明。
pub(crate) fn load_research_draft() -> (Skill, Option<String>) {
    let builtin = builtin_research_draft();
    let Ok(dir) = crate::storage::config_dir() else {
        return (builtin, None);
    };
    let path = dir.join("skills").join(RESEARCH_DRAFT_DIR).join("SKILL.md");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (builtin, None);
    };
    match parse(&text, &path.display().to_string()) {
        Ok(user) => (user.merged_with(&builtin), None),
        Err(error) => (
            builtin,
            Some(format!(
                "技能文件 {} 解析失败，已改用内置版本：{error}",
                path.display()
            )),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_skill_has_every_step_and_sane_parameters() {
        let skill = builtin_research_draft();
        assert_eq!(skill.name, "研究式起草");
        for step in [
            "动笔前澄清",
            "预研",
            "起草附加要求",
            "缺口修订",
            "来源核对",
            "核验",
        ] {
            assert!(skill.section(step).is_some(), "缺少步骤：{step}");
        }
        assert_eq!(skill.param_usize("max_rounds", 0, 1..=10), 3);
        assert!(skill.triggers.contains(&"起草".to_string()));
        // 变量都写在花括号里，渲染后不该残留。
        let rendered = skill
            .render(
                "缺口修订",
                &[("hint", "甲"), ("sentence", "乙"), ("evidence", "丙")],
            )
            .unwrap();
        assert!(!rendered.contains('{'), "{rendered}");
    }

    #[test]
    fn parsing_reads_lists_params_and_sections() {
        let text = "---\nname: 测试\ntriggers: [写, \"拟\"]\nmax_rounds: 9\n---\n# 题\n## 甲\n第一行\n\n第二行\n## 乙\n内容\n";
        let skill = parse(text, "测试").unwrap();
        assert_eq!(skill.triggers, ["写", "拟"]);
        assert_eq!(skill.section("甲"), Some("第一行\n\n第二行"));
        assert_eq!(skill.section("乙"), Some("内容"));
        assert_eq!(skill.param_usize("max_rounds", 3, 1..=5), 5, "越界夹回范围");
        assert_eq!(skill.param_usize("missing", 3, 1..=5), 3);
    }

    #[test]
    fn broken_files_are_rejected_with_a_reason() {
        assert!(
            parse("没有 frontmatter", "x")
                .unwrap_err()
                .contains("frontmatter")
        );
        assert!(parse("---\nname: 甲\n", "x").unwrap_err().contains("收尾"));
        assert!(
            parse("---\n乱写一行\n---\n", "x")
                .unwrap_err()
                .contains("看不懂")
        );
    }

    #[test]
    fn a_partial_user_skill_falls_back_to_builtin_steps() {
        let user = parse("---\nmax_rounds: 2\n---\n## 预研\n只查政策依据。\n", "用户").unwrap();
        let merged = user.merged_with(&builtin_research_draft());
        assert_eq!(merged.section("预研"), Some("只查政策依据。"));
        assert!(merged.section("缺口修订").is_some());
        assert_eq!(merged.param_usize("max_rounds", 3, 1..=10), 2);
        assert_eq!(merged.name, "研究式起草");
    }

    #[test]
    fn user_override_is_read_from_the_config_dir() {
        let dir = std::env::temp_dir().join(format!("gongwen-skill-{}", std::process::id()));
        let skill_dir = dir.join("skills").join(RESEARCH_DRAFT_DIR);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nmax_rounds: 1\n---\n").unwrap();
        crate::storage::set_test_config_dir(Some(dir.clone()));
        let (skill, note) = load_research_draft();
        assert!(note.is_none());
        assert_eq!(skill.param_usize("max_rounds", 3, 1..=10), 1);
        std::fs::write(skill_dir.join("SKILL.md"), "坏掉的文件").unwrap();
        let (skill, note) = load_research_draft();
        assert!(note.unwrap().contains("已改用内置版本"));
        assert_eq!(skill.param_usize("max_rounds", 3, 1..=10), 3);
        crate::storage::set_test_config_dir(None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
