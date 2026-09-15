//! 引号正规化：把直引号转为中文圆括号引号。
//!
//! 行为与 md_to_docx_rust::convert_quotes 完全等价（双引号 + 单引号都做配对切换）。

/// 将 ASCII / 中文混用的双/单引号正规化为中文圆引号。
///
/// 处理过程：
/// 1. 把任何已存在的中文左右引号统一拍平为 ASCII 引号；
/// 2. 从左到右扫描，按出现奇偶次数把 `"` 切换为 “ / ”，`'` 切换为 ‘ / ’。
///
/// 注意：和 md2tex/merger.rs::normalize_quotes 不同，这里**不**在换行处重置配对状态，
/// 与原 md_to_docx_rust 保持一致；如果需要"按行重置"语义，请使用
/// [`normalize_quotes_per_line`].
pub fn convert_quotes(text: &str) -> String {
    let mut text = text.to_string();
    text = text.replace(['\u{201c}', '\u{201d}'], "\"");
    text = text.replace(['\u{2018}', '\u{2019}'], "'");

    let mut chars: Vec<char> = text.chars().collect();
    let mut in_double = false;
    for ch in &mut chars {
        if *ch == '"' {
            *ch = if !in_double { '\u{201c}' } else { '\u{201d}' };
            in_double = !in_double;
        }
    }
    let text: String = chars.into_iter().collect();

    let mut chars: Vec<char> = text.chars().collect();
    let mut in_single = false;
    for ch in &mut chars {
        if *ch == '\'' {
            *ch = if !in_single { '\u{2018}' } else { '\u{2019}' };
            in_single = !in_single;
        }
    }
    chars.into_iter().collect()
}

/// 仅处理双引号的"按行重置"版本，对应旧 md2tex pipeline 的行为。
/// 公文路径**不要**用这个；当前 parser 使用 [`convert_quotes`]，并会跳过 fenced code block。
#[allow(dead_code)]
pub fn normalize_quotes_per_line(text: &str) -> String {
    const LEFT: char = '\u{201c}';
    const RIGHT: char = '\u{201d}';

    let mut result = String::with_capacity(text.len());
    let mut in_quote = false;

    for ch in text.chars() {
        if ch == '"' {
            if !in_quote {
                result.push(LEFT);
                in_quote = true;
            } else {
                result.push(RIGHT);
                in_quote = false;
            }
        } else {
            result.push(ch);
            if ch == '\n' {
                in_quote = false;
            }
        }
    }

    result
}
