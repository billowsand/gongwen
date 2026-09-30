//! Gold runner: `golds/envs.toml` is the Milestone 7 environment contract.

use latex_rust::{
    layout, layout_with_numbering, parse, BoxContent, MathBox, MathFont, MathStyle, NumberingState,
    ParseError,
};

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    style: String,
    input: String,
    expect: String,
    error: String,
    key: String,
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
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/envs.toml");
    let text = std::fs::read_to_string(path).expect("envs.toml");
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
            "style" => rec.style = v,
            "input" => rec.input = v,
            "expect" => rec.expect = v,
            "error" => rec.error = v,
            "key" => rec.key = v,
            _ => panic!("unknown gold field {k}"),
        }
    }
    if !rec.name.is_empty() {
        recs.push(rec);
    }
    recs
}

fn parse_style(s: &str) -> MathStyle {
    match s {
        "" | "display" => MathStyle::Display,
        "text" => MathStyle::Text,
        other => panic!("unknown style {other}"),
    }
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

fn glyph_xs(b: &MathBox, x: latex_rust::Dim, ch: char, out: &mut Vec<latex_rust::Dim>) {
    match &b.content {
        BoxContent::Glyph { ch: c, .. } if *c == ch => out.push(x),
        BoxContent::HList(v) => {
            let mut cx = x;
            for k in v {
                glyph_xs(k, cx.clone(), ch, out);
                cx = &cx + &k.width;
            }
        }
        BoxContent::VList(v) | BoxContent::Overlap(v) => {
            for k in v {
                glyph_xs(k, x.clone(), ch, out);
            }
        }
        BoxContent::Color(_, inner)
        | BoxContent::BackColor(_, inner)
        | BoxContent::Frame { inner, .. } => glyph_xs(inner, x, ch, out),
        _ => {}
    }
}

#[test]
fn env_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no env golds loaded");
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    for rec in recs {
        match rec.kind.as_str() {
            "ast" => {
                let got = parse(&rec.input).unwrap_or_else(|e| panic!("{}: {e}", rec.name));
                assert_eq!(got.gold(), rec.expect, "{}", rec.name);
            }
            "dims" => {
                let ast = parse(&rec.input).unwrap_or_else(|e| panic!("{}: parse {e}", rec.name));
                let bx = layout(&ast, &font, parse_style(&rec.style))
                    .unwrap_or_else(|e| panic!("{}: layout {e}", rec.name));
                assert_eq!(bx.dim_gold(), rec.expect, "{}", rec.name);
            }
            "err" => {
                let err = parse(&rec.input).expect_err(&rec.name);
                assert_eq!(variant_name(&err), rec.error, "{}: {err}", rec.name);
                assert!(err.to_string().contains(&rec.expect), "{}: {err}", rec.name);
            }
            "label" => {
                let ast = parse(&rec.input).unwrap_or_else(|e| panic!("{}: parse {e}", rec.name));
                let mut st = NumberingState::default();
                layout_with_numbering(&ast, &font, parse_style(&rec.style), &mut st)
                    .unwrap_or_else(|e| panic!("{}: layout {e}", rec.name));
                let got = st
                    .label(&rec.key)
                    .unwrap_or_else(|| panic!("{}: no label", rec.name));
                assert_eq!(got, rec.expect, "{}", rec.name);
            }
            "eq_x" => {
                let ast = parse(&rec.input).unwrap_or_else(|e| panic!("{}: parse {e}", rec.name));
                let bx = layout(&ast, &font, parse_style(&rec.style))
                    .unwrap_or_else(|e| panic!("{}: layout {e}", rec.name));
                let ch = rec.expect.chars().next().expect("eq_x char");
                let mut xs = Vec::new();
                glyph_xs(&bx, latex_rust::Dim::zero(), ch, &mut xs);
                assert!(
                    xs.len() >= 2,
                    "{}: need two {ch:?} glyphs, got {}",
                    rec.name,
                    xs.len()
                );
                let first = xs[0].clone();
                for x in &xs[1..] {
                    assert!(
                        first.eq_dim(x),
                        "{}: {ch:?} x {} vs {}",
                        rec.name,
                        first.to_dec_string(),
                        x.to_dec_string()
                    );
                }
            }
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
}
