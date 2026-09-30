//! Gold runner: `golds/layout.toml` is the layout-dimension contract.

use latex_rust::{layout, parse, BoxContent, Color, MathBox, MathFont, MathStyle};

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    style: String,
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
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/layout.toml");
    let text = std::fs::read_to_string(path).expect("layout.toml");
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
        "display" => MathStyle::Display,
        "display-cramped" => MathStyle::DisplayCramped,
        "text" => MathStyle::Text,
        "text-cramped" => MathStyle::TextCramped,
        "script" => MathStyle::Script,
        "script-cramped" => MathStyle::ScriptCramped,
        "scriptscript" => MathStyle::ScriptScript,
        "scriptscript-cramped" => MathStyle::ScriptScriptCramped,
        other => panic!("unknown style {other}"),
    }
}

fn first_color(b: &MathBox) -> Option<Color> {
    match &b.content {
        BoxContent::Color(c, inner) => first_color(inner).or(Some(*c)),
        BoxContent::BackColor(_, inner) | BoxContent::Frame { inner, .. } => first_color(inner),
        BoxContent::HList(v) | BoxContent::VList(v) | BoxContent::Overlap(v) => {
            v.iter().find_map(first_color)
        }
        _ => None,
    }
}

fn first_back_color(b: &MathBox) -> Option<Color> {
    match &b.content {
        BoxContent::BackColor(c, _) => Some(*c),
        BoxContent::Color(_, inner) | BoxContent::Frame { inner, .. } => first_back_color(inner),
        BoxContent::HList(v) | BoxContent::VList(v) | BoxContent::Overlap(v) => {
            v.iter().find_map(first_back_color)
        }
        _ => None,
    }
}

fn first_frame_stroke(b: &MathBox) -> Option<Color> {
    match &b.content {
        BoxContent::Frame { stroke, inner, .. } => stroke.or_else(|| first_frame_stroke(inner)),
        BoxContent::Color(_, inner) | BoxContent::BackColor(_, inner) => first_frame_stroke(inner),
        BoxContent::HList(v) | BoxContent::VList(v) | BoxContent::Overlap(v) => {
            v.iter().find_map(first_frame_stroke)
        }
        _ => None,
    }
}

fn has_nested_color(b: &MathBox, outer: Color, inner: Color) -> bool {
    match &b.content {
        BoxContent::Color(c, body) if *c == outer => first_color(body) == Some(inner),
        BoxContent::Color(_, body) => has_nested_color(body, outer, inner),
        BoxContent::BackColor(_, body) | BoxContent::Frame { inner: body, .. } => {
            has_nested_color(body, outer, inner)
        }
        BoxContent::HList(v) | BoxContent::VList(v) | BoxContent::Overlap(v) => {
            v.iter().any(|k| has_nested_color(k, outer, inner))
        }
        _ => false,
    }
}

fn lay(font: &MathFont, rec: &Rec) -> MathBox {
    let ast = parse(&rec.input).unwrap_or_else(|e| panic!("{}: parse {e}", rec.name));
    let style = if rec.style.is_empty() {
        MathStyle::Text
    } else {
        parse_style(&rec.style)
    };
    layout(&ast, font, style).unwrap_or_else(|e| panic!("{}: layout {e}", rec.name))
}

#[test]
fn layout_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no layout golds loaded");
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    for rec in recs {
        match rec.kind.as_str() {
            "dims" => {
                let bx = lay(&font, &rec);
                assert_eq!(bx.dim_gold(), rec.expect, "{}", rec.name);
            }
            "color" => {
                let bx = lay(&font, &rec);
                let c = first_color(&bx).unwrap_or_else(|| panic!("{}: no color box", rec.name));
                assert_eq!(c.css_hex(), rec.expect, "{}", rec.name);
            }
            "back_color" => {
                let bx = lay(&font, &rec);
                let c = first_back_color(&bx)
                    .unwrap_or_else(|| panic!("{}: no background color", rec.name));
                assert_eq!(c.css_hex(), rec.expect, "{}", rec.name);
            }
            "frame_stroke" => {
                let bx = lay(&font, &rec);
                let c = first_frame_stroke(&bx)
                    .unwrap_or_else(|| panic!("{}: no frame stroke", rec.name));
                assert_eq!(c.css_hex(), rec.expect, "{}", rec.name);
            }
            "color_nested" => {
                let bx = lay(&font, &rec);
                let outer = Color::rgb(0xff, 0x00, 0x00);
                let inner = Color::rgb(0x00, 0x00, 0xff);
                assert!(
                    has_nested_color(&bx, outer, inner),
                    "{}: nested color not found",
                    rec.name
                );
            }
            "err" => {
                let ast = parse(&rec.input).unwrap_or_else(|e| panic!("{}: parse {e}", rec.name));
                let style = parse_style(if rec.style.is_empty() {
                    "text"
                } else {
                    &rec.style
                });
                let err = layout(&ast, &font, style).expect_err(&rec.name);
                assert!(err.to_string().contains(&rec.expect), "{}: {err}", rec.name);
            }
            "style_frac" => {
                let outer = parse_style(&rec.style);
                let parts: Vec<&str> = rec.expect.split(' ').collect();
                assert_eq!(parts.len(), 2, "{}", rec.name);
                assert_eq!(outer.numerator().gold(), parts[0], "{}", rec.name);
                assert_eq!(outer.denominator().gold(), parts[1], "{}", rec.name);
            }
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
}
