//! 区段切换标记的识别：HTML 注释 `<!-- [摘要] -->` 等。
//!
//! docx_research / tex_official / tex_research 等 emitter 用标记切换模式；
//! docx_official 直接忽略标记块（公文不使用区段分段）。

use super::ast::MarkerKind;

/// 把一行尝试解析成 [`MarkerKind`]。
///
/// 接受形式（容忍前后空白、`[]` 内的中英括号变体；也兼容 pandoc
/// Lua filter 原先接受的无方括号英文/中文标记）：
/// - `<!-- [摘要] -->`
/// - `<!--[附录]-->`
/// - `<!-- [版本变更记录] -->`
/// - `<!-- [正文] -->`
/// - `<!-- [参考文献] -->`
/// - `<!-- abstract -->`
pub fn detect(line: &str) -> Option<MarkerKind> {
    let trimmed = line.trim();
    if !trimmed.starts_with("<!--") || !trimmed.ends_with("-->") {
        return None;
    }
    let inner = trimmed[4..trimmed.len() - 3].trim();
    // 去掉两侧 `[]` 或 `【】`
    let inner = inner
        .strip_prefix('[')
        .or_else(|| inner.strip_prefix('【'))
        .unwrap_or(inner);
    let inner = inner
        .strip_suffix(']')
        .or_else(|| inner.strip_suffix('】'))
        .unwrap_or(inner)
        .trim();

    match inner.to_ascii_lowercase().as_str() {
        "摘要" | "abstract" => Some(MarkerKind::Abstract),
        "附录" | "附件" | "appendix" => Some(MarkerKind::Appendix),
        "版本变更记录" | "changelog" => Some(MarkerKind::Changelog),
        "正文" => Some(MarkerKind::Body),
        "参考文献" | "reference" | "references" => Some(MarkerKind::Reference),
        _ => None,
    }
}
