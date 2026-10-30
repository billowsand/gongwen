use crate::{
    models::{VocabularyCategory, VocabularyEntry},
    theme,
    units::UnitDisplay,
};

fn people(vocab: &[VocabularyEntry]) -> impl Iterator<Item = &VocabularyEntry> {
    vocab
        .iter()
        .filter(|e| e.category == VocabularyCategory::Person)
}

fn person_label(vocab: &[VocabularyEntry], entry: &VocabularyEntry) -> String {
    if vocab
        .iter()
        .filter(|e| {
            e.category == VocabularyCategory::Person
                && e.canonical == entry.canonical
                && e.unit == entry.unit
        })
        .count()
        > 1
    {
        return crate::units::identity::person_label(vocab, entry);
    }
    let unit = UnitDisplay::new(vocab).abbr(&entry.unit);
    [
        entry.canonical.as_str(),
        entry.position.as_str(),
        unit.as_str(),
    ]
    .into_iter()
    .filter(|s| !s.trim().is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

pub(super) fn label(
    vocab: &[VocabularyEntry],
    owner_id: Option<u64>,
    owner_name: &str,
) -> (String, bool) {
    if let Some(entry) = owner_id.and_then(|id| people(vocab).find(|e| e.id == id)) {
        (person_label(vocab, entry), true)
    } else if owner_name.trim().is_empty() {
        ("未填".into(), false)
    } else {
        (format!("{}（词库中已没有此人）", owner_name), false)
    }
}

pub(super) fn match_name<'a>(vocab: &'a [VocabularyEntry], text: &str) -> Vec<&'a VocabularyEntry> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    people(vocab)
        .filter(|e| e.canonical.trim() == text || e.aliases.iter().any(|a| a.trim() == text))
        .collect()
}

pub(super) fn picker(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    vocab: &[VocabularyEntry],
    owner_id: &mut Option<u64>,
    owner_name: &mut String,
    query: &mut String,
) -> bool {
    let mut changed = false;
    if let Some(id) = *owner_id {
        let current = people(vocab).find(|e| e.id == id);
        let (tag, ok) = current
            .map(|e| (person_label(vocab, e), true))
            .unwrap_or_else(|| (format!("{}（词库中已没有此人）", owner_name), false));
        if ui.add(theme::removable_tag_button(&tag, !ok)).clicked() {
            *owner_id = None;
            owner_name.clear();
            changed = true;
        }
    }
    if people(vocab).next().is_none() {
        theme::caption(ui, "标准词库里还没有人员，请先在『词库 → 单位人员』里添加");
        return changed;
    }
    ui.add(theme::field(query, "搜索姓名、职务或单位", ui.available_width()).id_salt(id_salt));
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return changed;
    }
    let matches = people(vocab)
        .filter(|e| {
            let unit = UnitDisplay::new(vocab).full_name(&e.unit);
            e.canonical.to_lowercase().contains(&q)
                || e.position.to_lowercase().contains(&q)
                || e.aliases.iter().any(|a| a.to_lowercase().contains(&q))
                || unit.to_lowercase().contains(&q)
                || person_label(vocab, e).to_lowercase().contains(&q)
        })
        .take(8)
        .cloned()
        .collect::<Vec<_>>();
    for entry in matches {
        let text = person_label(vocab, &entry);
        if ui.selectable_label(false, text).clicked() {
            *owner_id = Some(entry.id);
            *owner_name = entry.canonical;
            query.clear();
            changed = true;
        }
    }
    changed
}

pub(super) fn label_bound(
    vocab: &[VocabularyEntry],
    owner_id: Option<u64>,
    owner_name: &str,
    identity: &str,
) -> (String, bool) {
    if identity.is_empty() {
        return label(vocab, owner_id, owner_name);
    }
    if let Some(entry) = people(vocab).find(|e| e.identity == identity) {
        (crate::units::identity::person_label(vocab, entry), true)
    } else {
        (format!("{owner_name}（人员身份已失效，请重新选择）"), false)
    }
}

pub(super) fn picker_bound(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    vocab: &[VocabularyEntry],
    owner_id: &mut Option<u64>,
    owner_name: &mut String,
    identity: &mut String,
    query: &mut String,
) -> bool {
    if !identity.is_empty() {
        *owner_id = people(vocab)
            .find(|e| e.identity == *identity)
            .map(|e| e.id);
    }
    if !identity.is_empty()
        && owner_id.is_none()
        && ui
            .small_button(format!("{}（身份已失效，点此清除）", owner_name))
            .clicked()
    {
        identity.clear();
        owner_name.clear();
        return true;
    }
    let changed = picker(ui, id_salt, vocab, owner_id, owner_name, query);
    if changed {
        *identity = owner_id
            .and_then(|id| people(vocab).find(|e| e.id == id))
            .map(|e| e.identity.clone())
            .unwrap_or_default();
    }
    changed
}
