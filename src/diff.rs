//! 版本对照引擎：正文段落级 diff（含段内字级高亮）与公文要素 / 全局配置的字段级对照。
//!
//! 全部是纯函数，不触碰存储与 UI，便于单元测试。
//!
//! 正文 diff 分三步：① 按非空行切块——公文一个自然段就是一行，空行只是分隔，不参与
//! 比较，免得多敲一个空行整篇位移；② 块序列做 LCS，再把相邻的删除块与新增块按相似度
//! 配成"修改"；③ 只对配上对的块跑 token 级 LCS，标出真正改掉的那几个字。少了第三步，
//! 改一个字就是整段标红加整段标绿——公文段落长，那种输出等于没有 diff。
//!
//! `draft_changes` / `profile_changes` / `config_changes` 按已知字段逐一比较，
//! 输出带中文标签的 `FieldChange`，让"公文要素对照"是清单式的，而不是原始 JSON 差异。
//! 对外入口 `manuscript_diff(old, new)` 的参数顺序即对照方向，调用方无从颠倒。

use crate::manuscript::VersionRecord;
use crate::models::{
    AppConfig, DraftInput, JointContact, TemplateKind, TemplateProfile, VocabularyCategory,
    VocabularyEntry,
};
use std::collections::HashMap;
use std::ops::Range;

/// 段内字级片段的类型：未变 / 旧版删掉的 / 新版加上的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Same,
    Removed,
    Added,
}

/// 段内一个连续片段；相邻同类片段在构造时已合并。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineSpan {
    pub kind: SpanKind,
    pub text: String,
}

/// 一处段落级改动的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// 新版新增的段。
    Insert,
    /// 旧版有、新版没有的段。
    Delete,
    /// 同一段被改写，`*_spans` 标出段内差异。
    Replace,
}

/// 块在 Markdown 里的角色：既用于在变更清单里给出可读位置，也用于限制配对
/// ——标题不会跟正文段配成"修改"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockRole {
    Heading,
    TableRow,
    ListItem,
    Paragraph,
}

impl BlockRole {
    pub fn label(self) -> &'static str {
        match self {
            BlockRole::Heading => "标题",
            BlockRole::TableRow => "表格行",
            BlockRole::ListItem => "列表项",
            BlockRole::Paragraph => "正文段",
        }
    }
}

/// 一处段落级改动。
#[derive(Debug, Clone)]
pub struct BlockChange {
    pub kind: ChangeKind,
    /// 旧版行号（1 基）；`Insert` 时为 0。
    pub old_line: usize,
    /// 新版行号（1 基）；`Delete` 时为 0。
    pub new_line: usize,
    /// 旧侧字级片段（`Same` + `Removed`）；`Insert` 时为空。
    pub before_spans: Vec<InlineSpan>,
    /// 新侧字级片段（`Same` + `Added`）；`Delete` 时为空。
    pub after_spans: Vec<InlineSpan>,
    /// 该段在新版正文中的字节范围，供"点击跳源码"。删掉的段在新版里根本不存在，
    /// 没有可跳的位置，所以 `Delete` 是 None。
    pub new_range: Option<Range<usize>>,
    pub role: BlockRole,
}

/// 未改动的一段；折叠区展开时按原样显示。
#[derive(Debug, Clone)]
pub struct ContextBlock {
    pub new_line: usize,
    pub text: String,
    pub new_range: Range<usize>,
}

/// 正文 diff 的一项：一串未改动的上下文，或一处改动。
#[derive(Debug, Clone)]
pub enum DiffBlock {
    Unchanged(Vec<ContextBlock>),
    Changed(BlockChange),
}

/// 正文 diff 结果。
#[derive(Debug, Clone, Default)]
pub struct BodyDiff {
    pub blocks: Vec<DiffBlock>,
    pub changed_count: usize,
}

impl BodyDiff {
    pub fn is_empty(&self) -> bool {
        self.changed_count == 0
    }
}

/// 一篇稿件两个版本的完整对照结果。
#[derive(Debug, Clone, Default)]
pub struct ManuscriptDiff {
    /// 公文要素（含版式）的字段变更清单。
    pub fields: Vec<FieldChange>,
    pub body: BodyDiff,
    /// 备注变更；未变时为 None。
    pub notes: Option<FieldChange>,
}

impl ManuscriptDiff {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.body.is_empty() && self.notes.is_none()
    }

    /// 变更项总数，用于"N 处修改"这类提示。
    pub fn total(&self) -> usize {
        self.fields.len() + self.body.changed_count + usize::from(self.notes.is_some())
    }
}

/// 一个字段的变化：标签 + 变更前 / 变更后。空串表示该侧缺省。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    pub label: &'static str,
    pub before: String,
    pub after: String,
}

/// 某文种的版式变化（`changes` 内的字段标签与起草页表单一致）。
#[derive(Debug, Clone)]
pub struct KindChanges {
    pub kind: TemplateKind,
    pub changes: Vec<FieldChange>,
}

/// 词库中一个词条的增删改。
#[derive(Debug, Clone)]
pub struct VocabChange {
    pub category: VocabularyCategory,
    /// "新增" / "删除" / "修改"。
    pub action: &'static str,
    /// 展示名：单位用规范名称，人员用姓名。
    pub label: String,
    /// 修改时的字段明细；新增/删除为空。
    pub changes: Vec<FieldChange>,
}

/// 配置版本对照结果：词库 / 版式 / 设置三大块。
#[derive(Debug, Clone, Default)]
pub struct ConfigChangeReport {
    pub vocabulary: Vec<VocabChange>,
    pub profiles: Vec<KindChanges>,
    pub settings: Vec<FieldChange>,
}

/// 一份"稿件内容"的轻量载体：既可由某版本构建，也可由起草页当前编辑内容构建。
#[derive(Debug, Clone)]
pub struct ContentSnapshot {
    pub snapshot: DraftInput,
    pub content_markdown: String,
    pub notes: String,
}

impl ContentSnapshot {
    pub fn new(snapshot: DraftInput, content_markdown: String, notes: String) -> Self {
        Self {
            snapshot,
            content_markdown,
            notes,
        }
    }
}

impl From<VersionRecord> for ContentSnapshot {
    fn from(record: VersionRecord) -> Self {
        Self {
            snapshot: record.snapshot,
            content_markdown: record.content_markdown,
            notes: record.notes,
        }
    }
}

/// 相似度低于此值的两段不配成"修改"，而是当作一删一增：改得太彻底的段落，
/// 逐字标差异只会得到一片红绿交错，不如整段对着看。
const REPLACE_SIMILARITY: f32 = 0.4;

/// 段内字级 LCS 的规模上限（两侧 token 数之积）。超了就退化为整段替换：
/// 公文段落一般 100~400 字，触不到这个上限；真触到了也只是精度降级，不卡帧。
const INLINE_TOKEN_BUDGET: usize = 400_000;

/// 一篇稿件两个版本的对照。参数顺序就是对照方向：`old` 在左，`new` 在右。
pub fn manuscript_diff(old: &ContentSnapshot, new: &ContentSnapshot) -> ManuscriptDiff {
    let mut notes = Vec::new();
    field(&mut notes, "备注", &old.notes, &new.notes);
    ManuscriptDiff {
        fields: draft_changes(&old.snapshot, &new.snapshot),
        body: body_diff(&old.content_markdown, &new.content_markdown),
        notes: notes.pop(),
    }
}

/// 正文段落级 diff：`old` 为旧版，`new` 为新版。空行不参与比较。
pub fn body_diff(old: &str, new: &str) -> BodyDiff {
    let old_blocks = split_blocks(old);
    let new_blocks = split_blocks(new);
    let a: Vec<&str> = old_blocks.iter().map(|block| block.text.trim()).collect();
    let b: Vec<&str> = new_blocks.iter().map(|block| block.text.trim()).collect();

    let mut blocks: Vec<DiffBlock> = Vec::new();
    let mut context: Vec<ContextBlock> = Vec::new();
    // 连续的删除 / 新增块先攒起来，等这一组结束再按相似度两两配对。
    let mut removed: Vec<usize> = Vec::new();
    let mut added: Vec<usize> = Vec::new();
    for op in lcs_ops(&a, &b) {
        match op {
            DiffOp::Same(_, j) => {
                flush_group(
                    &mut blocks,
                    &mut removed,
                    &mut added,
                    &old_blocks,
                    &new_blocks,
                );
                let block = &new_blocks[j];
                context.push(ContextBlock {
                    new_line: block.line,
                    text: block.text.clone(),
                    new_range: block.range.clone(),
                });
            }
            DiffOp::Delete(i) => {
                flush_context(&mut blocks, &mut context);
                removed.push(i);
            }
            DiffOp::Insert(j) => {
                flush_context(&mut blocks, &mut context);
                added.push(j);
            }
        }
    }
    flush_group(
        &mut blocks,
        &mut removed,
        &mut added,
        &old_blocks,
        &new_blocks,
    );
    flush_context(&mut blocks, &mut context);

    let changed_count = blocks
        .iter()
        .filter(|block| matches!(block, DiffBlock::Changed(_)))
        .count();
    BodyDiff {
        blocks,
        changed_count,
    }
}

/// 正文里的一个比较单元：一个非空行。公文一个自然段就是一行，所以"行"与"段"等价；
/// 表格行、列表项、标题各自独立成块，改一格只标那一格。
struct Block {
    text: String,
    /// 1 基行号，展示用。
    line: usize,
    /// 在原文中的字节范围，供"点击跳源码"。
    range: Range<usize>,
}

fn split_blocks(text: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (index, raw) in text.split('\n').enumerate() {
        // 统一按 \n 切分后再摘掉 \r，CRLF 文本的块内容才不会带尾巴。
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if !line.trim().is_empty() {
            out.push(Block {
                text: line.to_string(),
                line: index + 1,
                range: start..start + line.len(),
            });
        }
        start += raw.len() + 1;
    }
    out
}

fn block_role(text: &str) -> BlockRole {
    let trimmed = text.trim_start();
    if trimmed.starts_with('#') {
        BlockRole::Heading
    } else if trimmed.starts_with('|') {
        BlockRole::TableRow
    } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") || ordered_item(trimmed) {
        BlockRole::ListItem
    } else {
        BlockRole::Paragraph
    }
}

/// `1. ` / `12) ` 这类有序列表项。
fn ordered_item(text: &str) -> bool {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return false;
    }
    let rest = &text[digits.len()..];
    rest.starts_with(". ") || rest.starts_with(") ")
}

/// 把攒下的删除 / 新增块配对成改动项：同角色且相似度够高的配成 `Replace`，
/// 其余各自成 `Delete` / `Insert`。
fn flush_group(
    blocks: &mut Vec<DiffBlock>,
    removed: &mut Vec<usize>,
    added: &mut Vec<usize>,
    old_blocks: &[Block],
    new_blocks: &[Block],
) {
    let count = removed.len().max(added.len());
    for index in 0..count {
        match (removed.get(index).copied(), added.get(index).copied()) {
            (Some(i), Some(j)) => {
                let before = &old_blocks[i];
                let after = &new_blocks[j];
                let role = block_role(&before.text);
                if role == block_role(&after.text)
                    && similarity(before.text.trim(), after.text.trim()) >= REPLACE_SIMILARITY
                {
                    let (before_spans, after_spans) = inline_spans(&before.text, &after.text);
                    blocks.push(DiffBlock::Changed(BlockChange {
                        kind: ChangeKind::Replace,
                        old_line: before.line,
                        new_line: after.line,
                        before_spans,
                        after_spans,
                        new_range: Some(after.range.clone()),
                        role,
                    }));
                } else {
                    blocks.push(deleted_block(before));
                    blocks.push(inserted_block(after));
                }
            }
            (Some(i), None) => blocks.push(deleted_block(&old_blocks[i])),
            (None, Some(j)) => blocks.push(inserted_block(&new_blocks[j])),
            (None, None) => {}
        }
    }
    removed.clear();
    added.clear();
}

fn flush_context(blocks: &mut Vec<DiffBlock>, context: &mut Vec<ContextBlock>) {
    if !context.is_empty() {
        blocks.push(DiffBlock::Unchanged(std::mem::take(context)));
    }
}

fn deleted_block(block: &Block) -> DiffBlock {
    DiffBlock::Changed(BlockChange {
        kind: ChangeKind::Delete,
        old_line: block.line,
        new_line: 0,
        before_spans: vec![InlineSpan {
            kind: SpanKind::Removed,
            text: block.text.clone(),
        }],
        after_spans: Vec::new(),
        new_range: None,
        role: block_role(&block.text),
    })
}

fn inserted_block(block: &Block) -> DiffBlock {
    DiffBlock::Changed(BlockChange {
        kind: ChangeKind::Insert,
        old_line: 0,
        new_line: block.line,
        before_spans: Vec::new(),
        after_spans: vec![InlineSpan {
            kind: SpanKind::Added,
            text: block.text.clone(),
        }],
        new_range: Some(block.range.clone()),
        role: block_role(&block.text),
    })
}

/// 段内字级 diff：中文逐字比，连续的 ASCII 字母 / 数字算一个 token
/// （`2026` 不该被拆成四份，`教育局`→`教育厅` 该只标一个字）。
fn inline_spans(before: &str, after: &str) -> (Vec<InlineSpan>, Vec<InlineSpan>) {
    let a = tokenize(before);
    let b = tokenize(after);
    if a.len().saturating_mul(b.len()) > INLINE_TOKEN_BUDGET {
        return (
            vec![InlineSpan {
                kind: SpanKind::Removed,
                text: before.to_string(),
            }],
            vec![InlineSpan {
                kind: SpanKind::Added,
                text: after.to_string(),
            }],
        );
    }
    let mut before_spans = Vec::new();
    let mut after_spans = Vec::new();
    for op in lcs_ops(&a, &b) {
        match op {
            DiffOp::Same(i, j) => {
                push_span(&mut before_spans, SpanKind::Same, a[i]);
                push_span(&mut after_spans, SpanKind::Same, b[j]);
            }
            DiffOp::Delete(i) => push_span(&mut before_spans, SpanKind::Removed, a[i]),
            DiffOp::Insert(j) => push_span(&mut after_spans, SpanKind::Added, b[j]),
        }
    }
    (before_spans, after_spans)
}

fn tokenize(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < text.len() {
        if bytes[index].is_ascii_alphanumeric() {
            let start = index;
            while index < text.len() && bytes[index].is_ascii_alphanumeric() {
                index += 1;
            }
            out.push(&text[start..index]);
        } else {
            // 非 ASCII 字母数字：按字符走一步，汉字才不会被切在字节中间。
            let width = text[index..].chars().next().map_or(1, |ch| ch.len_utf8());
            out.push(&text[index..index + width]);
            index += width;
        }
    }
    out
}

fn push_span(out: &mut Vec<InlineSpan>, kind: SpanKind, text: &str) {
    if let Some(last) = out.last_mut()
        && last.kind == kind
    {
        last.text.push_str(text);
        return;
    }
    out.push(InlineSpan {
        kind,
        text: text.to_string(),
    });
}

/// 字符多重集交集占较长一侧的比例。顺序无关，够用来判断"还是同一段吗"。
fn similarity(a: &str, b: &str) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut counts: HashMap<char, usize> = HashMap::new();
    for ch in a.chars() {
        *counts.entry(ch).or_default() += 1;
    }
    let mut common = 0usize;
    for ch in b.chars() {
        if let Some(slot) = counts.get_mut(&ch)
            && *slot > 0
        {
            *slot -= 1;
            common += 1;
        }
    }
    let longer = a.chars().count().max(b.chars().count());
    common as f32 / longer as f32
}

/// LCS 的一步操作，索引分别指向各自的数组。
enum DiffOp {
    Same(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// 逐元素 LCS，回溯还原操作序列。块级与字级两处共用。
fn lcs_ops<T: PartialEq>(a: &[T], b: &[T]) -> Vec<DiffOp> {
    let n = a.len();
    let m = b.len();
    if n == 0 {
        return (0..m).map(DiffOp::Insert).collect();
    }
    if m == 0 {
        return (0..n).map(DiffOp::Delete).collect();
    }
    // LCS 长度表，回溯还原每一步。
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffOp::Same(i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(DiffOp::Delete(i));
            i += 1;
        } else {
            out.push(DiffOp::Insert(j));
            j += 1;
        }
    }
    while i < n {
        out.push(DiffOp::Delete(i));
        i += 1;
    }
    while j < m {
        out.push(DiffOp::Insert(j));
        j += 1;
    }
    out
}

/// 两份 `DraftInput` 公文要素的字段变化（含内嵌的版式配置）。
pub fn draft_changes(a: &DraftInput, b: &DraftInput) -> Vec<FieldChange> {
    let mut out = Vec::new();
    field(&mut out, "文种", a.kind.label(), b.kind.label());
    field(&mut out, "标题提示", &a.title_hint, &b.title_hint);
    field(&mut out, "成文日期", &a.date, &b.date);
    field(&mut out, "会议时间", &a.meeting_time, &b.meeting_time);
    field(&mut out, "参会人员", &a.attendees, &b.attendees);
    out.extend(profile_changes(&a.profile, &b.profile));
    out
}

/// 两份版式配置（`TemplateProfile`）的字段变化。
pub fn profile_changes(a: &TemplateProfile, b: &TemplateProfile) -> Vec<FieldChange> {
    let mut out = Vec::new();
    field(&mut out, "文种", a.kind.label(), b.kind.label());
    field(
        &mut out,
        "行文范围",
        a.correspondence_scope.label(),
        b.correspondence_scope.label(),
    );
    field(
        &mut out,
        "联合发文模式",
        a.joint_issuance_mode.label(),
        b.joint_issuance_mode.label(),
    );
    field(&mut out, "发文单位", &a.issuing_unit, &b.issuing_unit);
    field(
        &mut out,
        "联合发文单位",
        &a.joint_issuing_units,
        &b.joint_issuing_units,
    );
    field(
        &mut out,
        "主送单位",
        &a.main_issuing_unit,
        &b.main_issuing_unit,
    );
    field(&mut out, "机关代字", &a.department_code, &b.department_code);
    field(&mut out, "发文年份", &a.document_year, &b.document_year);
    field(&mut out, "主送", &a.recipient, &b.recipient);
    field(&mut out, "抄送", &a.copies_to, &b.copies_to);
    field(
        &mut out,
        "承办单位",
        &a.responsible_unit,
        &b.responsible_unit,
    );
    field(
        &mut out,
        "联合承办单位",
        &a.joint_responsible_units,
        &b.joint_responsible_units,
    );
    field(&mut out, "联系人", &a.contact_person, &b.contact_person);
    field(&mut out, "联系电话", &a.contact_phone, &b.contact_phone);
    field(
        &mut out,
        "联合联系人",
        joint_contacts(&a.joint_contacts),
        joint_contacts(&b.joint_contacts),
    );
    field(
        &mut out,
        "呈报领导",
        &a.reporting_leaders,
        &b.reporting_leaders,
    );
    field(&mut out, "落款单位", &a.signing_unit, &b.signing_unit);
    field(&mut out, "密级", &a.security_level, &b.security_level);
    field(&mut out, "保密期限", &a.security_period, &b.security_period);
    field(
        &mut out,
        "指人专办",
        yes_no(a.special_handling),
        yes_no(b.special_handling),
    );
    field(&mut out, "文号", &a.document_number, &b.document_number);
    field(
        &mut out,
        "会议地点",
        &a.meeting_location,
        &b.meeting_location,
    );
    field(
        &mut out,
        "函稿版本",
        a.letter_version.label(),
        b.letter_version.label(),
    );
    field(
        &mut out,
        "正文排版风格",
        a.style_mode.label(),
        b.style_mode.label(),
    );
    field(
        &mut out,
        "双面排版",
        yes_no(a.duplex_printing),
        yes_no(b.duplex_printing),
    );
    out
}

/// 两份全局配置（词库 / 版式 / 设置）的对照报告。
pub fn config_changes(a: &AppConfig, b: &AppConfig) -> ConfigChangeReport {
    let mut report = ConfigChangeReport {
        vocabulary: vocab_changes(a, b),
        profiles: profile_set_changes(a, b),
        settings: settings_changes(a, b),
    };
    report.profiles.retain(|kind| !kind.changes.is_empty());
    report
}

/// 词库变更：按稳定 id 匹配，新增 / 删除 / 修改。
fn vocab_changes(a: &AppConfig, b: &AppConfig) -> Vec<VocabChange> {
    let a_map: HashMap<u64, &VocabularyEntry> =
        a.vocabulary.iter().map(|entry| (entry.id, entry)).collect();
    let b_map: HashMap<u64, &VocabularyEntry> =
        b.vocabulary.iter().map(|entry| (entry.id, entry)).collect();
    let mut out = Vec::new();
    for entry in &b.vocabulary {
        if !a_map.contains_key(&entry.id) {
            out.push(VocabChange {
                category: entry.category,
                action: "新增",
                label: vocab_label(entry),
                changes: Vec::new(),
            });
        }
    }
    for entry in &a.vocabulary {
        if !b_map.contains_key(&entry.id) {
            out.push(VocabChange {
                category: entry.category,
                action: "删除",
                label: vocab_label(entry),
                changes: Vec::new(),
            });
        }
    }
    for entry in &b.vocabulary {
        if let Some(old) = a_map.get(&entry.id) {
            let changes = vocab_field_changes(old, entry);
            if !changes.is_empty() {
                out.push(VocabChange {
                    category: entry.category,
                    action: "修改",
                    label: vocab_label(entry),
                    changes,
                });
            }
        }
    }
    out
}

fn vocab_label(entry: &VocabularyEntry) -> String {
    if entry.category == VocabularyCategory::Person {
        entry.canonical.clone()
    } else {
        format!("{}（{}）", entry.canonical, entry.abbr.trim())
    }
}

fn vocab_field_changes(a: &VocabularyEntry, b: &VocabularyEntry) -> Vec<FieldChange> {
    let mut out = Vec::new();
    field(&mut out, "全称", &a.canonical, &b.canonical);
    field(&mut out, "简称", &a.abbr, &b.abbr);
    field(&mut out, "外部名称", &a.external_name, &b.external_name);
    field(&mut out, "机关代字", &a.department_code, &b.department_code);
    field(
        &mut out,
        "是否代章",
        yes_no(a.seal_on_behalf),
        yes_no(b.seal_on_behalf),
    );
    field(&mut out, "上级单位", &a.parent, &b.parent);
    field(&mut out, "别名", a.aliases.join("、"), b.aliases.join("、"));
    field(&mut out, "电话", &a.phone, &b.phone);
    field(&mut out, "职务", &a.position, &b.position);
    if a.can_handle_parent_unit != b.can_handle_parent_unit {
        out.push(FieldChange {
            label: "承办上级单位",
            before: if a.can_handle_parent_unit {
                "是"
            } else {
                "否"
            }
            .into(),
            after: if b.can_handle_parent_unit {
                "是"
            } else {
                "否"
            }
            .into(),
        });
    }
    field(&mut out, "备注", &a.note, &b.note);
    out
}

/// 逐文种的版式字段变化。
fn profile_set_changes(a: &AppConfig, b: &AppConfig) -> Vec<KindChanges> {
    let mut out = Vec::new();
    for kind in TemplateKind::ALL {
        let changes = profile_changes(&a.profile(kind), &b.profile(kind));
        if !changes.is_empty() {
            out.push(KindChanges { kind, changes });
        }
    }
    out
}

/// 设置项变化（词库与版式单列，这里比较 LM Studio / 输出 / 导出 / 密级规则等）。
fn settings_changes(a: &AppConfig, b: &AppConfig) -> Vec<FieldChange> {
    let mut out = Vec::new();
    field(
        &mut out,
        "接口地址",
        &a.lm_studio.base_url,
        &b.lm_studio.base_url,
    );
    field(&mut out, "模型", &a.lm_studio.model, &b.lm_studio.model);
    field(
        &mut out,
        "API Key",
        masked(&a.lm_studio.api_key),
        masked(&b.lm_studio.api_key),
    );
    field(
        &mut out,
        "温度",
        a.lm_studio.temperature.to_string(),
        b.lm_studio.temperature.to_string(),
    );
    field(
        &mut out,
        "最大输出 Token",
        a.lm_studio.max_tokens.to_string(),
        b.lm_studio.max_tokens.to_string(),
    );
    field(
        &mut out,
        "超时（秒）",
        a.lm_studio.timeout_seconds.to_string(),
        b.lm_studio.timeout_seconds.to_string(),
    );
    field(&mut out, "输出目录", &a.output_dir, &b.output_dir);
    field(
        &mut out,
        "导出：Markdown",
        yes_no(a.export.markdown),
        yes_no(b.export.markdown),
    );
    field(
        &mut out,
        "导出：Word",
        yes_no(a.export.docx),
        yes_no(b.export.docx),
    );
    field(
        &mut out,
        "导出：LaTeX",
        yes_no(a.export.tex),
        yes_no(b.export.tex),
    );
    field(
        &mut out,
        "覆盖同名文件",
        yes_no(a.export.overwrite),
        yes_no(b.export.overwrite),
    );
    field(
        &mut out,
        "生成后自动导出",
        yes_no(a.auto_export),
        yes_no(b.auto_export),
    );
    field(
        &mut out,
        "允许词库外手填",
        yes_no(a.allow_free_text),
        yes_no(b.allow_free_text),
    );
    field(
        &mut out,
        "编辑器显示行号",
        yes_no(a.show_editor_line_numbers),
        yes_no(b.show_editor_line_numbers),
    );
    field(
        &mut out,
        "秘密级上限（年）",
        a.security_rules.secret_max_years.to_string(),
        b.security_rules.secret_max_years.to_string(),
    );
    field(
        &mut out,
        "机密级上限（年）",
        a.security_rules.confidential_max_years.to_string(),
        b.security_rules.confidential_max_years.to_string(),
    );
    field(
        &mut out,
        "绝密级上限（年）",
        a.security_rules.top_secret_max_years.to_string(),
        b.security_rules.top_secret_max_years.to_string(),
    );
    field(
        &mut out,
        "允许“长期”期限",
        yes_no(a.security_rules.allow_long_term),
        yes_no(b.security_rules.allow_long_term),
    );
    out
}

fn joint_contacts(contacts: &[JointContact]) -> String {
    contacts
        .iter()
        .map(|contact| format!("{}：{}", contact.name, contact.phone))
        .collect::<Vec<_>>()
        .join("；")
}

fn yes_no(value: bool) -> String {
    if value {
        "是".to_string()
    } else {
        "否".to_string()
    }
}

fn masked(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        "••••••".to_string()
    }
}

fn field(
    out: &mut Vec<FieldChange>,
    label: &'static str,
    before: impl Into<String>,
    after: impl Into<String>,
) {
    let before = before.into();
    let after = after.into();
    if before != after {
        out.push(FieldChange {
            label,
            before,
            after,
        });
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    /// 各处改动，按出现顺序。
    fn changes(diff: &BodyDiff) -> Vec<&BlockChange> {
        diff.blocks
            .iter()
            .filter_map(|block| match block {
                DiffBlock::Changed(change) => Some(change),
                DiffBlock::Unchanged(_) => None,
            })
            .collect()
    }

    /// 某一侧字级片段里被标出来的文字（拼在一起），用于断言"只标了这几个字"。
    fn marked(spans: &[InlineSpan], kind: SpanKind) -> String {
        spans
            .iter()
            .filter(|span| span.kind == kind)
            .map(|span| span.text.as_str())
            .collect()
    }

    /// 一侧的完整文字：片段拼回原段落。
    fn side_text(spans: &[InlineSpan]) -> String {
        spans.iter().map(|span| span.text.as_str()).collect()
    }

    #[test]
    fn body_diff_marks_only_the_edited_characters() {
        // 公文的一个自然段就是一行；改一个字不该整段标红标绿。
        let old = "# 通知\n\n请某某市教育局于八月十日前报送材料。";
        let new = "# 通知\n\n请某某市教育厅于八月十日前报送材料。";
        let diff = body_diff(old, new);
        let changes = changes(&diff);
        assert_eq!(diff.changed_count, 1);
        assert_eq!(changes[0].kind, ChangeKind::Replace);
        assert_eq!(changes[0].role, BlockRole::Paragraph);
        assert_eq!(marked(&changes[0].before_spans, SpanKind::Removed), "局");
        assert_eq!(marked(&changes[0].after_spans, SpanKind::Added), "厅");
        // 标题段未改动 → 归入折叠的上下文，不计入改动数。
        assert!(
            diff.blocks
                .iter()
                .any(|block| matches!(block, DiffBlock::Unchanged(_)))
        );
    }

    #[test]
    fn body_diff_keeps_direction_old_to_new() {
        let diff = body_diff("旧内容", "新内容");
        let changes = changes(&diff);
        assert_eq!(side_text(&changes[0].before_spans), "旧内容");
        assert_eq!(side_text(&changes[0].after_spans), "新内容");
    }

    #[test]
    fn body_diff_reports_pure_insert_without_pairing() {
        let old = "第一段\n\n第三段";
        let new = "第一段\n\n第二段\n\n第三段";
        let diff = body_diff(old, new);
        let changes = changes(&diff);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Insert);
        assert_eq!(side_text(&changes[0].after_spans), "第二段");
        assert!(changes[0].before_spans.is_empty());
        // 新增段在新版正文里有位置，可以跳源码。
        let range = changes[0].new_range.clone().expect("新增段应有源码位置");
        assert_eq!(&new[range], "第二段");
    }

    #[test]
    fn body_diff_reports_pure_delete_without_source_anchor() {
        let diff = body_diff("第一段\n\n第二段", "第一段");
        let changes = changes(&diff);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Delete);
        assert_eq!(side_text(&changes[0].before_spans), "第二段");
        assert!(changes[0].after_spans.is_empty());
        // 删掉的段在新版里不存在，没有可跳的位置。
        assert!(changes[0].new_range.is_none());
    }

    #[test]
    fn body_diff_does_not_pair_unrelated_paragraphs() {
        // 相似度过低 → 一删一增，而不是硬配成"修改"后逐字标差异。
        let diff = body_diff("关于召开会议的安排", "兹定于下周开展专项检查工作");
        let kinds: Vec<ChangeKind> = changes(&diff).iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [ChangeKind::Delete, ChangeKind::Insert]);
    }

    #[test]
    fn body_diff_does_not_pair_heading_with_paragraph() {
        // 角色不同不配对：标题被删、正文段新增，各自成项。
        let diff = body_diff("## 一、总体要求", "一、总体要求");
        let kinds: Vec<ChangeKind> = changes(&diff).iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [ChangeKind::Delete, ChangeKind::Insert]);
    }

    #[test]
    fn body_diff_marks_single_table_cell() {
        let old = "| 项目 | 数量 |\n| --- | --- |\n| 甲 | 1 |\n| 乙 | 2 |";
        let new = "| 项目 | 数量 |\n| --- | --- |\n| 甲 | 3 |\n| 乙 | 2 |";
        let diff = body_diff(old, new);
        let changes = changes(&diff);
        assert_eq!(diff.changed_count, 1);
        assert_eq!(changes[0].role, BlockRole::TableRow);
        assert_eq!(marked(&changes[0].before_spans, SpanKind::Removed), "1");
        assert_eq!(marked(&changes[0].after_spans, SpanKind::Added), "3");
    }

    #[test]
    fn body_diff_ignores_blank_line_noise() {
        // 段间多敲空行不是内容变更，不该让整篇看起来全变了。
        let diff = body_diff("第一段\n\n第二段", "第一段\n\n\n\n第二段");
        assert_eq!(diff.changed_count, 0);
        assert!(diff.is_empty());
        // 相同文本同理。
        assert!(body_diff("甲\n\n乙", "甲\n\n乙").is_empty());
    }

    #[test]
    fn body_diff_handles_empty_sides() {
        assert!(body_diff("", "").is_empty());
        let added = body_diff("", "新起的一段");
        assert_eq!(added.changed_count, 1);
        assert_eq!(changes(&added)[0].kind, ChangeKind::Insert);
        let removed = body_diff("原有一段\n\n再一段", "");
        assert_eq!(removed.changed_count, 2);
        assert!(
            changes(&removed)
                .iter()
                .all(|change| change.kind == ChangeKind::Delete)
        );
    }

    #[test]
    fn body_diff_line_numbers_point_at_source_lines() {
        let old = "# 标题\n\n第一段\n\n第二段";
        let new = "# 标题\n\n第一段改了\n\n第二段";
        let diff = body_diff(old, new);
        let changes = changes(&diff);
        // 两侧都在第 3 行。
        assert_eq!(changes[0].old_line, 3);
        assert_eq!(changes[0].new_line, 3);
    }

    #[test]
    fn inline_diff_degrades_gracefully_on_huge_paragraphs() {
        // 超出 token 预算时退化为整段替换：只求不卡帧、不 panic。
        let old = "甲".repeat(700);
        let new = format!("{}乙", "甲".repeat(699));
        let diff = body_diff(&old, &new);
        let changes = changes(&diff);
        assert_eq!(changes[0].kind, ChangeKind::Replace);
        assert_eq!(changes[0].before_spans.len(), 1);
        assert_eq!(changes[0].before_spans[0].kind, SpanKind::Removed);
        assert_eq!(changes[0].after_spans.len(), 1);
        assert_eq!(changes[0].after_spans[0].kind, SpanKind::Added);
    }

    #[test]
    fn tokenize_groups_ascii_runs_and_splits_han() {
        assert_eq!(tokenize("2026年8月"), ["2026", "年", "8", "月"]);
        assert_eq!(tokenize("PDF件"), ["PDF", "件"]);
        // 数字整体替换只产生一处标记，不是逐位标记。
        let (before, after) = inline_spans("〔2026〕12号", "〔2026〕13号");
        assert_eq!(marked(&before, SpanKind::Removed), "12");
        assert_eq!(marked(&after, SpanKind::Added), "13");
    }

    #[test]
    fn draft_changes_report_form_fields() {
        let mut a = DraftInput::default();
        a.title_hint = "关于召开会议的通知".into();
        a.profile.document_number = "某教函〔2026〕12号".into();
        let mut b = a.clone();
        b.title_hint = "关于调整会议时间的通知".into();
        b.profile.responsible_unit = "教师工作处".into();
        let changes = draft_changes(&a, &b);
        let labels: Vec<&str> = changes.iter().map(|c| c.label).collect();
        assert!(labels.contains(&"标题提示"));
        assert!(labels.contains(&"承办单位"));
        assert!(
            changes
                .iter()
                .all(|c| c.label != "文种" && c.label != "文号")
        );
    }

    #[test]
    fn identical_drafts_produce_no_changes() {
        let a = DraftInput::default();
        let b = DraftInput::default();
        assert!(draft_changes(&a, &b).is_empty());
    }

    #[test]
    fn config_changes_split_vocabulary_profiles_and_settings() {
        let mut a = AppConfig::default();
        let mut b = AppConfig::default();
        assert!(config_changes(&a, &b).vocabulary.is_empty());
        assert!(config_changes(&a, &b).settings.is_empty());

        // 词库：新增一个单位。
        a.vocabulary.push(VocabularyEntry {
            id: 1,
            category: VocabularyCategory::Unit,
            canonical: "某某省教育厅".into(),
            ..Default::default()
        });
        b.vocabulary.push(VocabularyEntry {
            id: 1,
            category: VocabularyCategory::Unit,
            canonical: "某某省教育厅".into(),
            abbr: "省教育厅".into(),
            seal_on_behalf: true,
            ..Default::default()
        });
        b.vocabulary.push(VocabularyEntry {
            id: 2,
            category: VocabularyCategory::Person,
            canonical: "张三".into(),
            ..Default::default()
        });
        let report = config_changes(&a, &b);
        let actions: Vec<&str> = report.vocabulary.iter().map(|v| v.action).collect();
        assert!(actions.contains(&"新增"));
        assert!(actions.contains(&"修改"));
        assert!(report.vocabulary.iter().any(|change| {
            change
                .changes
                .iter()
                .any(|field| field.label == "是否代章" && field.after == "是")
        }));
        assert_eq!(
            report
                .vocabulary
                .iter()
                .filter(|v| v.action == "新增")
                .count(),
            1
        );

        // 设置：密级上限变化。
        b.security_rules.top_secret_max_years = 40;
        let report = config_changes(&a, &b);
        assert!(
            report
                .settings
                .iter()
                .any(|c| c.label == "绝密级上限（年）")
        );
    }

    #[test]
    fn manuscript_diff_splits_fields_body_and_notes() {
        let old = ContentSnapshot::new(DraftInput::default(), "# 甲".into(), "备注a".into());
        let mut snapshot = DraftInput::default();
        snapshot.title_hint = "关于甲的通知".into();
        let new = ContentSnapshot::new(snapshot, "# 甲\n\n正文".into(), "备注b".into());
        let diff = manuscript_diff(&old, &new);
        // 正文不再混进字段清单，各归各位。
        assert!(diff.fields.iter().all(|change| change.label != "正文"));
        assert!(diff.fields.iter().any(|change| change.label == "标题提示"));
        assert_eq!(diff.body.changed_count, 1);
        let notes = diff.notes.as_ref().expect("备注变了应有一项");
        assert_eq!(
            (notes.before.as_str(), notes.after.as_str()),
            ("备注a", "备注b")
        );
        assert_eq!(diff.total(), 3);
        assert!(!diff.is_empty());
    }

    #[test]
    fn manuscript_diff_of_identical_content_is_empty() {
        let one = ContentSnapshot::new(DraftInput::default(), "# 甲\n\n正文".into(), "备注".into());
        let two = one.clone();
        let diff = manuscript_diff(&one, &two);
        assert!(diff.is_empty());
        assert_eq!(diff.total(), 0);
        assert!(diff.notes.is_none());
    }

    #[test]
    fn version_record_converts_to_snapshot() {
        let record = VersionRecord {
            version_number: 1,
            name: "v".into(),
            comment: String::new(),
            snapshot: DraftInput::default(),
            content_markdown: "# 甲".into(),
            notes: "备注".into(),
            created_at: String::new(),
        };
        let snapshot: ContentSnapshot = record.into();
        assert_eq!(snapshot.content_markdown, "# 甲");
        assert_eq!(snapshot.notes, "备注");
    }
}
