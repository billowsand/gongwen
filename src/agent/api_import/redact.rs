//! 凭据处理：把令牌、Cookie、密钥类参数提成本机密钥（模板里写 `{secret:名字}`），
//! 发给模型的资料里同样遮掉。规则认不全所有敏感内容，所以界面上会给人看实际发出去的文字。

use regex::Regex;
use std::sync::LazyLock;

/// 遮蔽后的写法。
pub(crate) const MASK: &str = "******";

static BEARER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(bearer|basic|token)\s+[A-Za-z0-9._~+/=\-]{8,}").expect("令牌正则")
});
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)((?:access_?token|refresh_?token|token|api[_-]?key|app[_-]?secret|secret|password|passwd|signature|cookie)["']?\s*[:=]\s*["']?)([^\s"'&,;}]{6,})"#,
    )
    .expect("键值正则")
});

/// 名字像凭据的字段（请求头、查询参数、请求体字段）。
pub(crate) fn is_secret_key(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "authorization",
        "cookie",
        "token",
        "secret",
        "password",
        "passwd",
        "apikey",
        "accesskey",
        "appkey",
        "signature",
        "ticket",
        "session",
    ]
    .iter()
    .any(|word| name.contains(word))
        || name == "sign"
        || name == "key"
}

/// 文档里的占位写法（`<your token>`、`xxxx`、`{token}`），不是真值。
fn is_placeholder(value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || value.contains(['<', '>', '{', '}', '【', '…'])
        || value.chars().all(|c| matches!(c, 'x' | 'X' | '*' | '.'))
        || value.contains("你的")
        || value.contains("your")
}

/// 收集到的密钥：名字 → 值（文档里是占位写法时值为空，要用户补）。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Secrets {
    pub(crate) found: Vec<(String, String)>,
}

impl Secrets {
    /// 登记一个凭据，返回它的名字；同一个值只登记一次。
    pub(crate) fn add(&mut self, hint: &str, value: &str) -> String {
        let value = if is_placeholder(value) {
            ""
        } else {
            value.trim()
        };
        if !value.is_empty()
            && let Some((name, _)) = self.found.iter().find(|(_, v)| v == value)
        {
            return name.clone();
        }
        let base = secret_name(hint);
        // 文档里同一个字段写了好几次占位：算同一个密钥。
        if value.is_empty()
            && self
                .found
                .iter()
                .any(|(existing, v)| *existing == base && v.is_empty())
        {
            return base;
        }
        let mut name = base.clone();
        let mut n = 2;
        while self.found.iter().any(|(existing, _)| *existing == name) {
            name = format!("{base}{n}");
            n += 1;
        }
        self.found.push((name.clone(), value.to_string()));
        name
    }

    /// 请求头的值：`Bearer xxx` 只把令牌提出去，前缀留着。
    pub(crate) fn header_value(&mut self, header: &str, value: &str) -> String {
        let value = value.trim();
        for scheme in ["Bearer ", "Basic ", "Token ", "bearer ", "basic ", "token "] {
            if let Some(token) = value.strip_prefix(scheme) {
                let name = self.add(header, token);
                return format!("{scheme}{{secret:{name}}}");
            }
        }
        let name = self.add(header, value);
        format!("{{secret:{name}}}")
    }

    /// 把文字里出现的密钥值和像令牌的写法遮掉。
    pub(crate) fn redact(&self, text: &str) -> String {
        let mut text = text.to_string();
        for (_, value) in &self.found {
            if value.chars().count() >= 4 {
                text = text.replace(value.as_str(), MASK);
            }
        }
        let text = BEARER.replace_all(&text, |caps: &regex::Captures<'_>| {
            format!("{} {MASK}", &caps[1])
        });
        KEY_VALUE
            .replace_all(&text, |caps: &regex::Captures<'_>| {
                if caps[2].contains(MASK) {
                    caps[0].to_string()
                } else {
                    format!("{}{MASK}", &caps[1])
                }
            })
            .into_owned()
    }

    /// 文字里还有几处被遮蔽（给人看「遮了几处」）。
    pub(crate) fn count_masks(text: &str) -> usize {
        text.matches(MASK).count()
    }
}

/// 凭据名：只留英文字母数字下划线，统一小写；`Authorization` 叫 `token` 更好懂。
pub(crate) fn secret_name(hint: &str) -> String {
    let lower = hint.to_ascii_lowercase();
    if lower == "authorization" {
        return "token".into();
    }
    let name: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let name = name.trim_matches('_').to_string();
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("key_{name}")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_become_secrets_and_are_masked_in_text() {
        let mut secrets = Secrets::default();
        assert_eq!(
            secrets.header_value("Authorization", "Bearer eyJhbGciOi.abc"),
            "Bearer {secret:token}"
        );
        assert_eq!(
            secrets.header_value("X-Api-Key", "k-123456"),
            "{secret:x_api_key}"
        );
        assert_eq!(
            secrets.add("Authorization", "eyJhbGciOi.abc"),
            "token",
            "同一个值只登记一次"
        );
        assert_eq!(secrets.add("token", "<你的令牌>"), "token2");
        assert_eq!(
            secrets.found[2],
            ("token2".to_string(), String::new()),
            "占位不当真值"
        );

        let text = "curl -H 'Authorization: Bearer eyJhbGciOi.abc' -H 'X-Api-Key: k-123456'\n\
                    另一个令牌 access_token=abcdefgh123 和 Bearer zzzzzzzzzzzz";
        let redacted = secrets.redact(text);
        assert!(
            !redacted.contains("eyJhbGciOi") && !redacted.contains("k-123456"),
            "{redacted}"
        );
        assert!(
            !redacted.contains("abcdefgh123") && !redacted.contains("zzzzzzzz"),
            "{redacted}"
        );
        assert_eq!(Secrets::count_masks(&redacted), 4, "{redacted}");
    }

    #[test]
    fn secret_like_names_are_recognized() {
        for name in [
            "Authorization",
            "Cookie",
            "access_token",
            "appSecret",
            "sign",
            "X-API-KEY",
        ] {
            assert!(is_secret_key(name), "{name}");
        }
        for name in ["keyword", "region", "page_size", "monkey"] {
            assert!(!is_secret_key(name), "{name}");
        }
    }
}
