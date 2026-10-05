//! 技能文件的管理：启用 / 停用、保存（带校验）、复制内置、新建、删除、导入导出。
//!
//! 用户技能都在 `配置目录/skills/<id>/SKILL.md`；停用的 id 记在 `配置目录/skills/disabled.json`
//! （内置技能没有文件可改，停用只能记在这里）。`package` 子模块完整保留辅助文件与子目录，
//! 导入导出以整个技能包为单位；应用的数据接口配置与密钥不加入技能包。

use super::skill::{self, Skill};
use std::path::PathBuf;

pub(crate) mod package;
pub(crate) use package::{export, import, remove};

const DISABLED_FILE: &str = "disabled.json";

/// 新建技能的模板。
const TEMPLATE: &str = "---
name: 新技能
description: 一句话写清这个技能做什么、什么时候用
hint: 输入框里的提示，告诉用户该写些什么
triggers: []
when: { text: any }
output: proposal
tools: [doc.read, kb.search, llm.generate, ws.write]
flow:
  - step: retrieve
  - step: generate
    prompt: 生成
---

# 新技能

开头的 `flow` 是流程：`step` 是算子，`tool` 是工具，按顺序执行。正文每个二级标题是一段提示词。

## 生成

【证据】
{evidence}
";

fn skills_dir() -> anyhow::Result<PathBuf> {
    Ok(crate::storage::config_dir()?.join("skills"))
}

/// 技能 id 只能用英文字母、数字、下划线和短横线（它是文件夹名）。
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn check_id(id: &str) -> anyhow::Result<()> {
    if valid_id(id) {
        Ok(())
    } else {
        anyhow::bail!("技能 id「{id}」只能用英文字母、数字、下划线和短横线")
    }
}

pub(crate) fn user_file(id: &str) -> anyhow::Result<PathBuf> {
    check_id(id)?;
    Ok(skills_dir()?.join(id).join("SKILL.md"))
}

/// 停用的技能 id。
pub(crate) fn disabled_ids() -> Vec<String> {
    skills_dir()
        .ok()
        .and_then(|dir| std::fs::read_to_string(dir.join(DISABLED_FILE)).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(crate) fn set_enabled(id: &str, enabled: bool) -> anyhow::Result<()> {
    let mut ids = disabled_ids();
    ids.retain(|existing| existing != id);
    if !enabled {
        ids.push(id.to_string());
    }
    ids.sort();
    let dir = skills_dir()?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(DISABLED_FILE), serde_json::to_string_pretty(&ids)?)?;
    Ok(())
}

/// 编辑器里显示的原文：有用户文件就是用户文件，否则是内置原文。
pub(crate) fn source_text(id: &str) -> anyhow::Result<String> {
    let file = user_file(id)?;
    if file.exists() {
        return Ok(std::fs::read_to_string(&file)?);
    }
    skill::builtin_text(id)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("找不到技能「{id}」的文件"))
}

pub(crate) fn has_user_file(id: &str) -> bool {
    user_file(id).is_ok_and(|file| file.exists())
}

/// 按引擎认识的算子与工具校验一份技能原文（覆盖内置的按合并后的结果校验）。
/// 解析失败返回 Err，校验问题放在 Ok 里。
pub(crate) fn check_text(id: &str, text: &str) -> Result<Vec<String>, String> {
    let parsed = skill::parse(id, text, "编辑器")?;
    let merged = match skill::builtin_skills().into_iter().find(|s| s.id == id) {
        Some(builtin) => parsed.merged_with(&builtin),
        None => parsed,
    };
    Ok(problems_of(&merged))
}

pub(crate) fn problems_of(skill: &Skill) -> Vec<String> {
    skill::validate(skill, &super::ops::names(), &super::tools::ids())
}

/// 保存用户技能。解析不过拒绝保存；校验问题照样保存，返回给界面提示（加载时会停用或
/// 退回内置）。
pub(crate) fn save(id: &str, text: &str) -> Result<Vec<String>, String> {
    let problems = check_text(id, text)?;
    let file = package::destination(id, "SKILL.md").map_err(|e| e.to_string())?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, text).map_err(|e| format!("写入 {} 失败：{e}", file.display()))?;
    Ok(problems)
}

/// 新建一个技能（写入模板），返回原文。id 已存在就报错。
pub(crate) fn create(id: &str) -> anyhow::Result<String> {
    check_id(id)?;
    if has_user_file(id) || skill::builtin_text(id).is_some() {
        anyhow::bail!("已经有 id 为「{id}」的技能了");
    }
    save(id, TEMPLATE).map_err(anyhow::Error::msg)?;
    Ok(TEMPLATE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::skill::{POLISH, RESEARCH_DRAFT};
    use std::path::Path;

    fn with_dir<R>(name: &str, f: impl FnOnce(&Path) -> R) -> R {
        let dir =
            std::env::temp_dir().join(format!("gongwen-skillfiles-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::storage::set_test_config_dir(Some(dir.clone()));
        let result = f(&dir);
        crate::storage::set_test_config_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn disabling_hides_a_skill_from_loading() {
        with_dir("disable", |_| {
            set_enabled(POLISH, false).unwrap();
            assert_eq!(disabled_ids(), [POLISH]);
            let (skills, _) = skill::load_all();
            assert!(!skills.iter().find(|s| s.id == POLISH).unwrap().enabled);
            set_enabled(POLISH, true).unwrap();
            let (skills, _) = skill::load_all();
            assert!(skills.iter().find(|s| s.id == POLISH).unwrap().enabled);
        });
    }

    #[test]
    fn saving_checks_the_text_and_removing_restores_the_builtin() {
        with_dir("save", |_| {
            assert!(
                save(POLISH, "没有 frontmatter")
                    .unwrap_err()
                    .contains("frontmatter")
            );
            assert!(!has_user_file(POLISH), "解析不过不写文件");
            let problems =
                save(POLISH, "---\ntools: [乱写]\n---\n## 润色\n只改错别字。\n").unwrap();
            assert!(problems.iter().any(|p| p.contains("乱写")), "{problems:?}");
            assert!(source_text(POLISH).unwrap().contains("只改错别字"));
            remove(POLISH).unwrap();
            assert!(
                source_text(POLISH).unwrap().contains("name: 润色"),
                "删掉覆盖文件就回到内置"
            );
            assert!(
                check_text(RESEARCH_DRAFT, &source_text(RESEARCH_DRAFT).unwrap())
                    .unwrap()
                    .is_empty()
            );
        });
    }

    #[test]
    fn new_skills_start_from_a_valid_template() {
        with_dir("create", |_| {
            let text = create("my-skill").unwrap();
            assert!(
                check_text("my-skill", &text).unwrap().is_empty(),
                "模板本身要能通过校验"
            );
            assert!(
                create("my-skill")
                    .unwrap_err()
                    .to_string()
                    .contains("已经有")
            );
            assert!(create(POLISH).is_err(), "不能和内置重名");
            assert!(create("中文 id").is_err());
            assert!(
                package::save_file("my-skill", "skill.md", "").is_err(),
                "Windows 上不能绕过入口校验覆盖同名文件"
            );
            assert!(check_text("my-skill", &source_text("my-skill").unwrap()).is_ok());
        });
    }

    #[test]
    fn export_and_import_round_trip_as_folder_and_package() {
        with_dir("io", |dir| {
            create("mine").unwrap();
            let package = export("mine", &dir.join("mine.skill")).unwrap();
            let folder = export(POLISH, &dir.join("out")).unwrap();
            assert!(folder.ends_with("polish/SKILL.md") || folder.ends_with("polish\\SKILL.md"));
            remove("mine").unwrap();
            let (ids, notes) = import(&package).unwrap();
            assert_eq!(ids, ["mine"]);
            assert!(notes.is_empty(), "{notes:?}");
            assert!(has_user_file("mine"));
            let (ids, _) = import(&dir.join("out")).unwrap();
            assert_eq!(ids, [POLISH]);
            std::fs::create_dir_all(dir.join("empty")).unwrap();
            assert!(import(&dir.join("empty")).is_err());
        });
    }

    #[test]
    fn package_round_trip_preserves_nested_text_and_binary_resources() {
        with_dir("full-package", |dir| {
            create("multi").unwrap();
            package::save_file("multi", "references/引用规范.md", "# 引用规范\n保留出处。")
                .unwrap();
            package::save_file("multi", "scripts/check.py", "print('不执行')").unwrap();
            let root = package::folder("multi").unwrap();
            std::fs::create_dir_all(root.join("assets")).unwrap();
            std::fs::write(root.join("assets/example.bin"), [0, 255, 1, 128]).unwrap();
            let original = package::load("multi").unwrap().files;
            let zip = export("multi", &dir.join("full.skill")).unwrap();
            let folder = export("multi", &dir.join("out")).unwrap();
            assert!(
                folder
                    .parent()
                    .unwrap()
                    .join("references/引用规范.md")
                    .is_file()
            );
            remove("multi").unwrap();
            assert!(!root.exists(), "删除的是整个包");
            assert_eq!(import(&zip).unwrap().0, ["multi"]);
            assert_eq!(package::load("multi").unwrap().files, original);
            remove("multi").unwrap();
            import(folder.parent().unwrap()).unwrap();
            assert_eq!(package::load("multi").unwrap().files, original);
        });
    }

    #[test]
    fn unsafe_archive_paths_are_rejected_before_any_write() {
        use std::io::Write;
        with_dir("unsafe-package", |dir| {
            for (index, path) in [
                "mine/../escape.txt",
                "C:/escape.txt",
                "mine/references/CON.txt",
            ]
            .iter()
            .enumerate()
            {
                let zip_path = dir.join(format!("unsafe-{index}.skill"));
                let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
                zip.start_file("mine/SKILL.md", zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(TEMPLATE.as_bytes()).unwrap();
                zip.start_file(*path, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(b"invalid").unwrap();
                zip.finish().unwrap();
                assert!(import(&zip_path).is_err());
                assert!(!has_user_file("mine"));
            }
        });
    }

    #[test]
    fn invalid_entry_does_not_install_auxiliary_files() {
        with_dir("bad-entry", |dir| {
            let root = dir.join("bad");
            std::fs::create_dir_all(root.join("references")).unwrap();
            std::fs::write(root.join("SKILL.md"), "无 YAML 入口").unwrap();
            std::fs::write(root.join("references/材料.md"), "材料").unwrap();
            let (ids, notes) = import(&root).unwrap();
            assert!(ids.is_empty());
            assert!(!notes.is_empty());
            assert!(!package::folder("bad").unwrap().exists());
        });
    }
}
