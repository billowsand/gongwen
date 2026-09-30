//! Gold runner: `golds/parse.toml` is the parser contract.

use latex_rust::{parse, symbols, ParseError, SymbolKind};

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    error: String,
    input: String,
    expect: String,
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some(other) => out.push(other),
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_value(raw: &str) -> String {
    let t = raw.trim();
    if let Some(s) = t.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        unescape(s)
    } else {
        t.to_string()
    }
}

fn load_golds() -> Vec<Rec> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/parse.toml");
    let text = std::fs::read_to_string(path).expect("parse.toml");
    let mut recs = Vec::new();
    let mut rec = Rec::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[gold]]" {
            if !rec.name.is_empty() {
                recs.push(rec);
            }
            rec = Rec::default();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = parse_value(v);
        match k {
            "name" => rec.name = v,
            "kind" => rec.kind = v,
            "error" => rec.error = v,
            "input" => rec.input = v,
            "expect" => rec.expect = v,
            _ => panic!("unknown gold field {k}"),
        }
    }
    if !rec.name.is_empty() {
        recs.push(rec);
    }
    recs
}

fn variant_name(err: &ParseError) -> &'static str {
    match err {
        ParseError::TrailingBackslash => "TrailingBackslash",
        ParseError::Unsupported(_) => "Unsupported",
        ParseError::Unknown(_) => "Unknown",
        ParseError::Malformed(_) => "Malformed",
        ParseError::UnmatchedDelimiter => "UnmatchedDelimiter",
    }
}

#[test]
fn parse_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no parse golds loaded");
    for rec in recs {
        match rec.kind.as_str() {
            "ast" => {
                let got = parse(&rec.input).unwrap_or_else(|e| panic!("{}: {e}", rec.name));
                assert_eq!(got.gold(), rec.expect, "{}", rec.name);
            }
            "err" => {
                let err = parse(&rec.input).expect_err(&rec.name);
                assert_eq!(variant_name(&err), rec.error, "{}: {err}", rec.name);
                assert_eq!(err.to_string(), rec.expect, "{}", rec.name);
            }
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
}

fn catalog_sample(latex: &str, name: &str, kind: SymbolKind) -> String {
    if !latex.starts_with('\\') {
        return latex.to_string();
    }
    match name {
        "frac" => r"\frac{1}{2}".into(),
        "sqrt" if latex.contains('[') => r"\sqrt[n]{x}".into(),
        "sqrt" => r"\sqrt{x}".into(),
        "begin" if latex.contains("pmatrix") => r"\begin{pmatrix}a&b\\c&d\end{pmatrix}".into(),
        "begin" if latex.contains("vmatrix") => r"\begin{vmatrix}a&b\\c&d\end{vmatrix}".into(),
        "begin" => r"\begin{matrix}a&b\\c&d\end{matrix}".into(),
        "left" => r"\left(x\right)".into(),
        "sum" | "int" | "lim" => format!("\\{name}"),
        "bar"
        | "hat"
        | "tilde"
        | "vec"
        | "dot"
        | "ddot"
        | "dddot"
        | "ddddot"
        | "check"
        | "breve"
        | "acute"
        | "grave"
        | "widehat"
        | "widetilde"
        | "underbrace"
        | "overbrace"
        | "mathring"
        | "overline"
        | "underline"
        | "overleftarrow"
        | "overrightarrow"
        | "overleftrightarrow"
        | "underleftarrow"
        | "underrightarrow"
        | "underleftrightarrow"
        | "cancel"
        | "bcancel"
        | "xcancel"
        | "boxed"
        | "fbox" => {
            format!("\\{name}{{x}}")
        }
        "cancelto" => r"\cancelto{0}{x}".into(),
        "text" => r"\text{a}".into(),
        "mathcal" | "mathbb" => format!("\\{name}{{X}}"),
        "not" => r"\not\equiv".into(),
        _ if kind == SymbolKind::Modifier => format!("\\{name}{{x}}"),
        _ if kind == SymbolKind::Container && latex.contains("{}") => latex.replace("{}", "{x}"),
        _ if latex.contains(' ') => latex.to_string(),
        _ => format!("\\{name}"),
    }
}

#[test]
fn catalog_commands_parse() {
    let mut failed = Vec::new();
    for e in symbols() {
        let sample = catalog_sample(e.latex, e.command_name(), e.kind);
        if let Err(err) = parse(&sample) {
            failed.push(format!("{} [{}]: {err}", e.latex, sample));
        }
    }
    assert!(
        failed.is_empty(),
        "catalog commands failed to parse:\n{}",
        failed.join("\n")
    );
}
