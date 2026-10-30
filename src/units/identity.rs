//! 词条永久身份、唯一解析、消歧与稿件快照。名称无法唯一确定时不猜配。
use super::UnitDisplay;
use crate::models::{TemplateProfile, VocabularyCategory as Category, VocabularyEntry as Entry};
use std::collections::{HashMap, HashSet};

pub(crate) fn assign(entries: &mut [Entry]) -> bool {
    let mut changed = false;
    for entry in entries {
        if let Ok(id) = uuid::Uuid::parse_str(&entry.identity) {
            let normalized = id.to_string();
            if entry.identity != normalized {
                entry.identity = normalized;
                changed = true;
            }
        }
        if let Ok(id) = uuid::Uuid::parse_str(&entry.owner_identity) {
            let normalized = id.to_string();
            if entry.owner_identity != normalized {
                entry.owner_identity = normalized;
                changed = true;
            }
        }
        if entry.identity.is_empty() {
            entry.identity = uuid::Uuid::new_v4().to_string();
            changed = true;
        }
    }
    changed
}

pub(crate) fn is_ref(key: &str) -> bool {
    key.starts_with("@unit:") || key.starts_with("@person:") || key.starts_with("@unresolved:")
}

/// 旧字符串只能在所有可能解释指向唯一条目时解析，不能“先编码、后首个同名”。
pub(crate) fn resolve<'a>(
    entries: &'a [Entry],
    category: Category,
    key: &str,
) -> Option<&'a Entry> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let reference = if is_ref(key) {
        let prefix = if category == Category::Unit {
            "@unit:"
        } else {
            "@person:"
        };
        Some(
            uuid::Uuid::parse_str(key.strip_prefix(prefix)?)
                .ok()?
                .to_string(),
        )
    } else {
        None
    };
    let mut matches = entries.iter().filter(|entry| {
        entry.category == category
            && if let Some(identity) = &reference {
                entry.identity == *identity
                    || uuid::Uuid::parse_str(&entry.identity)
                        .is_ok_and(|id| id.to_string() == *identity)
            } else {
                entry.canonical.trim() == key
                    || (category == Category::Unit && !entry.code.is_empty() && entry.code == key)
            }
    });
    let entry = matches.next()?;
    matches.next().is_none().then_some(entry)
}

/// 优先永久父身份，其次明确的业务编码；真正旧格式仅兼容唯一的完整名称。
pub(crate) fn parent<'a>(entries: &'a [Entry], entry: &Entry) -> Option<&'a Entry> {
    if !entry.owner_identity.is_empty() {
        let mut found = entries
            .iter()
            .filter(|e| e.category == Category::Unit && e.identity == entry.owner_identity);
        let first = found.next()?;
        return found.next().is_none().then_some(first);
    }
    let code = if entry.category == Category::Unit {
        &entry.parent
    } else {
        &entry.unit
    };
    if code.is_empty() {
        return None;
    }
    let mut found = entries
        .iter()
        .filter(|e| e.category == Category::Unit && e.code == *code);
    let first = found.next();
    if found.next().is_some() {
        return None;
    }
    if first.is_some() {
        return first;
    }
    // 真正旧格式曾把名称写进 parent/unit：只有完全相等且唯一时兼容，不能取首个同名。
    let mut legacy = entries
        .iter()
        .filter(|e| e.category == Category::Unit && e.canonical.trim() == code.trim());
    let first = legacy.next()?;
    legacy.next().is_none().then_some(first)
}

pub(crate) fn validate(entries: &[Entry]) -> Result<(), String> {
    let mut identities = HashSet::new();
    let mut codes = HashSet::new();
    let mut names = HashSet::new();
    let mut people = HashSet::new();
    for e in entries {
        if e.canonical.trim().is_empty() {
            return Err("词条名称不能为空，请补齐或删除空词条".into());
        }
        if !e.identity.is_empty()
            && (uuid::Uuid::parse_str(&e.identity).is_err()
                || !identities.insert(uuid::Uuid::parse_str(&e.identity).ok()))
        {
            return Err(format!("身份编码无效或重复：{}", e.canonical));
        }
        if e.category == Category::Unit {
            if !e.code.is_empty() && !codes.insert(&e.code) {
                return Err(format!("单位编码重复：{}", e.code));
            }
            if !e.canonical.trim().is_empty()
                && !names.insert((e.parent.trim(), e.canonical.trim()))
            {
                return Err(format!(
                    "同一父节点下单位名称重复：{}（上级编码：{}）",
                    e.canonical, e.parent
                ));
            }
        } else if !e.code.is_empty() && !people.insert((e.unit.trim(), e.code.trim())) {
            return Err(format!("同一单位下人员编码重复：{} / {}", e.unit, e.code));
        }
        let mut visited = HashSet::new();
        let mut current = e;
        for _ in 0..=super::MAX_DEPTH {
            if !visited.insert(current.key()) {
                return Err(format!("单位关系成环：{}", e.canonical));
            }
            let owner = if current.category == Category::Unit {
                &current.parent
            } else {
                &current.unit
            };
            if owner.is_empty() {
                break;
            }
            current = parent(entries, current).ok_or_else(|| {
                format!(
                    "{}的所属／上级单位不存在或编码重复：{owner}",
                    current.canonical
                )
            })?;
            if visited.len() > super::MAX_DEPTH {
                return Err("单位层级超过安全上限".into());
            }
        }
    }
    Ok(())
}

fn base(e: &Entry, external: bool) -> String {
    if external && !e.external_name.trim().is_empty() {
        e.external_name.trim().into()
    } else {
        e.canonical.trim().into()
    }
}

/// 同时扩展全部冲突项，重新检查与未冲突名称的碰撞，绝不依赖候选顺序。
pub(crate) fn labels(
    entries: &[Entry],
    selected: &[&Entry],
    external: bool,
) -> HashMap<String, String> {
    qualified_labels(entries, selected, external).0
}

fn qualified_labels(
    entries: &[Entry],
    selected: &[&Entry],
    external: bool,
) -> (HashMap<String, String>, HashSet<String>) {
    let mut ambiguous = HashSet::new();
    let mut names: HashMap<String, String> = selected
        .iter()
        .map(|e| (e.key(), base(e, external)))
        .collect();
    let mut cursors: HashMap<String, &Entry> = selected.iter().map(|e| (e.key(), *e)).collect();
    for _ in 0..super::MAX_DEPTH {
        let mut counts = HashMap::new();
        for name in names.values() {
            *counts.entry(name.clone()).or_insert(0usize) += 1;
        }
        let conflicts: Vec<_> = names
            .iter()
            .filter(|(_, name)| counts[*name] > 1)
            .map(|(key, _)| key.clone())
            .collect();
        if conflicts.is_empty() {
            break;
        }
        let mut progressed = false;
        for key in conflicts {
            if let Some(p) = cursors.get(&key).and_then(|e| parent(entries, e)) {
                let prefix = base(p, external);
                let name = &names[&key];
                let next = if name.starts_with(&prefix) {
                    format!("{prefix} › {name}")
                } else {
                    format!("{prefix}{name}")
                };
                names.insert(key.clone(), next);
                cursors.insert(key, p);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    let mut counts = HashMap::new();
    for name in names.values() {
        *counts.entry(name.clone()).or_insert(0usize) += 1;
    }
    for e in selected {
        if counts[&names[&e.key()]] > 1 {
            ambiguous.insert(e.key());
            let name = names[&e.key()].clone();
            names.insert(
                e.key(),
                format!(
                    "{name}〔{}〕",
                    if e.code.is_empty() {
                        &e.identity
                    } else {
                        &e.code
                    }
                ),
            );
        }
    }
    (names, ambiguous)
}

pub(crate) fn unit_label(entries: &[Entry], entry: &Entry, external: bool) -> String {
    labels(
        entries,
        &entries
            .iter()
            .filter(|e| e.category == Category::Unit)
            .collect::<Vec<_>>(),
        external,
    )
    .remove(&entry.key())
    .unwrap_or_else(|| base(entry, external))
}

pub(crate) fn person_label(entries: &[Entry], entry: &Entry) -> String {
    let name = entry.canonical.trim();
    if entries
        .iter()
        .filter(|e| e.category == Category::Person && e.canonical.trim() == name)
        .count()
        < 2
    {
        return name.into();
    }
    let unit = parent(entries, entry)
        .map(|u| unit_label(entries, u, false))
        .unwrap_or_else(|| "未挂靠单位".into());
    let role = if entry.position.is_empty() {
        String::new()
    } else {
        format!(" · {}", entry.position)
    };
    // 同单位同名也要能区分，候选显示人员编号与永久身份。
    format!(
        "{name}（{unit}{role} · {}）",
        if entry.code.is_empty() {
            &entry.identity
        } else {
            &entry.code
        }
    )
}

pub(crate) fn label(entries: &[Entry], key: &str) -> String {
    for category in [Category::Unit, Category::Person] {
        if let Some(e) = resolve(entries, category, key) {
            return if category == Category::Unit {
                unit_label(entries, e, false)
            } else {
                person_label(entries, e)
            };
        }
    }
    if let Some(text) = key.strip_prefix("@unresolved:") {
        format!("{text}（同名待核对）")
    } else if is_ref(key) {
        "〔词条失效，请重新选择〕".into()
    } else {
        key.to_owned()
    }
}

fn fields(profile: &mut TemplateProfile, mut visit: impl FnMut(&mut String, Category)) {
    for value in [
        &mut profile.issuing_unit,
        &mut profile.joint_issuing_units,
        &mut profile.main_issuing_unit,
        &mut profile.recipient,
        &mut profile.copies_to,
        &mut profile.responsible_unit,
        &mut profile.joint_responsible_units,
        &mut profile.signing_unit,
    ] {
        visit(value, Category::Unit);
    }
    for value in [&mut profile.contact_person, &mut profile.reporting_leaders] {
        visit(value, Category::Person);
    }
    for contact in &mut profile.joint_contacts {
        visit(&mut contact.unit, Category::Unit);
        visit(&mut contact.name, Category::Person);
    }
}

pub(crate) fn selected(profile: &TemplateProfile) -> Vec<String> {
    let mut profile = profile.clone();
    let mut keys = Vec::new();
    fields(&mut profile, |value, _| {
        keys.extend(crate::models::split_units(value))
    });
    keys
}

/// 只在编辑活稿时调用；历史快照绝不原地迁移。只升级唯一且无歧义的旧值。
/// 已绑定词条保留原快照，词库改名／调动／删除不会重写既有稿件。
pub(crate) fn capture(profile: &mut TemplateProfile, vocabulary: &[Entry]) {
    let keep: Vec<_> = profile
        .vocabulary_snapshot
        .iter()
        .map(|e| e.key())
        .collect();
    capture_with_keys(profile, vocabulary, keep);
}

fn capture_with_keys(profile: &mut TemplateProfile, vocabulary: &[Entry], extra: Vec<String>) {
    let old = profile.vocabulary_snapshot.clone();
    let contact_name = profile.contact_person.clone();
    let contact_phone = profile.contact_phone.clone();
    fields(profile, |value, category| {
        let keys: Vec<_> = crate::models::split_units(value)
            .into_iter()
            .map(|key| {
                if !is_ref(&key)
                    && vocabulary
                        .iter()
                        .filter(|e| {
                            e.category == category
                                && (e.canonical.trim() == key
                                    || (category == Category::Unit && e.code == key))
                        })
                        .count()
                        > 1
                {
                    return format!("@unresolved:{key}");
                }
                resolve(vocabulary, category, &key)
                    .filter(|e| {
                        category != Category::Person
                            || key != contact_name
                            || is_ref(&key)
                            || contact_phone.is_empty()
                            || contact_phone == e.phone
                    })
                    .map_or(key.clone(), |e| {
                        if available(vocabulary, e) {
                            e.key()
                        } else {
                            format!("@unresolved:{}", e.canonical)
                        }
                    })
            })
            .collect();
        *value = crate::models::join_units(&keys);
    });
    let mut snapshots = Vec::new();
    let mut seen = HashSet::new();
    for key in selected(profile)
        .into_iter()
        .chain(extra)
        .filter(|k| is_ref(k))
    {
        let entry = old
            .iter()
            .find(|e| e.key() == key)
            .or_else(|| vocabulary.iter().find(|e| e.key() == key));
        let Some(mut entry) = entry else {
            continue;
        };
        for _ in 0..=super::MAX_DEPTH {
            if !seen.insert(entry.key()) {
                break;
            }
            snapshots.push(entry.clone());
            let source = if old.iter().any(|e| e.key() == entry.key()) {
                old.as_slice()
            } else {
                vocabulary
            };
            let Some(p) = parent(source, entry) else {
                break;
            };
            entry = p;
        }
    }
    profile.vocabulary_snapshot = snapshots;
}

impl<'a> UnitDisplay<'a> {
    /// 历史与当前稿均优先用稿件快照，整稿同名项使用一份显示计划。
    pub(crate) fn for_input(&self, input: &crate::models::DraftInput) -> UnitDisplay<'static> {
        let entries = if input.profile.vocabulary_snapshot.is_empty() {
            self.vocab.to_vec()
        } else {
            input.profile.vocabulary_snapshot.clone()
        };
        let mut keys = selected(&input.profile);
        keys.extend(extra_keys(input));
        let chosen: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.category == Category::Unit
                    && keys.iter().any(|k| {
                        resolve(&entries, Category::Unit, k)
                            .is_some_and(|entry| entry.key() == e.key())
                    })
            })
            .collect();
        let (labels, ambiguous) =
            qualified_labels(&entries, &chosen, input.uses_external_unit_names());
        UnitDisplay {
            vocab: std::borrow::Cow::Owned(entries),
            labels,
            ambiguous,
            internal_labels: std::sync::OnceLock::new(),
            external_labels: std::sync::OnceLock::new(),
        }
    }

    pub(crate) fn person_name(&self, key: &str) -> String {
        resolve(&self.vocab, Category::Person, key)
            .map(|e| {
                if self
                    .vocab
                    .iter()
                    .filter(|p| p.category == Category::Person && p.canonical == e.canonical)
                    .count()
                    > 1
                {
                    let unit = parent(&self.vocab, e)
                        .map(|u| self.full_name(&u.key()))
                        .unwrap_or_default();
                    format!(
                        "{unit}{}{}",
                        e.canonical,
                        if e.position.is_empty() {
                            String::new()
                        } else {
                            format!("（{}）", e.position)
                        }
                    )
                } else {
                    e.canonical.clone()
                }
            })
            .unwrap_or_else(|| {
                if is_ref(key) {
                    "〔人员失效，请重新选择〕".into()
                } else {
                    key.to_owned()
                }
            })
    }
}

/// 业务编码修改后按永久父身份恢复关联。无身份的旧关系留给旧格式迁移。
pub(crate) fn restore_owners(entries: &mut [Entry]) {
    let codes: HashMap<_, _> = entries
        .iter()
        .filter(|e| e.category == Category::Unit && !e.identity.is_empty())
        .map(|e| (e.identity.clone(), e.code.clone()))
        .collect();
    for entry in entries {
        if let Some(code) = codes.get(&entry.owner_identity) {
            if entry.category == Category::Unit {
                entry.parent = code.clone();
            } else {
                entry.unit = code.clone();
            }
        }
    }
}

pub(crate) fn update_owners(entries: &mut [Entry]) {
    let owners: Vec<_> = entries
        .iter()
        .map(|e| {
            let code = if e.category == Category::Unit {
                &e.parent
            } else {
                &e.unit
            };
            let mut found = entries
                .iter()
                .filter(|p| p.category == Category::Unit && !code.is_empty() && p.code == *code);
            let first = found.next();
            if found.next().is_none() {
                first.map(|p| p.identity.clone())
            } else {
                None
            }
        })
        .collect();
    for (entry, owner) in entries.iter_mut().zip(owners) {
        if let Some(owner) = owner {
            entry.owner_identity = owner;
        } else if entry.parent.is_empty() && entry.unit.is_empty() {
            entry.owner_identity.clear();
        }
    }
}

pub(crate) fn display_values(entries: &[Entry], value: &str) -> String {
    crate::models::split_units(value)
        .iter()
        .map(|key| label(entries, key))
        .collect::<Vec<_>>()
        .join("、")
}

impl UnitDisplay<'_> {
    pub(crate) fn ensure_resolved(&self, input: &crate::models::DraftInput) -> Result<(), String> {
        let mut profile = input.profile.clone();
        let mut errors = Vec::new();
        fields(&mut profile, |value, category| {
            for key in crate::models::split_units(value) {
                let matches = self
                    .vocab
                    .iter()
                    .filter(|e| {
                        e.category == category
                            && (e.key() == key
                                || e.canonical.trim() == key
                                || (category == Category::Unit && e.code == key))
                    })
                    .count();
                if (is_ref(&key) && resolve(&self.vocab, category, &key).is_none()) || matches > 1 {
                    errors.push(format!(
                        "要素存在同名歧义或失效引用，请在表单重新选择：{}",
                        label(&self.vocab, &key)
                    ));
                }
            }
        });
        for key in extra_keys(input) {
            let category = if key.starts_with("@person:") {
                Category::Person
            } else {
                Category::Unit
            };
            if is_ref(&key) && resolve(&self.vocab, category, &key).is_none() {
                errors.push("名单／来电／印发单位有失效引用，请重新选择".into());
            }
        }
        let chosen = selected(&input.profile);
        let people: Vec<_> = chosen
            .iter()
            .filter_map(|key| resolve(&self.vocab, Category::Person, key))
            .collect();
        for person in &people {
            if people.iter().any(|other| {
                other.key() != person.key()
                    && self.person_name(&other.key()) == self.person_name(&person.key())
            }) {
                errors.push(format!(
                    "同名人员的单位和职务仍无法区分，请核对称谓：{}",
                    person.canonical
                ));
            }
        }
        let mut pairs = crate::models::joint_responsible_entries(&input.profile)
            .into_iter()
            .map(|e| (e.unit, e.name))
            .collect::<Vec<_>>();
        pairs.push((
            input.profile.responsible_unit.clone(),
            input.profile.contact_person.clone(),
        ));
        for (unit, person) in pairs {
            if !unit.is_empty()
                && person.starts_with("@person:")
                && !self
                    .responsible_people_of(&unit)
                    .iter()
                    .any(|(key, _)| key == &person)
            {
                errors.push("联系人不属于所选承办单位或没有承办上级单位权限，请重新选择".into());
            }
        }
        for key in &self.ambiguous {
            if let Some(text) = self.labels.get(key) {
                errors.push(format!(
                    "单位名称补齐全部上级后仍重名，请维护可区分的公文用名：{text}"
                ));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("；"))
        }
    }
}

fn extra_keys(input: &crate::models::DraftInput) -> Vec<String> {
    [
        &input.phone_record.institution,
        &input.phone_record.caller_unit,
        &input.phone_record.caller_person,
        &input.minutes.printing_unit,
    ]
    .into_iter()
    .flat_map(|v| crate::models::split_units(v))
    .collect()
}

pub(crate) fn capture_input(input: &mut crate::models::DraftInput, vocabulary: &[Entry]) {
    for value in [
        &mut input.phone_record.institution,
        &mut input.phone_record.caller_unit,
        &mut input.minutes.printing_unit,
    ] {
        if let Some(unit) = resolve(vocabulary, Category::Unit, value) {
            *value = if available(vocabulary, unit) {
                unit.key()
            } else {
                format!("@unresolved:{}", unit.canonical)
            };
        } else if !is_ref(value)
            && vocabulary
                .iter()
                .filter(|e| {
                    e.category == Category::Unit
                        && (e.canonical == *value || (!value.is_empty() && e.code == *value))
                })
                .count()
                > 1
        {
            *value = format!("@unresolved:{value}");
        }
    }
    let extra = extra_keys(input);
    capture_with_keys(&mut input.profile, vocabulary, extra);
}

pub(crate) fn readable_phone_record(
    input: &crate::models::DraftInput,
    display: &UnitDisplay<'_>,
) -> crate::models::DraftInput {
    let mut result = input.clone();
    result.phone_record.institution = display.full_name(&input.phone_record.institution);
    result.phone_record.caller_unit = display.full_name(&input.phone_record.caller_unit);
    result.phone_record.caller_person = display.person_name(&input.phone_record.caller_person);
    result.profile.responsible_unit = display.abbr(&input.profile.responsible_unit);
    result.profile.contact_person = display.person_name(&input.profile.contact_person);
    result
}

/// 只禁用有冲突的节点及其下属范围；其余有效分支仍可正常填写。
pub(crate) fn available(entries: &[Entry], entry: &Entry) -> bool {
    let mut current = entry;
    let mut visited = HashSet::new();
    for _ in 0..=super::MAX_DEPTH {
        if !visited.insert(current.key()) {
            return false;
        }
        if !current.identity.is_empty()
            && entries
                .iter()
                .filter(|e| e.identity == current.identity)
                .count()
                != 1
        {
            return false;
        }
        if current.category == Category::Person
            && !current.code.is_empty()
            && entries
                .iter()
                .filter(|e| {
                    e.category == Category::Person
                        && e.unit == current.unit
                        && e.code == current.code
                })
                .count()
                != 1
        {
            return false;
        }
        if current.category == Category::Unit {
            if entries
                .iter()
                .filter(|e| e.category == Category::Unit && e.code == current.code)
                .count()
                != 1
            {
                return false;
            }
            if entries
                .iter()
                .filter(|e| {
                    e.category == Category::Unit
                        && e.parent == current.parent
                        && e.canonical.trim() == current.canonical.trim()
                })
                .count()
                != 1
            {
                return false;
            }
        }
        let owner = if current.category == Category::Unit {
            &current.parent
        } else {
            &current.unit
        };
        if owner.is_empty() {
            return true;
        }
        let Some(p) = parent(entries, current) else {
            return false;
        };
        current = p;
    }
    false
}

/// 只有用户明确重新选择时刷新该词条与祖先；其余已绑定快照保留。
pub(crate) fn refresh_snapshot(profile: &mut TemplateProfile, vocabulary: &[Entry], values: &str) {
    let mut refresh = HashSet::new();
    for key in crate::models::split_units(values) {
        let category = if key.starts_with("@unit:") {
            Category::Unit
        } else if key.starts_with("@person:") {
            Category::Person
        } else {
            continue;
        };
        let mut entry = resolve(vocabulary, category, &key);
        for _ in 0..=super::MAX_DEPTH {
            let Some(current) = entry else {
                break;
            };
            if !refresh.insert(current.key()) {
                break;
            }
            entry = parent(vocabulary, current);
        }
    }
    profile
        .vocabulary_snapshot
        .retain(|e| !refresh.contains(&e.key()));
}

pub(crate) fn snapshot_notice(
    input: &crate::models::DraftInput,
    vocabulary: &[Entry],
) -> Option<String> {
    let changed = input.profile.vocabulary_snapshot.iter().any(|old| {
        vocabulary
            .iter()
            .find(|e| e.identity == old.identity)
            .is_none_or(|e| {
                e.canonical != old.canonical
                    || e.external_name != old.external_name
                    || e.abbr != old.abbr
                    || e.phone != old.phone
                    || e.position != old.position
                    || e.owner_identity != old.owner_identity
                    || e.can_handle_parent_unit != old.can_handle_parent_unit
                    || e.seal_on_behalf != old.seal_on_behalf
                    || e.department_code != old.department_code
                    || e.approval_department_code != old.approval_department_code
            })
    });
    changed.then(|| {
        "词库资料与稿件快照不一致：当前稿仍按原快照显示；如需更新，请重新选择对应单位或人员。"
            .into()
    })
}
