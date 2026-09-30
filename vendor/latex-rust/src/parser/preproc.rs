//! String-level LaTeX math sanitizer run before tokenization.
//!
//! Rewrites that preserve math meaning for a renderer: `{a \over b}` →
//! `\frac`, `{n \choose k}` → `\binom`, `\tfrac`/`\dfrac` → `\frac`, plain-TeX
//! font switches, `\mbox` → `\text`. Extensible `\left`/`\right`, skips, and
//! accents are left intact.

/// Normalize raw math input for the tokenizer.
///
/// Rewrites that preserve math meaning: `{a \over b}` → `\frac`, `{n \choose k}`
/// → `\binom`, `\tfrac`/`\dfrac` → `\frac`, plain-TeX font switches, `\mbox` →
/// `\text`.
///
/// # Examples
///
/// ```
/// use latex_rust::preprocess;
///
/// assert!(preprocess(r"{a \over b}").contains(r"\frac"));
/// ```
#[must_use]
pub fn preprocess(raw_input: &str) -> String {
    let input = raw_input.trim().to_string();
    if input.is_empty() {
        return input;
    }

    let mut input = input;
    input = input.replace(r"{ (}", "(").replace(r"{ )}", ")");
    input = input.replace(r"{ ( }", "(").replace(r"{ ) }", ")");
    input = input.replace(r"{[}", "[").replace(r"{]}", "]");

    input = input
        .replace(r"\tfrac", r"\frac")
        .replace(r"\dfrac", r"\frac")
        .replace(r"\cfrac", r"\frac");

    input = convert_plain_tex_font_scopes(&input);
    input = convert_over_fractions(&input);
    input = convert_choose(&input);
    input = input.replace(r"\mbox{", r"\text{");
    input
}

fn convert_plain_tex_font_scopes(input: &str) -> String {
    let map = [
        (r"\rm", r"\mathrm"),
        (r"\bf", r"\mathbf"),
        (r"\cal", r"\mathcal"),
        (r"\it", r"\mathit"),
        (r"\sf", r"\mathsf"),
        (r"\tt", r"\mathtt"),
    ];
    let mut result = input.to_string();
    for (old, new) in map {
        let mut search = 0;
        while let Some(rel) = result[search..].find(old) {
            let abs = search + rel;
            let body = abs + old.len();
            if abs > 0 && result.as_bytes()[abs - 1] == b'{' {
                if let Some(close) = result[abs..].find('}') {
                    let end = abs + close;
                    let content = result[body..end].trim();
                    let rep = format!("{new}{{{content}}}");
                    result.replace_range((abs - 1)..=end, &rep);
                    search = abs - 1 + rep.len();
                    continue;
                }
            }
            search = abs + old.len();
        }
    }
    result
}

fn convert_over_fractions(input: &str) -> String {
    let mut result = input.to_string();
    while let Some(over) = result.find(r"\over") {
        let mut open = None;
        let mut depth = 0;
        for (i, ch) in result[..over].char_indices().rev() {
            match ch {
                '}' => depth += 1,
                '{' => {
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let mut close = None;
        depth = 0;
        for (i, ch) in result[over + 5..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    if depth == 0 {
                        close = Some(over + 5 + i);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        if let (Some(s), Some(e)) = (open, close) {
            let num = result[s + 1..over].trim();
            let den = result[over + 5..e].trim();
            let rep = format!(r"\frac{{{num}}}{{{den}}}");
            result.replace_range(s..=e, &rep);
        } else {
            break;
        }
    }
    result
}

/// `{n \choose k}` → `\binom{n}{k}` (same brace walk as `\over`).
fn convert_choose(input: &str) -> String {
    let mut result = input.to_string();
    while let Some(ch) = result.find(r"\choose") {
        let mut open = None;
        let mut depth = 0;
        for (i, c) in result[..ch].char_indices().rev() {
            match c {
                '}' => depth += 1,
                '{' => {
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let mut close = None;
        depth = 0;
        for (i, c) in result[ch + 7..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    if depth == 0 {
                        close = Some(ch + 7 + i);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        if let (Some(s), Some(e)) = (open, close) {
            let n = result[s + 1..ch].trim();
            let k = result[ch + 7..e].trim();
            let rep = format!(r"\binom{{{n}}}{{{k}}}");
            result.replace_range(s..=e, &rep);
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn left_right_kept() {
        let s = preprocess(r"\left(\frac{1}{2}\right)");
        assert!(s.contains(r"\left"));
        assert!(s.contains(r"\right"));
    }

    #[test]
    fn tfrac_becomes_frac() {
        assert_eq!(preprocess(r"\tfrac{a}{b}"), r"\frac{a}{b}");
    }

    #[test]
    fn over_becomes_frac() {
        assert_eq!(preprocess(r"{a \over b}"), r"\frac{a}{b}");
    }

    #[test]
    fn trailing_period_kept() {
        assert_eq!(preprocess("x^2."), "x^2.");
    }

    #[test]
    fn empty() {
        assert_eq!(preprocess("   "), "");
    }

    #[test]
    fn overline_kept() {
        assert_eq!(preprocess(r"\overline{z}"), r"\overline{z}");
    }

    #[test]
    fn choose_becomes_binom() {
        assert_eq!(preprocess(r"{n \choose k}"), r"\binom{n}{k}");
    }

    #[test]
    fn mbox_becomes_text() {
        assert_eq!(preprocess(r"\mbox{Diagonal}"), r"\text{Diagonal}");
    }

    #[test]
    fn spacing_kept() {
        let s = preprocess(r"a\,b");
        assert!(s.contains(r"\,"));
    }
}
