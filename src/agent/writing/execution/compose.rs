//! 拟纲后的两步：从已提炼的主题素材里给各节分配素材，再成文。
//!
//! 公文三五千字已算长文：逐节重读原文、逐节分组起草再归并，真机上 4 节 3000 字花了近 90 分钟、
//! 两百多次调用。现在各节素材一次分配完（不再重读原文）；篇幅不大时整篇一次写成，
//! 只有篇幅超过 [`ONE_SHOT_MAX_CHARS`] 或一次装不下时才退回逐节写。

use super::*;

/// 整篇一次成文的篇幅上限（目标字数之和）。超过就逐节写，避免单次输出过长被截断。
pub(crate) const ONE_SHOT_MAX_CHARS: usize = 6_000;

/// 拟纲时给模型看的素材目录：每条一行，带编号与出处。装得下就不必再让模型归纳目录。
pub(super) fn card_catalog(task: &WritingTask) -> String {
    task.current_topic_cards()
        .iter()
        .enumerate()
        .map(|(index, card)| {
            let title = task
                .sources
                .iter()
                .find(|source| source.id == card.source_id)
                .map_or("", |source| source.title.as_str());
            format!("C{} {}（《{}》）", index + 1, card.point, title)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 第四步：按确认的提纲，把主题素材分给各节。可一条多节，与各节都无关的不分。
/// 已分配过且提纲、素材都没变的章节保持原样（含用户手动增删的结果）。
pub(super) fn assign_cards(
    task: &mut WritingTask,
    model: &dyn crate::agent::backend::ModelBackend,
    save: &mut dyn FnMut(&WritingTask) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    task.ready_for(Stage::Collect)?;
    let pending: Vec<usize> = (0..task.sections.len())
        .filter(|&index| !task.section_assigned(&task.sections[index]))
        .collect();
    if pending.is_empty() {
        task.status = "各节素材已分配".into();
        task.stage = Stage::Collect;
        return Ok(());
    }
    let cards: Vec<MaterialCard> = task.current_topic_cards().into_iter().cloned().collect();
    let mut chosen: Vec<Vec<usize>> = vec![Vec::new(); task.sections.len()];
    if !cards.is_empty() {
        let outline = task
            .sections
            .iter()
            .enumerate()
            .map(|(index, section)| {
                format!(
                    "#{} {}（约 {} 字）：{}",
                    index + 1,
                    section.title,
                    section.target_chars,
                    section.points
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let system = "你是公文素材分配助手。把每条素材分到最能支撑的章节；一条素材可以分到多节，与各节都无关的不分。每一节都尽量分到素材：建议、对策、结论类章节分给能支撑它的问题与做法。每个章节一行：#章节编号：素材编号，逗号分隔，例如「#2：C3,C7,C12」。只输出这些行。";
        let fixed = format!(
            "{system}文种：{}\n主题：{}\n提纲：\n{outline}\n",
            task.kind, task.topic
        );
        let budget = tokens_to_bytes(extract::extract_tokens(
            model
                .window(crate::agent::backend::ModelRole::Assist)
                .tokens,
            &fixed,
        ));
        let lines: Vec<String> = cards
            .iter()
            .enumerate()
            .map(|(index, card)| {
                format!(
                    "C{} {}；依据：{}",
                    index + 1,
                    card.point,
                    clip_chars(&card.quote, 80)
                )
            })
            .collect();
        let groups = group_texts(&lines, budget.max(1024));
        for (number, group) in groups.iter().enumerate() {
            task.status = format!("分配各节素材第 {}/{} 组", number + 1, groups.len());
            save(task)?;
            let prompt = format!(
                "文种：{}\n主题：{}\n确认提纲：\n{outline}\n\n素材（第 {}/{} 组）：\n{group}",
                task.kind,
                task.topic,
                number + 1,
                groups.len()
            );
            let answer = complete(
                task,
                model,
                crate::agent::backend::ModelRole::Assist,
                system,
                &prompt,
            )?;
            let parsed = parse_assignment(&answer, task.sections.len(), cards.len());
            anyhow::ensure!(
                parsed.iter().any(|ids| !ids.is_empty()),
                "模型没有按「#章节编号：素材编号」格式分配素材，请重试"
            );
            for (section, ids) in chosen.iter_mut().zip(parsed) {
                for id in ids {
                    if !section.contains(&id) {
                        section.push(id);
                    }
                }
            }
        }
    }
    let mut empty = Vec::new();
    for index in pending {
        let query = task.section_query(&task.sections[index]);
        let assigned = task.assignment_query(&task.sections[index]);
        let section_id = task.sections[index].id.clone();
        let mut ids = chosen[index].clone();
        ids.sort_unstable();
        // 卡片 ID 与主题卡一致：核对状态在主题卡与各节之间同步。
        let section_cards: Vec<MaterialCard> = ids
            .into_iter()
            .map(|id| MaterialCard {
                section_id: Some(section_id.clone()),
                query: query.clone(),
                ..cards[id].clone()
            })
            .collect();
        let section = &mut task.sections[index];
        if section_cards.is_empty() {
            empty.push(section.title.clone());
        }
        section.cards = section_cards;
        section.assigned_query = assigned;
        section.status = SectionStatus::Collected;
    }
    task.stage = Stage::Collect;
    task.status = "各节素材已分配".into();
    if !empty.is_empty() {
        task.stale_reason = Some(format!(
            "这些章节没有分到素材，写作时只能按要点写，请核对或手动添加：{}",
            empty.join("、")
        ));
    }
    save(task)
}

/// 解析「#2：C3,C7」，返回每节的卡片下标；越界编号丢掉。
fn parse_assignment(raw: &str, sections: usize, cards: usize) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); sections];
    for line in raw.lines() {
        let line = line.trim().trim_start_matches(['-', '*', ' ']);
        let Some(rest) = line.strip_prefix('#') else {
            continue;
        };
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Some(section) = digits
            .parse::<usize>()
            .ok()
            .and_then(|number| number.checked_sub(1))
            .filter(|&index| index < sections)
        else {
            continue;
        };
        let mut chars = rest[digits.len()..].chars().peekable();
        while let Some(c) = chars.next() {
            if !matches!(c, 'C' | 'c') {
                continue;
            }
            let mut number = String::new();
            while let Some(d) = chars.peek().filter(|d| d.is_ascii_digit()) {
                number.push(*d);
                chars.next();
            }
            if let Some(card) = number
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .filter(|&index| index < cards)
                && !out[section].contains(&card)
            {
                out[section].push(card);
            }
        }
    }
    out
}

/// 第五步：需要写的章节（未锁定、非人工稿、输入变了或还没写）。篇幅不大时整篇一次写成。
pub(super) fn write_all(
    task: &mut WritingTask,
    model: &dyn crate::agent::backend::ModelBackend,
    save: &mut dyn FnMut(&WritingTask) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    task.ready_for(Stage::Write)?;
    let targets: Vec<usize> = (0..task.sections.len())
        .filter(|&index| {
            let section = &task.sections[index];
            !(section.locked
                || section.manually_edited
                || section.status == SectionStatus::Done
                    && section.input_hash == section_input_hash(task, index)
                    && !section.markdown.trim().is_empty())
        })
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    let total: usize = task.sections.iter().map(|s| s.target_chars).sum();
    let mut remaining = targets.clone();
    if total <= ONE_SHOT_MAX_CHARS {
        remaining = write_at_once(task, &targets, model, save)?;
    }
    // 整篇写不下或个别章节没对上的，退回逐节写。
    for index in remaining {
        write_section(task, index, None, model, save)?;
    }
    Ok(())
}

/// 整篇一次成文。返回没能从回答里认出来的章节（交给逐节写）；装不下窗口时全部交回。
fn write_at_once(
    task: &mut WritingTask,
    targets: &[usize],
    model: &dyn crate::agent::backend::ModelBackend,
    save: &mut dyn FnMut(&WritingTask) -> anyhow::Result<()>,
) -> anyhow::Result<Vec<usize>> {
    let mut outline = String::new();
    for (index, section) in task.sections.iter().enumerate() {
        outline.push_str(&format!(
            "\n### 第 {} 节：{}（约 {} 字）\n要点：{}\n",
            index + 1,
            assemble::section_heading(&section.title),
            section.target_chars,
            section.points
        ));
        if !targets.contains(&index) {
            outline.push_str(&format!(
                "本节已定稿，不要改写、不要输出，供衔接参考：\n{}\n",
                clip_chars(&section.markdown, 600)
            ));
            continue;
        }
        let cards = task.current_section_cards(section);
        if cards.is_empty() {
            // 真机上没分到素材的章节照搬了风格样稿里的期限与条目，数字过不了交付前核对。
            outline.push_str("素材：无。本节只写原则性表述，不写具体数字、期限、比例或机构，不照搬风格样稿里的事例与条目，篇幅宁短勿长。\n");
        }
        for card in cards {
            let title = task
                .sources
                .iter()
                .find(|source| source.id == card.source_id)
                .map_or("", |source| source.title.as_str());
            outline.push_str(&format!(
                "- 观点：{}；依据：{}（《{}》）\n",
                card.point, card.quote, title
            ));
        }
    }
    let total: usize = targets.iter().map(|&i| task.sections[i].target_chars).sum();
    let names: Vec<String> = targets
        .iter()
        .map(|&i| {
            format!(
                "「## {}」",
                assemble::section_heading(&task.sections[i].title)
            )
        })
        .collect();
    let system = format!(
        "你是公文正文起草助手。按确认的提纲一次写完需要写的各节，每节严格以给定的二级标题开头，按顺序输出：{}。节内需要分层时只用“### 小标题”，标题不写编号。只依据各节所列素材写，不得捏造数字、日期、机构或完成事实；素材不足处写成计划建议或标注待核实。不写引用标记、素材编号或参考文献。篇幅按各节字数，误差不超过一成，不为凑字数重复。只输出 Markdown 正文，不要全文标题，不要解释。",
        names.join("、")
    );
    let prompt = format!(
        "文种：{}\n总主题：{}\n全文目标篇幅：{} 字；本次需写约 {} 字\n写作风格（显式注入）：\n{}\n结构框架：\n{}\n\n确认提纲与各节素材：{}",
        task.kind, task.topic, task.target_chars, total, task.style_md, task.framework_md, outline
    );
    // 输入装不下或预计输出超过余量，就整篇交回逐节写。
    let window = model.window(crate::agent::backend::ModelRole::Draft).tokens;
    let input = crate::lmstudio::context::estimate_tokens(&system)
        + crate::lmstudio::context::estimate_tokens(&prompt)
        + crate::lmstudio::context::estimate_tokens(crate::prompt::kind_rules(task.kind));
    let output = total.saturating_mul(3) / 2;
    if input + output >= window || output >= crate::agent::budget::output_reserve(window) {
        return Ok(targets.to_vec());
    }
    task.status = format!("整篇一次成文：{} 节，约 {} 字", targets.len(), total);
    for &index in targets {
        task.sections[index].status = SectionStatus::Writing;
    }
    save(task)?;
    let answer = complete(
        task,
        model,
        crate::agent::backend::ModelRole::Draft,
        &system,
        &prompt,
    )?;
    let parts = split_sections(&answer);
    let mut remaining = Vec::new();
    for (position, &index) in targets.iter().enumerate() {
        let title = assemble::section_heading(&task.sections[index].title);
        // 先按标题认；标题都对不上但节数一致时按顺序认。
        let body = parts
            .iter()
            .find(|(heading, _)| assemble::section_heading(heading) == title)
            .or_else(|| (parts.len() == targets.len()).then(|| &parts[position]))
            .map(|(_, body)| body.trim().to_string())
            .filter(|body| !body.is_empty());
        let Some(body) = body else {
            task.sections[index].status = SectionStatus::Idle;
            remaining.push(index);
            continue;
        };
        let hash = section_input_hash(task, index);
        let section = &mut task.sections[index];
        if !section.markdown.trim().is_empty() && section.markdown != body {
            section.revision_history.push(section.markdown.clone());
        }
        section.markdown = body;
        section.status = SectionStatus::Done;
        section.manually_edited = false;
        section.write_parts.clear();
        section.merge_state = None;
        section.input_hash = hash;
        section.review_suggestion = None;
        section.review_kept_draft = false;
    }
    let written: usize = targets
        .iter()
        .filter(|index| !remaining.contains(index))
        .map(|&i| task.sections[i].markdown.chars().count())
        .sum();
    task.stage = Stage::Write;
    task.status = format!("整篇一次成文：约 {written} 字 / 目标 {total} 字");
    save(task)?;
    Ok(remaining)
}

/// 按行首的「## 标题」切开整篇回答；标题前的内容（全文标题、开场白）丢掉。
fn split_sections(text: &str) -> Vec<(String, String)> {
    let mut parts: Vec<(String, String)> = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let trimmed = line.trim_start();
        if !fenced && let Some(title) = trimmed.strip_prefix("## ") {
            parts.push((title.trim().to_string(), String::new()));
            continue;
        }
        if let Some((_, body)) = parts.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignment_lines_tolerate_wrapping_and_drop_unknown_ids() {
        let parsed = parse_assignment("- #1：C1, C3,c2\n**#2：** C9, C2\n#7：C1\n说明：无", 2, 3);
        assert_eq!(parsed, vec![vec![0, 2, 1], vec![1]]);
    }

    #[test]
    fn one_answer_splits_into_sections_by_second_level_headings() {
        let parts = split_sections(
            "# 全文标题\n开场白\n## 背景\n正文一\n### 小标题\n细则\n## 措施\n正文二\n",
        );
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].0, "背景");
        assert_eq!(parts[0].1, "正文一\n### 小标题\n细则\n");
        assert_eq!(parts[1], ("措施".to_string(), "正文二\n".to_string()));
    }
}
