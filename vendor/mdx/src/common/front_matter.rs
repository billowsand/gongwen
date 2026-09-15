/// Markdown 顶部 front matter 中的共享元数据。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub security: Option<String>,
    /// 保密年限，与密级分列两个变量；排版时以五角星连接（如 机密★5年）
    pub security_years: Option<String>,
    pub doc_type: Option<String>,
    pub doc_number: Option<String>,
    pub version: Option<String>,
    pub institution: Option<String>,
    pub date: Option<String>,
    pub title: Option<String>,
    pub bibliography: Option<String>,
}

/// 解析并剥离文档开头由 `---` 包围的简单 `键: 值` front matter。
///
/// 未闭合的 front matter 按普通 Markdown 原样返回。
pub fn parse(content: &str) -> (Metadata, String) {
    let mut lines = content.lines();
    if lines
        .next()
        .map(|line| line.trim_start_matches('\u{feff}').trim())
        != Some("---")
    {
        return (Metadata::default(), content.to_string());
    }

    let mut metadata = Metadata::default();
    let mut consumed = 1usize;
    let mut closed = false;

    for line in lines {
        consumed += 1;
        let trimmed = line.trim();
        if trimmed == "---" {
            closed = true;
            break;
        }
        let Some((key, value)) = trimmed.split_once([':', '：']) else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "密级" | "security" => metadata.security = Some(value.into()),
            "年限" | "保密年限" | "保密期限" | "years" => {
                metadata.security_years = Some(value.into())
            }
            "文件类型" | "类型" | "doctype" => metadata.doc_type = Some(value.into()),
            "文件编号" | "编号" | "number" => metadata.doc_number = Some(value.into()),
            "文件版本号" | "版本" | "version" => metadata.version = Some(value.into()),
            "撰写单位" | "单位" | "institution" => metadata.institution = Some(value.into()),
            "撰写时间" | "时间" | "日期" | "date" => metadata.date = Some(value.into()),
            "文件名称" | "标题" | "title" => metadata.title = Some(value.into()),
            "bibliography" => metadata.bibliography = Some(value.into()),
            _ => {}
        }
    }

    if !closed {
        return (Metadata::default(), content.to_string());
    }

    let body = content
        .lines()
        .skip(consumed)
        .collect::<Vec<_>>()
        .join("\n");
    (metadata, body)
}

/// 把常见年月写法统一为“YYYY 年 M 月”；无法识别时保留原文。
pub fn normalize_date(raw: &str) -> String {
    if let Ok(re) = regex::Regex::new(r"^(\d{4})\s*[-/.年]\s*(\d{1,2})\s*月?$") {
        if let Some(caps) = re.captures(raw.trim()) {
            let month: u32 = caps[2].parse().unwrap_or(0);
            return format!("{} 年 {} 月", &caps[1], month);
        }
    }
    raw.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_bibliography_and_removes_front_matter() {
        let md = "---\nbibliography: refs/library.bib\n密级: 内部\n---\n# 标题\n";
        let (meta, body) = parse(md);
        assert_eq!(meta.bibliography.as_deref(), Some("refs/library.bib"));
        assert_eq!(meta.security.as_deref(), Some("内部"));
        assert_eq!(body, "# 标题");
    }

    #[test]
    fn parses_security_and_years_as_separate_fields() {
        let md = "---\n密级: 机密\n年限: 5年\n---\n# 标题\n";
        let (meta, _body) = parse(md);
        assert_eq!(meta.security.as_deref(), Some("机密"));
        assert_eq!(meta.security_years.as_deref(), Some("5年"));
    }

    #[test]
    fn security_years_accepts_aliases() {
        for key in ["年限", "保密年限", "保密期限", "years"] {
            let md = format!("---\n{}: 长期\n---\n# 标题\n", key);
            let (meta, _body) = parse(&md);
            assert_eq!(meta.security_years.as_deref(), Some("长期"), "键 {}", key);
        }
    }

    #[test]
    fn parses_front_matter_after_utf8_bom() {
        let md = "\u{feff}---\nbibliography: refs.bib\n---\n正文 [@a]\n";
        let (meta, body) = parse(md);
        assert_eq!(meta.bibliography.as_deref(), Some("refs.bib"));
        assert_eq!(body, "正文 [@a]");
    }

    #[test]
    fn unclosed_front_matter_is_left_untouched() {
        let md = "---\nbibliography: refs.bib\n正文\n";
        let (meta, body) = parse(md);
        assert_eq!(meta, Metadata::default());
        assert_eq!(body, md);
    }

    #[test]
    fn normalizes_cover_date_to_month() {
        assert_eq!(normalize_date("2026-07"), "2026 年 7 月");
        assert_eq!(normalize_date("2026/7"), "2026 年 7 月");
        assert_eq!(normalize_date("2026年07月"), "2026 年 7 月");
        assert_eq!(normalize_date("二〇二六年七月"), "二〇二六年七月");
    }
}
