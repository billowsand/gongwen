//! 每次 Pi 运行一个持久工作区。
//!
//! ```text
//! <配置目录>/pi/workspaces/doc-<key>/job-<seq>-<时间>/
//!   workspace/
//!     input/current.md     本轮正文快照（只读）
//!     input/request.md     本轮要求 + 技能索引 + 材料索引（只读）
//!     input/materials/     材料快照与 index.json（只读）
//!     skills/gongwen-markdown/  随包语法资料（只读）
//!     skills/pi-skills/    5 份标准写作技能（只读）
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

#[derive(Debug, Clone)]
pub(crate) struct PiWorkspace {
    /// 模型可见的工作区根目录。
    pub(crate) root: PathBuf,
    /// Durable 的 SQLite 文件，在 `workspace/` 之外。
    pub(crate) state: PathBuf,
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
        seq: u64,
        current: &str,
        request: &str,
        materials: &[PiMaterialInput],
    ) -> Result<(Self, Vec<PiMaterialMeta>), String> {
        let doc_dir = base.join(format!("doc-{doc_key}"));
        std::fs::create_dir_all(&doc_dir)
            .map_err(|error| format!("建不了工作区子目录（{}）：{error}", doc_dir.display()))?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let mut name = format!("job-{seq}-{stamp}");
        let mut run_dir = doc_dir.join(&name);
        let mut attempt = 1u32;
        while run_dir.exists() {
            attempt = attempt.saturating_add(1);
            name = format!("job-{seq}-{stamp}-{attempt}");
            run_dir = doc_dir.join(&name);
        }
        let root = run_dir.join("workspace");
        for sub in ["input", "work", "resources"] {
            std::fs::create_dir_all(root.join(sub)).map_err(|error| {
                format!("建不了工作区目录（{}）：{error}", root.join(sub).display())
            })?;
        }
        write_text(&root.join("input").join("current.md"), current)?;
        let mut request_file = vec![request.trim().to_string()];
        request_file.push(String::new());
        request_file.push("## 技能".into());
        request_file.push(super::skills::index_text());
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
        super::skills::install(&root.join("skills"))?;
        let state_dir = run_dir.join("pi");
        std::fs::create_dir_all(&state_dir)
            .map_err(|error| format!("建不了 Pi 状态目录（{}）：{error}", state_dir.display()))?;
        let label = format!("pi/workspaces/doc-{doc_key}/{name}");
        Ok((
            Self {
                root,
                state: state_dir.join("session.sqlite"),
                label,
            },
            metas,
        ))
    }
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
        let dir = tempfile::tempdir().unwrap();
        let (workspace, _) =
            PiWorkspace::prepare(dir.path(), 1, 1, "正式正文", "继续改", &[]).unwrap();
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
}
