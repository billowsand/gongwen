//! 长文档：按节分段交给模型（一次塞不下、一次输出几十个接口也容易被截断）；整理完由程序按
//! 路径查漏——文档里出现过、却没整理成接口的地址列出来，交模型再看一遍，仍没有的告诉人。

use super::{Draft, path_key};
use regex::Regex;
use std::sync::LazyLock;

/// 文档里像接口路径的写法：至少两段，可以带服务器。
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:https?://[A-Za-z0-9.\-]+(?::\d+)?)?(/[A-Za-z_][A-Za-z0-9_\-{}.:]*(?:/[A-Za-z0-9_\-{}.:]+)+)")
        .expect("路径正则")
});
static NUMBERED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d+(?:\.\d+)+\.?\s*\S").expect("编号标题正则"));

/// 新的一节从这里开始（Markdown 标题或 `2.1 xxx` 一类编号标题）。
fn section_start(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('#') || (NUMBERED.is_match(line) && line.chars().count() <= 50)
}

/// 按节切成不超过 `max` 字的几段；一节本身超长的按行硬切。
pub(super) fn split(text: &str, max: usize) -> Vec<String> {
    if text.chars().count() <= max {
        return vec![text.to_string()];
    }
    let mut sections: Vec<String> = Vec::new();
    for line in text.lines() {
        if section_start(line) || sections.is_empty() {
            sections.push(String::new());
        }
        let current = sections.last_mut().expect("刚加过");
        current.push_str(line);
        current.push('\n');
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let flush = |current: &mut String, chunks: &mut Vec<String>| {
        if !current.trim().is_empty() {
            chunks.push(std::mem::take(current));
        }
        current.clear();
    };
    for section in sections {
        let size = section.chars().count();
        if size > max {
            flush(&mut current, &mut chunks);
            for line in section.lines() {
                if current.chars().count() + line.chars().count() + 1 > max {
                    flush(&mut current, &mut chunks);
                }
                current.push_str(line);
                current.push('\n');
            }
            flush(&mut current, &mut chunks);
        } else {
            if current.chars().count() + size > max {
                flush(&mut current, &mut chunks);
            }
            current.push_str(&section);
        }
    }
    flush(&mut current, &mut chunks);
    chunks
}

/// 文档开头的公共说明（服务地址、鉴权），分段时附在后面几段前面。
pub(super) fn preamble(text: &str, max: usize) -> String {
    let head: String = text.chars().take(max).collect();
    match head.rfind('\n') {
        Some(end) if end > 0 => head[..end].to_string(),
        _ => head,
    }
}

/// `needle` 出现处前后各 `lines` 行，最多 `hits` 处、`limit` 字。
pub(super) fn around(
    text: &str,
    needle: &str,
    lines: usize,
    limit: usize,
    hits: usize,
) -> Option<String> {
    let needle = needle.to_lowercase();
    let all: Vec<&str> = text.lines().collect();
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for (index, line) in all.iter().enumerate() {
        if taken.len() >= hits {
            break;
        }
        if line.to_lowercase().contains(&needle)
            && !taken.iter().any(|(a, b)| (*a..*b).contains(&index))
        {
            taken.push((
                index.saturating_sub(lines),
                (index + lines + 1).min(all.len()),
            ));
        }
    }
    if taken.is_empty() {
        return None;
    }
    let text = taken
        .iter()
        .map(|(a, b)| all[*a..*b].join("\n"))
        .collect::<Vec<_>>()
        .join("\n……\n");
    Some(if text.chars().count() > limit {
        text.chars().take(limit).collect::<String>() + "\n……（已截断）"
    } else {
        text
    })
}

/// 比较用的路径：数字段也当变量（`/policy/123` 与 `/policy/{id}` 是同一个接口）。
fn key_of(path: &str) -> String {
    path_key(path)
        .split('/')
        .map(|s| {
            if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
                "{}"
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// 文档里出现过、却没整理成接口的路径。只看和已认出接口同一个前缀（第一段）的，
/// 免得把「/data/items」这类字段路径也算进来；一个接口都没认出来时都列。
pub(super) fn uncovered(text: &str, drafts: &[Draft]) -> Vec<String> {
    let known: Vec<String> = drafts.iter().map(|d| key_of(&d.endpoint.url)).collect();
    let prefixes: Vec<String> = known
        .iter()
        .filter_map(|k| k.split('/').find(|s| !s.is_empty()).map(str::to_string))
        .collect();
    let mut found: Vec<String> = Vec::new();
    for caps in PATH.captures_iter(text) {
        let path = caps[1].trim_end_matches(['.', ':', ',']);
        let lower = path.to_lowercase();
        if [
            ".png", ".jpg", ".js", ".css", ".html", ".md", ".docx", ".pdf", ".xlsx",
        ]
        .iter()
        .any(|ext| lower.ends_with(ext))
        {
            continue;
        }
        let key = key_of(path);
        let first = key.split('/').find(|s| !s.is_empty()).unwrap_or_default();
        if known.contains(&key)
            || (!prefixes.is_empty() && !prefixes.iter().any(|p| p == first))
            || found.iter().any(|f| key_of(f) == key)
        {
            continue;
        }
        found.push(path.to_string());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::api_import::Material;

    #[test]
    fn long_documents_are_split_at_section_boundaries() {
        let doc = "# 总则\n服务地址\n\n## 2.1 甲\n".to_string()
            + &"甲的说明\n".repeat(30)
            + "## 2.2 乙\n"
            + &"乙的说明\n".repeat(30);
        let chunks = split(&doc, 200);
        assert!(chunks.len() >= 2, "{chunks:?}");
        assert!(chunks.iter().all(|c| c.chars().count() <= 200));
        assert!(
            chunks.iter().any(|c| c.starts_with("## 2.2 乙")),
            "从节的开头切"
        );
        assert_eq!(split("短文档", 200), ["短文档"]);
    }

    #[test]
    fn paths_mentioned_but_not_recognised_are_listed() {
        let doc = "请求地址：/api/policy/search\n请求方式：GET\n\nGET /api/stat/1\n\n\
另有详情接口 /api/policy/{id}，统计也写成 /api/stat/{year}。\n\
返回的 /data/items 是列表，图标见 /static/logo.png。";
        let material = Material::new(doc);
        let drafts = super::super::analyze(&material, None, &[]).drafts;
        assert_eq!(
            uncovered(doc, &drafts),
            ["/api/policy/{id}"],
            "/api/stat/1 与 /api/stat/{{year}} 是同一个且已认出；字段路径与图片不算"
        );
    }
}
