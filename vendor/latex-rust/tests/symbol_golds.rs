//! Gold runner: `golds/symbols.toml` plus a catalog corpus.

use latex_rust::{
    latex_to_svg, layout, parse, styled_char, symbol_atom_kind, symbols, AtomKind, BoxContent,
    Error, MathBox, MathFont, MathNode, MathStyle, SvgOptions, SymbolKind, TextStyle,
};

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    input: String,
    expect: String,
    class: String,
    lhs: String,
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
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/symbols.toml");
    let text = std::fs::read_to_string(path).expect("symbols.toml");
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
            "input" => rec.input = v,
            "expect" => rec.expect = v,
            "class" => rec.class = v,
            "lhs" => rec.lhs = v,
            _ => panic!("unknown gold field {k}"),
        }
    }
    if !rec.name.is_empty() {
        recs.push(rec);
    }
    recs
}

fn glyphs(b: &MathBox) -> Vec<char> {
    match &b.content {
        BoxContent::Glyph { ch, .. } => vec![*ch],
        BoxContent::HList(v) | BoxContent::VList(v) | BoxContent::Overlap(v) => {
            v.iter().flat_map(glyphs).collect()
        }
        BoxContent::Color(_, inner)
        | BoxContent::BackColor(_, inner)
        | BoxContent::Frame { inner, .. } => glyphs(inner),
        _ => vec![],
    }
}

fn class_name(k: AtomKind) -> &'static str {
    match k {
        AtomKind::Ord => "Ord",
        AtomKind::Op => "Op",
        AtomKind::Bin => "Bin",
        AtomKind::Rel => "Rel",
        AtomKind::Open => "Open",
        AtomKind::Close => "Close",
        AtomKind::Punct => "Punct",
        AtomKind::Inner => "Inner",
    }
}

fn lay(font: &MathFont, input: &str) -> MathBox {
    let ast = parse(input).unwrap_or_else(|e| panic!("parse {input}: {e}"));
    layout(&ast, font, MathStyle::Text).unwrap_or_else(|e| panic!("layout {input}: {e}"))
}

fn is_bare_latex(latex: &str) -> bool {
    let t = latex.trim();
    if !t.starts_with('\\') {
        return t.chars().count() == 1;
    }
    let rest = &t[1..];
    rest.chars().all(|c| c.is_ascii_alphabetic())
        || (rest.chars().count() == 1 && !rest.chars().next().unwrap().is_ascii_alphabetic())
}

#[test]
fn symbol_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no symbol golds loaded");
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    for rec in recs {
        match rec.kind.as_str() {
            "glyph" => {
                let bx = lay(&font, &rec.input);
                let gs = glyphs(&bx);
                assert!(
                    gs.contains(&rec.expect.chars().next().expect("glyph")),
                    "{}: glyphs {gs:?} missing {}",
                    rec.name,
                    rec.expect
                );
                let svg = latex_to_svg(&rec.input, &font, &SvgOptions::new())
                    .unwrap_or_else(|e| panic!("{}: svg {e}", rec.name));
                assert!(svg.contains("<path"), "{}: no path", rec.name);
                if !rec.class.is_empty() {
                    let ast = parse(&rec.input).unwrap();
                    if let MathNode::Symbol(name) = ast {
                        assert_eq!(
                            class_name(symbol_atom_kind(&name)),
                            rec.class,
                            "{}",
                            rec.name
                        );
                    }
                }
            }
            "not" => {
                let bx = lay(&font, &rec.input);
                let gs = glyphs(&bx);
                let want = rec.expect.chars().next().expect("glyph");
                assert!(
                    gs.contains(&want),
                    "{}: glyphs {gs:?} missing {want}",
                    rec.name
                );
                assert!(
                    gs.len() >= 2,
                    "{}: \\not should overlay a slash, got {gs:?}",
                    rec.name
                );
            }
            "wider" => {
                let a = lay(&font, &rec.lhs);
                let b = lay(&font, &rec.input);
                assert!(
                    b.width
                        .cmp(&a.width)
                        .is_some_and(|o| o == std::cmp::Ordering::Greater),
                    "{}: {} width {} not greater than {} width {}",
                    rec.name,
                    rec.input,
                    b.width,
                    rec.lhs,
                    a.width
                );
            }
            "err_layout" => {
                let ch = rec.input.chars().next().expect("char");
                let ast = MathNode::Atom(ch, AtomKind::Ord);
                let err = layout(&ast, &font, MathStyle::Text).expect_err(&rec.name);
                match err {
                    Error::Font(_) => {}
                    other => panic!("{}: {other}", rec.name),
                }
                assert!(err.to_string().contains(&rec.expect), "{}: {err}", rec.name);
            }
            "err_parse" => {
                let err = parse(&rec.input).expect_err(&rec.name);
                assert!(
                    matches!(err, latex_rust::ParseError::Unsupported(_)),
                    "{}: {err}",
                    rec.name
                );
                assert_eq!(err.to_string(), rec.expect, "{}", rec.name);
            }
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
}

#[test]
fn catalog_single_glyphs_layout() {
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let mut failed = Vec::new();
    for e in symbols() {
        if !matches!(e.kind, SymbolKind::Symbol | SymbolKind::Operator) {
            continue;
        }
        if e.glyph.chars().count() != 1 {
            continue;
        }
        if !is_bare_latex(e.latex) {
            continue;
        }
        let name = e.command_name();
        if matches!(
            name,
            "mathbb" | "mathcal" | "mathfrak" | "text" | "mathring" | "lim"
        ) {
            continue;
        }
        let input = if e.latex.starts_with('\\') {
            format!("\\{name}")
        } else {
            e.latex.to_string()
        };
        let ast = match parse(&input) {
            Ok(a) => a,
            Err(err) => {
                failed.push(format!("{} parse {err}", e.latex));
                continue;
            }
        };
        let bx = match layout(&ast, &font, MathStyle::Text) {
            Ok(b) => b,
            Err(err) => {
                failed.push(format!("{} layout {err}", e.latex));
                continue;
            }
        };
        let gs = glyphs(&bx);
        let ch = math_italic(e.glyph.chars().next().unwrap());
        if !gs.contains(&ch) {
            failed.push(format!(
                "{} glyph {ch} not in {gs:?} (ast {})",
                e.latex,
                ast.gold()
            ));
        }
        let class = symbol_atom_kind(name);
        if matches!(ast, MathNode::Symbol(_)) && class != symbol_atom_kind(name) {
            failed.push(format!("{} class", e.latex));
        }
        if let Err(err) = latex_to_svg(&input, &font, &SvgOptions::new()) {
            failed.push(format!("{} svg {err}", e.latex));
        }
        let _ = class;
    }
    assert!(
        failed.is_empty(),
        "catalog glyph golds failed:\n{}",
        failed.join("\n")
    );
}

#[test]
fn font_style_letter_classes() {
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let styles: &[(TextStyle, &str)] = &[
        (TextStyle::Rm, "mathrm"),
        (TextStyle::Bf, "mathbf"),
        (TextStyle::It, "mathit"),
        (TextStyle::Sf, "mathsf"),
        (TextStyle::Tt, "mathtt"),
        (TextStyle::Bb, "mathbb"),
        (TextStyle::Cal, "mathcal"),
        (TextStyle::Frak, "mathfrak"),
        (TextStyle::Scr, "mathscr"),
    ];
    let mut failed = Vec::new();
    for (ts, cmd) in styles {
        let letters: Vec<char> = match *ts {
            TextStyle::Bb | TextStyle::Cal | TextStyle::Scr => ('A'..='Z').collect(),
            TextStyle::Frak => ('A'..='Z').chain('a'..='z').collect(),
            TextStyle::It => ('A'..='Z').chain('a'..='z').collect(),
            _ => ('A'..='Z').chain('a'..='z').chain('0'..='9').collect(),
        };
        for c in letters {
            let input = format!("\\{cmd}{{{c}}}");
            let want = styled_char(c, *ts);
            let ast = match parse(&input) {
                Ok(a) => a,
                Err(err) => {
                    failed.push(format!("{input} parse {err}"));
                    continue;
                }
            };
            match layout(&ast, &font, MathStyle::Text) {
                Ok(bx) => {
                    let gs = glyphs(&bx);
                    if !gs.contains(&want) {
                        failed.push(format!("{input} want {want} got {gs:?}"));
                    }
                }
                Err(err) => failed.push(format!("{input} layout {err}")),
            }
        }
    }
    assert!(
        failed.is_empty(),
        "font-style letter golds failed:\n{}",
        failed.join("\n")
    );
}

/// Plain math draws lowercase Greek in the Mathematical Italic block (issue #2).
fn math_italic(c: char) -> char {
    let lower_greek = ('\u{03B1}'..='\u{03C9}').contains(&c);
    if lower_greek || matches!(c, 'ϵ' | 'ϑ' | 'ϰ' | 'ϕ' | 'ϱ' | 'ϖ') {
        styled_char(c, TextStyle::It)
    } else {
        c
    }
}
