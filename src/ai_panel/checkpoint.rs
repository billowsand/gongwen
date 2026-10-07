//! 技能运行检查点的落盘与找回（`docs/agent-kernel-hardening.md` 第六节，内核加固第 4 期）。
//!
//! 引擎在后台线程跑，每步成功之后落一份检查点（`agent::checkpoint`）；这里把检查点写进
//! 稿件库的 `ai_run_checkpoints` 表（存取与保留策略在 `manuscript::ai_checkpoints`）。
//! 库路径在界面线程解析好再传进后台线程（测试替换的配置目录只对当前线程有效）；
//! 后台线程自己开一个连接（同库，WAL），每次保存是单行、短事务。
//! 稿件没入库时不存检查点（用 `NoCheckpoint`），卡片行为保持现状。

use crate::agent::checkpoint::{Checkpoint, CheckpointSink};
use crate::manuscript::ManuscriptStore;
use std::cell::RefCell;
use std::path::PathBuf;

/// 写进稿件库的检查点去处。一轮一个：会话 id、轮次与技能在开跑时定下。
pub(crate) struct SqliteCheckpoints {
    store: RefCell<ManuscriptStore>,
    session_id: String,
    turn_id: i64,
    skill_id: String,
    skill_hash: String,
    use_rag: bool,
}

impl SqliteCheckpoints {
    pub(crate) fn open(
        path: PathBuf,
        session_id: String,
        turn_id: i64,
        skill_id: String,
        skill_hash: String,
        use_rag: bool,
    ) -> Result<Self, String> {
        let store = ManuscriptStore::open(&path).map_err(|e| format!("打开稿件库失败：{e:#}"))?;
        Ok(Self {
            store: RefCell::new(store),
            session_id,
            turn_id,
            skill_id,
            skill_hash,
            use_rag,
        })
    }
}

impl CheckpointSink for SqliteCheckpoints {
    fn save(&self, ckpt: &Checkpoint) -> Result<(), String> {
        self.store
            .borrow_mut()
            .save_run_checkpoint(
                &self.session_id,
                self.turn_id,
                &self.skill_id,
                &self.skill_hash,
                self.use_rag,
                ckpt,
            )
            .map_err(|e| format!("{e:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::board::Board;
    use crate::agent::checkpoint::{Checkpoint, NoCheckpoint, Reason};
    use crate::manuscript::NewManuscript;
    use crate::manuscript::ai_sessions::AiSessionRecord;

    fn ckpt(at: Vec<usize>) -> Checkpoint {
        Checkpoint {
            at,
            reason: Reason::Step,
            label: "已完成 工具 note".into(),
            board: Board {
                workspace: "工作稿".into(),
                ..Board::default()
            },
            partial: false,
        }
    }

    fn session(store: &mut ManuscriptStore) -> (String, i64) {
        let manuscript = store.create(&NewManuscript::default(), None).unwrap();
        let record = AiSessionRecord {
            id: "s1".into(),
            is_current: true,
            created_at: String::new(),
            updated_at: String::new(),
            ..AiSessionRecord::default()
        };
        store
            .save_ai_session(manuscript, &record, &[(7, "{}".into())], &[7])
            .unwrap();
        ("s1".into(), manuscript)
    }

    /// 真实 SQLite（临时目录）：后台线程的 sink 写进去，界面线程读得回来。
    #[test]
    fn a_saved_checkpoint_comes_back_through_another_connection() {
        let dir = std::env::temp_dir().join(format!("gongwen-ckpt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manuscripts.db");
        let _ = std::fs::remove_file(&path);
        let (session_id, manuscript) = {
            let mut store = ManuscriptStore::open(&path).unwrap();
            let (session_id, manuscript) = session(&mut store);
            let sink = SqliteCheckpoints::open(
                path.clone(),
                session_id.clone(),
                7,
                "polish".into(),
                "hash".into(),
                false,
            )
            .unwrap();
            sink.save(&ckpt(vec![1])).unwrap();
            sink.save(&ckpt(vec![2])).unwrap();
            (session_id, manuscript)
        };
        // 另一个连接（同库，WAL）读回：界面线程的稿件库连接。
        let store = ManuscriptStore::open(&path).unwrap();
        let latest = store
            .latest_run_checkpoint(&session_id, 7)
            .unwrap()
            .unwrap();
        assert_eq!(latest.checkpoint.at, [2]);
        assert_eq!(latest.checkpoint.board.workspace, "工作稿");
        assert_eq!(latest.label, "已完成 工具 note");
        assert_eq!(store.list_run_checkpoints(&session_id, 7).unwrap().len(), 2);
        assert!(
            store
                .list_ai_sessions(manuscript)
                .unwrap()
                .iter()
                .any(|s| s.id == session_id)
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 稿件没入库时用 `NoCheckpoint`：不报错，也什么都没写。
    #[test]
    fn the_no_op_sink_never_fails_and_writes_nothing() {
        NoCheckpoint.save(&ckpt(vec![1])).unwrap();
        NoCheckpoint.save(&ckpt(vec![2])).unwrap();
    }
}
