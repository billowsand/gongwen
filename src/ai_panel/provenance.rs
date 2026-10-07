//! 提案来源只随本机会话保存，不进入版本图或同步包。

pub(crate) fn source_line(skill: &str, models: &[String], ids: &[usize], at: &str) -> String {
    let model = if models.is_empty() {
        "未记录模型".into()
    } else {
        models.join("、")
    };
    format!("{skill} · {model} · {} · {at}", evidence_label(ids))
}

fn evidence_label(ids: &[usize]) -> String {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < ids.len() {
        let start = ids[i];
        let mut end = start;
        i += 1;
        while i < ids.len() && ids[i] == end + 1 {
            end = ids[i];
            i += 1;
        }
        ranges.push(if start == end {
            format!("K{start}")
        } else {
            format!("K{start}–K{end}")
        });
    }
    if ranges.is_empty() {
        "未引用证据".into()
    } else {
        format!("证据 {}", ranges.join("、"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cited_ids_are_sorted_and_compacted_without_unused_evidence() {
        assert_eq!(evidence_label(&[3, 1]), "证据 K1、K3");
        assert_eq!(evidence_label(&[7, 3, 2, 1, 2]), "证据 K1–K3、K7");
        assert_eq!(evidence_label(&[]), "未引用证据");
    }
}
