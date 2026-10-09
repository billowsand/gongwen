//! 上下文窗口（`docs/ai-agent-workbench.md` 16.15 A）：窗口多大、从哪来，token 怎么估，
//! 每次请求的输出上限怎么现算，服务端报超长怎么认。
//!
//! 窗口按这个顺序定：手动设置 → 本次运行观察到的上限（服务端报过超长）→ 服务自报
//! （`/v1/models` 的 `max_model_len` 一类字段）→ 按模型名查规模表 → 32k。
//! 不引入分词器，token 一律保守估：估多了只是少放点证据，估少了才会被拒。

use crate::models::LmStudioConfig;
use regex::Regex;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// 什么都查不到时按 32k 算：内网 vLLM 起服务时常见的最小设置。
pub const DEFAULT_WINDOW: usize = 32_768;
/// 输出至少要留这么多，留不出就不发请求。
pub const MIN_OUTPUT: usize = 2048;
/// 消息格式、特殊记号的余量。
const MARGIN: usize = 256;

/// 窗口大小的来源，界面上要写明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSource {
    Manual,
    /// 服务端报过超长，记下了它说的上限。
    Observed,
    Service,
    ModelTable,
    Default,
}

impl WindowSource {
    pub fn label(self) -> &'static str {
        match self {
            WindowSource::Manual => "手动设置",
            WindowSource::Observed => "服务端报超长时给出",
            WindowSource::Service => "来自服务",
            WindowSource::ModelTable => "按模型名估计",
            WindowSource::Default => "默认值",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub tokens: usize,
    pub source: WindowSource,
}

impl Window {
    /// 「128k（来自服务）」。
    pub fn label(&self) -> String {
        format!("{}（{}）", tokens_label(self.tokens), self.source.label())
    }
}

/// 32768 → 「32k」，200000 → 「195k」，不足 1k 的原样写。
pub fn tokens_label(tokens: usize) -> String {
    if tokens >= 1024 {
        format!("{}k", (tokens + 512) / 1024)
    } else {
        tokens.to_string()
    }
}

/// 模型名里含这些字样时的窗口（取各系列常见部署里偏小的那个）。按顺序匹配，先到先得。
const MODEL_TABLE: [(&str, usize); 10] = [
    // V4 / V4.1 与新版 Flash 别名支持百万上下文，必须先于系列兜底匹配。
    // https://api-docs.deepseek.com/quick_start/pricing/
    ("deepseek-v4", 1_048_576),
    ("deepseek-flash", 1_048_576),
    ("deepseek", 131_072),
    ("minimax", 196_608),
    ("qwen3", 32_768),
    ("qwen2.5", 32_768),
    ("glm-4", 131_072),
    ("kimi", 131_072),
    ("moonshot", 131_072),
    ("llama-3", 131_072),
];

fn key(base_url: &str, model: &str) -> String {
    format!("{}|{}", base_url.trim_end_matches('/'), model.trim())
}

/// 服务自报的窗口：键是「地址|模型」；值为 None 表示问过、服务没说。
static SERVICE: LazyLock<Mutex<HashMap<String, Option<usize>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// 服务端报超长时给出的上限，本进程内一直按它算。
static OBSERVED: LazyLock<Mutex<HashMap<String, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 记下 `/v1/models` 列出的各模型窗口（没给的记 None，免得再问）。
pub(super) fn record_service(base_url: &str, models: &[(String, Option<usize>)]) {
    let mut map = SERVICE.lock().unwrap_or_else(|e| e.into_inner());
    for (model, tokens) in models {
        map.insert(key(base_url, model), tokens.filter(|t| *t >= 1024));
    }
}

pub(super) fn record_observed(config: &LmStudioConfig, tokens: usize) {
    OBSERVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key(&config.base_url, &config.model), tokens);
}

/// 不联网，按已知的信息定窗口。界面线程用它。
///
/// 服务端报过的上限比手动设置还小时以服务端为准：设大了请求只会一次次被拒。
pub fn peek_window(config: &LmStudioConfig) -> Window {
    let key = key(&config.base_url, &config.model);
    let observed = OBSERVED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .copied();
    if let Some(tokens) =
        observed.filter(|t| config.context_window == 0 || *t < config.context_window as usize)
    {
        return Window {
            tokens,
            source: WindowSource::Observed,
        };
    }
    if config.context_window > 0 {
        return Window {
            tokens: config.context_window as usize,
            source: WindowSource::Manual,
        };
    }
    if let Some(Some(tokens)) = SERVICE.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Window {
            tokens: *tokens,
            source: WindowSource::Service,
        };
    }
    from_model_name(&config.model).unwrap_or(Window {
        tokens: DEFAULT_WINDOW,
        source: WindowSource::Default,
    })
}

fn from_model_name(model: &str) -> Option<Window> {
    // 提供商可能使用空格、下划线或省略连字符；统一后仍按具体版本优先匹配。
    let name: String = model
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.')
        .collect();
    MODEL_TABLE
        .iter()
        .find(|(pattern, _)| name.contains(&pattern.replace('-', "")))
        .map(|(_, tokens)| Window {
            tokens: *tokens,
            source: WindowSource::ModelTable,
        })
}

/// 自动模式下还没问过服务时，先问一次 `/v1/models`（每个「地址 + 模型」本进程只问一次），
/// 再按 [`peek_window`] 定。问不到不算错。在后台线程里调。
pub fn window(config: &LmStudioConfig) -> Window {
    if config.context_window == 0 && !config.model.trim().is_empty() {
        let key = key(&config.base_url, &config.model);
        let asked = SERVICE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&key);
        if !asked {
            if super::list_models_at(
                &config.base_url,
                &config.api_key,
                config.timeout_seconds.min(10),
            )
            .is_err()
            {
                // 问不通也记一笔，免得每次运行都卡在这里。
                record_service(&config.base_url, &[(config.model.clone(), None)]);
            }
            SERVICE
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .entry(key)
                .or_insert(None);
        }
    }
    peek_window(config)
}

/// 保守估 token：汉字与全角标点 1 字 1 个，其余 4 个字符 1 个，再加 10%。
pub fn estimate_tokens(text: &str) -> usize {
    let mut wide = 0usize;
    let mut narrow = 0usize;
    for c in text.chars() {
        // 中文引号、破折号、省略号在 U+2000 段，也按全角算。
        if (c as u32) >= 0x2E80 || ('\u{2000}'..='\u{206F}').contains(&c) {
            wide += 1;
        } else {
            narrow += 1;
        }
    }
    let raw = wide + narrow.div_ceil(4);
    raw + raw.div_ceil(10)
}

/// 这次请求的输出上限：`min(设置值, 窗口 − 输入 − 余量)`。连 `min(设置值, 2048)` 都留不出时返回 None。
pub fn output_limit(window: usize, input: usize, configured: u32) -> Option<u32> {
    let available = window.saturating_sub(input + MARGIN);
    let need = (configured as usize).min(MIN_OUTPUT);
    (available >= need).then(|| (configured as usize).min(available) as u32)
}

/// 输入放不下：本地估算就超了，或服务端报了超长、缩小重发也不行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextOverflow {
    /// 服务端说的上限（没说就是 None）。
    pub limit: Option<usize>,
    /// 服务端说的输入 token 数（没说就是 None）。
    pub input: Option<usize>,
    /// 是服务端报的（还没重试过），还是本地估算就放不下。
    pub from_server: bool,
    pub message: String,
}

impl std::fmt::Display for ContextOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ContextOverflow {}

impl ContextOverflow {
    /// 错误里写出实际调用的模型，避免把辅助模型的窗口误认为起草模型的窗口。
    pub(super) fn with_model(mut self, model: &str) -> Self {
        self.message = format!("模型「{}」：{}", model.trim(), self.message);
        self
    }
}

/// 本地估算就放不下时的报错。
pub fn local_overflow(window: Window, input: usize) -> ContextOverflow {
    ContextOverflow {
        limit: Some(window.tokens),
        input: Some(input),
        from_server: false,
        message: format!(
            "输入约 {} token，模型上下文 {} 放不下（还要给输出留至少 {MIN_OUTPUT}）。\
             请选中一部分再做，或换上下文更大的模型；上下文大小可在 AI 管理页的模型服务里设置。",
            input,
            window.label()
        ),
    }
}

/// 认服务端「超出上下文」的报错，尽量读出上限与输入 token 数。各家的说法：
/// - vLLM：`This model's maximum context length is 32768 tokens. However, you requested 40000 tokens
///   (8000 in the messages, 32000 in the completion)` / `... and your request has 1200 input tokens`；
/// - OpenAI：`context_length_exceeded`；
/// - LM Studio：`... the model is loaded with context length of only 4096 tokens`；
/// - 其他：`prompt is too long`、`exceeds the context window`、`input length ... exceeds`。
pub fn parse_overflow(body: &str) -> Option<ContextOverflow> {
    static LIMIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)(?:maximum context length is|context length of only|max_model_len[ =:]+|context window of|context_length[ =:]+)\s*(\d{3,7})",
        )
        .expect("上限正则")
    });
    static INPUT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:(\d{1,7}) in the messages|has (\d{1,7}) input tokens|(\d{1,7}) tokens? in the prompt)")
            .expect("输入正则")
    });
    let lower = body.to_lowercase();
    let hit = [
        "maximum context length",
        "context_length_exceeded",
        "context length of only",
        "prompt is too long",
        "exceeds the context",
        "context window",
        "too many tokens",
        "max_model_len",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
        || (lower.contains("input length") && lower.contains("exceed"));
    if !hit {
        return None;
    }
    let limit = LIMIT
        .captures(body)
        .and_then(|caps| caps[1].parse::<usize>().ok());
    let input = INPUT.captures(body).and_then(|caps| {
        (1..=3)
            .find_map(|i| caps.get(i))
            .and_then(|m| m.as_str().parse::<usize>().ok())
    });
    Some(ContextOverflow {
        limit,
        input,
        from_server: true,
        message: format!("模型服务报输入超出上下文：{}", body.trim()),
    })
}

/// 服务端报超长之后：记下真实上限（没说就按当前窗口的 75%），按服务端说的输入数（没说就用估算）
/// 重算输出上限。还放得下返回新的上限，放不下返回本地报错。
pub(super) fn after_overflow(
    config: &LmStudioConfig,
    overflow: &ContextOverflow,
    estimated_input: usize,
    configured: u32,
) -> Result<u32, ContextOverflow> {
    let before = peek_window(config);
    let limit = overflow
        .limit
        .unwrap_or(before.tokens * 3 / 4)
        .min(before.tokens);
    record_observed(config, limit);
    let input = overflow
        .input
        .map(|n| n + n / 20)
        .unwrap_or(estimated_input)
        .max(estimated_input.min(limit));
    let window = Window {
        tokens: limit,
        source: WindowSource::Observed,
    };
    output_limit(limit, input, configured)
        .ok_or_else(|| local_overflow(window, input).with_model(&config.model))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(model: &str, window: u32) -> LmStudioConfig {
        LmStudioConfig {
            base_url: format!("http://ctx-test-{model}/v1"),
            model: model.into(),
            context_window: window,
            ..LmStudioConfig::default()
        }
    }

    #[test]
    fn estimates_are_conservative() {
        assert_eq!(estimate_tokens(""), 0);
        // 11 个汉字 → 11，加 10%（向上取整）→ 13。
        assert_eq!(estimate_tokens("关于做好森林防火工作的"), 13);
        // 40 个 ASCII 字符 → 10，加 10% → 11。
        assert_eq!(estimate_tokens(&"abcd".repeat(10)), 11);
        assert_eq!(estimate_tokens("，。“”"), 5);
    }

    #[test]
    fn the_output_limit_fits_what_is_left() {
        assert_eq!(
            output_limit(32_768, 1000, 32_000),
            Some(32_768 - 1000 - 256)
        );
        assert_eq!(output_limit(131_072, 1000, 32_000), Some(32_000));
        assert_eq!(output_limit(32_768, 31_000, 32_000), None, "留不出 2048");
        assert_eq!(
            output_limit(4096, 3500, 64),
            Some(64),
            "设置值本来就小时按它要"
        );
    }

    #[test]
    fn window_sources_follow_the_order() {
        assert_eq!(
            peek_window(&config("whatever", 65_536)),
            Window {
                tokens: 65_536,
                source: WindowSource::Manual
            }
        );
        assert_eq!(
            peek_window(&config("DeepSeek-V4-Flash", 0)).tokens,
            1_048_576
        );
        assert_eq!(
            peek_window(&config("MiniMax-M2.7", 0)).source,
            WindowSource::ModelTable
        );
        let unknown = config("my-model", 0);
        assert_eq!(peek_window(&unknown).source, WindowSource::Default);
        record_service(&unknown.base_url, &[("my-model".into(), Some(40_960))]);
        assert_eq!(
            peek_window(&unknown),
            Window {
                tokens: 40_960,
                source: WindowSource::Service
            }
        );
        record_observed(&unknown, 30_000);
        assert_eq!(peek_window(&unknown).source, WindowSource::Observed);
        assert_eq!(peek_window(&unknown).label(), "29k（服务端报超长时给出）");
    }

    #[test]
    fn deepseek_versions_and_provider_spellings_are_recognised() {
        for model in [
            "DeepSeek-V4-Flash",
            "DeepSeek-V4.1-Flash",
            "deepseek-ai/DeepSeek-V4.1-Flash",
            "deepseek v4.1 flash",
            "deepseek_v4.1_flash",
            "deepseekv4.1flash",
            "deepseek-flash",
            "DeepSeek-V4-Pro-0813",
        ] {
            let window = peek_window(&config(model, 0));
            assert_eq!(window.tokens, 1_048_576, "{model}");
            assert_eq!(window.source, WindowSource::ModelTable);
            assert_eq!(output_limit(window.tokens, 192_844, 32_000), Some(32_000));
        }
        assert_eq!(peek_window(&config("DeepSeek-V3.2", 0)).tokens, 131_072);
        assert_eq!(peek_window(&config("Qwen3-4B", 0)).tokens, 32_768);
    }

    #[test]
    fn manual_and_service_windows_override_the_model_table() {
        let mut config = config("DeepSeek-V4.1-Flash-priority", 0);
        record_service(&config.base_url, &[(config.model.clone(), Some(65_536))]);
        assert_eq!(peek_window(&config).tokens, 65_536);
        assert_eq!(peek_window(&config).source, WindowSource::Service);
        config.context_window = 262_144;
        assert_eq!(peek_window(&config).tokens, 262_144);
        assert_eq!(peek_window(&config).source, WindowSource::Manual);
        record_observed(&config, 32_768);
        assert_eq!(peek_window(&config).tokens, 32_768);
        assert_eq!(peek_window(&config).source, WindowSource::Observed);
        let error = local_overflow(peek_window(&config), 192_844).with_model(&config.model);
        assert!(error.message.contains(&config.model));
        assert_eq!(error.limit, Some(32_768));
    }

    #[test]
    fn overflow_messages_are_recognised() {
        let vllm = r#"{"object":"error","message":"This model's maximum context length is 32768 tokens. However, you requested 40000 tokens (8000 in the messages, 32000 in the completion). Please reduce the length of the messages or completion.","type":"BadRequestError","code":400}"#;
        let parsed = parse_overflow(vllm).unwrap();
        assert_eq!(parsed.limit, Some(32_768));
        assert_eq!(parsed.input, Some(8000));
        let newer = "'max_tokens' or 'max_completion_tokens' is too large: 32000. This model's maximum context length is 32768 tokens and your request has 1200 input tokens (32000 > 32768 - 1200).";
        let parsed = parse_overflow(newer).unwrap();
        assert_eq!((parsed.limit, parsed.input), (Some(32_768), Some(1200)));
        let lm = "Trying to keep the first 5000 tokens when context the overflows. However, the model is loaded with context length of only 4096 tokens";
        assert_eq!(parse_overflow(lm).unwrap().limit, Some(4096));
        let openai = r#"{"error":{"code":"context_length_exceeded","message":"too long"}}"#;
        assert_eq!(parse_overflow(openai).unwrap().limit, None);
        assert!(parse_overflow(r#"{"error": "tools not supported"}"#).is_none());
    }

    #[test]
    fn after_an_overflow_the_real_limit_is_used() {
        let config = config("after-overflow", 0);
        let overflow = parse_overflow("maximum context length is 32768 tokens. However, you requested 40000 tokens (8000 in the messages, 32000 in the completion)").unwrap();
        let limit = after_overflow(&config, &overflow, 6000, 32_000).unwrap();
        assert_eq!(limit as usize, 32_768 - 8400 - 256);
        assert_eq!(peek_window(&config).tokens, 32_768);
        let too_long = parse_overflow("maximum context length is 32768 tokens. However, you requested 70000 tokens (38000 in the messages, 32000 in the completion)").unwrap();
        let error = after_overflow(&config, &too_long, 30_000, 32_000).unwrap_err();
        assert!(!error.from_server);
        assert!(error.message.contains("放不下"), "{}", error.message);
    }
}
