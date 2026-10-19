//! 导入前的文本清洗：规范化空格与标点、去掉转换器噪音、整理表格。
//!
//! Word / PDF / RTF 经 anydoc 转出来的 Markdown 带着转换器的痕迹：汉字之间夹着
//! 半角空格、中文语境里混着半角标点、目录行与内联锚点、脚注定义、代码围栏行、
//! 列数对不齐的表格。这里全是确定性纯函数，不调用模型，供导入管线与起草页的
//! 「整理结构」共用。
//!
//! 顺序固定为 [`strip_converter_noise`] → [`normalize_spaces`] →
//! [`normalize_punctuation`] → [`normalize_tables`] → [`tidy`]，见
//! [`clean_before_structure`]：先删噪音行，再归一空格与标点，然后重排表格，最后
//! 收拾空行。编号 → Markdown 标题的结构还原由 `doc_import::structure` 在这之后做。

use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

// ---------------------------------------------------------------- 入口

/// 导入与「整理结构」共用的完整清洗：顺序固定，见模块文档。
pub(crate) fn clean_before_structure(markdown: &str) -> String {
    let text = strip_converter_noise(markdown);
    let text = normalize_spaces(&text);
    let text = normalize_punctuation(&text);
    let text = normalize_tables(&text);
    tidy(&text)
}

// ---------------------------------------------------------------- 字符判定

/// 汉字（含扩展 A、兼容区与扩展 B 之后的常用区）。
fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2EBEF | 0x30000..=0x3134F
    )
}

/// 中文标点。
fn is_cjk_punct(ch: char) -> bool {
    matches!(
        ch,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '（'
            | '）'
            | '〈'
            | '〉'
            | '《'
            | '》'
            | '「'
            | '」'
            | '『'
            | '』'
            | '【'
            | '】'
            | '〔'
            | '〕'
            | '〖'
            | '〗'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '…'
            | '—'
            | '·'
            | '～'
    )
}

/// 汉字或中文标点，即「CJK 一侧」。
fn is_cjk_side(ch: char) -> bool {
    is_cjk(ch) || is_cjk_punct(ch)
}

/// 零宽、软连字符与除换行、制表符以外的控制字符：转换器常有，正文里不该出现。
fn is_invisible(ch: char) -> bool {
    matches!(
        ch,
        '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}'
    ) || (ch.is_control() && ch != '\n' && ch != '\t')
}

/// 全角数字 ０-９ 转 ASCII，供编号识别。
fn normalize_digit(ch: char) -> char {
    if ('０'..='９').contains(&ch) {
        char::from_u32('0' as u32 + (ch as u32 - '０' as u32)).unwrap_or(ch)
    } else {
        ch
    }
}

/// 统一换行符。
fn unify_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

// ---------------------------------------------------------------- 空格

/// 规范化空格：删汉字之间、汉字与中文标点之间、汉字与数字之间的空格，压缩行内
/// 连续空格，删零宽字符与控制字符，去行尾空白，全角数字转半角。西文单词之间的
/// 空格保留；行首缩进原样保留（嵌套列表与代码块靠它）。
pub(crate) fn normalize_spaces(text: &str) -> String {
    unify_newlines(text)
        .split('\n')
        .map(normalize_spaces_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_spaces_line(line: &str) -> String {
    let visible: String = line.chars().filter(|ch| !is_invisible(*ch)).collect();
    // 行首缩进有意义，单独摘出来不动。
    let indent_end = visible
        .find(|ch: char| !ch.is_whitespace())
        .unwrap_or(visible.len());
    let (indent, rest) = visible.split_at(indent_end);
    let mut out = String::with_capacity(visible.len());
    out.push_str(indent);
    out.push_str(&collapse_inline_spaces(rest));
    out.trim_end().to_string()
}

fn collapse_inline_spaces(rest: &str) -> String {
    let chars: Vec<char> = rest.chars().collect();
    let mut out = String::with_capacity(rest.len());
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if matches!(ch, ' ' | '\t' | '\u{3000}' | '\u{00A0}') {
            let mut end = index;
            while end < chars.len() && matches!(chars[end], ' ' | '\t' | '\u{3000}' | '\u{00A0}') {
                end += 1;
            }
            // 「附件：1.清单 2.清单」里分隔两个编号项的空格要留着，否则
            // 两项会粘成「清单2.清单」。
            let starts_enumeration = chars.get(end).is_some_and(|ch| ch.is_ascii_digit())
                && chars
                    .get(end + 1)
                    .is_some_and(|ch| matches!(ch, '.' | '．' | '、' | ')' | '）'));
            let keep = match (out.chars().next_back(), chars.get(end).copied()) {
                (Some(left), Some(right)) => {
                    !space_between(left, right) || (starts_enumeration && is_cjk_side(left))
                }
                // 行尾空白不留；行首空白已在摘缩进时处理过。
                _ => false,
            };
            if keep {
                out.push(' ');
            }
            index = end;
            continue;
        }
        out.push(normalize_digit(ch));
        index += 1;
    }
    out
}

/// 这个空格要不要删：
/// - 两侧都在 CJK 一侧（汉字或中文标点）→ 删；
/// - 汉字与半角数字之间（「2026 年」「5 人」）→ 删；
/// - 汉字与百分号 / 千分号之间 → 删；
/// - 其余（西文单词之间、西文与汉字之间）保留。
fn space_between(left: char, right: char) -> bool {
    // 「第一条 为……」这类章条 / 条文标签后的空格是排版用的，不能删。
    if matches!(left, '条' | '章' | '节' | '款' | '项' | '编' | '分') && is_cjk_side(right) {
        return false;
    }
    let left_cjk = is_cjk_side(left);
    let right_cjk = is_cjk_side(right);
    (left_cjk && right_cjk)
        || (left_cjk && right.is_ascii_digit())
        || (left.is_ascii_digit() && right_cjk)
        || (left_cjk && matches!(right, '%' | '‰'))
        || (matches!(left, '%' | '‰') && right_cjk)
}

// ---------------------------------------------------------------- 标点

/// 中文语境下的半角标点转全角：`,` `.` `;` `:` `!` `?` `(` `)`。
///
/// 不动的地方：数字之间（3.14、1,000、14:30）、西文、URL、独占一行的图片引用
/// `![...](...)`、Markdown 链接、代码围栏与行内代码、HTML 注释、行内公式
/// `$...$`、反斜杠转义。
pub(crate) fn normalize_punctuation(text: &str) -> String {
    let text = unify_newlines(text);
    let mut lines: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut in_comment = false;
    for line in text.split('\n') {
        let trimmed = line.trim_start();
        if !in_comment && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = !in_fence;
            lines.push(line.to_string());
            continue;
        }
        if in_fence {
            lines.push(line.to_string());
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        lines.push(convert_punctuation(&chars, &mut in_comment));
    }
    lines.join("\n")
}

fn convert_punctuation(chars: &[char], in_comment: &mut bool) -> String {
    let mut out = String::with_capacity(chars.len());
    let mut index = 0;
    while index < chars.len() {
        // 跨行 HTML 注释：整段原样保留到 --> 为止。
        if *in_comment {
            match find_sequence(chars, index, "-->") {
                Some(end) => {
                    out.extend(&chars[index..end + 3]);
                    *in_comment = false;
                    index = end + 3;
                }
                None => {
                    out.extend(&chars[index..]);
                    return out;
                }
            }
            continue;
        }
        let ch = chars[index];
        // 反斜杠转义：整对跳过。
        if ch == '\\' {
            out.push(ch);
            if let Some(next) = chars.get(index + 1) {
                out.push(*next);
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if ch == '<' && starts_with(chars, index, "<!--") {
            match find_sequence(chars, index + 4, "-->") {
                Some(end) => {
                    out.extend(&chars[index..end + 3]);
                    index = end + 3;
                }
                None => {
                    out.extend(&chars[index..]);
                    *in_comment = true;
                    return out;
                }
            }
            continue;
        }
        if ch == '`' {
            if let Some(end) = find_char(chars, index + 1, '`') {
                out.extend(&chars[index..=end]);
                index = end + 1;
                continue;
            }
            out.push(ch);
            index += 1;
            continue;
        }
        if ch == '$' {
            if let Some(end) = dollar_end(chars, index) {
                out.extend(&chars[index..end]);
                index = end;
                continue;
            }
            out.push(ch);
            index += 1;
            continue;
        }
        if ch == '!'
            && chars.get(index + 1) == Some(&'[')
            && let Some(end) = image_end(chars, index)
        {
            out.extend(&chars[index..end]);
            index = end;
            continue;
        }
        if ch == '['
            && let Some(end) = link_end(chars, index)
        {
            out.extend(&chars[index..end]);
            index = end;
            continue;
        }
        // 裸 URL：保护到空白或右括号、右尖括号、右方括号为止。
        if ch == 'h'
            && (starts_with(chars, index, "http://") || starts_with(chars, index, "https://"))
        {
            let end = chars[index..]
                .iter()
                .position(|ch| ch.is_whitespace() || matches!(ch, ')' | '>' | ']' | '}'))
                .map(|offset| index + offset)
                .unwrap_or(chars.len());
            out.extend(&chars[index..end]);
            index = end;
            continue;
        }
        // 成对括号整体判断语境，避免只转一侧。
        if ch == '('
            && let Some(close) = matching_paren(chars, index)
        {
            let left = last_visible(&out);
            let right = next_visible(chars, close + 1);
            if in_cjk_context(left, right) {
                let mut inner_comment = false;
                out.push('（');
                out.push_str(&convert_punctuation(
                    &chars[index + 1..close],
                    &mut inner_comment,
                ));
                out.push('）');
                index = close + 1;
                continue;
            }
        }
        if let Some(fullwidth) = fullwidth_of(ch) {
            let left = last_visible(&out);
            let right = next_visible(chars, index + 1);
            if should_convert(ch, left, right) {
                out.push(fullwidth);
                index += 1;
                continue;
            }
        }
        if ch == ')' {
            let left = last_visible(&out);
            let right = next_visible(chars, index + 1);
            if in_cjk_context(left, right) {
                out.push('）');
                index += 1;
                continue;
            }
        }
        out.push(ch);
        index += 1;
    }
    out
}

fn fullwidth_of(ch: char) -> Option<char> {
    Some(match ch {
        ',' => '，',
        '.' => '。',
        ';' => '；',
        ':' => '：',
        '!' => '！',
        '?' => '？',
        _ => return None,
    })
}

/// 是否处于中文语境。左侧是中文（或中文标点）算；左侧是西文字母数字则不算，
/// 免得把「Hello, world」里的逗号转全角；左侧没有实际字符时看右侧。
fn in_cjk_context(left: Option<char>, right: Option<char>) -> bool {
    match left {
        Some(ch) if is_cjk_side(ch) => true,
        Some(ch) if ch.is_ascii_alphanumeric() => false,
        _ => right.is_some_and(is_cjk_side),
    }
}

fn should_convert(ch: char, left: Option<char>, right: Option<char>) -> bool {
    if !in_cjk_context(left, right) {
        return false;
    }
    match ch {
        // 数字之间的小数点、千分位逗号、时刻冒号不动。
        ',' if is_digit(left) && is_digit(right) => false,
        '.' if left.is_some_and(|ch| ch.is_ascii_alphanumeric()) => false,
        '.' if right.is_some_and(|ch| ch.is_ascii_alphanumeric()) => false,
        ':' if left.is_some_and(|ch| ch.is_ascii_alphanumeric()) => false,
        ':' if is_digit(left) && is_digit(right) => false,
        _ => true,
    }
}

fn is_digit(ch: Option<char>) -> bool {
    ch.is_some_and(|ch| ch.is_ascii_digit())
}

/// 已经写出的最后一个非空白字符。用输出判断而不是输入：成对括号转全角后，
/// 紧跟其后的标点要看到的是「）」而不是原来的半角「)」。
fn last_visible(out: &str) -> Option<char> {
    out.chars().rev().find(|ch| !ch.is_whitespace())
}

fn next_visible(chars: &[char], from: usize) -> Option<char> {
    chars
        .get(from..)?
        .iter()
        .copied()
        .find(|ch| !ch.is_whitespace())
}

fn starts_with(chars: &[char], index: usize, text: &str) -> bool {
    text.chars()
        .enumerate()
        .all(|(offset, ch)| chars.get(index + offset) == Some(&ch))
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&index| chars[index] == target)
}

fn find_sequence(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || from > chars.len() {
        return None;
    }
    let end = chars.len().saturating_sub(needle.len());
    if from > end {
        return None;
    }
    (from..=end).find(|&start| chars[start..start + needle.len()] == needle[..])
}

/// 行内公式 / 公式块的收尾位置（返回值是开区间终点）。
fn dollar_end(chars: &[char], start: usize) -> Option<usize> {
    if starts_with(chars, start, "$$") {
        return find_sequence(chars, start + 2, "$$").map(|end| end + 2);
    }
    let mut index = start + 1;
    while index < chars.len() {
        if chars[index] == '$' && chars[index - 1] != '\\' {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

/// 图片引用 `![alt](src)` 的收尾位置。
fn image_end(chars: &[char], start: usize) -> Option<usize> {
    let close_bracket = find_char(chars, start + 2, ']')?;
    if chars.get(close_bracket + 1) != Some(&'(') {
        return None;
    }
    let close_paren = find_char(chars, close_bracket + 2, ')')?;
    Some(close_paren + 1)
}

/// Markdown 链接 `[文字](url)` 的收尾位置。
fn link_end(chars: &[char], start: usize) -> Option<usize> {
    let close_bracket = find_char(chars, start + 1, ']')?;
    if chars.get(close_bracket + 1) != Some(&'(') {
        return None;
    }
    let close_paren = find_char(chars, close_bracket + 2, ')')?;
    Some(close_paren + 1)
}

/// 与 `start` 处的左括号配对的右括号位置。
fn matching_paren(chars: &[char], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, ch) in chars[start..].iter().enumerate() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------- 转换器噪音

/// 去掉转换器留下的噪音：
/// - 目录行（点线接尾页码，或指向 `](#_Toc` / `](#` 的链接）；
/// - Markdown 链接只留文字（文字为空时留 URL），删 `<a id="..."></a>` 内联锚点；
/// - 脚注定义收集后把正文 `[^id]` 换成「（注：内容）」，定义行删掉；
/// - 非 mermaid 代码围栏去掉围栏行，内容按普通段落保留；
/// - `---` / `***` / `___` 分隔线整行删；
/// - 保留 `<!-- [...] -->` 标记行与 mermaid 围栏。
pub(crate) fn strip_converter_noise(text: &str) -> String {
    let text = unify_newlines(text);
    let lines: Vec<&str> = text.split('\n').collect();
    let (definitions, dropped) = collect_footnotes(&lines);
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<(&'static str, bool)> = None;
    let mut in_comment = false;
    for (index, raw) in lines.iter().enumerate() {
        let line = *raw;
        let trimmed = line.trim_start();
        if let Some((marker, mermaid)) = fence {
            if trimmed.starts_with(marker) {
                if mermaid {
                    out.push(line.to_string());
                }
                fence = None;
                continue;
            }
            if mermaid {
                out.push(line.to_string());
            } else {
                // 非 mermaid 围栏：围栏行删掉，内容当普通段落保留。
                out.push(line.trim_end().to_string());
            }
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            let language = trimmed[marker.len()..].trim();
            let mermaid = language
                .split_whitespace()
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case("mermaid"));
            if mermaid {
                out.push(line.to_string());
            }
            fence = Some((marker, mermaid));
            continue;
        }
        if is_thematic_break(trimmed) || is_toc_line(trimmed) {
            continue;
        }
        if dropped.get(index).copied().unwrap_or(false) {
            continue;
        }
        match clean_line(line, &definitions, &mut in_comment) {
            Some(cleaned) => out.push(cleaned),
            None => out.push(String::new()),
        }
    }
    out.join("\n")
}

fn fence_marker(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// 把一行的转换器噪音清掉；整行是注释时返回空串，跨行注释用 `in_comment` 记状态。
fn clean_line(
    line: &str,
    definitions: &HashMap<String, String>,
    in_comment: &mut bool,
) -> Option<String> {
    if *in_comment {
        return match line.find("-->") {
            Some(end) => {
                *in_comment = false;
                clean_line(&line[end + 3..], definitions, in_comment)
            }
            None => None,
        };
    }
    if line.trim().is_empty() {
        return Some(String::new());
    }
    let trimmed = line.trim();
    if is_marker_comment(trimmed) {
        return Some(line.trim_end().to_string());
    }
    let mut text = remove_inline_comments(line, in_comment);
    text = anchor_re().replace_all(&text, "").into_owned();
    text = replace_footnote_refs(&text, definitions);
    text = strip_link_markup(&text);
    Some(text.trim_end().to_string())
}

/// `<!-- [附件] -->` 这类结构标记行要原样保留。
fn is_marker_comment(trimmed: &str) -> bool {
    trimmed.starts_with("<!--")
        && trimmed.ends_with("-->")
        && trimmed.contains('[')
        && trimmed.contains(']')
}

/// 删行内 HTML 注释；注释没在这一行收尾时记进 `in_comment`。
fn remove_inline_comments(line: &str, in_comment: &mut bool) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    loop {
        let Some(start) = rest.find("<!--") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start + 4..];
        match after.find("-->") {
            Some(end) => rest = &after[end + 3..],
            None => {
                *in_comment = true;
                break;
            }
        }
    }
    out
}

fn anchor_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)<a\b[^>]*?(?:/>|>[\s\u{200B}\u{FEFF}]*</a>)")
            .expect("内联锚点正则必须合法")
    })
}

fn footnote_definition_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\[\^([^\]\s]+)\]:\s*(.*)$").expect("脚注定义正则必须合法"))
}

fn footnote_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\^([^\]\s]+)\]").expect("脚注引用正则必须合法"))
}

fn link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?P<image>!)?\[(?P<text>[^\]]*)\]\((?P<url>[^)]*)\)")
            .expect("Markdown 链接正则必须合法")
    })
}

fn dotted_leader_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?:\.{2,}|·{2,}|…{2,}|．{2,}|-{3,}|_{3,})\s*\d{1,4}\s*$")
            .expect("目录点线正则必须合法")
    })
}

fn replace_footnote_refs(text: &str, definitions: &HashMap<String, String>) -> String {
    footnote_ref_re()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let id = &captures[1];
            match definitions.get(id) {
                Some(content) if !content.trim().is_empty() => {
                    format!("（注：{}）", content.trim())
                }
                Some(_) => "（注）".to_string(),
                None => captures[0].to_string(),
            }
        })
        .into_owned()
}

/// `[文字](url)` → 文字；`[](url)` → url；图片引用原样保留。
fn strip_link_markup(text: &str) -> String {
    link_re()
        .replace_all(text, |captures: &regex::Captures<'_>| {
            if captures.name("image").is_some() {
                return captures[0].to_string();
            }
            let label = captures.name("text").map(|m| m.as_str()).unwrap_or("");
            if label.trim().is_empty() {
                captures
                    .name("url")
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default()
            } else {
                label.to_string()
            }
        })
        .into_owned()
}

/// 收集脚注定义（`[^id]: 内容`，缩进续行并入内容），并标出要整行删掉的下标。
fn collect_footnotes(lines: &[&str]) -> (HashMap<String, String>, Vec<bool>) {
    let mut definitions: HashMap<String, String> = HashMap::new();
    let mut dropped = vec![false; lines.len()];
    let mut fence: Option<&'static str> = None;
    let mut current: Option<String> = None;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            if trimmed.starts_with(marker) {
                fence = None;
            }
            current = None;
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            current = None;
            continue;
        }
        if let Some(captures) = footnote_definition_re().captures(line) {
            let id = captures[1].trim().to_string();
            let content = captures[2].trim().to_string();
            definitions.insert(id.clone(), content);
            dropped[index] = true;
            current = Some(id);
            continue;
        }
        match &current {
            Some(id) if line.starts_with("    ") || line.starts_with('\t') => {
                let content = line.trim().to_string();
                if let Some(text) = definitions.get_mut(id) {
                    if !text.is_empty() && !content.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&content);
                }
                dropped[index] = true;
            }
            _ => current = None,
        }
    }
    (definitions, dropped)
}

fn is_thematic_break(trimmed: &str) -> bool {
    let marks: String = trimmed.chars().filter(|ch| !ch.is_whitespace()).collect();
    marks.chars().count() >= 3
        && (marks.chars().all(|ch| ch == '-')
            || marks.chars().all(|ch| ch == '*')
            || marks.chars().all(|ch| ch == '_'))
}

fn is_toc_line(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains("](#_Toc") || trimmed.contains("](#_toc") {
        return true;
    }
    let starts_with_link = trimmed.starts_with('[')
        || trimmed.starts_with("* [")
        || trimmed.starts_with("- [")
        || trimmed.starts_with("+ [");
    if starts_with_link && trimmed.contains("](#") {
        return true;
    }
    trimmed.ends_with(|ch: char| ch.is_ascii_digit()) && dotted_leader_re().is_match(trimmed)
}

// ---------------------------------------------------------------- 表格

/// 逐张 GFM 表格按分隔行的列数补齐 / 截断每一行，单元格里的 `<br>` 换成「；」，
/// 去单元格首尾空白；分隔行每格至少 `---` 并保留对齐冒号；表头行全空时把第一
/// 数据行提作表头（只换位，不增删行数）。`^^` 与 `||` 保持原样，非表格行不动。
pub(crate) fn normalize_tables(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<&'static str> = None;
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim_start();
        if let Some(marker) = fence {
            out.push(line.to_string());
            if trimmed.starts_with(marker) {
                fence = None;
            }
            index += 1;
            continue;
        }
        if let Some(marker) = fence_marker(trimmed) {
            fence = Some(marker);
            out.push(line.to_string());
            index += 1;
            continue;
        }
        if index + 1 < lines.len()
            && is_table_header(line)
            && let Some(separator) = parse_separator(lines[index + 1])
        {
            let mut rows: Vec<&str> = vec![line, lines[index + 1]];
            let mut next = index + 2;
            while next < lines.len() && is_table_row(lines[next]) {
                rows.push(lines[next]);
                next += 1;
            }
            out.extend(render_table(&rows, separator));
            index = next;
            continue;
        }
        out.push(line.to_string());
        index += 1;
    }
    out.join("\n")
}

fn is_table_header(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.contains('|')
        && !trimmed.starts_with('#')
        && !trimmed.starts_with('>')
        && !is_thematic_break(trimmed)
}

fn is_table_row(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && trimmed.contains('|')
        && !trimmed.starts_with('#')
        && !trimmed.starts_with('>')
        && !trimmed.starts_with("```")
        && !trimmed.starts_with("~~~")
}

/// 拆一行表格：返回单元格、是否带首尾竖线。`\|` 保持转义不拆。
fn split_row(line: &str) -> (Vec<String>, bool, bool) {
    let trimmed = line.trim();
    let leading = trimmed.starts_with('|');
    let body = if leading { &trimmed[1..] } else { trimmed };
    let trailing = body.ends_with('|') && !body.ends_with("\\|");
    let body = if trailing {
        &body[..body.len() - 1]
    } else {
        body
    };
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = body.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(&next) = chars.peek()
                && next == '|'
            {
                cell.push('\\');
                cell.push('|');
                chars.next();
                continue;
            }
            cell.push(ch);
            continue;
        }
        if ch == '|' {
            cells.push(cell.clone());
            cell.clear();
            continue;
        }
        cell.push(ch);
    }
    cells.push(cell);
    (cells, leading, trailing)
}

fn normalize_cell(cell: &str) -> String {
    fn br_re() -> &'static Regex {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"(?i)<br\b[^>]*>").expect("换行标记正则必须合法"))
    }
    br_re().replace_all(cell, "；").trim().to_string()
}

fn join_row(cells: &[String], leading: bool, trailing: bool) -> String {
    let body = cells.join(" | ");
    match (leading, trailing) {
        (true, true) => format!("| {body} |"),
        (true, false) => format!("| {body}"),
        (false, true) => format!("{body} |"),
        (false, false) => body,
    }
}

/// 解析分隔行：返回归一后的单元格、首尾竖线。不是分隔行返回 `None`。
fn parse_separator(line: &str) -> Option<(Vec<String>, bool, bool)> {
    let (cells, leading, trailing) = split_row(line);
    let mut normalized = Vec::with_capacity(cells.len());
    for cell in &cells {
        let cell = cell.trim();
        let left = cell.starts_with(':');
        let right = cell.ends_with(':') && cell.len() > 1;
        let core = cell.trim_matches(':').trim();
        if core.is_empty() || !core.chars().all(|ch| ch == '-') {
            return None;
        }
        let mut out = String::new();
        if left {
            out.push(':');
        }
        out.push_str(&"-".repeat(core.chars().count().max(3)));
        if right {
            out.push(':');
        }
        normalized.push(out);
    }
    Some((normalized, leading, trailing))
}

fn render_table(rows: &[&str], separator: (Vec<String>, bool, bool)) -> Vec<String> {
    let (specs, separator_leading, separator_trailing) = separator;
    let columns = specs.len().max(1);
    let mut rendered: Vec<String> = Vec::with_capacity(rows.len());
    for (row_index, row) in rows.iter().enumerate() {
        // `||` 是合并单元格的占位，整行原样保留。
        if row.contains("||") {
            rendered.push(row.trim_end().to_string());
            continue;
        }
        if row_index == 1 {
            let mut cells = specs.clone();
            cells.resize(columns, "---".to_string());
            rendered.push(join_row(&cells, separator_leading, separator_trailing));
            continue;
        }
        let (cells, leading, trailing) = split_row(row);
        let mut cells: Vec<String> = cells.iter().map(|cell| normalize_cell(cell)).collect();
        cells.resize(columns, String::new());
        rendered.push(join_row(&cells, leading, trailing));
    }
    // 表头行全空：把第一数据行提作表头，只换位，行数不变。
    if rendered.len() > 2 && header_is_empty(rows[0]) {
        rendered.swap(0, 2);
    }
    rendered
}

fn header_is_empty(row: &str) -> bool {
    let (cells, _, _) = split_row(row);
    cells.iter().all(|cell| normalize_cell(cell).is_empty())
}

// ---------------------------------------------------------------- 收尾

/// 连续空行压成一行、去行尾空白、去首尾空行；非空时以单个换行结尾。
pub(crate) fn tidy(text: &str) -> String {
    let text = unify_newlines(text);
    let mut lines: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end();
        if line.is_empty() && lines.last().is_some_and(|last| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        String::new()
    } else {
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空格_删汉字与数字之间的空格() {
        assert_eq!(normalize_spaces("2026 年 5 人"), "2026年5人");
        assert_eq!(normalize_spaces("工作 的 通知"), "工作的通知");
        assert_eq!(normalize_spaces("正文  ，下一句"), "正文，下一句");
    }

    #[test]
    fn 空格_保留西文单词之间的空格() {
        assert_eq!(normalize_spaces("Hello world 再次"), "Hello world 再次");
        assert_eq!(normalize_spaces("PDF 文件"), "PDF 文件");
    }

    #[test]
    fn 空格_压缩连续空格并去行尾空白() {
        assert_eq!(normalize_spaces("foo   bar  "), "foo bar");
        assert_eq!(normalize_spaces("第一段   \n\n第二段"), "第一段\n\n第二段");
    }

    #[test]
    fn 空格_全角数字转半角并删零宽字符() {
        assert_eq!(
            normalize_spaces("２０２６年\u{200B}５人\u{FEFF}"),
            "2026年5人"
        );
    }

    #[test]
    fn 空格_保留行首缩进() {
        assert_eq!(normalize_spaces("  - 第一项"), "  - 第一项");
    }

    #[test]
    fn 标点_中文语境转全角() {
        assert_eq!(
            normalize_punctuation("工作,通知;时间:上午(附件)!"),
            "工作，通知；时间：上午（附件）！"
        );
    }

    #[test]
    fn 标点_数字西文与网址不动() {
        assert_eq!(
            normalize_punctuation("3.14 与 1,000、14:30"),
            "3.14 与 1,000、14:30"
        );
        assert_eq!(normalize_punctuation("e.g. 示例"), "e.g. 示例");
        let raw = "见 http://example.com/a?b=1&c=2 的说明";
        assert_eq!(normalize_punctuation(raw), raw);
    }

    #[test]
    fn 标点_受保护片段不动() {
        let raw = "![图](图 1.png) 与 [链接](http://a.b/c?d=1) 与 `a,b` 与 $x,y$ 与 \\. 与 <!-- 注,释 -->";
        assert_eq!(normalize_punctuation(raw), raw);
    }

    #[test]
    fn 标点_代码围栏内不动() {
        let raw = "```\nif a,b: pass\n```\n正文,下一句";
        assert_eq!(
            normalize_punctuation(raw),
            "```\nif a,b: pass\n```\n正文，下一句"
        );
    }

    #[test]
    fn 噪音_删目录行与分隔线() {
        let raw = "目录\n\n一、总体要求........1\n\n[二、工作安排](#_Toc123)\n\n---\n\n正文。";
        let cleaned = strip_converter_noise(raw);
        assert!(!cleaned.contains("总体要求........1"), "{cleaned}");
        assert!(!cleaned.contains("_Toc"), "{cleaned}");
        assert!(!cleaned.contains("---"), "{cleaned}");
        assert!(cleaned.contains("正文。"), "{cleaned}");
    }

    #[test]
    fn 噪音_链接留文字且删内联锚点() {
        let raw = r#"<a id="_Toc1"></a>见[附件说明](files/a.docx)与[](https://example.com)。"#;
        assert_eq!(
            strip_converter_noise(raw),
            "见附件说明与https://example.com。"
        );
    }

    #[test]
    fn 噪音_脚注转注文并删定义行() {
        let raw = "正文[^1]。\n\n[^1]: 这是脚注内容。";
        let cleaned = strip_converter_noise(raw);
        assert_eq!(cleaned, "正文（注：这是脚注内容。）。\n");
    }

    #[test]
    fn 噪音_非mermaid围栏去围栏且mermaid保留() {
        let raw = "```text\n1.代码\n```\n\n```mermaid\ngraph TD;\nA-->B;\n```";
        let cleaned = strip_converter_noise(raw);
        assert!(!cleaned.contains("```text"), "{cleaned}");
        assert!(cleaned.contains("1.代码"), "{cleaned}");
        assert!(cleaned.contains("```mermaid"), "{cleaned}");
        assert!(cleaned.contains("A-->B;"), "{cleaned}");
    }

    #[test]
    fn 噪音_保留附件标记行并删其余注释() {
        let raw = "<!-- [附件] -->\n\n正文。\n<!-- 隐藏说明 -->";
        let cleaned = strip_converter_noise(raw);
        assert!(cleaned.contains("<!-- [附件] -->"), "{cleaned}");
        assert!(!cleaned.contains("隐藏说明"), "{cleaned}");
    }

    #[test]
    fn 表格_按列数补齐截断并转br() {
        let raw =
            "| 序号 | 事项 | 备注 |\n| --- | --- | --- |\n| 1 | 报送<br>审核 | 无 |\n| 2 | 完成 |";
        assert_eq!(
            normalize_tables(raw),
            "| 序号 | 事项 | 备注 |\n| --- | --- | --- |\n| 1 | 报送；审核 | 无 |\n| 2 | 完成 |  |"
        );
    }

    #[test]
    fn 表格_空表头提第一数据行() {
        let raw = "|  |  |\n| --- | --- |\n| 序号 | 事项 |\n| 1 | 报送 |";
        assert_eq!(
            normalize_tables(raw),
            "| 序号 | 事项 |\n| --- | --- |\n|  |  |\n| 1 | 报送 |"
        );
    }

    #[test]
    fn 表格_分隔行最短三横线并保留对齐() {
        let raw = "a | b | c\n:- | :-: | -:\n1 | 2 | 3";
        assert_eq!(
            normalize_tables(raw),
            "a | b | c\n:--- | :---: | ---:\n1 | 2 | 3"
        );
    }

    #[test]
    fn 表格_列数按分隔行截断() {
        let raw = "| a |\n| --- |\n| 1 | 2 | 3 |";
        assert_eq!(normalize_tables(raw), "| a |\n| --- |\n| 1 |");
    }

    #[test]
    fn 表格_合并占位符与非表格行不动() {
        let raw = "正文\n\n| a | b |\n| --- | --- |\n| ^^ | x || y |\n\n结尾";
        let cleaned = normalize_tables(raw);
        assert!(cleaned.contains("| ^^ | x || y |"), "{cleaned}");
        assert!(cleaned.contains("正文"), "{cleaned}");
        assert!(cleaned.contains("结尾"), "{cleaned}");
    }

    #[test]
    fn 收尾_压缩空行并统一结尾() {
        assert_eq!(tidy("\n\n第一段  \n\n\n\n第二段\n\n"), "第一段\n\n第二段\n");
        assert_eq!(tidy("   \n\n"), "");
    }

    #[test]
    fn 全流程_按固定顺序清洗() {
        let raw = "工作 的 通知\n\n［目录］\n\n一、总体要求 ....1\n\n正文，请详见下表。\n\n| 项目 | 说明 |\n| --- | --- |\n| 甲 | 乙<br>丙 |";
        let cleaned = clean_before_structure(raw);
        assert!(cleaned.starts_with("工作的通知"), "{cleaned}");
        assert!(cleaned.contains("正文，请详见下表。"), "{cleaned}");
        assert!(!cleaned.contains("....1"), "{cleaned}");
        assert!(cleaned.contains("| 甲 | 乙；丙 |"), "{cleaned}");
        assert!(cleaned.ends_with('\n'), "{cleaned}");
    }
}
