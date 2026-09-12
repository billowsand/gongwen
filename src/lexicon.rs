//! 公文词表：把长期积累的公文变成输入法词库。
//!
//! 与「标准词库」（`AppConfig.vocabulary`，维护单位人员要素）和「校对词表」
//! （`proofread`，维护正误对照）不同，这里是一张**派生**表：词条由稿件库、
//! 知识库的正文累积而来，再叠加前两者的权威词，最后导出成小鹤双拼用户码表。
//!
//! ## 为什么是派生表而不是一次性导出
//!
//! 一次性导出的词表工具，每次重扫都会丢掉上次的人工修订（删掉的误词、标好的
//! 多音字），用一次就废。所以这里的三条状态都**永久有效**：
//!
//! - `state = Rejected` 的词重扫不再冒头；
//! - `locked = true` 的词条，人工标的读音与自定义编码重扫不覆盖；
//! - 已扫过的来源按内容哈希记账（`lexicon_scans`），正文没变就不重复计数。
//!
//! ## 为什么词频不是排序主键
//!
//! 小鹤词组是四码定长，所以收益能精确算：一个 n 字词正常要敲 2n 键，进了用户
//! 码表只要 4 键，省 `2n-4` 键。二字词省 0 键——收进去只会占编码空间、多一次
//! 翻页。因此排序权重是 `省键数 × 出现篇数`（[`LexiconTerm::weight`]），
//! 一篇里刷二十次的项目代号，价值低于出现在八篇里的科室简称。

pub mod export;
pub mod flypy;
pub mod scan;
pub mod segmenter;

use crate::knowledge::content_hash;
use crate::models::{VocabularyCategory, VocabularyEntry};
use crate::proofread;
use anyhow::{Context, Result};
use chrono::Local;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

/// 词条能进词表的最短与最长字数。单字没有收益，超长词多半是分词事故。
pub const MIN_TERM_CHARS: usize = 2;
pub const MAX_TERM_CHARS: usize = 12;

/// 词条的来源。决定它需不需要人工确认，以及导出时的优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TermOrigin {
    /// 标准词库里的单位、人员、简称、机关代字。权威，自动接受。
    Vocabulary,
    /// 校对词表的「建议写法」。权威，自动接受。
    Proofread,
    /// 从稿件库/知识库正文扫出来的词。需要人工过一遍。
    Corpus,
    /// 用户在词表页自己加的。
    Manual,
}

impl TermOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vocabulary => "vocabulary",
            Self::Proofread => "proofread",
            Self::Corpus => "corpus",
            Self::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "vocabulary" => Self::Vocabulary,
            "proofread" => Self::Proofread,
            "manual" => Self::Manual,
            _ => Self::Corpus,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Vocabulary => "标准词库",
            Self::Proofread => "校对词表",
            Self::Corpus => "语料",
            Self::Manual => "手工",
        }
    }

    /// 权威词源不需要人工确认——用户在标准词库/校对词表里已经确认过一次了。
    pub fn auto_accepted(self) -> bool {
        matches!(self, Self::Vocabulary | Self::Proofread | Self::Manual)
    }
}

/// 词条的确认状态。`Rejected` 是永久的，重扫不会把它拉回待确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermState {
    Candidate,
    Accepted,
    Rejected,
}

impl TermState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            _ => Self::Candidate,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Candidate => "待确认",
            Self::Accepted => "已接受",
            Self::Rejected => "已拒绝",
        }
    }
}

/// 词表里的一条词。
#[derive(Debug, Clone)]
pub struct LexiconTerm {
    pub id: i64,
    pub term: String,
    /// 人工标注的读音（空格分隔的无声调拼音）；空串表示按字典默认读音出码。
    pub pinyin: String,
    /// 人工指定的编码；空串表示按小鹤四码规则出码。
    pub code_override: String,
    pub freq_total: i64,
    pub doc_count: i64,
    pub origin: TermOrigin,
    pub state: TermState,
    /// 人工动过读音或编码，重扫不覆盖。
    pub locked: bool,
    /// jieba 自带词典里就有这个词——通用词，输入法多半也有，收进码表收益存疑。
    pub in_base_dict: bool,
    pub group_name: String,
    pub note: String,
    pub first_seen: String,
    pub last_seen: String,
}

impl LexiconTerm {
    pub fn char_count(&self) -> usize {
        self.term.chars().count()
    }

    /// 进用户码表能省下的键数：n 字词双拼要 2n 键，码表里一律 4 键。
    /// 二字及以下为 0——收进去只占编码空间。
    pub fn saved_keys(&self) -> i64 {
        (self.char_count() as i64 * 2 - 4).max(0)
    }

    /// 导出排序权重：省键数 × 出现篇数。权威词源的篇数可能为 0，按 1 计。
    pub fn weight(&self) -> i64 {
        self.saved_keys() * self.doc_count.max(1)
    }

    /// 这条词最终写进码表的编码。自定义编码优先，其次人工标音，最后字典读音。
    pub fn code(&self) -> Result<String, flypy::EncodeError> {
        let override_code = self.code_override.trim();
        if !override_code.is_empty() {
            return Ok(override_code.to_ascii_lowercase());
        }
        let syllables = if self.pinyin.trim().is_empty() {
            flypy::word_pinyin(&self.term)?
        } else {
            flypy::parse_pinyin(&self.pinyin)
        };
        if syllables.len() != self.char_count() {
            return Err(flypy::EncodeError::mismatch(
                &self.term,
                self.char_count(),
                syllables.len(),
            ));
        }
        flypy::encode_word_from_pinyin(&syllables)
    }
}

/// 列表过滤条件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermFilter {
    /// None 表示不按状态过滤。
    pub state: Option<TermState>,
    pub origin: Option<TermOrigin>,
    pub search: String,
    pub min_chars: usize,
    pub limit: usize,
}

impl Default for TermFilter {
    fn default() -> Self {
        Self {
            state: None,
            origin: None,
            search: String::new(),
            min_chars: 0,
            // 词表页一次只看得过来这么多；排序把最该看的排在前面。
            limit: 500,
        }
    }
}

/// 词表概况，显示在页面顶部。
#[derive(Debug, Clone, Default)]
pub struct LexiconStats {
    pub total: i64,
    pub accepted: i64,
    pub candidate: i64,
    pub rejected: i64,
    /// 已扫过的来源数（稿件 + 外部知识库文档）。
    pub scanned_sources: i64,
    pub last_scan_at: String,
}

/// 词表表结构。与稿件库、知识库同一个库文件，独立连接。
///
/// `lexicon_source_terms` 记每个来源各贡献了哪些词、各多少次——聚合列
/// （`freq_total` / `doc_count`）全部由它重算。稿件改完再存要重新计数，
/// 只增不减的累加会把同一篇算两遍，所以来源级明细不能省。
pub(crate) const DDL_LEXICON: &str = r#"
CREATE TABLE IF NOT EXISTS lexicon_terms (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    term          TEXT    NOT NULL UNIQUE,
    pinyin        TEXT    NOT NULL DEFAULT '',
    code_override TEXT    NOT NULL DEFAULT '',
    freq_total    INTEGER NOT NULL DEFAULT 0,
    doc_count     INTEGER NOT NULL DEFAULT 0,
    origin        TEXT    NOT NULL DEFAULT 'corpus',
    state         TEXT    NOT NULL DEFAULT 'candidate',
    locked        INTEGER NOT NULL DEFAULT 0,
    in_base_dict  INTEGER NOT NULL DEFAULT 0,
    group_name    TEXT    NOT NULL DEFAULT '',
    note          TEXT    NOT NULL DEFAULT '',
    first_seen    TEXT    NOT NULL,
    last_seen     TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_lexicon_terms_state  ON lexicon_terms(state);
CREATE INDEX IF NOT EXISTS idx_lexicon_terms_origin ON lexicon_terms(origin);

-- 已扫过的来源与当时的正文哈希。正文没变就整篇跳过。
CREATE TABLE IF NOT EXISTS lexicon_scans (
    source_kind  TEXT    NOT NULL,
    source_id    INTEGER NOT NULL,
    content_hash TEXT    NOT NULL,
    title        TEXT    NOT NULL DEFAULT '',
    term_count   INTEGER NOT NULL DEFAULT 0,
    scanned_at   TEXT    NOT NULL,
    PRIMARY KEY (source_kind, source_id)
);

-- 来源级明细：聚合列由它重算，重扫一篇先删后插即可保持精确。
CREATE TABLE IF NOT EXISTS lexicon_source_terms (
    source_kind TEXT    NOT NULL,
    source_id   INTEGER NOT NULL,
    term        TEXT    NOT NULL,
    count       INTEGER NOT NULL,
    PRIMARY KEY (source_kind, source_id, term)
);
CREATE INDEX IF NOT EXISTS idx_lexicon_source_terms_term ON lexicon_source_terms(term);
"#;

/// 幂等地建齐词表相关的表。`ManuscriptStore` 与 `LexiconStore` 谁先打开谁建。
pub(crate) fn ensure_lexicon_schema(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(DDL_LEXICON)
}

pub struct LexiconStore {
    conn: Connection,
}

impl LexiconStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).context("创建词表目录失败")?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("无法打开词表 {}", path.display()))?;
        // 扫描线程可能正持有写事务，读连接要肯等。
        conn.busy_timeout(Duration::from_millis(10_000))?;
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;")?;
        ensure_lexicon_schema(&conn)?;
        Ok(Self { conn })
    }

    fn now() -> String {
        Local::now().to_rfc3339()
    }

    pub fn stats(&self) -> Result<LexiconStats> {
        let mut stats = LexiconStats::default();
        let mut stmt = self
            .conn
            .prepare("SELECT state, COUNT(*) FROM lexicon_terms GROUP BY state")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let state: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            stats.total += count;
            match TermState::parse(&state) {
                TermState::Accepted => stats.accepted = count,
                TermState::Candidate => stats.candidate = count,
                TermState::Rejected => stats.rejected = count,
            }
        }
        stats.scanned_sources =
            self.conn
                .query_row("SELECT COUNT(*) FROM lexicon_scans", [], |row| row.get(0))?;
        stats.last_scan_at = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(scanned_at), '') FROM lexicon_scans",
                [],
                |row| row.get(0),
            )
            .unwrap_or_default();
        Ok(stats)
    }

    /// 按过滤条件列词，权重高的在前。
    pub fn list(&self, filter: &TermFilter) -> Result<Vec<LexiconTerm>> {
        // 权重要在 SQL 里排，否则 limit 截出来的就不是最该看的那一批。
        // length(term) 在 SQLite 里对 TEXT 按字符计，正好是字数。
        let mut sql = String::from(
            "SELECT id, term, pinyin, code_override, freq_total, doc_count, origin, state,
                    locked, in_base_dict, group_name, note, first_seen, last_seen
             FROM lexicon_terms WHERE 1 = 1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(state) = filter.state {
            args.push(Box::new(state.as_str().to_string()));
            sql.push_str(&format!(" AND state = ?{}", args.len()));
        }
        if let Some(origin) = filter.origin {
            args.push(Box::new(origin.as_str().to_string()));
            sql.push_str(&format!(" AND origin = ?{}", args.len()));
        }
        let search = filter.search.trim();
        if !search.is_empty() {
            args.push(Box::new(format!("%{search}%")));
            sql.push_str(&format!(" AND term LIKE ?{}", args.len()));
        }
        if filter.min_chars > 0 {
            args.push(Box::new(filter.min_chars as i64));
            sql.push_str(&format!(" AND length(term) >= ?{}", args.len()));
        }
        args.push(Box::new(filter.limit.max(1) as i64));
        sql.push_str(&format!(
            " ORDER BY (MAX(length(term) * 2 - 4, 0) * MAX(doc_count, 1)) DESC,
                       doc_count DESC, freq_total DESC, term ASC
              LIMIT ?{}",
            args.len()
        ));

        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(args.iter().map(|value| value.as_ref()));
        let rows = stmt.query_map(params, map_term)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 导出用：取所有未被拒绝的词，不分页。过滤与预算交给 `export`。
    pub fn export_candidates(&self) -> Result<Vec<LexiconTerm>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, term, pinyin, code_override, freq_total, doc_count, origin, state,
                    locked, in_base_dict, group_name, note, first_seen, last_seen
             FROM lexicon_terms WHERE state <> 'rejected'",
        )?;
        let rows = stmt.query_map([], map_term)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_state(&mut self, id: i64, state: TermState) -> Result<()> {
        self.conn.execute(
            "UPDATE lexicon_terms SET state = ?2, last_seen = ?3 WHERE id = ?1",
            params![id, state.as_str(), Self::now()],
        )?;
        Ok(())
    }

    /// 批量改状态。确认工作台的「全部接受」用它，一条条改会慢得肉眼可见。
    pub fn set_state_many(&mut self, ids: &[i64], state: TermState) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt =
                tx.prepare("UPDATE lexicon_terms SET state = ?2, last_seen = ?3 WHERE id = ?1")?;
            let now = Local::now().to_rfc3339();
            for id in ids {
                stmt.execute(params![id, state.as_str(), now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 改读音或自定义编码。人工动过就锁定，后续重扫不覆盖。
    pub fn set_reading(&mut self, id: i64, pinyin: &str, code_override: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE lexicon_terms
             SET pinyin = ?2, code_override = ?3, locked = 1, last_seen = ?4
             WHERE id = ?1",
            params![
                id,
                pinyin.trim(),
                code_override.trim().to_ascii_lowercase(),
                Self::now()
            ],
        )?;
        Ok(())
    }

    /// 手工加一条词。已存在则只把它拉回待确认，不动已有的读音与统计。
    pub fn add_manual(&mut self, term: &str) -> Result<()> {
        let term = term.trim();
        let chars = term.chars().count();
        if !(MIN_TERM_CHARS..=MAX_TERM_CHARS).contains(&chars) {
            anyhow::bail!("词长要在 {MIN_TERM_CHARS}–{MAX_TERM_CHARS} 字之间");
        }
        if !flypy::is_all_han(term) {
            anyhow::bail!("「{term}」含有无法转成拼音的字符");
        }
        let now = Self::now();
        self.conn.execute(
            "INSERT INTO lexicon_terms (term, origin, state, first_seen, last_seen)
             VALUES (?1, 'manual', 'accepted', ?2, ?2)
             ON CONFLICT(term) DO UPDATE SET state = 'accepted', last_seen = ?2",
            params![term, now],
        )?;
        Ok(())
    }

    /// 清空整张词表，连同扫描记账——下次扫描从头再来。
    pub fn clear_all(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM lexicon_source_terms;
             DELETE FROM lexicon_scans;
             DELETE FROM lexicon_terms;",
        )?;
        Ok(())
    }

    /// 把标准词库的单位、人员、简称、机关代字并进词表。
    ///
    /// 这是「一致性」的核心：输入法里打出来的单位名，和导出公文落款里的单位名，
    /// 是同一个字符串。权威词不设词频门槛，直接置为已接受。
    pub fn sync_vocabulary(&mut self, entries: &[VocabularyEntry]) -> Result<usize> {
        let mut wanted: Vec<(String, String)> = Vec::new();
        for entry in entries {
            let group = match entry.category {
                VocabularyCategory::Unit => "单位",
                VocabularyCategory::Person => "人员",
            };
            let mut names = vec![
                entry.canonical.clone(),
                entry.external_name.clone(),
                entry.abbr.clone(),
            ];
            names.extend(entry.aliases.iter().cloned());
            if entry.category == VocabularyCategory::Unit {
                names.push(entry.department_code.clone());
            }
            for name in names {
                let name = name.trim().to_string();
                if is_acceptable_term(&name) {
                    wanted.push((name, group.to_string()));
                }
            }
        }
        self.upsert_authoritative(&wanted, TermOrigin::Vocabulary)
    }

    /// 把校对词表的「建议写法」并进词表，并返回「错误写法」黑名单。
    ///
    /// 黑名单是集成才有的能力：单独的词表工具从语料里扫到什么收什么，等于把
    /// 错别字固化进输入法。这里错误写法既不入库、也不导出。
    pub fn sync_proofread(&mut self, lexicon: &proofread::Lexicon) -> Result<(usize, Vec<String>)> {
        let mut wanted = Vec::new();
        let mut blacklist = Vec::new();
        for entry in &lexicon.entries {
            if !entry.enabled {
                continue;
            }
            let wrong = entry.wrong.trim();
            if is_acceptable_term(wrong) {
                blacklist.push(wrong.to_string());
            }
            // 建议写法不都是词：「改为具体措施名称」「写明谁重视、怎么重视」这类
            // 是给人看的操作说明，进了码表就是一条打不出也没意义的词。判断直接
            // 复用校对模块自己的 `is_replaceable`——它对「建议写法是不是一个能
            // 原样替换进正文的纯词」已有定论，不必在这里另立一套近似规则。
            if entry.is_replaceable() {
                let suggestion = entry.suggestion.trim();
                if is_acceptable_term(suggestion) {
                    wanted.push((suggestion.to_string(), entry.group.clone()));
                }
            }
        }
        // 建议写法同时也是别处的错误写法时，以黑名单为准：宁可少收一个词。
        wanted.retain(|(term, _)| !blacklist.contains(term));
        let count = self.upsert_authoritative(&wanted, TermOrigin::Proofread)?;
        blacklist.sort();
        blacklist.dedup();
        Ok((count, blacklist))
    }

    /// 权威词入库：新词直接已接受；已有词只在它还是语料词时升级来源，
    /// 绝不改动人工设过的读音、编码与「已拒绝」。
    fn upsert_authoritative(
        &mut self,
        terms: &[(String, String)],
        origin: TermOrigin,
    ) -> Result<usize> {
        if terms.is_empty() {
            return Ok(0);
        }
        let now = Self::now();
        let tx = self.conn.transaction()?;
        let mut added = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO lexicon_terms (term, origin, state, group_name, first_seen, last_seen)
                 VALUES (?1, ?2, 'accepted', ?3, ?4, ?4)
                 ON CONFLICT(term) DO UPDATE SET
                     origin     = ?2,
                     group_name = ?3,
                     last_seen  = ?4,
                     -- 用户明确拒绝过的词不因为它出现在标准词库里就复活
                     state      = CASE WHEN state = 'rejected' THEN 'rejected' ELSE 'accepted' END
                 WHERE lexicon_terms.origin = 'corpus' OR lexicon_terms.origin = ?2",
            )?;
            let mut seen = std::collections::HashSet::new();
            for (term, group) in terms {
                if !seen.insert(term.clone()) {
                    continue;
                }
                added += stmt.execute(params![term, origin.as_str(), group, now])?;
            }
        }
        tx.commit()?;
        Ok(added)
    }

    /// 已扫过的来源及其当时的正文哈希。扫描线程据此跳过没变过的稿件。
    pub fn scanned_hashes(&self) -> Result<HashMap<(String, i64), String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT source_kind, source_id, content_hash FROM lexicon_scans")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                (row.get::<_, String>(0)?, row.get::<_, i64>(1)?),
                row.get::<_, String>(2)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
    }

    /// 已经拒绝过的词。扫描时据此跳过，不让它们重新冒出来。
    pub fn rejected_terms(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT term FROM lexicon_terms WHERE state = 'rejected'")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?)
    }

    /// 记下一个来源的分词结果：先清掉它上次的明细再写新的，所以同一篇稿件
    /// 反复保存、反复扫描也不会把词频算重。
    pub fn record_source(
        &mut self,
        source_kind: &str,
        source_id: i64,
        title: &str,
        hash: &str,
        counts: &[(String, i64, bool)],
    ) -> Result<()> {
        let now = Self::now();
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM lexicon_source_terms WHERE source_kind = ?1 AND source_id = ?2",
            params![source_kind, source_id],
        )?;
        {
            let mut term_stmt = tx.prepare(
                "INSERT INTO lexicon_terms (term, origin, state, in_base_dict, first_seen, last_seen)
                 VALUES (?1, 'corpus', 'candidate', ?2, ?3, ?3)
                 ON CONFLICT(term) DO UPDATE SET last_seen = ?3, in_base_dict = ?2",
            )?;
            let mut source_stmt = tx.prepare(
                "INSERT INTO lexicon_source_terms (source_kind, source_id, term, count)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (term, count, in_base_dict) in counts {
                term_stmt.execute(params![term, in_base_dict, now])?;
                source_stmt.execute(params![source_kind, source_id, term, count])?;
            }
        }
        tx.execute(
            "INSERT INTO lexicon_scans (source_kind, source_id, content_hash, title, term_count, scanned_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(source_kind, source_id) DO UPDATE SET
                 content_hash = ?3, title = ?4, term_count = ?5, scanned_at = ?6",
            params![
                source_kind,
                source_id,
                hash,
                title,
                counts.len() as i64,
                now
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 丢掉已经不存在的来源（稿件被删、知识库文档被删）的明细与记账。
    pub fn forget_missing_sources(&mut self, kind: &str, alive: &[i64]) -> Result<usize> {
        let alive: std::collections::HashSet<i64> = alive.iter().copied().collect();
        let mut stale = Vec::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT source_id FROM lexicon_scans WHERE source_kind = ?1")?;
            let rows = stmt.query_map(params![kind], |row| row.get::<_, i64>(0))?;
            for id in rows {
                let id = id?;
                if !alive.contains(&id) {
                    stale.push(id);
                }
            }
        }
        if stale.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        for id in &stale {
            tx.execute(
                "DELETE FROM lexicon_source_terms WHERE source_kind = ?1 AND source_id = ?2",
                params![kind, id],
            )?;
            tx.execute(
                "DELETE FROM lexicon_scans WHERE source_kind = ?1 AND source_id = ?2",
                params![kind, id],
            )?;
        }
        tx.commit()?;
        Ok(stale.len())
    }

    /// 从来源明细重算全部聚合列。扫描一整批之后调一次，不是每篇都调。
    pub fn recompute_aggregates(&mut self) -> Result<()> {
        self.conn.execute_batch(
            "UPDATE lexicon_terms SET
                freq_total = COALESCE(
                    (SELECT SUM(count) FROM lexicon_source_terms WHERE term = lexicon_terms.term), 0),
                doc_count = COALESCE(
                    (SELECT COUNT(*) FROM lexicon_source_terms WHERE term = lexicon_terms.term), 0)",
        )?;
        Ok(())
    }

    /// 供 jieba 用户词典使用的词。已接受、且长度值得单独成词的那些。
    ///
    /// 这是词表回喂给应用自己的那一路：RAG 的关键词召回（`rag.rs`）和标题
    /// 断行（`export/title.rs`）此前都不认识「新舆处」这类本单位专名，会把它
    /// 切碎，检索和换行都因此变差。
    pub fn jieba_user_dict(&self) -> Result<String> {
        let mut stmt = self.conn.prepare(
            "SELECT term, freq_total FROM lexicon_terms
             WHERE state = 'accepted' AND length(term) >= 2
             ORDER BY term",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut out = String::new();
        for row in rows {
            let (term, freq) = row?;
            // jieba 的词频是「这个词该有多大权重」，0 会让它几乎不可能被切出来。
            out.push_str(&format!("{term} {}\n", freq.max(3)));
        }
        Ok(out)
    }
}

fn map_term(row: &rusqlite::Row) -> rusqlite::Result<LexiconTerm> {
    Ok(LexiconTerm {
        id: row.get(0)?,
        term: row.get(1)?,
        pinyin: row.get(2)?,
        code_override: row.get(3)?,
        freq_total: row.get(4)?,
        doc_count: row.get(5)?,
        origin: TermOrigin::parse(&row.get::<_, String>(6)?),
        state: TermState::parse(&row.get::<_, String>(7)?),
        locked: row.get::<_, i64>(8)? != 0,
        in_base_dict: row.get::<_, i64>(9)? != 0,
        group_name: row.get(10)?,
        note: row.get(11)?,
        first_seen: row.get(12)?,
        last_seen: row.get(13)?,
    })
}

/// 能不能作为词表词条：字数在区间内，且整串都是查得到拼音的汉字。
pub fn is_acceptable_term(term: &str) -> bool {
    let chars = term.chars().count();
    (MIN_TERM_CHARS..=MAX_TERM_CHARS).contains(&chars) && flypy::is_all_han(term)
}

/// 正文哈希，用来判断一个来源要不要重扫。与知识库共用同一套归一化规则。
pub fn source_hash(markdown: &str) -> String {
    content_hash(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(text: &str, doc_count: i64) -> LexiconTerm {
        LexiconTerm {
            id: 0,
            term: text.to_string(),
            pinyin: String::new(),
            code_override: String::new(),
            freq_total: doc_count * 3,
            doc_count,
            origin: TermOrigin::Corpus,
            state: TermState::Candidate,
            locked: false,
            in_base_dict: false,
            group_name: String::new(),
            note: String::new(),
            first_seen: String::new(),
            last_seen: String::new(),
        }
    }

    #[test]
    fn two_char_terms_save_nothing() {
        // 小鹤词组四码定长：二字词双拼本来就是 4 键，进码表白占编码空间。
        assert_eq!(term("公文", 9).saved_keys(), 0);
        assert_eq!(term("公文", 9).weight(), 0);
        assert_eq!(term("计算机", 1).saved_keys(), 2);
        assert_eq!(term("中华人民共和国", 1).saved_keys(), 10);
    }

    #[test]
    fn weight_prefers_long_terms_over_frequent_short_ones() {
        // 出现 3 篇的 8 字机构名，价值高于出现 12 篇的三字词。
        let long = term("全国安全生产专项整治", 3);
        let short = term("进一步", 12);
        assert!(
            long.weight() > short.weight(),
            "{} vs {}",
            long.weight(),
            short.weight()
        );
    }

    #[test]
    fn code_prefers_override_then_manual_pinyin() {
        // 无多音字时走字典默认读音：gong wen bao → g + w + bc。
        assert_eq!(term("公文包", 1).code().unwrap(), "gwbc");
        // 多音字要人工标音纠正：chong qing shi → i + q + ui。
        let mut t = term("重庆市", 1);
        t.pinyin = "chong qing shi".into();
        assert_eq!(t.code().unwrap(), "iqui");
        // 自定义短码优先于一切，并归一化成小写。
        t.code_override = "CQS".into();
        assert_eq!(t.code().unwrap(), "cqs");
    }

    #[test]
    fn manual_pinyin_must_match_character_count() {
        let mut t = term("重庆市", 1);
        t.pinyin = "chong qing".into();
        assert!(t.code().is_err());
    }

    #[test]
    fn term_acceptance_rejects_non_han_and_out_of_range() {
        assert!(is_acceptable_term("公文助手"));
        assert!(!is_acceptable_term("函"));
        assert!(!is_acceptable_term("2026年"));
        assert!(!is_acceptable_term("一二三四五六七八九十十一二三"));
    }

    fn store() -> (tempfile::TempDir, LexiconStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = LexiconStore::open(&dir.path().join("t.db")).unwrap();
        (dir, store)
    }

    #[test]
    fn rescanning_a_changed_source_does_not_double_count() {
        let (_dir, mut store) = store();
        store
            .record_source(
                "manuscript",
                1,
                "甲",
                "h1",
                &[("专项整治".into(), 5, false)],
            )
            .unwrap();
        store.recompute_aggregates().unwrap();
        let first = store.list(&TermFilter::default()).unwrap();
        assert_eq!(first[0].freq_total, 5);
        assert_eq!(first[0].doc_count, 1);

        // 同一篇稿件改完再存，重新扫一次：频次是覆盖，不是累加。
        store
            .record_source(
                "manuscript",
                1,
                "甲",
                "h2",
                &[("专项整治".into(), 2, false)],
            )
            .unwrap();
        store.recompute_aggregates().unwrap();
        let second = store.list(&TermFilter::default()).unwrap();
        assert_eq!(second[0].freq_total, 2);
        assert_eq!(second[0].doc_count, 1);

        // 换一篇稿件贡献同一个词，篇数才涨。
        store
            .record_source(
                "manuscript",
                2,
                "乙",
                "h3",
                &[("专项整治".into(), 1, false)],
            )
            .unwrap();
        store.recompute_aggregates().unwrap();
        let third = store.list(&TermFilter::default()).unwrap();
        assert_eq!(third[0].freq_total, 3);
        assert_eq!(third[0].doc_count, 2);
    }

    #[test]
    fn rejected_terms_are_remembered_across_rescans() {
        let (_dir, mut store) = store();
        store
            .record_source(
                "manuscript",
                1,
                "甲",
                "h1",
                &[("有关事项".into(), 3, false)],
            )
            .unwrap();
        let id = store.list(&TermFilter::default()).unwrap()[0].id;
        store.set_state(id, TermState::Rejected).unwrap();

        assert!(store.rejected_terms().unwrap().contains("有关事项"));
        // 重扫不会把它拉回待确认。
        store
            .record_source(
                "manuscript",
                1,
                "甲",
                "h2",
                &[("有关事项".into(), 9, false)],
            )
            .unwrap();
        let rows = store.list(&TermFilter::default()).unwrap();
        assert_eq!(rows[0].state, TermState::Rejected);
    }

    #[test]
    fn vocabulary_terms_are_authoritative_but_never_resurrect_rejected() {
        let (_dir, mut store) = store();
        let mut unit = VocabularyEntry {
            category: VocabularyCategory::Unit,
            canonical: "市新闻舆情处".into(),
            abbr: "新舆处".into(),
            ..Default::default()
        };
        unit.aliases.push("新闻舆情处".into());
        store.sync_vocabulary(std::slice::from_ref(&unit)).unwrap();
        let rows = store.list(&TermFilter::default()).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|row| row.state == TermState::Accepted));
        assert!(rows.iter().all(|row| row.origin == TermOrigin::Vocabulary));

        let id = rows.iter().find(|row| row.term == "新舆处").unwrap().id;
        store.set_state(id, TermState::Rejected).unwrap();
        store.sync_vocabulary(std::slice::from_ref(&unit)).unwrap();
        let rows = store.list(&TermFilter::default()).unwrap();
        let abbr = rows.iter().find(|row| row.term == "新舆处").unwrap();
        assert_eq!(abbr.state, TermState::Rejected, "拒绝过的词不该被同步复活");
    }

    #[test]
    fn proofread_sync_collects_suggestions_and_blacklists_wrong_spellings() {
        let (_dir, mut store) = store();
        let lexicon = proofread::Lexicon::builtin();
        let (_, blacklist) = store.sync_proofread(&lexicon).unwrap();
        // 「布署」是内置词表里的必错条目，绝不能进码表。
        assert!(blacklist.contains(&"布署".to_string()));
        let rows = store.list(&TermFilter::default()).unwrap();
        assert!(rows.iter().any(|row| row.term == "部署"));
        assert!(!rows.iter().any(|row| row.term == "布署"));
        // 「改为具体措施名称」是给人看的操作说明，不是词，不该混进码表。
        assert!(
            !rows.iter().any(|row| row.term == "改为具体措施名称"),
            "操作说明不该被当成词条：{:?}",
            rows.iter().map(|row| &row.term).collect::<Vec<_>>()
        );
    }

    #[test]
    fn jieba_user_dict_only_exports_accepted_terms() {
        let (_dir, mut store) = store();
        store.add_manual("新舆处").unwrap();
        store
            .record_source(
                "manuscript",
                1,
                "甲",
                "h1",
                &[("待确认词".into(), 4, false)],
            )
            .unwrap();
        store.recompute_aggregates().unwrap();
        let dict = store.jieba_user_dict().unwrap();
        assert!(dict.contains("新舆处"));
        assert!(!dict.contains("待确认词"));
    }

    #[test]
    fn deleting_a_vanished_source_drops_its_contribution() {
        let (_dir, mut store) = store();
        store
            .record_source(
                "manuscript",
                1,
                "甲",
                "h1",
                &[("专项整治".into(), 5, false)],
            )
            .unwrap();
        store
            .record_source(
                "manuscript",
                2,
                "乙",
                "h2",
                &[("专项整治".into(), 5, false)],
            )
            .unwrap();
        store.recompute_aggregates().unwrap();
        assert_eq!(store.list(&TermFilter::default()).unwrap()[0].doc_count, 2);

        // 稿件 2 被删掉了：它贡献的词频要一起消失。
        assert_eq!(store.forget_missing_sources("manuscript", &[1]).unwrap(), 1);
        store.recompute_aggregates().unwrap();
        let rows = store.list(&TermFilter::default()).unwrap();
        assert_eq!(rows[0].doc_count, 1);
        assert_eq!(rows[0].freq_total, 5);
    }
}
