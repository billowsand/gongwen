//! Gold runner: `golds/milestone1.toml` is the contract.

use latex_rust::{
    category_count, format_tokens, lookup, named_color, parse_color_spec, symbols, tokenize,
    ColorTable, Dim, Error, MathBox, MathFont, ParseError, STIX_TWO_MATH_OTF, STIX_TWO_MATH_SHA256,
};

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    op: String,
    model: String,
    input: String,
    lhs: String,
    rhs: String,
    widths: String,
    heights: String,
    depths: String,
    expect: String,
    expect_width: String,
    expect_height: String,
    expect_depth: String,
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
                Some('u') => {
                    if chars.next() == Some('{') {
                        let mut hex = String::new();
                        for h in chars.by_ref() {
                            if h == '}' {
                                break;
                            }
                            hex.push(h);
                        }
                        let cp = u32::from_str_radix(&hex, 16).expect("unicode");
                        out.push(char::from_u32(cp).expect("char"));
                    }
                }
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
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/milestone1.toml");
    let text = std::fs::read_to_string(path).expect("milestone1.toml");
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
            "op" => rec.op = v,
            "model" => rec.model = v,
            "input" => rec.input = v,
            "lhs" => rec.lhs = v,
            "rhs" => rec.rhs = v,
            "widths" => rec.widths = v,
            "heights" => rec.heights = v,
            "depths" => rec.depths = v,
            "expect" => rec.expect = v,
            "expect_width" => rec.expect_width = v,
            "expect_height" => rec.expect_height = v,
            "expect_depth" => rec.expect_depth = v,
            _ => panic!("unknown gold field {k}"),
        }
    }
    if !rec.name.is_empty() {
        recs.push(rec);
    }
    recs
}

fn csv_dims(s: &str) -> Vec<Dim> {
    s.split(',').map(|p| Dim::parse(p.trim())).collect()
}

#[test]
fn milestone1_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no golds loaded");
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    for rec in recs {
        match rec.kind.as_str() {
            "tokenize" => {
                let got = format_tokens(&tokenize(&rec.input).expect(&rec.name));
                assert_eq!(got, rec.expect, "{}", rec.name);
            }
            "tokenize_err" => {
                let err = tokenize(&rec.input).expect_err(&rec.name);
                assert_eq!(err, ParseError::TrailingBackslash, "{}", rec.name);
                assert!(err.to_string().contains(&rec.expect), "{}", rec.name);
            }
            "dim" => match rec.op.as_str() {
                "add" => {
                    let g = Dim::parse(&rec.lhs) + Dim::parse(&rec.rhs);
                    assert!(g.eq_dim(&Dim::parse(&rec.expect)), "{}", rec.name);
                }
                "mul" => {
                    let g = Dim::parse(&rec.lhs) * Dim::parse(&rec.rhs);
                    assert!(g.eq_dim(&Dim::parse(&rec.expect)), "{}", rec.name);
                }
                "div" => {
                    let g = Dim::parse(&rec.lhs) / Dim::parse(&rec.rhs);
                    assert!(g.eq_dim(&Dim::parse(&rec.expect)), "{}", rec.name);
                }
                "from_mu" => {
                    let g = Dim::from_mu(&Dim::parse(&rec.lhs));
                    assert!(g.eq_dim(&Dim::parse(&rec.expect)), "{}", rec.name);
                }
                "as_ratio" => {
                    let (n, d) = Dim::parse(&rec.lhs)
                        .as_ratio()
                        .unwrap_or_else(|| panic!("{}: NaN", rec.name));
                    assert_eq!(format!("{n}/{d}"), rec.expect, "{}", rec.name);
                }
                "font_units" => {
                    let units: i64 = rec.lhs.parse().expect("units");
                    let upem: u16 = rec.rhs.parse().expect("upem");
                    let g = Dim::from_font_units(units, upem);
                    assert!(g.eq_dim(&Dim::parse(&rec.expect)), "{}", rec.name);
                }
                other => panic!("{}: unknown dim op {other}", rec.name),
            },
            "box" => match rec.op.as_str() {
                "hpack" => {
                    let kids: Vec<MathBox> = csv_dims(&rec.widths)
                        .into_iter()
                        .map(|w| MathBox::rule(w, Dim::zero(), Dim::zero()))
                        .collect();
                    let b = MathBox::hpack(kids);
                    assert!(
                        b.width.eq_dim(&Dim::parse(&rec.expect_width)),
                        "{}: {}",
                        rec.name,
                        b.width
                    );
                }
                "vpack" => {
                    let hs = csv_dims(&rec.heights);
                    let ds = csv_dims(&rec.depths);
                    assert_eq!(hs.len(), ds.len(), "{}", rec.name);
                    let kids: Vec<MathBox> = hs
                        .into_iter()
                        .zip(ds)
                        .map(|(h, d)| MathBox::rule(Dim::zero(), h, d))
                        .collect();
                    let b = MathBox::vpack(kids);
                    assert!(
                        b.height.eq_dim(&Dim::parse(&rec.expect_height)),
                        "{}",
                        rec.name
                    );
                    assert!(
                        b.depth.eq_dim(&Dim::parse(&rec.expect_depth)),
                        "{}: {}",
                        rec.name,
                        b.depth
                    );
                }
                other => panic!("{}: unknown box op {other}", rec.name),
            },
            "font" => match rec.op.as_str() {
                "units_per_em" => {
                    assert_eq!(font.units_per_em().to_string(), rec.expect, "{}", rec.name);
                }
                "hhea" => {
                    let got = format!("{},{}", font.ascender_fu(), font.descender_fu());
                    assert_eq!(got, rec.expect, "{}", rec.name);
                }
                "advance" => {
                    let ch = rec.input.chars().next().expect("char");
                    let g = font.glyph(ch).expect(&rec.name);
                    assert_eq!(g.advance_fu.to_string(), rec.expect, "{}", rec.name);
                }
                "sha256" => {
                    assert_eq!(STIX_TWO_MATH_SHA256, rec.expect, "{}", rec.name);
                    assert_eq!(
                        MathFont::sha256_hex(STIX_TWO_MATH_OTF),
                        rec.expect,
                        "{}",
                        rec.name
                    );
                }
                "missing" => {
                    let ch = rec.input.chars().next().expect("char");
                    let err = font.glyph(ch).expect_err(&rec.name);
                    match err {
                        Error::Font(_) => {}
                        other => panic!("{}: {other}", rec.name),
                    }
                    assert!(err.to_string().contains(&rec.expect), "{}", rec.name);
                }
                other => panic!("{}: unknown font op {other}", rec.name),
            },
            "symbol" => match rec.op.as_str() {
                "count" => {
                    assert_eq!(symbols().len().to_string(), rec.expect, "{}", rec.name);
                }
                "lookup" => {
                    let e = lookup(&rec.input).expect(&rec.name);
                    if rec.expect.chars().count() == 1 {
                        assert_eq!(e.glyph, rec.expect, "{}", rec.name);
                    } else {
                        assert_eq!(e.latex, rec.expect, "{}", rec.name);
                    }
                }
                "category_count" => {
                    assert_eq!(
                        category_count(&rec.input).to_string(),
                        rec.expect,
                        "{}",
                        rec.name
                    );
                }
                other => panic!("{}: unknown symbol op {other}", rec.name),
            },
            "color" => match rec.op.as_str() {
                "named" => {
                    let c = named_color(&rec.input).expect(&rec.name);
                    assert_eq!(c.css_hex(), rec.expect, "{}", rec.name);
                }
                "named_count" => {
                    assert_eq!(
                        ColorTable::new().len().to_string(),
                        rec.expect,
                        "{}",
                        rec.name
                    );
                }
                "model" => {
                    let c = parse_color_spec(&rec.model, &rec.input, None).expect(&rec.name);
                    assert_eq!(c.css_hex(), rec.expect, "{}", rec.name);
                }
                "define" => {
                    let mut t = ColorTable::new();
                    let c = t.define(&rec.lhs, &rec.model, &rec.input).expect(&rec.name);
                    assert_eq!(c.css_hex(), rec.expect, "{}", rec.name);
                    assert_eq!(
                        t.get(&rec.lhs).expect("lookup").css_hex(),
                        rec.expect,
                        "{}",
                        rec.name
                    );
                }
                "error" => {
                    let err = parse_color_spec(&rec.model, &rec.input, None).expect_err(&rec.name);
                    assert!(
                        matches!(err, Error::Unsupported { .. }),
                        "{}: {err}",
                        rec.name
                    );
                    assert!(
                        err.to_string().to_ascii_lowercase().contains(&rec.expect),
                        "{}: {err}",
                        rec.name
                    );
                }
                "error_named" => {
                    let err = named_color(&rec.input).expect_err(&rec.name);
                    assert!(
                        matches!(err, Error::Unsupported { .. }),
                        "{}: {err}",
                        rec.name
                    );
                    assert!(
                        err.to_string().to_ascii_lowercase().contains(&rec.expect),
                        "{}: {err}",
                        rec.name
                    );
                }
                "error_malformed" => {
                    let err = parse_color_spec(&rec.model, &rec.input, None).expect_err(&rec.name);
                    assert!(
                        matches!(err, Error::Malformed { .. }),
                        "{}: {err}",
                        rec.name
                    );
                    assert!(
                        err.to_string().to_ascii_lowercase().contains(&rec.expect),
                        "{}: {err}",
                        rec.name
                    );
                }
                other => panic!("{}: unknown color op {other}", rec.name),
            },
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
}
