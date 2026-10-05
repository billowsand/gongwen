//! 确定性扫描：从接口文档里找 cURL 命令、「GET /路径」式的请求行、JSON 示例与服务器地址。
//!
//! 这一步不调模型，认出来的东西来源可靠，后面合并时优先于模型的整理。

use crate::agent::api::ApiMethod;
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// 文档里的一个请求。
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct RawRequest {
    /// 文档里明确写了方法才有；cURL 带 `-d` 算 POST。
    pub(crate) method: Option<ApiMethod>,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Option<String>,
    /// 在原文里的位置：把后面的返回示例挂到最近的请求上。
    pub(crate) offset: usize,
    /// 认出来但不支持的写法（`-u 用户名:密码`、表单请求体……）。
    pub(crate) notes: Vec<String>,
}

/// 文档里的一段 JSON 示例。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonBlock {
    pub(crate) value: Value,
    pub(crate) offset: usize,
    pub(crate) role: BlockRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockRole {
    /// 前文说的是「返回 / 响应」。
    Response,
    /// 前文说的是「请求体 / 入参」。
    Request,
    Unknown,
}

static CURL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)(^|[\s`>$])curl\s").expect("curl 正则"));
static REQUEST_LINE: LazyLock<Regex> = LazyLock::new(|| {
    // 行首直接写，或者前面有个短标签（「请求方式：」「删除记录：」）。
    Regex::new(r"(?m)^(?:[ \t>*`|-]*|[^\n]{0,24}?[:：][ \t]*)(GET|POST|PUT|DELETE|PATCH)[ \t]+(https?://\S+|/\S*)")
        .expect("请求行正则")
});
static ORIGIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://[A-Za-z0-9.\-]+(?::\d+)?").expect("地址正则"));

/// 找出全部请求，按在原文里的先后排好。
pub(crate) fn requests(text: &str) -> Vec<RawRequest> {
    let mut found: Vec<RawRequest> = Vec::new();
    for caps in CURL.captures_iter(text) {
        let start = caps.get(0).expect("整段").start() + caps[1].len();
        if let Some(request) = parse_curl(&curl_command(&text[start..]), start) {
            found.push(request);
        }
    }
    for caps in REQUEST_LINE.captures_iter(text) {
        let offset = caps.get(1).expect("方法").start();
        // cURL 那一行已经认过了。
        if same_line_curl(text, offset) {
            continue;
        }
        let url = caps[2]
            .trim_end_matches(['`', '"', '\'', ',', '，', '。', ')', '）'])
            .to_string();
        found.push(RawRequest {
            method: method_of(&caps[1]),
            url,
            offset,
            ..RawRequest::default()
        });
    }
    found.sort_by_key(|r| r.offset);
    found
}

fn same_line_curl(text: &str, offset: usize) -> bool {
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    text[line_start..line_end].contains("curl")
}

fn method_of(word: &str) -> Option<ApiMethod> {
    match word.to_ascii_uppercase().as_str() {
        "GET" => Some(ApiMethod::Get),
        "POST" => Some(ApiMethod::Post),
        // PUT / DELETE / PATCH 不是查询，这里记成 None，后面按「不是查询」处理。
        _ => None,
    }
}

/// 文档里写的方法是不是改数据的（PUT / DELETE / PATCH）。
pub(crate) fn write_method_near(text: &str, offset: usize) -> bool {
    let end = text[offset..].find('\n').map_or(text.len(), |i| offset + i);
    let line = text[offset..end].to_ascii_uppercase();
    [
        "PUT ",
        "DELETE ",
        "PATCH ",
        "-X PUT",
        "-X DELETE",
        "-X PATCH",
    ]
    .iter()
    .any(|word| line.contains(word))
}

/// 从 `curl` 开始取一条命令：行尾 `\`（bash）、`^`（cmd）、`` ` ``（PowerShell）续行。
fn curl_command(rest: &str) -> String {
    let mut command = String::new();
    for line in rest.lines() {
        let trimmed = line.trim_end();
        if let Some(head) = trimmed
            .strip_suffix('\\')
            .or_else(|| trimmed.strip_suffix('^'))
            .or_else(|| trimmed.strip_suffix('`'))
        {
            command.push_str(head);
            command.push(' ');
        } else {
            command.push_str(trimmed);
            break;
        }
    }
    command
}

/// 按 shell 规则切词：单引号原样，双引号里认 `\"`。
fn shell_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    current.push(c);
                }
            }
            '"' => {
                started = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\' | '$' | '`')) => {
                            current.push(chars.next().expect("刚看过"));
                        }
                        other => current.push(other),
                    }
                }
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    started = true;
                }
            }
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            other => {
                current.push(other);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

fn parse_curl(command: &str, offset: usize) -> Option<RawRequest> {
    let words = shell_words(command);
    let mut words = words.into_iter().skip(1);
    let mut request = RawRequest {
        offset,
        ..RawRequest::default()
    };
    let mut explicit_method = None;
    let mut get_flag = false;
    let mut data: Vec<String> = Vec::new();
    while let Some(word) = words.next() {
        match word.as_str() {
            "-X" | "--request" => explicit_method = words.next(),
            "-H" | "--header" => {
                if let Some(header) = words.next()
                    && let Some((name, value)) = header.split_once(':')
                {
                    request
                        .headers
                        .push((name.trim().to_string(), value.trim().to_string()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii"
            | "--data-urlencode" => data.extend(words.next()),
            "--json" => {
                data.extend(words.next());
                request
                    .headers
                    .push(("Content-Type".into(), "application/json".into()));
            }
            "-b" | "--cookie" => {
                if let Some(cookie) = words.next() {
                    request.headers.push(("Cookie".into(), cookie));
                }
            }
            "-u" | "--user" => {
                words.next();
                request
                    .notes
                    .push("cURL 里的 -u 用户名密码登录暂不支持，需要改成请求头里的令牌".into());
            }
            "--url" => {
                if let Some(url) = words.next() {
                    request.url = url;
                }
            }
            "-G" | "--get" => get_flag = true,
            "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-A"
            | "--user-agent" | "-e" | "--referer" | "-w" | "--write-out" | "--retry" => {
                words.next();
            }
            other if other.starts_with("http://") || other.starts_with("https://") => {
                request.url = other.to_string();
            }
            _ => {}
        }
    }
    if request.url.is_empty() {
        return None;
    }
    let body = data.join("&");
    if get_flag && !body.is_empty() {
        let joiner = if request.url.contains('?') { '&' } else { '?' };
        request.url = format!("{}{joiner}{body}", request.url);
    } else if !body.is_empty() {
        request.body = Some(body);
    }
    request.method = match explicit_method {
        Some(method) => method_of(&method),
        None if request.body.is_some() => Some(ApiMethod::Post),
        None => Some(ApiMethod::Get),
    };
    Some(request)
}

/// 文档里出现过的服务器地址（`http://主机:端口`），去重、按出现先后。
pub(crate) fn origins(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for m in ORIGIN.find_iter(text) {
        let origin = m.as_str().trim_end_matches('.').to_string();
        if !found.contains(&origin) {
            found.push(origin);
        }
    }
    found
}

/// 找出文档里能解析的 JSON 示例（对象或数组，至少两个字符以上）。
pub(crate) fn json_blocks(text: &str) -> Vec<JsonBlock> {
    let mut blocks = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if (c == b'{' || c == b'[')
            && at_line_start(text, i)
            && let Some(end) = balanced_end(text, i)
            && let Ok(value) = serde_json::from_str::<Value>(&text[i..end])
            && (value.is_object() || value.is_array())
        {
            blocks.push(JsonBlock {
                value,
                offset: i,
                role: role_before(text, i),
            });
            i = end;
            continue;
        }
        i += 1;
    }
    blocks
}

/// `{` 前面同一行只有空白、引用号或列表符号（也认 `返回示例：{` 这种同行写法）。
fn at_line_start(text: &str, index: usize) -> bool {
    let line_start = text[..index].rfind('\n').map_or(0, |i| i + 1);
    let before = text[line_start..index].trim();
    before.is_empty()
        || before.ends_with(':')
        || before.ends_with('：')
        || before.chars().all(|c| matches!(c, '>' | '*' | '-' | '|'))
}

/// 从 `start` 的 `{` / `[` 找到配对的收尾（跳过字符串里的括号），返回收尾之后的位置。
pub(crate) fn balanced_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, c) in text[start..].char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + offset + c.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

/// 看示例前面 160 个字符里最后提到的是「返回」还是「请求」。
fn role_before(text: &str, index: usize) -> BlockRole {
    let start = text[..index]
        .char_indices()
        .rev()
        .nth(160)
        .map_or(0, |(i, _)| i);
    let before = text[start..index].to_lowercase();
    let last = |words: &[&str]| words.iter().filter_map(|w| before.rfind(w)).max();
    let response = last(&["返回", "响应", "response", "输出", "结果示例", "出参"]);
    let request = last(&[
        "请求体",
        "请求参数",
        "请求示例",
        "入参",
        "request",
        "body",
        "提交",
    ]);
    match (response, request) {
        (Some(a), Some(b)) if a > b => BlockRole::Response,
        (Some(_), None) => BlockRole::Response,
        (_, Some(_)) => BlockRole::Request,
        _ => BlockRole::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn curl_commands_across_lines_are_parsed() {
        let text = "政策检索接口：\n\n```bash\ncurl -X POST 'http://10.0.0.9:8080/api/policy/search' \\\n  -H 'Authorization: Bearer abc.def' \\\n  -H \"Content-Type: application/json\" \\\n  -d '{\"keyword\": \"中小企业\", \"size\": 10}'\n```\n";
        let found = requests(text);
        assert_eq!(found.len(), 1, "{found:?}");
        let request = &found[0];
        assert_eq!(request.method, Some(ApiMethod::Post));
        assert_eq!(request.url, "http://10.0.0.9:8080/api/policy/search");
        assert_eq!(
            request.headers[0],
            ("Authorization".to_string(), "Bearer abc.def".to_string())
        );
        assert_eq!(
            request.body.as_deref(),
            Some("{\"keyword\": \"中小企业\", \"size\": 10}")
        );
    }

    #[test]
    fn a_curl_without_method_is_get_and_data_makes_it_post() {
        let get = requests("curl \"http://x/api/stat?region=全省&year=2025\"");
        assert_eq!(get[0].method, Some(ApiMethod::Get));
        assert_eq!(get[0].url, "http://x/api/stat?region=全省&year=2025");
        let post = requests("curl http://x/api --data-raw '{\"a\":1}'");
        assert_eq!(post[0].method, Some(ApiMethod::Post));
        let get_flag = requests("curl -G http://x/api -d region=全省");
        assert_eq!(get_flag[0].url, "http://x/api?region=全省");
        assert!(get_flag[0].body.is_none());
        let user = requests("curl -u admin:123 http://x/api");
        assert!(user[0].notes[0].contains("-u"));
    }

    #[test]
    fn request_lines_and_origins_are_found() {
        let text = "服务地址：http://10.1.2.3:9000\n\n## 月度统计\n\n请求方式：GET /api/stat/monthly?month=2026-09\n\n## 删除记录\n\nDELETE /api/stat/1\n";
        let found = requests(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].method, Some(ApiMethod::Get));
        assert_eq!(found[0].url, "/api/stat/monthly?month=2026-09");
        assert_eq!(found[1].method, None, "DELETE 不认成查询");
        assert!(write_method_near(text, found[1].offset));
        assert_eq!(origins(text), ["http://10.1.2.3:9000"]);
    }

    #[test]
    fn json_examples_are_found_with_their_role() {
        let text = "请求体：\n{\"keyword\": \"a\"}\n\n返回示例：\n```json\n{\n  \"code\": 0,\n  \"data\": {\"items\": [{\"id\": 1, \"title\": \"x{y}\"}]}\n}\n```\n正文里的 {不是 json} 不算。";
        let blocks = json_blocks(text);
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(blocks[0].role, BlockRole::Request);
        assert_eq!(blocks[0].value, json!({"keyword": "a"}));
        assert_eq!(blocks[1].role, BlockRole::Response);
        assert_eq!(blocks[1].value["data"]["items"][0]["title"], "x{y}");
    }
}
