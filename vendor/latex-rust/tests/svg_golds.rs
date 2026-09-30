//! Gold runner: `golds/svg.toml` is the SVG renderer contract.

use latex_rust::{latex_to_svg, MathFont, SvgOptions};

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    style: String,
    input: String,
    expect: String,
    paths: String,
    rects: String,
    lines: String,
    fill: String,
    contains: String,
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
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/svg.toml");
    let text = std::fs::read_to_string(path).expect("svg.toml");
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
            "paths" => rec.paths = v,
            "rects" => rec.rects = v,
            "lines" => rec.lines = v,
            "fill" => rec.fill = v,
            "contains" => rec.contains = v,
            _ => panic!("unknown gold field {k}"),
        }
    }
    if !rec.name.is_empty() {
        recs.push(rec);
    }
    recs
}

fn count_tag(svg: &str, tag: &str) -> usize {
    let open = format!("<{tag} ");
    let open2 = format!("<{tag}>");
    svg.matches(&open).count() + svg.matches(&open2).count()
}

fn svg_for(font: &MathFont, rec: &Rec) -> String {
    let mut opt = SvgOptions::new();
    opt.display = rec.style == "display";
    latex_to_svg(&rec.input, font, &opt).unwrap_or_else(|e| panic!("{}: {e}", rec.name))
}

#[test]
fn svg_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no svg golds loaded");
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    for rec in recs {
        match rec.kind.as_str() {
            "svg" => {
                let svg = svg_for(&font, &rec);
                assert!(
                    svg.contains("xmlns=\"http://www.w3.org/2000/svg\""),
                    "{}: missing xmlns",
                    rec.name
                );
                if !rec.paths.is_empty() {
                    assert_eq!(
                        count_tag(&svg, "path").to_string(),
                        rec.paths,
                        "{}: path count\n{svg}",
                        rec.name
                    );
                }
                if !rec.rects.is_empty() {
                    assert_eq!(
                        count_tag(&svg, "rect").to_string(),
                        rec.rects,
                        "{}: rect count\n{svg}",
                        rec.name
                    );
                }
                if !rec.lines.is_empty() {
                    assert_eq!(
                        count_tag(&svg, "line").to_string(),
                        rec.lines,
                        "{}: line count\n{svg}",
                        rec.name
                    );
                }
                if !rec.fill.is_empty() {
                    assert!(
                        svg.contains(&rec.fill),
                        "{}: missing fill {}\n{svg}",
                        rec.name,
                        rec.fill
                    );
                }
                if !rec.contains.is_empty() {
                    assert!(
                        svg.contains(&rec.contains),
                        "{}: missing {}\n{svg}",
                        rec.name,
                        rec.contains
                    );
                }
                if !rec.expect.is_empty() {
                    assert!(
                        svg.contains(&rec.expect),
                        "{}: missing expect {}\n{svg}",
                        rec.name,
                        rec.expect
                    );
                }
            }
            "err" => {
                let mut opt = SvgOptions::new();
                opt.display = rec.style == "display";
                let err = latex_to_svg(&rec.input, &font, &opt).expect_err(&rec.name);
                assert!(err.to_string().contains(&rec.expect), "{}: {err}", rec.name);
            }
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
}

#[test]
fn missing_glyph_id_is_err() {
    use latex_rust::{render_svg, BoxContent, Dim, MathBox, MathFont, SvgOptions};
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let bx = MathBox {
        width: Dim::one(),
        height: Dim::one(),
        depth: Dim::zero(),
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::glyph('x', 65535, Dim::one()),
    };
    let err = render_svg(&bx, &font, &SvgOptions::new()).expect_err("missing");
    assert!(err.to_string().contains("missing glyph"), "{err}");
}
