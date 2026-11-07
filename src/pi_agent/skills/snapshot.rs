//! 每轮 Pi 任务的技能快照。
//!
//! 任务准备时把**本轮已启用**的技能包（含 references / assets / scripts）复制到
//! `<工作区>/skills/pi-skills/<name>/`，并写 `input/skills.json` 固定名字、说明、
//! 文件哈希。索引与实际复制的文件来自同一份内存包，避免“列一版、复制另一版”。
//! 之后改全局技能不影响已开始的任务；恢复时仍读原工作区。

use super::parser;
use super::{Catalog, store};
use crate::ai_services::skill_package::{Limits, Package, destination_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// 工作区里的技能目录（相对工作区根）。
pub(crate) const SKILLS_DIR: &str = "skills/pi-skills";
/// 本轮技能索引（相对工作区根）。host 与模型都以这一份为准。
pub(crate) const INDEX_FILE: &str = "input/skills.json";
const INDEX_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillFile {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillIndexEntry {
    pub(crate) name: String,
    pub(crate) display_name: String,
    pub(crate) description: String,
    /// 工作区相对路径，固定为 `skills/pi-skills/<name>/SKILL.md` 的父目录。
    pub(crate) path: String,
    pub(crate) files: Vec<SkillFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SkillIndex {
    pub(crate) protocol: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) selected: Option<String>,
    pub(crate) skills: Vec<SkillIndexEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct SkillSnapshot {
    index: SkillIndex,
    /// 与 `index.skills` 一一对应，写入磁盘的字节来自这里。
    packages: Vec<(String, Package)>,
}

impl SkillSnapshot {
    /// 从当前目录生成本轮快照。选中技能必须在这份快照里且可执行。
    pub(crate) fn build(catalog: &Catalog, selected: Option<&str>) -> Result<Self, String> {
        Self::build_with(catalog, selected, store::load_package)
    }

    /// 合成校准只读取编入程序的技能，不读用户目录或停用清单。
    pub(crate) fn build_builtin(selected: Option<&str>) -> Result<Self, String> {
        Self::build_with(
            &super::catalog::builtins(),
            selected,
            store::builtin_package,
        )
    }

    fn build_with(
        catalog: &Catalog,
        selected: Option<&str>,
        load_package: impl Fn(&str) -> Result<Package, String>,
    ) -> Result<Self, String> {
        let selected = catalog.resolve(selected)?;
        let enabled: Vec<_> = catalog
            .enabled()
            .into_iter()
            .filter(|entry| {
                if selected.as_deref() == Some(super::STYLE_SKILL) {
                    entry.name == super::STYLE_SKILL
                } else {
                    entry.name != super::STYLE_SKILL
                }
            })
            .collect();
        if enabled.is_empty() {
            return Err(
                "现在没有启用的 Pi 技能，Pi 不会启动。请到「AI 管理 → 技能」里启用至少一个。"
                    .into(),
            );
        }
        let mut skills = Vec::new();
        let mut packages = Vec::new();
        let mut total_bytes = 0usize;
        for entry in enabled {
            let package = load_package(&entry.name)?;
            total_bytes =
                total_bytes.saturating_add(package.files.values().map(Vec::len).sum::<usize>());
            if total_bytes > 128 * 1024 * 1024 {
                return Err("本轮启用的技能包合计超过 128 MB，请停用不需要的技能后重试。".into());
            }
            let Some(bytes) = package.files.get("SKILL.md") else {
                return Err(format!(
                    "技能「{}」缺了 SKILL.md，不能开始这一轮。",
                    entry.name
                ));
            };
            let text = std::str::from_utf8(bytes)
                .map_err(|_| format!("技能「{}」的 SKILL.md 不是 UTF-8 文本。", entry.name))?;
            let parsed = parser::parse(&entry.name, text)?;
            let files = package
                .files
                .iter()
                .map(|(path, bytes)| SkillFile {
                    path: path.clone(),
                    bytes: bytes.len() as u64,
                    sha256: hash_bytes(bytes),
                })
                .collect::<Vec<_>>();
            skills.push(SkillIndexEntry {
                name: entry.name.clone(),
                display_name: parsed
                    .display_name
                    .clone()
                    .unwrap_or_else(|| entry.label.clone()),
                description: parsed.description,
                path: format!("{SKILLS_DIR}/{}", entry.name),
                files,
            });
            packages.push((entry.name.clone(), package));
        }
        let snapshot = Self {
            index: SkillIndex {
                protocol: INDEX_VERSION,
                selected,
                skills,
            },
            packages,
        };
        if snapshot.index_json().len() > 512 * 1024 {
            return Err("本轮技能索引超过 512 KB，请停用不需要的技能后重试。".into());
        }
        Ok(snapshot)
    }

    pub(crate) fn selected(&self) -> Option<&str> {
        self.index.selected.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn entries(&self) -> &[SkillIndexEntry] {
        &self.index.skills
    }

    /// `input/request.md` 里的技能索引。与 `input/skills.json` 是同一份快照。
    pub(crate) fn request_block(&self) -> String {
        let mut lines = vec![
            "## 技能".to_string(),
            String::new(),
            "本轮可用技能（只读快照，位于 skills/pi-skills/<目录>/SKILL.md）。它们是自然语言步骤，不是流程脚本。".to_string(),
            "本轮技能、说明与文件哈希另存为 input/skills.json，调试时以它为准。".to_string(),
        ];
        if let Some(selected) = self.selected() {
            lines.push(format!("本轮指定技能：{selected}。"));
        }
        for entry in &self.index.skills {
            lines.push(format!(
                "- {}（{}）：{} → {}/SKILL.md",
                entry.name, entry.display_name, entry.description, entry.path
            ));
        }
        lines.join("\n")
    }

    /// 写到工作区：技能包与 `input/skills.json`。
    pub(crate) fn write_to(&self, workspace: &Path) -> Result<(), String> {
        if crate::vault::key_handle().is_some() {
            let root = workspace.join(SKILLS_DIR);
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let mut tree = super::super::sealed_tree::Tree::new();
            for (name, package) in &self.packages {
                for (relative, bytes) in &package.files {
                    tree.insert(
                        format!("{name}/{relative}"),
                        super::super::sealed_tree::encode(bytes),
                    );
                }
            }
            super::super::sealed_tree::write(&workspace.join("skill-files.json"), &tree)
                .map_err(|e| e.to_string())?;
            return crate::vault::write_path(
                &workspace.join(INDEX_FILE),
                "pi",
                self.index_json().as_bytes(),
            )
            .map_err(|e| e.to_string());
        }
        let root = workspace.join(SKILLS_DIR);
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("建不了工作区技能目录（{}）：{error}", root.display()))?;
        for (name, package) in &self.packages {
            let skill_root = root.join(name);
            std::fs::create_dir_all(&skill_root).map_err(|error| {
                format!("建不了工作区技能目录（{}）：{error}", skill_root.display())
            })?;
            for (relative, bytes) in &package.files {
                let path = destination_with(&skill_root, relative, Limits::PI)
                    .map_err(|error| format!("{error:#}"))?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| {
                        format!("建不了工作区技能目录（{}）：{error}", parent.display())
                    })?;
                }
                crate::vault::write_path(&path, "pi", bytes).map_err(|error| {
                    format!("写不出工作区技能文件（{}）：{error:#}", path.display())
                })?;
            }
        }
        let index = workspace.join(INDEX_FILE);
        crate::vault::write_path(&index, "pi", self.index_json().as_bytes())
            .map_err(|error| format!("写不出本轮技能索引（{}）：{error:#}", index.display()))
    }

    pub(crate) fn index_json(&self) -> String {
        serde_json::to_string_pretty(&self.index).unwrap_or_else(|error| {
            format!("{{\"protocol\":{INDEX_VERSION},\"error\":\"{error}\"}}")
        })
    }
}

/// 内容身份不含本轮选择；相同技能内容的自动/显式选择共用校准。
pub(crate) fn content_digest(workspace: &Path) -> Result<String, String> {
    let bytes = super::super::recovery::read_bounded(&workspace.join(INDEX_FILE), 512 * 1024)?;
    let index: SkillIndex =
        serde_json::from_slice(&bytes).map_err(|error| format!("技能快照索引无效：{error}"))?;
    let bytes = serde_json::to_vec(&(index.protocol, index.skills, crate::skill_pack::FILES))
        .map_err(|error| format!("无法核对技能内容：{error}"))?;
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `input/skills.json` 不存在或读不出来时，恢复旧工作区仍要能跑：从磁盘上的
/// 技能目录重建一份最小索引（只取目录名与 frontmatter 说明）。
#[cfg(test)]
pub(crate) fn read_index_from_workspace(workspace: &Path) -> Vec<SkillIndexEntry> {
    let path = workspace.join(INDEX_FILE);
    if let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(index) = serde_json::from_str::<SkillIndex>(&text)
    {
        return index.skills;
    }
    let root = workspace.join(SKILLS_DIR);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !parser::valid_name(name) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path.join("SKILL.md")) else {
            continue;
        };
        let Ok(parsed) = parser::parse(name, &text) else {
            continue;
        };
        skills.push(SkillIndexEntry {
            name: name.to_string(),
            display_name: parsed.display_name.unwrap_or_else(|| name.to_string()),
            description: parsed.description,
            path: format!("{SKILLS_DIR}/{name}"),
            files: Vec::new(),
        });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ConfigDir {
        dir: tempfile::TempDir,
    }
    impl ConfigDir {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            crate::storage::set_test_config_dir(Some(dir.path().to_path_buf()));
            Self { dir }
        }
    }
    impl Drop for ConfigDir {
        fn drop(&mut self) {
            crate::storage::set_test_config_dir(None);
            let _ = &self.dir;
        }
    }

    #[test]
    fn calibration_builtin_snapshot_ignores_user_overrides_and_selection_is_not_content_identity() {
        let _config = ConfigDir::new();
        store::copy_builtin("gongwen-draft").unwrap();
        store::save_text(
            "gongwen-draft",
            "references/private.md",
            "不得送入合成校准的用户私有内容",
        )
        .unwrap();
        store::set_enabled("gongwen-draft", false).unwrap();
        let selected = SkillSnapshot::build_builtin(Some("gongwen-draft")).unwrap();
        assert!(
            !selected
                .packages
                .iter()
                .any(|(_, package)| package.files.contains_key("references/private.md"))
        );
        let automatic = SkillSnapshot::build_builtin(None).unwrap();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        selected.write_to(a.path()).unwrap();
        automatic.write_to(b.path()).unwrap();
        assert_eq!(
            content_digest(a.path()).unwrap(),
            content_digest(b.path()).unwrap()
        );
        let mut changed = automatic.clone();
        changed.index.skills[0].files[0].sha256 = "different-content".into();
        changed.write_to(b.path()).unwrap();
        assert_ne!(
            content_digest(a.path()).unwrap(),
            content_digest(b.path()).unwrap()
        );
    }

    #[test]
    fn snapshot_copies_only_enabled_skills_and_hashes_the_same_files() {
        let _config = ConfigDir::new();
        store::create("custom-skill", Some("自定义技能")).unwrap();
        store::save_text("custom-skill", "references/依据.md", "辅助文件").unwrap();
        store::save_text("custom-skill", "scripts/run.sh", "echo 不执行").unwrap();
        store::set_enabled("gongwen-imitate", false).unwrap();
        let snapshot = SkillSnapshot::build(
            &crate::pi_agent::pi_skills::load_catalog(),
            Some("custom-skill"),
        )
        .unwrap();
        assert_eq!(snapshot.selected(), Some("custom-skill"));
        assert!(
            snapshot
                .entries()
                .iter()
                .all(|entry| entry.name != "gongwen-imitate"),
            "停用的技能不进快照"
        );
        let custom = snapshot
            .entries()
            .iter()
            .find(|entry| entry.name == "custom-skill")
            .unwrap();
        assert_eq!(custom.display_name, "自定义技能");
        assert!(
            custom
                .files
                .iter()
                .any(|file| file.path == "references/依据.md")
        );
        assert!(
            custom
                .files
                .iter()
                .any(|file| file.path == "scripts/run.sh")
        );
        let dir = tempfile::tempdir().unwrap();
        snapshot.write_to(dir.path()).unwrap();
        let text = std::fs::read_to_string(
            dir.path()
                .join(SKILLS_DIR)
                .join("custom-skill/references/依据.md"),
        )
        .unwrap();
        assert_eq!(text, "辅助文件");
        let index = read_index_from_workspace(dir.path());
        assert_eq!(index.len(), snapshot.entries().len());
        assert_eq!(index[0].name, snapshot.entries()[0].name);
        let request = snapshot.request_block();
        assert!(request.contains("custom-skill"), "{request}");
        assert!(!request.contains("gongwen-imitate"), "{request}");
    }

    #[test]
    fn builtin_skill_references_reach_the_workspace_snapshot() {
        let _config = ConfigDir::new();
        let snapshot = SkillSnapshot::build(
            &crate::pi_agent::pi_skills::load_catalog(),
            Some("gongwen-revise"),
        )
        .unwrap();
        let entry = snapshot
            .entries()
            .iter()
            .find(|entry| entry.name == "gongwen-revise")
            .unwrap();
        assert!(
            entry
                .files
                .iter()
                .any(|file| file.path == "references/condense.md"),
            "{:?}",
            entry
                .files
                .iter()
                .map(|file| &file.path)
                .collect::<Vec<_>>()
        );
        let dir = tempfile::tempdir().unwrap();
        snapshot.write_to(dir.path()).unwrap();
        let text = std::fs::read_to_string(
            dir.path()
                .join(SKILLS_DIR)
                .join("gongwen-revise/references/condense.md"),
        )
        .unwrap();
        assert!(text.contains("责任单位"), "{text}");
        // 索引里的文件哈希与实际写的文件一致。
        let index = std::fs::read_to_string(dir.path().join(INDEX_FILE)).unwrap();
        let parsed: SkillIndex = serde_json::from_str(&index).unwrap();
        let revise = parsed
            .skills
            .iter()
            .find(|skill| skill.name == "gongwen-revise")
            .unwrap();
        let condense = revise
            .files
            .iter()
            .find(|file| file.path == "references/condense.md")
            .unwrap();
        assert_eq!(condense.sha256, hash_bytes(text.as_bytes()));
        assert_eq!(condense.bytes, text.len() as u64);
    }

    #[test]
    fn selected_disabled_or_missing_skill_fails_instead_of_falling_back_to_auto() {
        let _config = ConfigDir::new();
        store::set_enabled("gongwen-revise", false).unwrap();
        let catalog = crate::pi_agent::pi_skills::load_catalog();
        let error = SkillSnapshot::build(&catalog, Some("gongwen-revise")).unwrap_err();
        assert!(error.contains("已停用"), "{error}");
        let error = SkillSnapshot::build(&catalog, Some("deleted-skill")).unwrap_err();
        assert!(error.contains("删除"), "{error}");
        SkillSnapshot::build(&catalog, None)
            .unwrap_or_else(|error| panic!("自动快照失败：{error}"));
        for name in crate::pi_agent::pi_skills::BUILTINS {
            store::set_enabled(name, false).unwrap();
        }
        let error =
            SkillSnapshot::build(&crate::pi_agent::pi_skills::load_catalog(), None).unwrap_err();
        assert!(error.contains("没有启用"), "{error}");
    }

    #[test]
    fn a_snapshot_is_frozen_after_the_global_skill_changes() {
        let _config = ConfigDir::new();
        store::create("frozen", None).unwrap();
        let snapshot =
            SkillSnapshot::build(&crate::pi_agent::pi_skills::load_catalog(), Some("frozen"))
                .unwrap();
        let dir = tempfile::tempdir().unwrap();
        snapshot.write_to(dir.path()).unwrap();
        let before =
            std::fs::read_to_string(dir.path().join(SKILLS_DIR).join("frozen/SKILL.md")).unwrap();
        let index_before = read_index_from_workspace(dir.path());
        // 全局技能改掉、停用：工作区快照不变。
        store::save_text(
            "frozen",
            "SKILL.md",
            "---\nname: frozen\ndescription: 新版本\n---\n新版正文",
        )
        .unwrap();
        store::set_enabled("frozen", false).unwrap();
        let after =
            std::fs::read_to_string(dir.path().join(SKILLS_DIR).join("frozen/SKILL.md")).unwrap();
        assert_eq!(before, after);
        assert_eq!(index_before, read_index_from_workspace(dir.path()));
        assert_eq!(
            index_before
                .iter()
                .find(|entry| entry.name == "frozen")
                .unwrap()
                .description,
            "一句话写清这个技能做什么、什么时候用。"
        );
    }

    #[test]
    fn legacy_workspace_without_skills_json_is_rebuilt_from_its_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(SKILLS_DIR).join("gongwen-draft");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("SKILL.md"),
            "---\nname: gongwen-draft\ndescription: 旧工作区\n---\n正文",
        )
        .unwrap();
        let index = read_index_from_workspace(dir.path());
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].name, "gongwen-draft");
        assert_eq!(index[0].description, "旧工作区");
        assert!(index[0].files.is_empty());
    }
}
