use super::model::{Status, Task};
use std::collections::{HashMap, HashSet, VecDeque};

/// 任务指向前置、上级指向子任务，统一检查完成依赖；拓扑排序避免深链递归。
pub fn validate_graph(tasks: &[Task], candidate: &Task) -> Result<(), String> {
    let old = tasks.iter().find(|t| t.id == candidate.id);
    let mut map: HashMap<&str, &Task> = tasks
        .iter()
        .filter(|t| !t.deleted && t.id != candidate.id)
        .map(|t| (t.id.as_str(), t))
        .collect();
    map.insert(&candidate.id, candidate);
    if let Some(parent) = &candidate.parent_id {
        let p = map.get(parent.as_str()).ok_or("上级任务不存在")?;
        if old.is_none_or(|t| t.parent_id != candidate.parent_id)
            && (p.status.terminal() || p.archived)
        {
            return Err("已完成、取消或归档的上级须先重开，才能增加子任务".into());
        }
    }
    let mut edges: HashMap<&str, HashSet<&str>> = HashMap::new();
    for t in map.values() {
        edges.entry(&t.id).or_default();
        if let Some(parent) = &t.parent_id {
            if !map.contains_key(parent.as_str()) {
                return Err("上级任务不存在或已删除".into());
            }
            edges.entry(parent).or_default().insert(&t.id);
        }
        for prerequisite in &t.prerequisites {
            if !map.contains_key(prerequisite.as_str()) {
                return Err("前置任务不存在或已删除".into());
            }
            edges.entry(&t.id).or_default().insert(prerequisite);
        }
    }
    let mut incoming: HashMap<&str, usize> = map.keys().map(|id| (*id, 0)).collect();
    for next in edges.values().flatten() {
        *incoming.entry(next).or_default() += 1;
    }
    let mut ready: VecDeque<&str> = incoming
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        if let Some(next) = edges.get(id) {
            for next_id in next {
                if let Some(count) = incoming.get_mut(next_id) {
                    *count -= 1;
                    if *count == 0 {
                        ready.push_back(next_id);
                    }
                }
            }
        }
    }
    if visited != map.len() {
        return Err("上级与前置任务组合会形成循环或完成死锁".into());
    }
    Ok(())
}

pub fn blocked(tasks: &[Task], task: &Task) -> bool {
    task.needs_review
        || task.prerequisites.iter().any(|id| {
            !tasks
                .iter()
                .any(|p| p.id == *id && !p.deleted && p.status == Status::Done && !p.needs_review)
        })
}

pub fn can_enter(tasks: &[Task], task: &Task, status: Status) -> Result<(), String> {
    let old = tasks.iter().find(|old| old.id == task.id);
    // 保留已完成但待核对的历史结论，不据此确认新的完成结论。
    if old.is_some_and(|old| old.status == Status::Done)
        && status == Status::Done
        && task.needs_review
    {
        return Ok(());
    }
    if matches!(status, Status::InProgress | Status::Done) && blocked(tasks, task) {
        return Err("前置任务尚未确认完成，或本任务关系仍待核对".into());
    }
    if status == Status::Done
        && tasks.iter().any(|t| {
            !t.deleted
                && t.parent_id.as_deref() == Some(task.id.as_str())
                && (t.status != Status::Done || t.needs_review)
        })
    {
        return Err("仍有未确认完成的子任务".into());
    }
    Ok(())
}

/// 反向追踪完成依赖，覆盖全部祖先和间接受影响的后续任务。
pub fn reopening_impact(tasks: &[Task], id: &str) -> Vec<(String, bool)> {
    let mut queue = VecDeque::from([id.to_owned()]);
    let mut seen = HashSet::from([id.to_owned()]);
    let mut result = Vec::new();
    while let Some(changed) = queue.pop_front() {
        let parent = tasks
            .iter()
            .find(|t| t.id == changed)
            .and_then(|t| t.parent_id.as_ref());
        for t in tasks.iter().filter(|t| !t.deleted) {
            let dependency = t.prerequisites.contains(&changed);
            if (dependency || parent == Some(&t.id)) && seen.insert(t.id.clone()) {
                result.push((t.id.clone(), dependency));
                queue.push_back(t.id.clone());
            }
        }
    }
    result
}
