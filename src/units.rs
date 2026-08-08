//! 单位层级解析：标准词库中单位词条可设置 `parent`（上级单位）与 `abbr`（简称），
//! 人员词条可设置 `unit`（所属单位）。生成提示词与 DOCX/LaTeX 导出共用同一套解析，
//! 确保界面上看到的显示结果与最终成品一致。
//!
//! 规格 §2.1/2.2/2.3/3.1：
//! - 选下级单位时自动补全上级单位名称（“中央网信办新闻舆论处”）；
//! - 同属一个上级的下级单位用顿号连接且后续不重复上级，跨上级单位用逗号分隔；
//! - 人员绑定所属单位，按承办单位过滤（“人随事走”）；获授权人员也可承办其上级单位；
//! - 公函落款用全称、承办单位用简称、电话通知落款用简称且少于 5 字时逐字加空格。

use crate::models::{VocabularyCategory, VocabularyEntry};
use std::collections::HashMap;

/// 层级编码每一级占用的位数。超过 99 个同级单位时该级会自动加宽，
/// 因此解析上下级关系一律用“最长前缀匹配”，不要按固定位数切片。
pub const CODE_WIDTH: usize = 2;

/// 单位层级的最大深度保护值，防止上级关系成环时无限递归。
const MAX_DEPTH: usize = 16;

/// 下一个可用的词条 id。
pub fn next_id(vocab: &[VocabularyEntry]) -> u64 {
    vocab.iter().map(|entry| entry.id).max().unwrap_or(0) + 1
}

/// 给尚未编号（`id == 0`）的词条补号。
pub fn assign_missing_ids(vocab: &mut [VocabularyEntry]) {
    let mut next = vocab.iter().map(|entry| entry.id).max().unwrap_or(0) + 1;
    for entry in vocab.iter_mut() {
        if entry.id == 0 {
            entry.id = next;
            next += 1;
        }
    }
}

/// 把词库整理成与树形界面一致的顺序，并重新生成层级编码：
/// 单位深度优先排列，每个单位后面紧跟它的人员，再排下级单位。
/// 单位使用层级编码；人员在各自所属单位内使用独立的两位顺序编码（01、02……）。
/// 上级单位不存在或自环的单位按顶层处理，无归属人员排在最后。
/// 词库顺序同时决定起草页下拉框和多选字段的输出顺序，所以任何增删改动之后都要调用。
pub fn normalize(vocab: &mut Vec<VocabularyEntry>) {
    normalize_preserving_codes(vocab);
}

/// 把词库整理成与树形界面一致的顺序：**用户填的层级编码原样保留**，
/// 只有编码为空的单位才在 walk 中按位置生成。
/// 旧版的 `normalize` 全量重排会导致用户手工维护的编码被覆盖；本函数保留它们的字面值，
/// 树形顺序仍然由编码字典序给出（用户填的合法编码只要彼此前缀关系正确就直接生效）。
///
/// 第一步仍把空 code 的单位分配 `@{id}` 占位，是为了在排序与 walk 时把它们聚合在一起；
/// 占位编码以 `@` 结尾，不会与用户填的合法编码（`^[A-Za-z0-9._\-]+$`）冲突。
pub fn normalize_preserving_codes(vocab: &mut Vec<VocabularyEntry>) {
    assign_missing_ids(vocab);
    let mut source = std::mem::take(vocab);

    // 旧配置可能完全没有编码。先给空 code 单位放入临时占位编码，
    // 后续 walk 时才按位置生成正式编码；用户填的合法编码不会被替换。
    for entry in source
        .iter_mut()
        .filter(|entry| entry.category == VocabularyCategory::Unit && entry.code.trim().is_empty())
    {
        entry.code = format!("@{}", entry.id);
    }

    // `parent` / `unit` 现在以单位编码关联。兼容旧配置：若字段不是现有编码，
    // 仅在单位名称唯一时把名称迁移为编码；同名旧数据无法可靠猜测，按未挂靠处理。
    let unit_codes = source
        .iter()
        .filter(|entry| entry.category == VocabularyCategory::Unit)
        .map(|entry| entry.code.trim())
        .filter(|code| !code.is_empty())
        .collect::<Vec<_>>();
    let resolve_ref = |value: &str| -> Option<String> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if unit_codes.contains(&value) {
            return Some(value.to_string());
        }
        let matches = source
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .filter(|entry| entry.canonical.trim() == value)
            .map(|entry| entry.code.trim())
            .filter(|code| !code.is_empty())
            .collect::<Vec<_>>();
        (matches.len() == 1).then(|| matches[0].to_string())
    };

    let mut roots: Vec<usize> = Vec::new();
    let mut children: HashMap<String, Vec<usize>> = HashMap::new();
    let mut people: HashMap<String, Vec<usize>> = HashMap::new();
    let mut orphan_people: Vec<usize> = Vec::new();

    for (index, entry) in source.iter().enumerate() {
        match entry.category {
            VocabularyCategory::Unit => {
                let parent = resolve_ref(&entry.parent);
                let self_code = entry.code.trim();
                if let Some(parent) = parent.filter(|parent| parent != self_code) {
                    children.entry(parent).or_default().push(index);
                } else {
                    roots.push(index);
                }
            }
            VocabularyCategory::Person => {
                if let Some(unit) = resolve_ref(&entry.unit) {
                    people.entry(unit).or_default().push(index);
                } else {
                    orphan_people.push(index);
                }
            }
        }
    }

    let mut slots: Vec<Option<VocabularyEntry>> = source.into_iter().map(Some).collect();
    let mut output = Vec::with_capacity(slots.len());
    let mut root_order = 0;
    let mut place_root = |index: usize,
                          slots: &mut Vec<Option<VocabularyEntry>>,
                          output: &mut Vec<VocabularyEntry>| {
        if slots[index].is_none() {
            return;
        }
        let placeholder = format!("@{}", slots[index].as_ref().unwrap().id);
        let is_placeholder = slots[index].as_ref().unwrap().code.trim() == placeholder;
        if is_placeholder {
            walk(
                index,
                &format!("{root_order:0width$}", width = CODE_WIDTH),
                slots,
                &children,
                &people,
                output,
                None,
            );
        } else {
            // 用户已填真实编码：保留它的字面值进入 walk，但 child/people
            // 仍按 placeholder 收集的人员树走完。
            let code = slots[index].as_ref().unwrap().code.trim().to_string();
            walk_preserve_code(index, &code, slots, &children, &people, output, None);
        }
        root_order += 1;
    };
    for index in roots {
        place_root(index, &mut slots, &mut output);
    }
    // 上级关系成环 / 父级缺失的单位不会被上面的深度优先遍历访问到，兜底按顶层补回。
    for index in 0..slots.len() {
        if !orphan_people.contains(&index) {
            place_root(index, &mut slots, &mut output);
        }
    }
    for index in orphan_people {
        if let Some(mut entry) = slots[index].take() {
            entry.code.clear();
            output.push(entry);
        }
    }
    *vocab = output;
    // 整理完后再按编码前缀回填 parent（占位编码已被替换为真实编码，前缀关系可直接读出）。
    rebuild_parents_from_codes(vocab);
}

fn walk(
    index: usize,
    code: &str,
    slots: &mut [Option<VocabularyEntry>],
    children: &HashMap<String, Vec<usize>>,
    people: &HashMap<String, Vec<usize>>,
    output: &mut Vec<VocabularyEntry>,
    parent_code: Option<&str>,
) {
    let Some(mut entry) = slots[index].take() else {
        return;
    };
    let old_code = entry.code.trim().to_string();
    entry.code = code.to_string();
    entry.parent = parent_code.unwrap_or_default().to_string();
    output.push(entry);

    // 人员是单位的末端子节点，编码只表示其在本单位内的顺序。
    for (order, person) in people.get(&old_code).into_iter().flatten().enumerate() {
        if let Some(mut entry) = slots[*person].take() {
            entry.code = format!("{:0width$}", order + 1, width = CODE_WIDTH);
            entry.unit = code.to_string();
            output.push(entry);
        }
    }
    for (order, child) in children.get(&old_code).into_iter().flatten().enumerate() {
        walk(
            *child,
            &format!("{code}{:0width$}", order + 1, width = CODE_WIDTH),
            slots,
            children,
            people,
            output,
            Some(code),
        );
    }
}

/// 用户填的合法编码：原样保留。child 子树里若有空 code 单位（占位），按位置生成；
/// 若子单位也有真实编码，则同样保留。下级人员的所属单位由 walk 路径上的真实编码负责写回。
fn walk_preserve_code(
    index: usize,
    code: &str,
    slots: &mut [Option<VocabularyEntry>],
    children: &HashMap<String, Vec<usize>>,
    people: &HashMap<String, Vec<usize>>,
    output: &mut Vec<VocabularyEntry>,
    parent_code: Option<&str>,
) {
    let Some(mut entry) = slots[index].take() else {
        return;
    };
    let old_code = entry.code.trim().to_string();
    let code = if old_code.starts_with('@') {
        // 占位：按位置生成（仅顶层 / 父级缺失时由 place_root 决定）
        code.to_string()
    } else {
        old_code.clone()
    };
    entry.code = code.clone();
    entry.parent = parent_code.unwrap_or_default().to_string();
    output.push(entry);

    for (order, person) in people.get(&old_code).into_iter().flatten().enumerate() {
        if let Some(mut entry) = slots[*person].take() {
            entry.code = format!("{:0width$}", order + 1, width = CODE_WIDTH);
            entry.unit = code.clone();
            output.push(entry);
        }
    }
    for (order, child) in children.get(&old_code).into_iter().flatten().enumerate() {
        let child_placeholder = slots[*child].as_ref().map(|c| c.code.trim().to_string());
        let child_is_placeholder = child_placeholder
            .as_deref()
            .is_some_and(|c| c.starts_with('@'));
        if child_is_placeholder {
            walk(
                *child,
                &format!("{code}{:0width$}", order + 1, width = CODE_WIDTH),
                slots,
                children,
                people,
                output,
                Some(&code),
            );
        } else {
            walk_preserve_code(*child, "", slots, children, people, output, Some(&code));
        }
    }
}

/// 依据层级编码重建上级关系：某个单位的上级，是编码为它最长真前缀的那个单位。
/// Markdown 导入时用它把表格里的编码列还原成树。
pub fn rebuild_parents_from_codes(vocab: &mut [VocabularyEntry]) {
    let units: Vec<(String, String)> = vocab
        .iter()
        .filter(|entry| entry.category == VocabularyCategory::Unit)
        .map(|entry| {
            (
                entry.code.trim().to_string(),
                entry.canonical.trim().to_string(),
            )
        })
        .filter(|(code, _)| !code.is_empty())
        .collect();
    for entry in vocab
        .iter_mut()
        .filter(|entry| entry.category == VocabularyCategory::Unit)
    {
        let code = entry.code.trim().to_string();
        if code.is_empty() {
            continue;
        }
        let parent = units
            .iter()
            .filter(|(candidate, _)| {
                candidate.len() < code.len() && code.starts_with(candidate.as_str())
            })
            .max_by_key(|(candidate, _)| candidate.len())
            .map(|(candidate, _)| candidate.clone());
        if let Some(parent) = parent {
            entry.parent = parent;
        }
    }
    // 兼容旧模板：人员表若填写的是所属单位名称，只在名称唯一时迁移为编码。
    for entry in vocab
        .iter_mut()
        .filter(|entry| entry.category == VocabularyCategory::Person)
    {
        let value = entry.unit.trim();
        if value.is_empty() || units.iter().any(|(code, _)| code == value) {
            continue;
        }
        let matches = units
            .iter()
            .filter(|(_, name)| name == value)
            .map(|(code, _)| code)
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            entry.unit = matches[0].clone();
        }
    }
}

/// 词库中出现过的机关代字，按单位的层级顺序去重。
pub fn department_codes(vocab: &[VocabularyEntry]) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for entry in vocab
        .iter()
        .filter(|entry| entry.category == VocabularyCategory::Unit)
    {
        let code = entry.department_code.trim();
        if !code.is_empty() && !codes.iter().any(|existing| existing == code) {
            codes.push(code.to_string());
        }
    }
    codes
}

/// 指定单位的直属下级单位在词库中的下标，按词库顺序。
pub fn child_units(vocab: &[VocabularyEntry], unit_code: &str) -> Vec<usize> {
    let unit_code = unit_code.trim();
    vocab
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.category == VocabularyCategory::Unit && entry.parent.trim() == unit_code
        })
        .map(|(index, _)| index)
        .collect()
}

/// 指定单位的直属人员在词库中的下标，按词库顺序。
pub fn unit_people(vocab: &[VocabularyEntry], unit_code: &str) -> Vec<usize> {
    let unit_code = unit_code.trim();
    vocab
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.category == VocabularyCategory::Person && entry.unit.trim() == unit_code
        })
        .map(|(index, _)| index)
        .collect()
}

/// 删除单位时要一并清掉的整棵子树：单位自身、全部下级单位，以及它们名下的人员。
pub fn subtree_indices(vocab: &[VocabularyEntry], index: usize) -> Vec<usize> {
    let mut collected = vec![index];
    let mut frontier = vec![vocab[index].code.trim().to_string()];
    let mut guard = 0;
    while let Some(code) = frontier.pop() {
        guard += 1;
        if guard > vocab.len() + MAX_DEPTH {
            break;
        }
        for child in child_units(vocab, &code) {
            if !collected.contains(&child) {
                collected.push(child);
                frontier.push(vocab[child].code.trim().to_string());
            }
        }
        for person in unit_people(vocab, &code) {
            if !collected.contains(&person) {
                collected.push(person);
            }
        }
    }
    collected.sort_unstable();
    collected
}

/// 词库层级解析器。`vocab` 仅借用，不持有。
#[derive(Debug, Clone, Copy)]
pub struct UnitDisplay<'a> {
    vocab: &'a [VocabularyEntry],
}

impl<'a> UnitDisplay<'a> {
    pub fn new(vocab: &'a [VocabularyEntry]) -> Self {
        Self { vocab }
    }

    fn find(&self, key: &str) -> Option<&VocabularyEntry> {
        let key = key.trim();
        self.vocab
            .iter()
            .find(|entry| entry.category == VocabularyCategory::Unit && entry.code.trim() == key)
            .or_else(|| {
                self.vocab.iter().find(|entry| {
                    entry.category == VocabularyCategory::Unit && entry.canonical.trim() == key
                })
            })
    }

    fn name_entry(entry: &VocabularyEntry, external: bool) -> &str {
        if external && !entry.external_name.trim().is_empty() {
            entry.external_name.trim()
        } else {
            entry.canonical.trim()
        }
    }

    fn full_name_entry(
        &self,
        entry: &VocabularyEntry,
        visited: &mut Vec<String>,
        external: bool,
    ) -> String {
        let code = entry.code.trim();
        if !code.is_empty() && visited.iter().any(|value| value == code) {
            return Self::name_entry(entry, external).to_string();
        }
        if !code.is_empty() {
            visited.push(code.to_string());
        }
        let own = Self::name_entry(entry, external);
        let parent = entry.parent.trim();
        let result = if parent.is_empty() {
            own.to_string()
        } else if let Some(parent_entry) = self.find(parent) {
            let parent_full = self.full_name_entry(parent_entry, visited, external);
            if parent_full.is_empty() {
                own.to_string()
            } else {
                format!("{parent_full}{own}")
            }
        } else {
            own.to_string()
        };
        if !code.is_empty() {
            visited.pop();
        }
        result
    }

    /// 完整名称：递归上溯上级单位，得到“上级全称 + 本级名称”。
    /// 顶层单位直接返回规范名称；词库中查不到的按原样返回。
    pub fn full_name(&self, key: &str) -> String {
        self.full_name_for(key, false)
    }

    /// 按行文范围展开完整名称。外部模式下每一级优先使用外部名称，未设置则回退正常名称。
    pub fn full_name_for(&self, key: &str, external: bool) -> String {
        let Some(entry) = self.find(key) else {
            return key.trim().to_string();
        };
        self.full_name_entry(entry, &mut Vec::new(), external)
    }

    /// 本级显示名称，不拼接上级。
    pub fn name_for(&self, key: &str, external: bool) -> String {
        self.find(key)
            .map(|entry| Self::name_entry(entry, external).to_string())
            .unwrap_or_else(|| key.trim().to_string())
    }

    /// 上级单位的规范名称；顶层单位或词库中查不到时返回空串。
    pub fn parent_of(&self, key: &str) -> String {
        self.find(key)
            .and_then(|entry| self.find(entry.parent.trim()))
            .map(|entry| entry.canonical.trim().to_string())
            .unwrap_or_default()
    }

    /// 把层级编码或旧的单位名称统一解析成当前单位编码。
    pub fn code_of(&self, key: &str) -> String {
        self.find(key)
            .map(|entry| entry.code.trim().to_string())
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| key.trim().to_string())
    }

    /// 两个引用是否指向同一单位；兼容“编码”和旧“名称”混用的配置。
    pub fn same_unit(&self, left: &str, right: &str) -> bool {
        !left.trim().is_empty()
            && !right.trim().is_empty()
            && self.code_of(left) == self.code_of(right)
    }

    /// 简称：优先取词条 `abbr`，空则回落规范名称。
    pub fn abbr(&self, key: &str) -> String {
        self.find(key)
            .and_then(|entry| {
                let abbr = entry.abbr.trim();
                (!abbr.is_empty()).then(|| abbr.to_string())
            })
            .unwrap_or_else(|| key.trim().to_string())
    }

    /// 电话通知落款用简称：字符数少于 5 时逐字插入半角空格（“网信办” → “网 信 办”）。
    pub fn abbr_spaced(&self, key: &str) -> String {
        let abbr = self.abbr(key);
        let chars = abbr.chars().collect::<Vec<_>>();
        if chars.len() < 5 {
            chars
                .iter()
                .map(char::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            abbr
        }
    }

    /// 规格 §2.2 层级展开显示：
    /// - 选择下级单位时补全上级全称；
    /// - 一般并列单位一律用顿号“、”连接；
    /// - 只有同属一个上级的多个下级并列、后续下级省略上级名称时，才把这一组
    ///   视为一个复合单位组，并用逗号“，”与其他发文单位/复合组分隔；
    /// - 顶层单位和只有一个下级入选的单位都按一般并列单位处理。
    ///
    /// 分组与组内顺序按词库顺序（调用方传入的 canonicals 已按词库排序）。
    #[cfg(test)]
    pub fn join_hierarchical(&self, canonicals: &[String]) -> String {
        self.join_hierarchical_for(canonicals, false)
    }

    /// 按行文范围展开并合并单位层级；简称逻辑不经过这里，因此不受外部名称影响。
    pub fn join_hierarchical_for(&self, canonicals: &[String], external: bool) -> String {
        let mut items: Vec<(String, Option<String>, String, String)> = Vec::new();
        for canonical in canonicals {
            let canonical = canonical.trim();
            if canonical.is_empty() {
                continue;
            }
            let entry = self.find(canonical);
            let has_parent = entry.map(|e| !e.parent.trim().is_empty()).unwrap_or(false);
            let parent_key = entry
                .filter(|_| has_parent)
                .map(|entry| self.code_of(entry.parent.trim()));
            let own_name = if has_parent {
                entry
                    .map(|e| Self::name_entry(e, external).to_string())
                    .unwrap_or_else(|| canonical.to_string())
            } else {
                self.full_name_for(canonical, external)
            };
            let full = self.full_name_for(canonical, external);
            if !items.iter().any(|(key, _, _, _)| key == canonical) {
                items.push((canonical.to_string(), parent_key, full, own_name));
            }
        }

        let sibling_count = |parent: &str| {
            items
                .iter()
                .filter(|(_, item_parent, _, _)| item_parent.as_deref() == Some(parent))
                .count()
        };
        let has_compound_group = items.iter().any(|(_, parent, _, _)| {
            parent
                .as_deref()
                .is_some_and(|parent| sibling_count(parent) > 1)
        });
        if !has_compound_group {
            return items
                .iter()
                .map(|(_, _, full, _)| full.as_str())
                .collect::<Vec<_>>()
                .join("、");
        }

        let mut emitted_parents = Vec::new();
        let mut groups = Vec::new();
        for (_, parent, full, _) in &items {
            let Some(parent) = parent.as_deref() else {
                groups.push(full.to_string());
                continue;
            };
            if sibling_count(parent) <= 1 {
                groups.push(full.to_string());
                continue;
            }
            if emitted_parents.iter().any(|seen| seen == parent) {
                continue;
            }
            emitted_parents.push(parent.to_string());
            let siblings = items
                .iter()
                .filter(|(_, item_parent, _, _)| item_parent.as_deref() == Some(parent))
                .enumerate()
                .map(|(index, (_, _, full, own))| {
                    if index == 0 {
                        full.as_str()
                    } else {
                        own.as_str()
                    }
                })
                .collect::<Vec<_>>()
                .join("、");
            groups.push(siblings);
        }
        groups.join("，")
    }

    /// 返回给定单位及其上级链中尚未维护外部名称的单位，按首次出现顺序去重。
    pub fn missing_external_names(&self, keys: &[String]) -> Vec<String> {
        let mut missing = Vec::new();
        let mut seen = Vec::new();
        for key in keys {
            let Some(mut entry) = self.find(key) else {
                continue;
            };
            for _ in 0..MAX_DEPTH {
                let identity = if entry.code.trim().is_empty() {
                    entry.canonical.trim()
                } else {
                    entry.code.trim()
                };
                if !seen.iter().any(|value| value == identity) {
                    seen.push(identity.to_string());
                    if entry.external_name.trim().is_empty() {
                        let key = if entry.code.trim().is_empty() {
                            entry.canonical.trim()
                        } else {
                            entry.code.trim()
                        };
                        missing.push(self.full_name(key));
                    }
                }
                let parent = entry.parent.trim();
                if parent.is_empty() {
                    break;
                }
                let Some(parent_entry) = self.find(parent) else {
                    break;
                };
                entry = parent_entry;
            }
        }
        missing
    }

    /// 返回属于指定单位的人员（姓名、电话）对，用于“人随事走”过滤。
    pub fn people_of(&self, unit: &str) -> Vec<(String, String)> {
        let unit = self
            .find(unit)
            .map(|entry| entry.code.trim())
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| unit.trim());
        self.vocab
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Person)
            .filter(|entry| entry.unit.trim() == unit)
            .map(|entry| {
                (
                    entry.canonical.trim().to_string(),
                    entry.phone.trim().to_string(),
                )
            })
            .filter(|(name, _)| !name.is_empty())
            .collect()
    }

    /// 返回可在指定单位公函版记中担任联系人的人员。
    ///
    /// 直属人员始终可选；勾选“承办上级单位”的人员，还可在其所属单位的任一级
    /// 上级单位中被选中。该权限只用于公函版记，不影响白头件呈报领导过滤。
    pub fn responsible_people_of(&self, unit: &str) -> Vec<(String, String)> {
        let selected = self
            .find(unit)
            .map(|entry| entry.code.trim())
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| unit.trim());

        self.vocab
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Person)
            .filter(|person| {
                let person_unit = self
                    .find(&person.unit)
                    .map(|entry| entry.code.trim())
                    .filter(|code| !code.is_empty())
                    .unwrap_or_else(|| person.unit.trim());
                if person_unit == selected {
                    return true;
                }
                if !person.can_handle_parent_unit || selected.is_empty() {
                    return false;
                }

                let mut current = person_unit;
                for _ in 0..MAX_DEPTH {
                    let Some(entry) = self.find(current) else {
                        return false;
                    };
                    let parent = entry.parent.trim();
                    if parent.is_empty() || parent == current {
                        return false;
                    }
                    if parent == selected {
                        return true;
                    }
                    current = parent;
                }
                false
            })
            .map(|entry| {
                (
                    entry.canonical.trim().to_string(),
                    entry.phone.trim().to_string(),
                )
            })
            .filter(|(name, _)| !name.is_empty())
            .collect()
    }

    /// 白头件呈报领导称谓：按词库中的人员顺序，把相同职务合并为一组。
    /// 每人只取姓名首字作为姓；同职务用顿号，不同职务用逗号，职务放在本组末尾。
    /// 词库外的手填值或未维护职务的人员保持原样，并以顿号连接。
    pub fn reporting_leaders(&self, selected: &str) -> String {
        let selected = crate::models::split_units(selected);
        if selected.is_empty() {
            return String::new();
        }

        let mut ordered = self
            .vocab
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Person)
            .filter(|entry| selected.iter().any(|name| name == entry.canonical.trim()))
            .collect::<Vec<_>>();
        let known = ordered
            .iter()
            .map(|entry| entry.canonical.trim())
            .collect::<Vec<_>>();

        // 旧配置中的词库外值保持勾选/手填时的相对顺序，并排在已编码人员之后。
        let unknown = selected
            .iter()
            .filter(|name| !known.contains(&name.as_str()))
            .cloned()
            .collect::<Vec<_>>();

        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for entry in ordered.drain(..) {
            let name = entry.canonical.trim();
            let position = entry.position.trim();
            if position.is_empty() {
                match groups.iter_mut().find(|(title, _)| title.is_empty()) {
                    Some((_, names)) => names.push(name.to_string()),
                    None => groups.push((String::new(), vec![name.to_string()])),
                }
                continue;
            }
            let surname = person_surname(name);
            match groups.iter_mut().find(|(title, _)| title == position) {
                Some((_, names)) => names.push(surname),
                None => groups.push((position.to_string(), vec![surname])),
            }
        }
        for name in unknown {
            match groups.iter_mut().find(|(title, _)| title.is_empty()) {
                Some((_, names)) => names.push(name),
                None => groups.push((String::new(), vec![name])),
            }
        }

        groups
            .into_iter()
            .map(|(position, names)| format!("{}{position}", names.join("、")))
            .collect::<Vec<_>>()
            .join("，")
    }

    /// 单位绑定的机关代字。处室通常不自编代字，用的是所在机关的代字，
    /// 因此本级没有绑定时逐级向上找；整棵链都没有绑定才返回空串。
    pub fn department_code_of(&self, key: &str) -> String {
        let mut current = key.trim().to_string();
        for _ in 0..MAX_DEPTH {
            let Some(entry) = self.find(&current) else {
                return String::new();
            };
            let code = entry.department_code.trim();
            if !code.is_empty() {
                return code.to_string();
            }
            let parent = entry.parent.trim();
            if parent.is_empty() || parent == current {
                return String::new();
            }
            current = parent.to_string();
        }
        String::new()
    }

    /// 指定单位是否配置为代章单位。代章是单位自身属性，不随上级单位继承。
    pub fn seals_on_behalf(&self, key: &str) -> bool {
        self.find(key).is_some_and(|entry| entry.seal_on_behalf)
    }
}

/// 常见复姓保留两个字，其余姓名取首字。
fn person_surname(name: &str) -> String {
    const COMPOUND_SURNAMES: &[&str] = &[
        "欧阳", "司马", "上官", "诸葛", "东方", "皇甫", "尉迟", "公孙", "慕容", "令狐", "宇文",
        "长孙", "司徒", "司空", "夏侯", "独孤", "南宫", "万俟", "闻人", "赫连", "澹台", "濮阳",
        "淳于", "单于", "申屠", "轩辕", "钟离", "端木", "百里", "呼延",
    ];
    let name = name.trim();
    COMPOUND_SURNAMES
        .iter()
        .find(|surname| name.starts_with(**surname))
        .map(|surname| (*surname).to_string())
        .or_else(|| name.chars().next().map(|ch| ch.to_string()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::VocabularyEntry;

    fn vocab() -> Vec<VocabularyEntry> {
        vec![
            VocabularyEntry {
                canonical: "中央网信办".into(),
                abbr: "网信办".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "新闻舆论处".into(),
                parent: "中央网信办".into(),
                abbr: "新舆处".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "信访接待处".into(),
                parent: "中央网信办".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "中央宣传部".into(),
                abbr: "中宣部".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "网络信息处".into(),
                parent: "中央宣传部".into(),
                abbr: "网信处".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "张三".into(),
                category: VocabularyCategory::Person,
                unit: "新闻舆论处".into(),
                phone: "010-1".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "李四".into(),
                category: VocabularyCategory::Person,
                unit: "信访接待处".into(),
                phone: "010-2".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn full_name_expands_the_parent_prefix() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        assert_eq!(display.full_name("新闻舆论处"), "中央网信办新闻舆论处");
        assert_eq!(display.full_name("中央网信办"), "中央网信办");
        assert_eq!(display.full_name("网络信息处"), "中央宣传部网络信息处");
        assert_eq!(display.full_name("不在词库的单位"), "不在词库的单位");
    }

    #[test]
    fn external_names_expand_with_fallback_and_do_not_change_abbr() {
        let mut v = vocab();
        v[0].external_name = "国家互联网信息办公室".into();
        v[1].external_name = "新闻传播管理处".into();
        let display = UnitDisplay::new(&v);
        assert_eq!(
            display.full_name_for("新闻舆论处", true),
            "国家互联网信息办公室新闻传播管理处"
        );
        // 本级未维护外部名称时回退正常名称；上级仍使用已维护的外部名称。
        assert_eq!(
            display.full_name_for("信访接待处", true),
            "国家互联网信息办公室信访接待处"
        );
        assert_eq!(display.abbr("新闻舆论处"), "新舆处");
        assert_eq!(
            display.missing_external_names(&["信访接待处".into()]),
            ["中央网信办信访接待处"]
        );
    }

    #[test]
    fn abbr_falls_back_to_canonical() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        assert_eq!(display.abbr("新闻舆论处"), "新舆处");
        assert_eq!(display.abbr("信访接待处"), "信访接待处");
        assert_eq!(display.abbr("中央宣传部"), "中宣部");
    }

    #[test]
    fn abbr_spaced_adds_spaces_below_five_chars() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        // 简称“中宣部”3 字，逐字加半角空格。
        assert_eq!(display.abbr_spaced("中央宣传部"), "中 宣 部");
        // 无简称且满 5 字的单位不插空格。
        assert_eq!(display.abbr_spaced("信访接待处"), "信访接待处");
    }

    #[test]
    fn same_parent_children_join_with_ton() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        let selected = vec!["新闻舆论处".to_string(), "信访接待处".to_string()];
        assert_eq!(
            display.join_hierarchical(&selected),
            "中央网信办新闻舆论处、信访接待处"
        );
    }

    #[test]
    fn different_parents_join_with_comma() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        let selected = vec![
            "新闻舆论处".to_string(),
            "信访接待处".to_string(),
            "网络信息处".to_string(),
        ];
        assert_eq!(
            display.join_hierarchical(&selected),
            "中央网信办新闻舆论处、信访接待处，中央宣传部网络信息处"
        );
    }

    #[test]
    fn ordinary_units_use_ideographic_comma() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        let selected = vec!["中央网信办".to_string(), "网络信息处".to_string()];
        assert_eq!(
            display.join_hierarchical(&selected),
            "中央网信办、中央宣传部网络信息处"
        );
    }

    #[test]
    fn compound_child_group_uses_comma_between_issuing_units() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        let selected = vec![
            "新闻舆论处".to_string(),
            "信访接待处".to_string(),
            "中央宣传部".to_string(),
        ];
        assert_eq!(
            display.join_hierarchical(&selected),
            "中央网信办新闻舆论处、信访接待处，中央宣传部"
        );
    }

    #[test]
    fn people_are_filtered_by_their_unit() {
        let v = vocab();
        let display = UnitDisplay::new(&v);
        assert_eq!(
            display.people_of("新闻舆论处"),
            vec![("张三".to_string(), "010-1".to_string())]
        );
        assert!(display.people_of("不存在").is_empty());
    }

    #[test]
    fn authorized_people_can_handle_parent_units_in_records() {
        let mut v = vocab();
        let zhang = v
            .iter_mut()
            .find(|entry| entry.canonical == "张三")
            .unwrap();
        zhang.can_handle_parent_unit = true;
        let display = UnitDisplay::new(&v);

        assert_eq!(
            display.responsible_people_of("新闻舆论处"),
            vec![("张三".to_string(), "010-1".to_string())]
        );
        assert_eq!(
            display.responsible_people_of("中央网信办"),
            vec![("张三".to_string(), "010-1".to_string())]
        );
        assert!(display.responsible_people_of("中央宣传部").is_empty());
        // 原有直属人员过滤保持不变，供白头件呈报领导使用。
        assert!(display.people_of("中央网信办").is_empty());
    }

    #[test]
    fn normalize_orders_depth_first_and_numbers_the_tree() {
        let mut list = vocab();
        normalize(&mut list);
        let order = list
            .iter()
            .map(|entry| format!("{}{}", entry.code, entry.canonical))
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            [
                "00中央网信办",
                "0001新闻舆论处",
                "01张三",
                "0002信访接待处",
                "01李四",
                "01中央宣传部",
                "0101网络信息处",
            ]
        );
        // 层级编码按字典序排列即得到树形顺序。
        let mut codes = list
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .map(|entry| entry.code.clone())
            .collect::<Vec<_>>();
        let sorted = {
            let mut copy = codes.clone();
            copy.sort();
            copy
        };
        codes.dedup();
        assert_eq!(codes, sorted);
    }

    #[test]
    fn people_are_numbered_within_each_unit_and_leader_titles_are_grouped() {
        let mut list = vec![
            VocabularyEntry {
                canonical: "办公室".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Person,
                canonical: "王庭".into(),
                position: "主任".into(),
                unit: "办公室".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Person,
                canonical: "文强".into(),
                position: "副主任".into(),
                unit: "办公室".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Person,
                canonical: "夏天".into(),
                position: "副主任".into(),
                unit: "办公室".into(),
                ..Default::default()
            },
        ];
        normalize(&mut list);
        let people = list
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Person)
            .map(|entry| (entry.code.as_str(), entry.canonical.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(people, [("01", "王庭"), ("02", "文强"), ("03", "夏天")]);

        // 勾选顺序不影响结果；按人员编码排序，再按职务归组。
        let display = UnitDisplay::new(&list);
        assert_eq!(
            display.reporting_leaders("文强、王庭、夏天"),
            "王主任，文、夏副主任"
        );
        assert_eq!(person_surname("欧阳明"), "欧阳");
    }

    #[test]
    fn normalize_keeps_orphans_and_breaks_cycles() {
        let mut list = vec![
            VocabularyEntry {
                canonical: "甲".into(),
                parent: "乙".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "乙".into(),
                parent: "甲".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "丙处".into(),
                parent: "查无此单位".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "无归属的人".into(),
                category: VocabularyCategory::Person,
                ..Default::default()
            },
        ];
        normalize(&mut list);
        assert_eq!(list.len(), 4);
        // 上级查不到的单位按顶层处理，成环的两个单位也都保留下来。
        assert!(list.iter().all(|entry| entry.id != 0));
        assert_eq!(list.last().unwrap().canonical, "无归属的人");
        assert!(list.last().unwrap().code.is_empty());
    }

    #[test]
    fn parents_are_rebuilt_from_hierarchy_codes() {
        let mut list = vec![
            VocabularyEntry {
                canonical: "中央网信办".into(),
                code: "00".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "新闻舆论处".into(),
                code: "0001".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "舆情组".into(),
                code: "000101".into(),
                ..Default::default()
            },
        ];
        rebuild_parents_from_codes(&mut list);
        assert_eq!(list[0].parent, "");
        assert_eq!(list[1].parent, "00");
        assert_eq!(list[2].parent, "0001");
    }

    #[test]
    fn duplicate_names_in_deep_branches_are_linked_by_code() {
        let mut list = vec![
            VocabularyEntry {
                code: "00".into(),
                canonical: "甲厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "0001".into(),
                canonical: "办公室".into(),
                parent: "00".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "000101".into(),
                canonical: "综合科".into(),
                parent: "0001".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "01".into(),
                canonical: "乙厅".into(),
                ..Default::default()
            },
            VocabularyEntry {
                code: "0101".into(),
                canonical: "办公室".into(),
                parent: "01".into(),
                ..Default::default()
            },
            VocabularyEntry {
                category: VocabularyCategory::Person,
                code: "01".into(),
                canonical: "张三".into(),
                unit: "0101".into(),
                ..Default::default()
            },
        ];
        normalize(&mut list);
        let display = UnitDisplay::new(&list);
        assert_eq!(display.full_name("000101"), "甲厅办公室综合科");
        assert_eq!(display.full_name("0101"), "乙厅办公室");
        assert_eq!(display.people_of("0101")[0].0, "张三");
        let depths = list
            .iter()
            .filter(|entry| entry.category == VocabularyCategory::Unit)
            .map(|entry| entry.code.len() / CODE_WIDTH - 1)
            .collect::<Vec<_>>();
        assert_eq!(depths, [0, 1, 2, 0, 1]);
    }

    #[test]
    fn subtree_collects_descendants_and_their_people() {
        let mut list = vocab();
        normalize(&mut list);
        let index = list
            .iter()
            .position(|entry| entry.canonical == "中央网信办")
            .unwrap();
        let names = subtree_indices(&list, index)
            .into_iter()
            .map(|index| list[index].canonical.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["中央网信办", "新闻舆论处", "张三", "信访接待处", "李四"]
        );
    }

    #[test]
    fn department_codes_are_collected_from_units() {
        let mut list = vocab();
        list[0].department_code = "网信函".into();
        list[3].department_code = "宣函".into();
        assert_eq!(department_codes(&list), ["网信函", "宣函"]);
    }

    #[test]
    fn department_code_falls_back_to_the_parent_unit() {
        let mut list = vocab();
        list[0].department_code = "X政函".into();
        let display = UnitDisplay::new(&list);
        assert_eq!(display.department_code_of("中央网信办"), "X政函");
        // 处室不自编代字，沿用所在机关的。
        assert_eq!(display.department_code_of("新闻舆论处"), "X政函");
        // 本级绑定的优先于上级。
        list[1].department_code = "新舆函".into();
        let display = UnitDisplay::new(&list);
        assert_eq!(display.department_code_of("新闻舆论处"), "新舆函");
        // 整条链都没有绑定，或单位不在词库中，返回空串。
        assert_eq!(display.department_code_of("网络信息处"), "");
        assert_eq!(display.department_code_of("查无此单位"), "");
    }

    /// 用户填的层级编码即事实：导入或重排后字面值不被覆盖，树形顺序以编码字典序给出。
    /// 新规则允许编码是任意 `^[A-Za-z0-9._\-]+$` 字符，旧版的两位进制数字只是惯例。
    #[test]
    fn normalize_preserves_user_provided_codes() {
        let mut list = vec![
            VocabularyEntry {
                canonical: "中央网信办".into(),
                code: "XHQ".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "新闻舆论处".into(),
                code: "XHQ-01".into(),
                parent: "XHQ".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "网信处".into(),
                code: "XHQ.02".into(),
                parent: "XHQ".into(),
                ..Default::default()
            },
        ];
        normalize(&mut list);
        let pair = list
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| (e.code.clone(), e.canonical.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            pair,
            [
                ("XHQ".into(), "中央网信办".into()),
                ("XHQ-01".into(), "新闻舆论处".into()),
                ("XHQ.02".into(), "网信处".into()),
            ]
        );
        // 父子关系仍然按编码前缀还原。
        let parent_of = |code: &str| {
            list.iter()
                .find(|e| e.category == VocabularyCategory::Unit && e.code == code)
                .map(|e| e.parent.clone())
        };
        assert_eq!(parent_of("XHQ-01"), Some("XHQ".into()));
        assert_eq!(parent_of("XHQ.02"), Some("XHQ".into()));
    }

    /// 用户填的编码与"未填而自动补"的编码能在同一棵树里共存。
    #[test]
    fn normalize_mixes_user_and_generated_codes() {
        let mut list = vec![
            VocabularyEntry {
                canonical: "顶层甲".into(),
                code: "A1".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "自动补的处".into(),
                parent: "顶层甲".into(),
                ..Default::default()
            },
            VocabularyEntry {
                canonical: "顶层乙".into(),
                code: "B2".into(),
                ..Default::default()
            },
        ];
        normalize(&mut list);
        let order = list
            .iter()
            .filter(|e| e.category == VocabularyCategory::Unit)
            .map(|e| format!("{}-{}", e.code, e.canonical))
            .collect::<Vec<_>>();
        // 用户填的保留字面；未填的按位置生成。
        assert_eq!(order, ["A1-顶层甲", "A101-自动补的处", "B2-顶层乙"]);
    }
}
