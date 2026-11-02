//! 每次 Pi 运行一个持久工作区。
//!
//! ```text
//! <配置目录>/pi/workspaces/doc-<key>/run-<时间>-<UUID>/
//!   workspace/
//!     input/current.md     本轮正文快照（只读）
//!     input/request.md     本轮要求 + 技能索引 + 材料索引（只读）
//!     input/materials/     材料快照与 index.json（只读）
//!     skills/gongwen-markdown/  随包语法资料（只读）
//!     skills/pi-skills/    本轮启用的标准写作技能完整快照（只读）
//!     work/                模型的工作文件
//!     resources/           预留只读资源
//!   pi/session.sqlite      Durable 数据库，位于模型可读写区域之外
//! ```
//!
//! 工作区不删除：中断、停止以后仍能查看本次文件。数据库也不放进 `workspace/`，
//! 模型没有路径能读到它。

use anyhow::Context;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::materials::{self, PiMaterialInput, PiMaterialMeta};
use super::skills::{Catalog, SkillSnapshot};

#[derive(Debug, Clone)]
pub(crate) struct PiWorkspace {
    /// 模型可见的工作区根目录。
    pub(crate) root: PathBuf,
    /// Durable 的 SQLite 文件，在 `workspace/` 之外。
    pub(crate) state: PathBuf,
    /// 运行目录（`workspace/` 的上一级）：运行记录、回执、成品快照都在这里。
    pub(crate) run: PathBuf,
    /// 给人看的相对位置（不含用户主目录）。
    pub(crate) label: String,
}

/// 工作区根：`配置目录/pi/workspaces`，不存在就建。
pub(crate) fn base_dir() -> Result<PathBuf, String> {
    let dir = crate::storage::config_dir()
        .map_err(|error| format!("读不到配置目录：{error:#}"))?
        .join("pi")
        .join("workspaces");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("建不了 Pi 工作区目录（{}）：{error}", dir.display()))?;
    Ok(dir)
}

impl PiWorkspace {
    /// 未采纳候选稿仅供继续修改，不覆盖正式快照，也不登记为事实来源。
    pub(crate) fn write_candidate(&self, text: &str) -> Result<(), String> {
        write_text(&self.root.join("input/candidate.md"), text)?;
        let request_path = self.root.join("input/request.md");
        let mut request = std::fs::read_to_string(&request_path)
            .map_err(|error| format!("读取本轮要求失败：{error}"))?;
        request.push_str("\n\n## 继续修改未采纳稿\n请读取 input/candidate.md，以此作为本轮修改基准。它不是事实来源；事实仍以 input/current.md、用户要求和事实材料为准。只改选区时，以正式快照中的选区及其外部原文为准。\n");
        write_text(&request_path, &request)
    }

    pub(crate) fn prepare(
        base: &Path,
        doc_key: u64,
        current: &str,
        request: &str,
        materials: &[PiMaterialInput],
        catalog: &Catalog,
        selected: Option<&str>,
    ) -> Result<(Self, Vec<PiMaterialMeta>), String> {
        let doc_dir = base.join(format!("doc-{doc_key}"));
        std::fs::create_dir_all(&doc_dir)
            .map_err(|error| format!("建不了工作区子目录（{}）：{error}", doc_dir.display()))?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        // 目录名带 UUID：两个窗口的 doc_key / seq 可能相同，不能靠重名探测区分。
        let name = format!("run-{stamp}-{}", short_uuid());
        let run_dir = doc_dir.join(&name);
        let root = run_dir.join("workspace");
        for sub in ["input", "work", "resources"] {
            std::fs::create_dir_all(root.join(sub)).map_err(|error| {
                format!("建不了工作区目录（{}）：{error}", root.join(sub).display())
            })?;
        }
        write_text(&root.join("input").join("current.md"), current)?;
        // 先按同一份内存快照落盘技能与索引，再用同一份快照写要求文件。
        let skills = SkillSnapshot::build(catalog, selected)?;
        skills.write_to(&root)?;
        let mut request_file = vec![request.trim().to_string()];
        request_file.push(String::new());
        request_file.push(skills.request_block());
        request_file.push(String::new());
        request_file.push("## 材料".into());
        let metas = materials::write_all(&root, materials)?;
        request_file.push(materials::prompt_block(&metas));
        write_text(
            &root.join("input").join("request.md"),
            &request_file.join("\n"),
        )?;
        crate::skill_pack::write_directory(&root.join("skills"))
            .map_err(|error| format!("写不出随包技能：{error:#}"))?;
        let state_dir = run_dir.join("pi");
        std::fs::create_dir_all(&state_dir)
            .map_err(|error| format!("建不了 Pi 状态目录（{}）：{error}", state_dir.display()))?;
        let label = format!("pi/workspaces/doc-{doc_key}/{name}");
        Ok((
            Self {
                root,
                state: state_dir.join("session.sqlite"),
                run: run_dir,
                label,
            },
            metas,
        ))
    }

    /// 恢复时重新打开原工作区：路径由 `recovery::resolve_workspace` 校验过。
    pub(crate) fn reopen(root: PathBuf, label: String) -> Result<Self, String> {
        let run = root.parent().ok_or("Pi 工作区没有运行目录")?.to_path_buf();
        let state = run.join("pi").join("session.sqlite");
        Ok(Self {
            root,
            state,
            run,
            label,
        })
    }

    /// 工作区里的材料元数据（恢复时按 `index.json` 读回）。
    pub(crate) fn read_materials(&self) -> Result<Vec<PiMaterialMeta>, String> {
        let path = self.root.join("input/materials/index.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = super::recovery::read_bounded(&path, 256 * 1024)?;
        let metas: Vec<PiMaterialMeta> = serde_json::from_slice(&bytes)
            .map_err(|error| format!("Pi 材料索引读不出来：{error}"))?;
        Ok(metas
            .into_iter()
            .take(super::materials::MAX_MATERIALS)
            .collect())
    }

    /// 模型实际读到的要求文件（要求原文 + 技能索引 + 材料索引）。
    pub(crate) fn read_request(&self) -> Result<String, String> {
        let bytes = super::recovery::read_bounded(
            &self.root.join("input/request.md"),
            super::recovery::MAX_SNAPSHOT_BYTES,
        )?;
        String::from_utf8(bytes).map_err(|_| "工作区里的要求文件不是 UTF-8".to_string())
    }

    /// 按元数据读出材料全文；恢复新一轮工作区时用，读不到就明确失败。
    pub(crate) fn read_material_texts(&self) -> Result<Vec<PiMaterialInput>, String> {
        let mut inputs = Vec::new();
        for meta in self.read_materials()? {
            let path = self.root.join(&meta.file);
            let bytes =
                super::recovery::read_bounded(&path, super::materials::MAX_MATERIAL_BYTES as u64)?;
            let text = String::from_utf8(bytes)
                .map_err(|_| format!("材料《{}》不是合法 UTF-8。", meta.title))?;
            inputs.push(PiMaterialInput {
                title: meta.title,
                source: meta.source,
                role: meta.role,
                text,
            });
        }
        Ok(inputs)
    }
}

/// 目录名里用的短 UUID（只要唯一，不做密码学用途）。
fn short_uuid() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect()
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    crate::storage::atomic_write(path, |file| {
        file.write_all(text.as_bytes())
            .context("写工作区输入文件失败")
    })
    .map_err(|error| format!("写工作区输入文件失败（{}）：{error:#}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_is_delivered_separately_from_the_formal_snapshot() {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        let dir = tempfile::tempdir().unwrap();
        let catalog = super::super::skills::load_catalog();
        let (workspace, _) =
            PiWorkspace::prepare(dir.path(), 1, "正式正文", "继续改", &[], &catalog, None).unwrap();
        workspace.write_candidate("尚未采纳的修改").unwrap();
        assert_eq!(
            std::fs::read_to_string(workspace.root.join("input/current.md")).unwrap(),
            "正式正文"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.root.join("input/candidate.md")).unwrap(),
            "尚未采纳的修改"
        );
        let request = std::fs::read_to_string(workspace.root.join("input/request.md")).unwrap();
        assert!(request.contains("不是事实来源"));
    }

    #[test]
    fn a_new_task_freezes_the_enabled_skill_catalog_into_its_workspace() {
        let config = tempfile::tempdir().unwrap();
        crate::storage::set_test_config_dir(Some(config.path().to_path_buf()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::storage::set_test_config_dir(None);
            }
        }
        let _reset = Reset;
        use super::super::skills::store as skills;
        skills::create("custom-skill", Some("自定义技能")).unwrap();
        skills::save_text("custom-skill", "references/依据.md", "辅助依据").unwrap();
        skills::set_enabled("gongwen-draft", false).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let catalog = super::super::skills::load_catalog();
        let (workspace, metas) = PiWorkspace::prepare(
            dir.path(),
            1,
            "正式正文",
            "按自定义技能写",
            &[],
            &catalog,
            Some("custom-skill"),
        )
        .unwrap();
        let request = workspace.read_request().unwrap();
        assert!(request.contains("custom-skill"), "{request}");
        assert!(request.contains("自定义技能"), "{request}");
        assert!(
            !request.contains("gongwen-draft"),
            "停用技能不进本轮：{request}"
        );
        assert_eq!(
            std::fs::read_to_string(
                workspace
                    .root
                    .join("skills/pi-skills/custom-skill/references/依据.md")
            )
            .unwrap(),
            "辅助依据"
        );
        let index = std::fs::read_to_string(workspace.root.join("input/skills.json")).unwrap();
        assert!(index.contains("custom-skill"), "{index}");
        assert!(index.contains("sha256"), "{index}");
        assert!(!index.contains("gongwen-draft"), "{index}");
        // 全局技能改动不会改已经准备好的工作区。
        skills::save_text(
            "custom-skill",
            "SKILL.md",
            "---\nname: custom-skill\ndescription: 新版本\n---\n新版正文",
        )
        .unwrap();
        skills::set_enabled("custom-skill", false).unwrap();
        let frozen = std::fs::read_to_string(
            workspace
                .root
                .join("skills/pi-skills/custom-skill/SKILL.md"),
        )
        .unwrap();
        assert!(frozen.contains("一句话写清"), "{frozen}");
        std::fs::write(&workspace.state, b"sqlite").unwrap();
        let record = super::super::PiRunRecord::start(
            "run-frozen".into(),
            "request-frozen".into(),
            &workspace,
            "正式正文",
            "正式正文",
            "正式正文",
            &workspace.read_request().unwrap(),
            None,
            "按自定义技能写",
            "通知",
            "2026-11-02",
            Some("custom-skill"),
            None,
            metas,
            "测试服务商",
            "测试模型",
        );
        record.write(&workspace).unwrap();
        // 恢复只认原快照，即使全局技能已经停用。
        super::super::recovery::check_workspace_files(&workspace, &record).unwrap();
    }
}
