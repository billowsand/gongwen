use super::model::{Task, valid_task};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
const DDL: &str = "CREATE TABLE IF NOT EXISTS task_categories(name TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY,name TEXT NOT NULL,category TEXT NOT NULL,role TEXT NOT NULL,collaborators TEXT NOT NULL,goal TEXT NOT NULL,next_step TEXT NOT NULL,next_deadline TEXT,deadline TEXT,status TEXT NOT NULL,parent_id TEXT,needs_review INTEGER NOT NULL DEFAULT 0,archived INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,revision INTEGER NOT NULL DEFAULT 1,updated_at TEXT NOT NULL); CREATE TABLE IF NOT EXISTS task_prerequisites(task_id TEXT NOT NULL, prerequisite_id TEXT NOT NULL, PRIMARY KEY(task_id,prerequisite_id)); CREATE TABLE IF NOT EXISTS task_history(id INTEGER PRIMARY KEY, task_id TEXT NOT NULL, happened_at TEXT NOT NULL, action TEXT NOT NULL, detail TEXT NOT NULL, source_id TEXT); CREATE TABLE IF NOT EXISTS task_sources(id TEXT PRIMARY KEY,title TEXT NOT NULL,content TEXT NOT NULL,fingerprint TEXT NOT NULL UNIQUE,source_ref TEXT NOT NULL,created_at TEXT NOT NULL); CREATE TABLE IF NOT EXISTS task_candidates(id TEXT PRIMARY KEY,source_id TEXT NOT NULL,task_id TEXT,task_revision INTEGER,kind TEXT NOT NULL,payload TEXT NOT NULL,evidence TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',created_at TEXT NOT NULL,FOREIGN KEY(source_id) REFERENCES task_sources(id)); CREATE TABLE IF NOT EXISTS task_preferences(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT OR IGNORE INTO task_categories(name) VALUES('未分类');";
pub type CandidateRow = (String, String, String, String, String);
pub struct Store {
    pub(super) conn: Connection,
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path).context("打开任务助手数据库失败")?;
        conn.busy_timeout(std::time::Duration::from_millis(10_000))
            .context("设置任务数据库等待时间失败")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .context("启用任务助手外键失败")?;
        conn.execute_batch(DDL).context("初始化任务助手表失败")?;
        Ok(Self { conn })
    }
    pub fn list(&self, include_closed: bool) -> Result<Vec<Task>> {
        load_tasks(&self.conn, include_closed)
    }
    pub fn save(&mut self, t: &Task, action: &str, detail: &str) -> Result<()> {
        valid_task(t).map_err(anyhow::Error::msg)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let current = load_tasks(&tx, true)?;
        if let Some(old) = current.iter().find(|old| old.id == t.id) {
            if old.archived || old.deleted {
                bail!("已归档或已删除任务只读，请先恢复");
            }
            if old.revision != t.revision {
                bail!("任务在编辑后已变化，请重新打开编辑，避免覆盖新内容");
            }
        } else if t.revision != 0 {
            bail!("任务不存在，不能用旧草稿重建");
        }
        if t.deleted {
            bail!("删除须走回收站确认流程");
        }
        if t.archived && !t.status.terminal() {
            bail!("仅已完成或取消的任务可以归档");
        }
        super::relations::validate_graph(&current, t).map_err(anyhow::Error::msg)?;
        super::relations::can_enter(&current, t, t.status).map_err(anyhow::Error::msg)?;
        let category_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_categories WHERE name=?1)",
            [&t.category],
            |r| r.get(0),
        )?;
        if !category_exists {
            bail!("所选分类不存在，请重新选择");
        }
        let old: Option<i64> = tx
            .query_row("SELECT revision FROM tasks WHERE id=?1", [&t.id], |r| {
                r.get(0)
            })
            .optional()?;
        let old_status: Option<String> = tx
            .query_row("SELECT status FROM tasks WHERE id=?1", [&t.id], |r| {
                r.get(0)
            })
            .optional()?;
        if t.status == super::model::Status::Cancelled
            && old_status.as_deref() != Some("已取消")
            && detail.trim().is_empty()
        {
            bail!("取消任务必须填写理由");
        }
        let revision = old.unwrap_or(0) + 1;
        let now = chrono::Local::now().to_rfc3339();
        tx.execute("INSERT INTO tasks(id,name,category,role,collaborators,goal,next_step,next_deadline,deadline,status,parent_id,needs_review,archived,deleted,revision,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16) ON CONFLICT(id) DO UPDATE SET name=excluded.name,category=excluded.category,role=excluded.role,collaborators=excluded.collaborators,goal=excluded.goal,next_step=excluded.next_step,next_deadline=excluded.next_deadline,deadline=excluded.deadline,status=excluded.status,parent_id=excluded.parent_id,needs_review=excluded.needs_review,archived=excluded.archived,deleted=excluded.deleted,revision=excluded.revision,updated_at=excluded.updated_at",params![t.id,t.name,t.category,t.role.as_str(),t.collaborators,t.goal,t.next_step,t.next_deadline,t.deadline,t.status.as_str(),t.parent_id,t.needs_review,t.archived,t.deleted,revision,now])?;
        tx.execute("DELETE FROM task_prerequisites WHERE task_id=?1", [&t.id])?;
        for p in &t.prerequisites {
            tx.execute(
                "INSERT INTO task_prerequisites(task_id,prerequisite_id) VALUES(?1,?2)",
                params![t.id, p],
            )?;
        }
        if old_status.as_deref() == Some("已完成")
            && (t.status != super::model::Status::Done || t.needs_review)
        {
            for (id, dependency) in super::relations::reopening_impact(&current, &t.id) {
                tx.execute("UPDATE tasks SET needs_review=CASE WHEN status='已完成' OR ?1 THEN 1 ELSE needs_review END,status=CASE WHEN ?1 AND status='进行中' THEN '阻塞' ELSE status END,revision=revision+1,updated_at=?2 WHERE id=?3",params![dependency,now,id])?;
                tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail) VALUES(?1,?2,'依赖复核',?3)",params![id,now,format!("关联任务「{}」已重开或待核对，请重新检查安排",t.name)])?;
            }
        }
        tx.execute(
            "INSERT INTO task_history(task_id,happened_at,action,detail) VALUES(?1,?2,?3,?4)",
            params![
                t.id,
                now,
                action,
                change_detail(current.iter().find(|old| old.id == t.id), t, detail)
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn categories(&self) -> Result<Vec<String>> {
        let mut s = self
            .conn
            .prepare("SELECT name FROM task_categories ORDER BY name")?;
        Ok(s.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn add_category(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 {
            bail!("分类名称不能为空且最多 80 字")
        }
        self.conn
            .execute("INSERT INTO task_categories(name) VALUES(?1)", [name])
            .context("分类已存在或无法保存")?;
        Ok(())
    }
    pub fn move_category(&mut self, from: &str, to: &str) -> Result<()> {
        if from == "未分类" || from == to {
            bail!("不能删除未分类或迁移到自身")
        }
        let tx = self.conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_categories WHERE name=?1)",
            [to],
            |r| r.get(0),
        )?;
        if !exists {
            bail!("迁移目标分类不存在")
        }
        let now = chrono::Local::now().to_rfc3339();
        tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail) SELECT id,?1,'分类迁移',?2 FROM tasks WHERE category=?3",params![now,format!("分类从「{from}」迁移到「{to}」"),from])?;
        tx.execute(
            "UPDATE tasks SET category=?1,revision=revision+1,updated_at=?2 WHERE category=?3",
            params![to, now, from],
        )?;
        tx.execute("DELETE FROM task_categories WHERE name=?1", [from])?;
        tx.commit()?;
        Ok(())
    }
    pub fn has_source(&self, fingerprint: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_sources WHERE fingerprint=?1)",
            [fingerprint],
            |r| r.get(0),
        )?)
    }
    pub fn set_auto(&self, on: bool) -> Result<()> {
        self.conn.execute("INSERT INTO task_preferences(key,value) VALUES('auto_organize',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[if on{"1"}else{"0"}])?;
        Ok(())
    }
    pub fn auto(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT value='1' FROM task_preferences WHERE key='auto_organize'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }
}
fn parse_role(s: &str) -> super::model::Role {
    match s {
        "配合" => super::model::Role::Collaborate,
        "跟进" => super::model::Role::Follow,
        "关注" => super::model::Role::Watch,
        _ => super::model::Role::Responsible,
    }
}
fn parse_status(s: &str) -> super::model::Status {
    match s {
        "进行中" => super::model::Status::InProgress,
        "等待" => super::model::Status::Waiting,
        "阻塞" => super::model::Status::Blocked,
        "已完成" => super::model::Status::Done,
        "已取消" => super::model::Status::Cancelled,
        _ => super::model::Status::Todo,
    }
}
impl Store {
    pub fn candidates(&self) -> Result<Vec<CandidateRow>> {
        let mut s = self.conn.prepare(
            "SELECT id,kind,payload,evidence,state FROM task_candidates ORDER BY created_at DESC",
        )?;
        Ok(s.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn candidate_done(&self, id: &str, state: &str) -> Result<()> {
        if !["pending", "later", "ignored"].contains(&state) {
            bail!("非法候选状态")
        }
        self.conn.execute(
            "UPDATE task_candidates SET state=?1 WHERE id=?2 AND state!='accepted'",
            rusqlite::params![state, id],
        )?;
        Ok(())
    }
    pub fn apply_candidate(&mut self, id: &str, chosen_task_id: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        use super::extract::Proposal;
        let (recorded_task, recorded_revision, payload, evidence, source_id, source_content): (Option<String>,Option<i64>,String,String,String,String)=tx.query_row("SELECT c.task_id,c.task_revision,c.payload,c.evidence,c.source_id,s.content FROM task_candidates c JOIN task_sources s ON s.id=c.source_id WHERE c.id=?1 AND c.state!='accepted'",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        if evidence.trim().is_empty() || !source_content.contains(&evidence) {
            bail!("来源摘录校验失败，不能采纳");
        }
        if recorded_task
            .as_deref()
            .is_some_and(|target| target != chosen_task_id)
        {
            bail!("候选已有匹配目标，请先重新核对");
        }
        let proposal: Proposal = serde_json::from_str(&payload)?;
        let is_note = matches!(proposal.kind.as_str(), "opinion" | "decision");
        let mut tasks = load_tasks(&tx, true)?;
        let Some(index) = tasks
            .iter()
            .position(|t| t.id == chosen_task_id && !t.deleted && !t.archived)
        else {
            bail!("目标任务不存在、已归档或已删除");
        };
        if recorded_task.is_some() && recorded_revision != Some(tasks[index].revision) {
            bail!("候选已过期：目标任务在识别后发生了人工修改，请重新整理材料");
        }
        let mut task = tasks[index].clone();
        if let Some(name) = proposal.name {
            task.name = name;
        }
        if let Some(goal) = proposal.goal {
            task.goal = goal;
        }
        if let Some(next) = proposal.next_step {
            task.next_step = next;
        }
        if let Some(deadline) = proposal.deadline {
            task.deadline = Some(deadline);
        }
        if let Some(collaborators) = proposal.collaborators {
            task.collaborators = collaborators;
        }
        if proposal.kind == "completion_evidence" {
            task.status = super::model::Status::Done;
        }
        if !is_note {
            super::model::valid_task(&task).map_err(anyhow::Error::msg)?;
            super::relations::validate_graph(&tasks, &task).map_err(anyhow::Error::msg)?;
            super::relations::can_enter(&tasks, &task, task.status).map_err(anyhow::Error::msg)?;
        }
        let actual: Option<i64> = tx
            .query_row(
                "SELECT revision FROM tasks WHERE id=?1",
                [chosen_task_id],
                |r| r.get(0),
            )
            .optional()?;
        let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM task_candidates WHERE id=?1 AND state IN ('pending','later'))",[id],|r|r.get(0))?;
        if !pending {
            bail!("候选已处理，请先恢复到待整理");
        }
        if actual != Some(tasks[index].revision) {
            bail!("任务已变化，请重新核对候选");
        }
        let next_revision = tasks[index].revision + 1;
        let now = chrono::Local::now().to_rfc3339();
        if is_note {
            let detail = format!(
                "提出主体：{}；针对：{}；意见/待决：{}；摘录：{}",
                proposal.speaker.as_deref().unwrap_or("未注明"),
                proposal.about.as_deref().unwrap_or(&tasks[index].name),
                proposal.note.as_deref().unwrap_or("（无结构化意见内容）"),
                evidence
            );
            tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail,source_id) VALUES(?1,?2,?3,?4,?5)",params![chosen_task_id,now,if proposal.kind=="decision"{"记录待决事项"}else{"记录来源意见"},detail,source_id])?;
            tx.execute(
                "UPDATE task_candidates SET state='accepted' WHERE id=?1",
                [id],
            )?;
            tx.commit()?;
            return Ok(());
        }
        tx.execute("UPDATE tasks SET name=?1,category=?2,role=?3,collaborators=?4,goal=?5,next_step=?6,next_deadline=?7,deadline=?8,status=?9,parent_id=?10,needs_review=?14,revision=?11,updated_at=?12 WHERE id=?13",params![task.name,task.category,task.role.as_str(),task.collaborators,task.goal,task.next_step,task.next_deadline,task.deadline,task.status.as_str(),task.parent_id,next_revision,now,chosen_task_id,task.needs_review])?;
        tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail,source_id) VALUES(?1,?2,'采纳材料建议',?3,?4)",params![chosen_task_id,now,change_detail(Some(&tasks[index]), &task, &evidence),source_id])?;
        tx.execute(
            "UPDATE task_candidates SET state='accepted' WHERE id=?1",
            [id],
        )?;
        tx.commit()?;
        tasks[index].revision = next_revision;
        Ok(())
    }
    pub fn recheck_candidate(&mut self, id: &str, chosen_task_id: &str) -> Result<()> {
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND deleted=0 AND archived=0)",
            [chosen_task_id],
            |r| r.get(0),
        )?;
        if !exists {
            bail!("重新核对目标不存在或只读");
        }
        let revision: i64 = self.conn.query_row(
            "SELECT revision FROM tasks WHERE id=?1",
            [chosen_task_id],
            |r| r.get(0),
        )?;
        let changed=self.conn.execute("UPDATE task_candidates SET task_id=?1,task_revision=?2,state='pending' WHERE id=?3 AND state!='accepted'",params![chosen_task_id,revision,id])?;
        if changed == 0 {
            bail!("候选已采纳或不存在");
        }
        Ok(())
    }
    pub fn edit_candidate(&self, id: &str, proposal: &super::extract::Proposal) -> Result<()> {
        let (old_kind,evidence,source):(String,String,String)=self.conn.query_row("SELECT c.kind,c.evidence,s.content FROM task_candidates c JOIN task_sources s ON s.id=c.source_id WHERE c.id=?1 AND c.state!='accepted'",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        if proposal.kind != old_kind || proposal.evidence != evidence || !source.contains(&evidence)
        {
            bail!("候选类型或来源证据不一致，不能保存");
        }
        let envelope = serde_json::json!({"items":[proposal]}).to_string();
        super::extract::parse_response(&envelope, &source, &self.list(true)?)?;
        let payload = serde_json::to_string(proposal)?;
        self.conn.execute(
            "UPDATE task_candidates SET payload=?1 WHERE id=?2",
            params![payload, id],
        )?;
        Ok(())
    }
    pub fn history(&self, task_id: &str) -> Result<Vec<(String, String, String)>> {
        let mut q=self.conn.prepare("SELECT happened_at,action,detail FROM task_history WHERE task_id=?1 ORDER BY id DESC LIMIT 100")?;
        Ok(
            q.query_map([task_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        )
    }
    pub fn source_matches(&self, task_id: &str, query: &str) -> Result<bool> {
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        Ok(self.conn.query_row("SELECT EXISTS(SELECT 1 FROM task_history h JOIN task_sources s ON s.id=h.source_id WHERE h.task_id=?1 AND (s.title LIKE ?2 ESCAPE '\\' OR s.content LIKE ?2 ESCAPE '\\' OR h.detail LIKE ?2 ESCAPE '\\'))",params![task_id,pattern],|r|r.get(0))?)
    }
    pub fn record_progress(&self, task_id: &str, detail: &str) -> Result<()> {
        if detail.trim().is_empty() || detail.chars().count() > 2000 {
            bail!("进展内容不能为空且最多 2000 字");
        }
        let tx = self.conn.unchecked_transaction()?;
        let now = chrono::Local::now().to_rfc3339();
        let changed=tx.execute("UPDATE tasks SET revision=revision+1,updated_at=?1 WHERE id=?2 AND archived=0 AND deleted=0",params![now,task_id])?;
        if changed == 0 {
            bail!("任务不存在或只读，不能记进展");
        }
        tx.execute(
            "INSERT INTO task_history(task_id,happened_at,action,detail) VALUES(?1,?2,'进展',?3)",
            [task_id, &now, detail],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn soft_delete(&mut self, id: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let tasks = load_tasks(&tx, true)?;
        let task = tasks
            .iter()
            .find(|t| t.id == id && !t.deleted)
            .ok_or_else(|| anyhow::anyhow!("任务不存在或已删除"))?;
        let now = chrono::Local::now().to_rfc3339();
        tx.execute("UPDATE tasks SET deleted=1,parent_id=NULL,needs_review=1,revision=revision+1,updated_at=?1 WHERE id=?2",params![now,id])?;
        for related in tasks.iter().filter(|t| {
            !t.deleted
                && (t.parent_id.as_deref() == Some(id)
                    || t.prerequisites.iter().any(|p| p == id)
                    || Some(&t.id) == task.parent_id.as_ref())
        }) {
            tx.execute("UPDATE tasks SET parent_id=CASE WHEN parent_id=?1 THEN NULL ELSE parent_id END,needs_review=1,revision=revision+1,updated_at=?2 WHERE id=?3",params![id,now,related.id])?;
            tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail) VALUES(?1,?2,'关系解除',?3)",params![related.id,now,format!("关联任务「{}」移入回收站，关系已解除，请重新核对安排",task.name)])?;
        }
        tx.execute(
            "DELETE FROM task_prerequisites WHERE task_id=?1 OR prerequisite_id=?1",
            [id],
        )?;
        tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail) VALUES(?1,?2,'删除','用户确认删除；恢复不自动重建关系')",params![id,now])?;
        tx.commit()?;
        Ok(())
    }
    pub fn rename_category(&self, old: &str, new: &str) -> Result<()> {
        if old == "未分类" {
            bail!("未分类不可重命名")
        }
        if new.trim().is_empty() || new.chars().count() > 80 {
            bail!("新分类名称不能为空且最多 80 字");
        }
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("INSERT INTO task_categories(name) VALUES(?1)", [new.trim()])?;
        let now = chrono::Local::now().to_rfc3339();
        tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail) SELECT id,?1,'分类重命名',?2 FROM tasks WHERE category=?3",params![now,format!("分类从「{old}」改为「{}」",new.trim()),old])?;
        tx.execute(
            "UPDATE tasks SET category=?1,revision=revision+1,updated_at=?2 WHERE category=?3",
            rusqlite::params![new.trim(), now, old],
        )?;
        tx.execute("DELETE FROM task_categories WHERE name=?1", [old])?;
        tx.commit()?;
        Ok(())
    }
}

impl Store {
    pub fn create_from_candidate(&mut self, id: &str, task: &Task) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (kind,payload,evidence,source_id,source_content):(String,String,String,String,String)=tx.query_row("SELECT c.kind,c.payload,c.evidence,c.source_id,s.content FROM task_candidates c JOIN task_sources s ON s.id=c.source_id WHERE c.id=?1 AND c.state IN ('pending','later')",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
        let proposal: super::extract::Proposal = serde_json::from_str(&payload)?;
        if kind != "new_task" || proposal.task_id.is_some() {
            bail!("此候选不是待人工创建的新任务");
        }
        if evidence.trim().is_empty() || !source_content.contains(&evidence) {
            bail!("来源摘录校验失败，不能采纳");
        }
        if task.status == super::model::Status::Cancelled {
            bail!("请先创建任务，再填写理由取消");
        }
        super::model::valid_task(task).map_err(anyhow::Error::msg)?;
        let current = load_tasks(&tx, true)?;
        super::relations::validate_graph(&current, task).map_err(anyhow::Error::msg)?;
        super::relations::can_enter(&current, task, task.status).map_err(anyhow::Error::msg)?;
        let category_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_categories WHERE name=?1)",
            [&task.category],
            |r| r.get(0),
        )?;
        if !category_exists {
            bail!("所选分类不存在");
        }
        let now = chrono::Local::now().to_rfc3339();
        tx.execute("INSERT INTO tasks(id,name,category,role,collaborators,goal,next_step,next_deadline,deadline,status,parent_id,needs_review,archived,deleted,revision,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0,0,1,?13)",params![task.id,task.name,task.category,task.role.as_str(),task.collaborators,task.goal,task.next_step,task.next_deadline,task.deadline,task.status.as_str(),task.parent_id,task.needs_review,now])?;
        for prerequisite in &task.prerequisites {
            tx.execute(
                "INSERT INTO task_prerequisites(task_id,prerequisite_id) VALUES(?1,?2)",
                params![task.id, prerequisite],
            )?;
        }
        tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail,source_id) VALUES(?1,?2,'采纳新任务候选',?3,?4)",params![task.id,now,evidence,source_id])?;
        tx.execute(
            "UPDATE task_candidates SET state='accepted' WHERE id=?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// 原文快照与全部候选一次提交；模型失败不会留下去重标记。
    pub fn commit_extraction(
        &mut self,
        title: &str,
        content: &str,
        fingerprint: &str,
        source_ref: &str,
        items: &[super::extract::Proposal],
        tasks: &[Task],
    ) -> Result<usize> {
        if title.trim().is_empty()
            || title.chars().count() > 200
            || content.trim().is_empty()
            || content.len() > super::extract::MAX_SOURCE_BYTES
        {
            bail!("材料标题、原文为空或超过上限");
        }
        let envelope = serde_json::json!({"items":items}).to_string();
        let validated = super::extract::parse_response(&envelope, content, tasks)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_sources WHERE fingerprint=?1)",
            [fingerprint],
            |r| r.get(0),
        )?;
        if exists {
            return Ok(0);
        }
        let source_id = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO task_sources(id,title,content,fingerprint,source_ref,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![source_id,title,content,fingerprint,source_ref,chrono::Local::now().to_rfc3339()])?;
        for p in &validated {
            let id = uuid::Uuid::new_v4().to_string();
            let revision = p
                .task_id
                .as_ref()
                .and_then(|id| tasks.iter().find(|t| &t.id == id))
                .map(|t| t.revision);
            let payload = serde_json::to_string(p)?;
            tx.execute("INSERT INTO task_candidates(id,source_id,task_id,task_revision,kind,payload,evidence,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',?8)",params![id,source_id,p.task_id,revision,p.kind,payload,p.evidence,chrono::Local::now().to_rfc3339()])?;
        }
        tx.commit()?;
        Ok(validated.len())
    }
}

fn load_tasks(conn: &Connection, include_closed: bool) -> Result<Vec<Task>> {
    let mut s=conn.prepare("SELECT id,name,category,role,collaborators,goal,next_step,next_deadline,deadline,status,parent_id,needs_review,archived,deleted,revision,updated_at FROM tasks WHERE (?1=1 OR (archived=0 AND deleted=0)) ORDER BY updated_at DESC")?;
    let rows = s.query_map([include_closed], |r| {
        let id: String = r.get(0)?;
        let mut t = Task {
            id: id.clone(),
            name: r.get(1)?,
            category: r.get(2)?,
            role: parse_role(&r.get::<_, String>(3)?),
            collaborators: r.get(4)?,
            goal: r.get(5)?,
            next_step: r.get(6)?,
            next_deadline: r.get(7)?,
            deadline: r.get(8)?,
            status: parse_status(&r.get::<_, String>(9)?),
            parent_id: r.get(10)?,
            prerequisites: vec![],
            needs_review: r.get(11)?,
            archived: r.get(12)?,
            deleted: r.get(13)?,
            revision: r.get(14)?,
            updated_at: r.get(15)?,
        };
        let _ = &mut t;
        Ok(t)
    })?;
    let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for t in &mut out {
        let mut q =
            conn.prepare("SELECT prerequisite_id FROM task_prerequisites WHERE task_id=?1")?;
        t.prerequisites = q
            .query_map([&t.id], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    Ok(out)
}

impl Store {
    pub fn restore_task(&mut self, id: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let now = chrono::Local::now().to_rfc3339();
        let n=tx.execute("UPDATE tasks SET deleted=0,archived=0,needs_review=CASE WHEN deleted=1 THEN 1 ELSE needs_review END,revision=revision+1,updated_at=?1 WHERE id=?2 AND (deleted=1 OR archived=1)",params![now,id])?;
        if n != 1 {
            bail!("任务不存在或无需恢复");
        }
        tx.execute("INSERT INTO task_history(task_id,happened_at,action,detail) VALUES(?1,?2,'恢复','保留原状态；回收站已解除的关系须重新核对设置')",params![id,now])?;
        tx.commit()?;
        Ok(())
    }
    pub fn sources(&self, id: &str) -> Result<Vec<(String, String)>> {
        let mut q=self.conn.prepare("SELECT DISTINCT s.title,s.content FROM task_sources s JOIN task_history h ON h.source_id=s.id WHERE h.task_id=?1 ORDER BY s.created_at DESC LIMIT 30")?;
        Ok(q.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn candidate_source(&self, id: &str) -> Result<(String, String)> {
        Ok(self.conn.query_row("SELECT s.title,s.content FROM task_sources s JOIN task_candidates c ON c.source_id=s.id WHERE c.id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?)))?)
    }
}

fn change_detail(old: Option<&Task>, t: &Task, note: &str) -> String {
    let mut changes = vec![note.to_owned()];
    if let Some(old) = old {
        for (label, a, b) in [
            ("名称", old.name.clone(), t.name.clone()),
            ("分类", old.category.clone(), t.category.clone()),
            (
                "角色",
                old.role.as_str().to_owned(),
                t.role.as_str().to_owned(),
            ),
            (
                "状态",
                old.status.as_str().to_owned(),
                t.status.as_str().to_owned(),
            ),
            ("完成条件", old.goal.clone(), t.goal.clone()),
            ("下一步", old.next_step.clone(), t.next_step.clone()),
            (
                "任务期限",
                old.deadline.clone().unwrap_or_default(),
                t.deadline.clone().unwrap_or_default(),
            ),
            (
                "下一步期限",
                old.next_deadline.clone().unwrap_or_default(),
                t.next_deadline.clone().unwrap_or_default(),
            ),
            ("分工", old.collaborators.clone(), t.collaborators.clone()),
            (
                "上级",
                old.parent_id.clone().unwrap_or_default(),
                t.parent_id.clone().unwrap_or_default(),
            ),
            (
                "前置",
                old.prerequisites.join("、"),
                t.prerequisites.join("、"),
            ),
        ] {
            if a != b {
                changes.push(format!("{label}：{a} → {b}"));
            }
        }
    } else {
        changes.push(format!("{}；完成条件：{}", t.name, t.goal));
    }
    changes.join("\n")
}
