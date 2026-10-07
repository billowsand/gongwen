//! 技能运行检查点的存储（`docs/agent-kernel-hardening.md` 第六节，内核加固第 4 期）。
//!
//! 一步成功之后由后台线程落一份（整块黑板 + 下一步的位置），崩溃 / 停止 / 出错后从最近一份
//! 接着跑。检查点随会话走：会话是本机工作现场，不进版本快照、不进同步 ZIP；外键挂在
//! `ai_session_turns` 上，轮次被删（清空任务流）或会话被删时级联清除，不留孤儿行。
//!
//! 表结构（DDL 在 `ai_sessions.rs`，与会话同一份幂等建表）：
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS ai_run_checkpoints (
//!     session_id TEXT    NOT NULL,
//!     turn_id    INTEGER NOT NULL,
//!     seq        INTEGER NOT NULL,   -- 这一轮里的序号，从 1 起
//!     at_path    TEXT    NOT NULL,   -- "[2,1]"，给人看的
//!     reason     TEXT    NOT NULL,
//!     label      TEXT    NOT NULL DEFAULT '',
//!     skill_id   TEXT    NOT NULL DEFAULT '',   -- 崩溃恢复时按它找回技能
//!     skill_hash TEXT    NOT NULL DEFAULT '',
//!     use_rag    INTEGER NOT NULL DEFAULT 0,
//!     partial    INTEGER NOT NULL DEFAULT 0,    -- 超尺寸被截过证据包
//!     data       TEXT    NOT NULL,   -- Checkpoint 的 JSON
//!     created_at TEXT    NOT NULL,
//!     PRIMARY KEY (session_id, turn_id, seq),
//!     FOREIGN KEY (session_id, turn_id)
//!         REFERENCES ai_session_turns(session_id, turn_id) ON DELETE CASCADE
//! );
//! ```
//!
//! 保留策略：每轮留「序号最小的一个 + 最近 5 个」，其余从中间删——`Board` 含工作稿与
//! 证据包，单份可能到几十万字节。单份超过 [`MAX_BYTES`] 时证据包只留最近
//! [`EVIDENCE_KEEP`] 条并标 `partial`。

use super::ManuscriptStore;
use crate::agent::checkpoint::{Checkpoint, Reason, StepPath};
use anyhow::{Context, Result};
use rusqlite::params;

/// 单份检查点的尺寸上限（JSON 字节数）。宁可少存也不要把库撑爆。
const MAX_BYTES: usize = 512 * 1024;
/// 超尺寸时证据包只留最近这么多条。
const EVIDENCE_KEEP: usize = 20;
/// 每轮保留最近几份（外加序号最小的一份）。
const KEEP_RECENT: i64 = 5;

/// 从库里读回来的一份检查点。
#[derive(Debug, Clone)]
pub(crate) struct StoredCheckpoint {
    pub(crate) seq: i64,
    pub(crate) label: String,
    pub(crate) skill_id: String,
    /// 技能指纹与是否用知识库：恢复时按新版技能接着跑要用（第 4 期④）。
    pub(crate) skill_hash: String,
    pub(crate) use_rag: bool,
    /// `data` 解析出的现场。
    pub(crate) checkpoint: Checkpoint,
    /// 什么时候存的（卡片上「停在哪一步、什么时候存的」要用，第 4 期⑤）。
    pub(crate) created_at: String,
}

fn now() -> String {
    chrono::Local::now().to_rfc3339()
}

/// 卡片上给「从这里重跑」用的候选（一行一份，不带黑板——黑板只在真正点时才读回来）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckpointSummary {
    pub(crate) seq: i64,
    /// 停在哪一步，如「已完成 算子 gap_loop」。
    pub(crate) label: String,
    /// 什么时候存的（卡片上显示）。
    pub(crate) when: String,
    /// 证据包被截过（超尺寸上限），从这里重跑缺一部分证据。
    pub(crate) partial: bool,
}

impl StoredCheckpoint {
    /// 卡片上的一行（「从这里重跑」的候选）。
    pub(crate) fn summary(&self) -> CheckpointSummary {
        CheckpointSummary {
            seq: self.seq,
            label: if self.label.trim().is_empty() {
                format!(
                    "第 {} 步",
                    self.checkpoint.at.first().copied().unwrap_or(0) + 1
                )
            } else {
                self.label.clone()
            },
            when: self.created_at.clone(),
            partial: self.checkpoint.partial,
        }
    }
}

fn reason_str(reason: Reason) -> &'static str {
    match reason {
        Reason::Start => "start",
        Reason::Step => "step",
        Reason::Ask => "ask",
        Reason::Limit => "limit",
        Reason::Error => "error",
    }
}

/// 超尺寸就截证据包并标 `partial`；返回要存的 JSON。
fn serialize(ckpt: &Checkpoint) -> Result<(String, bool)> {
    let json = serde_json::to_string(ckpt).context("检查点序列化失败")?;
    if json.len() <= MAX_BYTES {
        return Ok((json, false));
    }
    let mut capped = ckpt.clone();
    capped.board.evidence.keep_last(EVIDENCE_KEEP);
    capped.partial = true;
    let json = serde_json::to_string(&capped).context("检查点序列化失败")?;
    Ok((json, true))
}

impl ManuscriptStore {
    /// 落一份检查点并顺手执行保留策略。单行、短事务——后台线程每步之后调一次，
    /// 与界面线程写会话（WAL + busy_timeout）并存。
    pub(crate) fn save_run_checkpoint(
        &mut self,
        session_id: &str,
        turn_id: i64,
        skill_id: &str,
        skill_hash: &str,
        use_rag: bool,
        ckpt: &Checkpoint,
    ) -> Result<()> {
        let (data, partial) = serialize(ckpt)?;
        let at_path = serde_json::to_string(&ckpt.at).context("检查点路径序列化失败")?;
        let tx = self.conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM ai_run_checkpoints
             WHERE session_id = ?1 AND turn_id = ?2",
            params![session_id, turn_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO ai_run_checkpoints
             (session_id, turn_id, seq, at_path, reason, label, skill_id, skill_hash,
              use_rag, partial, data, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                session_id,
                turn_id,
                seq,
                at_path,
                reason_str(ckpt.reason),
                ckpt.label,
                skill_id,
                skill_hash,
                use_rag as i64,
                partial as i64,
                data,
                now(),
            ],
        )?;
        // 保留「序号最小的一个 + 最近 5 个」，其余从中间删。
        tx.execute(
            "DELETE FROM ai_run_checkpoints
             WHERE session_id = ?1 AND turn_id = ?2
               AND seq <> (SELECT MIN(seq) FROM ai_run_checkpoints
                           WHERE session_id = ?1 AND turn_id = ?2)
               AND seq <= (SELECT COALESCE(MAX(seq), 0) FROM ai_run_checkpoints
                           WHERE session_id = ?1 AND turn_id = ?2) - ?3",
            params![session_id, turn_id, KEEP_RECENT],
        )?;
        tx.commit()?;
        Ok(())
    }
}

/// 读取侧：内核加固第 4 期④「接着跑」、⑤界面显示、⑥「从这里重跑」接入。
impl ManuscriptStore {
    fn read_run_checkpoint(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredCheckpoint> {
        let data: String = row.get(6)?;
        let at: Option<StepPath> = serde_json::from_str(&row.get::<_, String>(1)?).ok();
        let mut checkpoint: Checkpoint = serde_json::from_str(&data).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Text,
                format!("检查点 JSON 读不出来：{error}").into(),
            )
        })?;
        // 以列里的路径为准；列读不出来就用 data 里存的那份——不能拿序号顶替路径，
        // 序号是这一轮的第几份检查点，不是第几步，顶替了会从错的步骤接着跑。
        if let Some(at) = at {
            checkpoint.at = at;
        }
        Ok(StoredCheckpoint {
            seq: row.get(0)?,
            label: row.get(2)?,
            skill_id: row.get(3)?,
            skill_hash: row.get(4)?,
            use_rag: row.get::<_, i64>(5)? != 0,
            checkpoint,
            created_at: row.get(7)?,
        })
    }

    const CHECKPOINT_COLUMNS: &'static str =
        "seq, at_path, label, skill_id, skill_hash, use_rag, data, created_at";

    /// 这一轮最新的一份检查点（崩溃 / 停止 / 出错后「接着跑」用）。
    pub(crate) fn latest_run_checkpoint(
        &self,
        session_id: &str,
        turn_id: i64,
    ) -> Result<Option<StoredCheckpoint>> {
        let sql = format!(
            "SELECT {} FROM ai_run_checkpoints
             WHERE session_id = ?1 AND turn_id = ?2 ORDER BY seq DESC LIMIT 1",
            Self::CHECKPOINT_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![session_id, turn_id], Self::read_run_checkpoint)?;
        Ok(rows.next().transpose()?)
    }

    /// 这一轮存下的全部检查点，按序号（「从这里重跑」的候选列表用）。
    pub(crate) fn list_run_checkpoints(
        &self,
        session_id: &str,
        turn_id: i64,
    ) -> Result<Vec<StoredCheckpoint>> {
        let sql = format!(
            "SELECT {} FROM ai_run_checkpoints
             WHERE session_id = ?1 AND turn_id = ?2 ORDER BY seq",
            Self::CHECKPOINT_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![session_id, turn_id], Self::read_run_checkpoint)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 删掉这一轮的全部检查点（「丢弃」），返回删了几份。
    pub(crate) fn delete_run_checkpoints_from(
        &mut self,
        session_id: &str,
        turn_id: i64,
    ) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM ai_run_checkpoints WHERE session_id = ?1 AND turn_id = ?2",
                params![session_id, turn_id],
            )
            .map_err(Into::into)
    }

    /// 删掉 `seq` 之后的检查点（「从这里重跑」覆盖该点之后的产物）。
    pub(crate) fn delete_run_checkpoints_after(
        &mut self,
        session_id: &str,
        turn_id: i64,
        seq: i64,
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM ai_run_checkpoints WHERE session_id = ?1 AND turn_id = ?2 AND seq > ?3",
            params![session_id, turn_id, seq],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::board::Board;
    use crate::manuscript::NewManuscript;
    use crate::manuscript::ai_sessions::AiSessionRecord;
    use std::path::Path;

    /// 建好稿件、会话与一轮，返回（库，会话 id，稿件 id）。检查点的外键挂在这一轮上。
    fn fixture() -> (ManuscriptStore, String, i64) {
        let mut store = ManuscriptStore::open(Path::new(":memory:")).unwrap();
        let manuscript = store.create(&NewManuscript::default(), None).unwrap();
        let session = AiSessionRecord {
            id: "s1".into(),
            is_current: true,
            created_at: now(),
            updated_at: now(),
            ..AiSessionRecord::default()
        };
        store
            .save_ai_session(manuscript, &session, &[(1, "{}".into())], &[1])
            .unwrap();
        (store, "s1".into(), manuscript)
    }

    fn ckpt(at: StepPath) -> Checkpoint {
        Checkpoint {
            at,
            reason: Reason::Step,
            label: "已完成 算子 gap_loop".into(),
            board: Board::default(),
            partial: false,
        }
    }

    fn save(store: &mut ManuscriptStore, at: StepPath) {
        store
            .save_run_checkpoint("s1", 1, "skill", "hash", false, &ckpt(at))
            .unwrap();
    }

    #[test]
    fn checkpoints_round_trip_and_keep_the_first_plus_recent_five() {
        let (mut store, session, _) = fixture();
        for i in 1..=8 {
            save(&mut store, vec![i]);
        }
        let all = store.list_run_checkpoints(&session, 1).unwrap();
        let seqs: Vec<i64> = all.iter().map(|c| c.seq).collect();
        assert_eq!(seqs, [1, 4, 5, 6, 7, 8], "序号最小的一个 + 最近 5 个");
        let latest = store.latest_run_checkpoint(&session, 1).unwrap().unwrap();
        assert_eq!(latest.seq, 8);
        assert_eq!(latest.checkpoint.at, [8]);
        assert_eq!(latest.label, "已完成 算子 gap_loop");
        assert_eq!(latest.skill_id, "skill");
        assert!(!latest.checkpoint.partial);
    }

    /// 路径列坏了就用 data 里存的路径，不能拿序号顶替（序号是第几份检查点，不是第几步）。
    #[test]
    fn a_broken_path_column_falls_back_to_the_path_inside_the_data() {
        let (mut store, session, _) = fixture();
        for _ in 0..3 {
            store
                .save_run_checkpoint(&session, 1, "polish", "h", false, &ckpt(vec![4, 2]))
                .unwrap();
        }
        store
            .conn
            .execute("UPDATE ai_run_checkpoints SET at_path = '坏了'", [])
            .unwrap();
        let latest = store.latest_run_checkpoint(&session, 1).unwrap().unwrap();
        assert_eq!(latest.seq, 3);
        assert_eq!(latest.checkpoint.at, [4, 2]);
    }

    #[test]
    fn an_oversized_checkpoint_keeps_only_recent_evidence_and_is_marked_partial() {
        let (mut store, session, _) = fixture();
        let mut ckpt = ckpt(vec![1]);
        // 造一份超过 512 KB 的黑板：工作稿顶着上限，证据包 30 条各 1 万字。
        ckpt.board.workspace = "稿".repeat(400 * 1024);
        let docs: Vec<_> = (0..30)
            .map(|i| crate::agent::evidence::EvidenceDoc {
                key: format!("k{i}"),
                title: format!("资料{i}"),
                section: String::new(),
                kind_label: "知识库".into(),
                text: "据".repeat(10_000),
            })
            .collect();
        ckpt.board.evidence.absorb_docs("测试", &docs);
        store
            .save_run_checkpoint(&session, 1, "skill", "hash", false, &ckpt)
            .unwrap();
        let latest = store.latest_run_checkpoint(&session, 1).unwrap().unwrap();
        assert!(latest.checkpoint.partial, "超尺寸的标 partial");
        assert_eq!(
            latest.checkpoint.board.evidence.items().len(),
            EVIDENCE_KEEP,
            "证据包只留最近 20 条"
        );
        assert_eq!(
            latest.checkpoint.board.workspace.len(),
            ckpt.board.workspace.len(),
            "工作稿不截"
        );
    }

    #[test]
    fn checkpoints_disappear_with_the_turn_and_the_session() {
        let (mut store, session, manuscript) = fixture();
        save(&mut store, vec![1]);
        save(&mut store, vec![2]);
        assert_eq!(store.list_run_checkpoints(&session, 1).unwrap().len(), 2);

        // 轮次被删（清空任务流）：检查点跟着删，不留孤儿行。
        let record = store
            .list_ai_sessions(manuscript)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        store
            .save_ai_session(manuscript, &record, &[], &[])
            .unwrap();
        assert!(store.list_run_checkpoints(&session, 1).unwrap().is_empty());

        // 会话被删：级联清除（先存一轮再删会话）。
        store
            .save_ai_session(manuscript, &record, &[(1, "{}".into())], &[1])
            .unwrap();
        save(&mut store, vec![1]);
        store.delete_ai_session(&session).unwrap();
        assert!(store.list_run_checkpoints(&session, 1).unwrap().is_empty());
    }

    #[test]
    fn a_checkpoint_needs_its_turn_row_first() {
        // 外键挂在 ai_session_turns 上：会话行或这一轮还没入库时写不进去。
        // 所以开跑前（界面线程）要先强制存一次会话。
        let (mut store, session, _) = fixture();
        let result = store.save_run_checkpoint(&session, 99, "s", "h", false, &ckpt(vec![1]));
        assert!(result.is_err(), "没有这一轮就写不进去");
    }
}
