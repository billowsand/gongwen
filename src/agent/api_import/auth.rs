//! 鉴权：从文档里认出「密钥怎么带」（请求头名或查询参数名、是否 Bearer），套到同一份
//! 文档里还没带鉴权的接口上。接口文档常在开头单独写一节「认证方式」，下面的接口不再重复，
//! 只看单个请求就会漏掉。
//!
//! 认不出的交给人在界面上补（[`apply`]）；要签名或要先登录换令牌的，程序做不了，说清楚。

use super::redact::is_secret_key;
use crate::agent::api::{ApiEndpoint, ApiHeader};
use regex::Regex;
use std::sync::LazyLock;

static AUTH_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)鉴权|认证|授权|身份验证|令牌|密钥|秘钥|token|api[ _-]?key|app[ _-]?key|access[ _-]?key|authorization")
        .expect("鉴权词正则")
});
/// `X-API-Key: <你的密钥>`、`Authorization：Bearer xxx`。
static NAME_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\s>*`|+-]*([A-Za-z][A-Za-z0-9_-]{1,40})`?\s*[:：]\s*(.*)$").expect("名值正则")
});
/// 「在请求头中携带 X-API-Key」「Header 里加 `token`」。
static HEADER_SENTENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:请求头|header|headers|http\s*头|头部|头信息)[^。\n]{0,30}?([A-Za-z][A-Za-z0-9_-]{1,40})")
        .expect("请求头句正则")
});
/// 「在地址参数中带 access_token」「URL 参数 appKey」。
static QUERY_SENTENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:url|地址|查询|query)\s*(?:参数|中|里|上)[^。\n]{0,30}?([A-Za-z][A-Za-z0-9_]{1,40})")
        .expect("查询参数句正则")
});
static TOKEN_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._~+/=\-]{6,}$").expect("令牌正则"));

/// 密钥放在哪。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AuthPlace {
    #[default]
    Header,
    Query,
}

impl AuthPlace {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Header => "请求头",
            Self::Query => "地址参数",
        }
    }
}

/// 一种鉴权方式。
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct AuthSpec {
    pub(crate) place: AuthPlace,
    /// 请求头名或参数名。
    pub(crate) name: String,
    /// 请求头的值写成 `Bearer 密钥`。
    pub(crate) bearer: bool,
    /// 文档原话，给人看依据。
    pub(crate) basis: String,
    /// 文档里写的值；占位写法为空。
    pub(crate) value: String,
}

impl AuthSpec {
    /// 「请求头 X-API-Key」「请求头 Authorization: Bearer」「地址参数 access_token」。
    pub(crate) fn describe(&self) -> String {
        match (self.place, self.bearer) {
            (AuthPlace::Header, true) => format!("请求头 {}: Bearer 密钥", self.name),
            _ => format!("{} {}", self.place.label(), self.name),
        }
    }

    /// 名字合不合用：英文字母开头，只有字母、数字、下划线和短横线（参数名不带短横线）。
    pub(crate) fn valid_name(&self) -> bool {
        let name = self.name.trim();
        name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name.len() <= 60
            && name.chars().all(|c| {
                c.is_ascii_alphanumeric()
                    || c == '_'
                    || (c == '-' && self.place == AuthPlace::Header)
            })
    }
}

/// 不是鉴权字段的请求头名（句子里常跟在「请求头」后面）。
fn ordinary(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "http" | "https" | "header" | "headers" | "content-type" | "accept" | "json" | "bearer"
    )
}

fn token_value(value: &str) -> (bool, String) {
    let value = value
        .trim()
        .trim_matches(['`', '"', '\'', '“', '”', '「', '」'])
        .trim();
    let (bearer, rest) = match value.get(..7) {
        Some(head) if head.eq_ignore_ascii_case("bearer ") => (true, value[7..].trim()),
        _ => (false, value),
    };
    let rest = rest.trim_matches(['`', '"', '\'']).trim();
    (
        bearer,
        if TOKEN_LIKE.is_match(rest) && !rest.chars().all(|c| matches!(c, 'x' | 'X' | '*' | '.')) {
            rest.to_string()
        } else {
            String::new()
        },
    )
}

fn clip(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() > 80 {
        line.chars().take(80).collect::<String>() + "…"
    } else {
        line.to_string()
    }
}

/// 从一行里认鉴权字段。`window` 是这一行附近的文字，用来判断放在请求头还是地址参数。
fn from_line(line: &str, window: &str) -> Option<AuthSpec> {
    let trimmed = line.trim();
    let lower_window = window.to_lowercase();
    let says_header = ["请求头", "header", "头部", "头信息"]
        .iter()
        .any(|w| lower_window.contains(w));
    let says_query = ["参数", "query", "url", "地址栏"]
        .iter()
        .any(|w| lower_window.contains(w));
    let place = if says_query && !says_header {
        AuthPlace::Query
    } else {
        AuthPlace::Header
    };
    let bearer_mentioned = lower_window.contains("bearer");
    // 表格行：第一格是字段名。
    if trimmed.starts_with('|') {
        if !says_header && !lower_window.contains("鉴权") && !lower_window.contains("认证") {
            return None;
        }
        let cells: Vec<&str> = trimmed
            .trim_matches('|')
            .split('|')
            .map(|c| c.trim().trim_matches('`'))
            .collect();
        let name = cells.first()?.trim_matches('*');
        if !is_secret_key(name) || ordinary(name) || name.contains(' ') {
            return None;
        }
        let (bearer, value) = cells[1..]
            .iter()
            .map(|c| token_value(c))
            .find(|(_, v)| !v.is_empty())
            .unwrap_or((false, String::new()));
        return Some(AuthSpec {
            place,
            name: name.to_string(),
            bearer: bearer || (bearer_mentioned && place == AuthPlace::Header),
            basis: clip(line),
            value,
        });
    }
    // JSON 里的字段（登录接口的返回之类）不算。
    if !trimmed.starts_with(['"', '\'', '{'])
        && let Some(caps) = NAME_VALUE.captures(trimmed)
    {
        let name = &caps[1];
        if is_secret_key(name)
            && !ordinary(name)
            && (says_header || says_query || name.eq_ignore_ascii_case("authorization"))
        {
            let (bearer, value) = token_value(&caps[2]);
            return Some(AuthSpec {
                place,
                name: name.to_string(),
                bearer: bearer
                    || (place == AuthPlace::Header
                        && bearer_mentioned
                        && name.eq_ignore_ascii_case("authorization")),
                basis: clip(line),
                value,
            });
        }
    }
    for (regex, place) in [
        (&*HEADER_SENTENCE, AuthPlace::Header),
        (&*QUERY_SENTENCE, AuthPlace::Query),
    ] {
        for caps in regex.captures_iter(trimmed) {
            let name = &caps[1];
            if is_secret_key(name) && !ordinary(name) {
                return Some(AuthSpec {
                    place,
                    name: name.to_string(),
                    bearer: place == AuthPlace::Header && bearer_mentioned,
                    basis: clip(line),
                    value: String::new(),
                });
            }
        }
    }
    None
}

/// 文档里的鉴权方式，以及程序做不了的情况（签名、先登录换令牌）。
pub(crate) fn detect(text: &str) -> (Option<AuthSpec>, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = None;
    'outer: for (index, line) in lines.iter().enumerate() {
        if !AUTH_WORD.is_match(line) {
            continue;
        }
        let end = (index + 8).min(lines.len());
        // 往前多看几行：「公共请求头」这类小标题常在表格上面。
        let window = lines[index.saturating_sub(6)..end].join("\n");
        for candidate in &lines[index..end] {
            if let Some(spec) = from_line(candidate, &window) {
                found = Some(spec);
                break 'outer;
            }
        }
    }
    let mut notes = Vec::new();
    let lower: String = text
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let signs = ["签名", "signature", "sign="]
        .iter()
        .any(|w| lower.contains(w));
    let digest = [
        "时间戳",
        "timestamp",
        "nonce",
        "hmac",
        "md5",
        "sha256",
        "sha1",
    ]
    .iter()
    .any(|w| lower.contains(w));
    if signs && digest {
        notes.push(
            "文档要求按时间戳等计算签名，程序暂不会自动签名：请向接口方要一个固定的令牌或 Key。"
                .into(),
        );
    }
    if [
        "登录接口",
        "获取token",
        "获取令牌",
        "换取token",
        "换取令牌",
        "获取access_token",
        "token接口",
        "令牌接口",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        notes.push(
            "令牌要先调登录接口换取，程序暂不会自动换：请先用账号取得令牌，粘贴到「鉴权」里；令牌过期后要重新粘贴。"
                .into(),
        );
    }
    (found, notes)
}

/// 接口带没带鉴权：模板里有没有 `{secret:…}`。
pub(crate) fn has_auth(endpoint: &ApiEndpoint) -> bool {
    !endpoint.secret_names().is_empty()
}

/// 给接口加上鉴权，密钥名 `secret`。已经带了同名请求头 / 参数的，把值换成密钥引用。
pub(crate) fn apply(endpoint: &mut ApiEndpoint, spec: &AuthSpec, secret: &str) {
    let reference = format!("{{secret:{secret}}}");
    match spec.place {
        AuthPlace::Header => {
            let value = if spec.bearer {
                format!("Bearer {reference}")
            } else {
                reference
            };
            match endpoint
                .headers
                .iter_mut()
                .find(|h| h.name.trim().eq_ignore_ascii_case(spec.name.trim()))
            {
                Some(header) => header.value = value,
                None => endpoint.headers.push(ApiHeader {
                    name: spec.name.trim().to_string(),
                    value,
                }),
            }
        }
        AuthPlace::Query => {
            let name = spec.name.trim();
            let (base, query) = endpoint
                .url
                .split_once('?')
                .map_or((endpoint.url.as_str(), ""), |(b, q)| (b, q));
            let mut pairs: Vec<String> = query
                .split('&')
                .filter(|pair| !pair.is_empty())
                .filter(|pair| pair.split('=').next() != Some(name))
                .map(str::to_string)
                .collect();
            pairs.push(format!("{name}={reference}"));
            endpoint.url = format!("{base}?{}", pairs.join("&"));
        }
    }
}

/// 从已经带了鉴权的接口反推鉴权方式（同一份文档里 cURL 带了、请求行没带的那些照着补）。
pub(crate) fn from_endpoint(endpoint: &ApiEndpoint) -> Option<(AuthSpec, String)> {
    static SECRET: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{secret:([A-Za-z_][A-Za-z0-9_]*)\}").expect("密钥占位正则"));
    static QUERY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"[?&]([A-Za-z][A-Za-z0-9_]*)=\{secret:([A-Za-z_][A-Za-z0-9_]*)\}")
            .expect("查询密钥正则")
    });
    let basis = format!("同一份文档里「{}」带的鉴权", endpoint.url);
    for header in &endpoint.headers {
        if let Some(caps) = SECRET.captures(&header.value) {
            return Some((
                AuthSpec {
                    place: AuthPlace::Header,
                    name: header.name.clone(),
                    bearer: header.value.trim_start().starts_with("Bearer "),
                    basis,
                    value: String::new(),
                },
                caps[1].to_string(),
            ));
        }
    }
    QUERY.captures(&endpoint.url).map(|caps| {
        (
            AuthSpec {
                place: AuthPlace::Query,
                name: caps[1].to_string(),
                bearer: false,
                basis,
                value: String::new(),
            },
            caps[2].to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_sections_in_prose_are_recognised() {
        let (spec, notes) =
            detect("## 1. 鉴权\n\n所有请求在请求头中携带 `X-API-Key`，值向管理员申请。\n");
        let spec = spec.unwrap();
        assert_eq!(
            (spec.place, spec.name.as_str(), spec.bearer),
            (AuthPlace::Header, "X-API-Key", false)
        );
        assert!(spec.basis.contains("请求头中携带"));
        assert!(notes.is_empty());

        let (spec, _) = detect("认证方式：\n\n```\nAuthorization: Bearer <你的令牌>\n```\n");
        let spec = spec.unwrap();
        assert_eq!(spec.name, "Authorization");
        assert!(spec.bearer && spec.value.is_empty());

        let (spec, _) = detect("调用时在 URL 参数中带上 access_token=abcdef123456 即可。");
        let spec = spec.unwrap();
        assert_eq!(
            (spec.place, spec.name.as_str()),
            (AuthPlace::Query, "access_token")
        );

        let (spec, _) = detect(
            "### 公共请求头\n\n| 参数名 | 必填 | 说明 |\n|---|---|---|\n| token | 是 | 身份令牌 |\n",
        );
        assert_eq!(spec.unwrap().name, "token");
    }

    #[test]
    fn json_fields_and_unrelated_text_are_not_auth() {
        let (spec, _) = detect("登录成功返回：\n{\n  \"token\": \"eyJhbGciOi\"\n}\n");
        assert!(spec.is_none());
        let (spec, notes) = detect("先调用登录接口获取 token，再访问业务接口。");
        assert!(spec.is_none());
        assert!(notes[0].contains("登录接口"));
        let (_, notes) = detect("签名算法：sign = md5(appKey + timestamp)");
        assert!(notes.iter().any(|n| n.contains("签名")));
    }

    #[test]
    fn auth_is_applied_to_headers_or_query() {
        let mut endpoint = ApiEndpoint {
            url: "http://h/api/x?q={q}".into(),
            ..ApiEndpoint::default()
        };
        let header = AuthSpec {
            name: "Authorization".into(),
            bearer: true,
            ..AuthSpec::default()
        };
        apply(&mut endpoint, &header, "token");
        assert_eq!(endpoint.headers[0].value, "Bearer {secret:token}");
        assert!(has_auth(&endpoint));
        let query = AuthSpec {
            place: AuthPlace::Query,
            name: "access_token".into(),
            ..AuthSpec::default()
        };
        apply(&mut endpoint, &query, "access_token");
        assert_eq!(
            endpoint.url,
            "http://h/api/x?q={q}&access_token={secret:access_token}"
        );
        let (spec, secret) = from_endpoint(&endpoint).unwrap();
        assert_eq!(
            (spec.name.as_str(), secret.as_str()),
            ("Authorization", "token")
        );
        assert!(spec.bearer);
    }
}
