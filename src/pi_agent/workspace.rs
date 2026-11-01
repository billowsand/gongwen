//! 每次 Pi 运行一个持久工作区。
//!
//! ```text
//! <配置目录>/pi/workspaces/doc-<key>/job-<seq>-<时间>/
//!   workspace/
//!     input/current.md     本轮正文快照（只读）
//!     input/request.md     本轮要求（只读）
//!     skills/gongwen-markdown/  随包技能（只读）
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
    pub(crate) fn prepare(
        base: &Path,
        doc_key: u64,
        seq: u64,
        current: &str,
        request: &str,
    ) -> Result<Self, String> {
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
        write_text(&root.join("input").join("request.md"), request)?;
        crate::skill_pack::write_directory(&root.join("skills"))
            .map_err(|error| format!("写不出随包技能：{error:#}"))?;
        let state_dir = run_dir.join("pi");
        std::fs::create_dir_all(&state_dir)
            .map_err(|error| format!("建不了 Pi 状态目录（{}）：{error}", state_dir.display()))?;
        let label = format!("pi/workspaces/doc-{doc_key}/{name}");
        Ok(Self {
            root,
            state: state_dir.join("session.sqlite"),
            label,
        })
    }
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    crate::storage::atomic_write(path, |file| {
        file.write_all(text.as_bytes())
            .context("写工作区输入文件失败")
    })
    .map_err(|error| format!("写工作区输入文件失败（{}）：{error:#}", path.display()))
}
