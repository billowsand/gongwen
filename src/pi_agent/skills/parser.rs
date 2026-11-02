//! 标准 SKILL.md 解析：YAML frontmatter + 自然语言正文。
//!
//! 这里只读 `name` / `description` / 可选显示名，并识别旧流程字段。**不解释**
//! flow、step、variables、when、tools 等执行字段——检测到就要求改写成自然语言标准
//! 技能，不把旧流程当成 Pi 指令。未知的标准 metadata 原样留在文件里，本模块不重写
//! 用户文本。

use serde_yaml::Value;

/// frontmatter 上限。正常技能只有几行元数据，超过这个数多半不是技能文件。
pub(crate) const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;
/// description 是给模型看的短说明，不允许拿它塞整篇文档。
const MAX_DESCRIPTION_CHARS: usize = 4_000;
const MAX_DISPLAY_NAME_CHARS: usize = 64;

/// 旧流程技能会出现的顶层字段。只检查 frontmatter 的键，不因为正文里出现
/// `flow:` 字样就误拒绝自然语言技能。
const LEGACY_FIELDS: &[&str] = &[
    "flow",
    "steps",
    "step",
    "variables",
    "when",
    "tools",
    "triggers",
    "hint",
    "output",
    "prompt",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parsed {
    pub(crate) name: String,
    pub(crate) description: String,
    /// 标准 metadata 里的可选显示名；没有就由界面用内置中文名或 name 兜底。
    pub(crate) display_name: Option<String>,
    /// frontmatter 之后的正文（保留原文，只用于判空与摘要）。
    pub(crate) body: String,
}

/// 技能名：小写字母、数字与单个短横线，最长 64；不允许路径与 Windows 设备名。
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !is_windows_device(name)
}

fn is_windows_device(name: &str) -> bool {
    matches!(
        name.split('.')
            .next()
            .unwrap_or_default()
            .to_uppercase()
            .as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// 解析一份标准技能。`expected` 是目录名，frontmatter 的 name 必须与它一致。
pub(crate) fn parse(expected: &str, content: &str) -> Result<Parsed, String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    // CRLF 只影响解析；保存时仍写用户编辑缓冲的原样内容。
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let rest = normalized
        .strip_prefix("---\n")
        .ok_or_else(|| format!("{expected}：缺少 --- 包围的 frontmatter"))?;
    let end = rest
        .split('\n')
        .scan(0usize, |offset, line| {
            let start = *offset;
            *offset += line.len() + 1;
            Some((start, line))
        })
        .find_map(|(start, line)| (line.trim_end() == "---").then_some(start))
        .ok_or_else(|| format!("{expected}：frontmatter 没有结束的 ---"))?;
    let yaml = &rest[..end];
    if yaml.len() > MAX_FRONTMATTER_BYTES {
        return Err(format!(
            "{expected}：frontmatter 超过 {} KB",
            MAX_FRONTMATTER_BYTES / 1024
        ));
    }
    let value: Value = serde_yaml::from_str(yaml)
        .map_err(|error| format!("{expected}：frontmatter 不是合法 YAML：{error}"))?;
    let map = value
        .as_mapping()
        .ok_or_else(|| format!("{expected}：frontmatter 必须是 YAML 映射"))?;
    for field in LEGACY_FIELDS {
        if map.get(Value::String((*field).to_string())).is_some() {
            return Err(format!(
                "{expected}：这份技能用的是旧流程格式（frontmatter 里还有 {field} 字段）。\
                 Pi 只执行自然语言标准技能：请把步骤改写成正文里的中文说明，去掉 flow / step / \
                 variables / when / tools 等执行字段后再保存或导入。"
            ));
        }
    }
    let name = string_field(map, "name")
        .ok_or_else(|| format!("{expected}：frontmatter 缺少 name"))?
        .trim()
        .to_string();
    if !valid_name(&name) {
        return Err(format!(
            "{expected}：name「{name}」不合规；技能名只能用小写字母、数字与单个短横线，最长 64"
        ));
    }
    if name != expected {
        return Err(format!("{expected}：frontmatter 的 name 应是 {expected}"));
    }
    let description = string_field(map, "description")
        .ok_or_else(|| format!("{expected}：frontmatter 缺少 description"))?
        .trim()
        .to_string();
    if description.is_empty() {
        return Err(format!("{expected}：description 不能为空"));
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "{expected}：description 超过 {MAX_DESCRIPTION_CHARS} 字"
        ));
    }
    let display_name = display_name_field(map).filter(|text| !text.is_empty());
    if display_name
        .as_ref()
        .is_some_and(|text| text.chars().count() > MAX_DISPLAY_NAME_CHARS)
    {
        return Err(format!(
            "{expected}：显示名超过 {MAX_DISPLAY_NAME_CHARS} 字"
        ));
    }
    let after = &rest[end..];
    let body = after
        .find('\n')
        .map(|index| &after[index + 1..])
        .unwrap_or_default()
        .to_string();
    if body.trim().is_empty() {
        return Err(format!("{expected}：正文不能为空"));
    }
    Ok(Parsed {
        name,
        description,
        display_name,
        body,
    })
}

/// 复制为另一个技能：仅重建新副本的元数据，保留未知 YAML 值与正文，不改原文件。
pub(super) fn renamed(
    source: &str,
    content: &str,
    target: &str,
    display: Option<&str>,
) -> Result<String, String> {
    let parsed = parse(source, content)?;
    let normalized = content
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let yaml = normalized
        .lines()
        .skip(1)
        .take_while(|line| line.trim_end() != "---")
        .collect::<Vec<_>>()
        .join("\n");
    let mut value: Value = serde_yaml::from_str(&yaml).map_err(|error| error.to_string())?;
    let map = value
        .as_mapping_mut()
        .ok_or("frontmatter 必须是 YAML 映射")?;
    map.insert(Value::String("name".into()), Value::String(target.into()));
    if let Some(display) = display {
        map.insert(
            Value::String("display_name".into()),
            Value::String(display.into()),
        );
    }
    let yaml = serde_yaml::to_string(&value).map_err(|error| error.to_string())?;
    let text = format!("---\n{yaml}---\n{}", parsed.body);
    parse(target, &text)?;
    Ok(text)
}

fn string_field(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(Value::String(key.to_string()))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn display_name_field(map: &serde_yaml::Mapping) -> Option<String> {
    if let Some(text) = string_field(map, "display_name") {
        return non_empty(text);
    }
    let metadata = map
        .get(Value::String("metadata".to_string()))
        .and_then(Value::as_mapping)?;
    for key in ["display_name", "display-name", "title"] {
        if let Some(text) = string_field(metadata, key) {
            return non_empty(text);
        }
    }
    None
}

fn non_empty(text: String) -> Option<String> {
    let text = text.trim().replace(['\r', '\n'], " ");
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANDARD: &str = "---\nname: my-skill\ndescription: |\n  第一行说明\n  第二行说明\nmetadata:\n  display_name: 我的技能\n  author: 谁\n---\n\n# 正文\n\n按自然语言写步骤。\n";

    #[test]
    fn standard_frontmatter_supports_multiline_description_and_metadata() {
        let parsed = parse("my-skill", STANDARD).unwrap();
        assert_eq!(parsed.name, "my-skill");
        assert_eq!(parsed.description, "第一行说明\n第二行说明");
        assert_eq!(parsed.display_name.as_deref(), Some("我的技能"));
        assert!(parsed.body.contains("按自然语言写步骤"));
        // CRLF 与 BOM 照常解析，不丢正文。
        let crlf = format!("\u{feff}{}", STANDARD.replace('\n', "\r\n"));
        let parsed = parse("my-skill", &crlf).unwrap();
        assert_eq!(parsed.description, "第一行说明\n第二行说明");
        assert!(parsed.body.contains("按自然语言写步骤"));
    }

    #[test]
    fn names_are_strict_and_match_the_directory() {
        for name in ["my-skill", "a1", "a-b-c"] {
            assert!(valid_name(name), "{name}");
        }
        for name in [
            "", "-a", "a-", "a--b", "A", "中文", "a_b", "CON", "lpt1.txt",
        ] {
            assert!(!valid_name(name), "{name}");
        }
        assert!(parse("other", STANDARD).is_err());
        assert!(
            parse("my-skill", "---\nname: my-skill\n---\n正文")
                .unwrap_err()
                .contains("description")
        );
        assert!(parse("my-skill", "---\nname: my-skill\ndescription: x\n---\n   ").is_err());
    }

    #[test]
    fn legacy_flow_fields_are_refused_but_prose_about_flow_is_fine() {
        let legacy = "---\nname: my-skill\ndescription: x\nflow:\n  - step: a\n---\n正文";
        let error = parse("my-skill", legacy).unwrap_err();
        assert!(error.contains("旧流程"), "{error}");
        assert!(error.contains("自然语言"), "{error}");
        let prose = "---\nname: my-skill\ndescription: x\n---\n正文里可以说 flow: 是旧格式。\n";
        assert!(parse("my-skill", prose).is_ok());
    }

    #[test]
    fn oversized_frontmatter_is_refused() {
        let padding = "x".repeat(MAX_FRONTMATTER_BYTES + 1);
        let text = format!("---\nname: my-skill\ndescription: {padding}\n---\n正文");
        let error = parse("my-skill", &text).unwrap_err();
        assert!(error.contains("frontmatter"), "{error}");
    }
}
