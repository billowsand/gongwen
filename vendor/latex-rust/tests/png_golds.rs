//! Gold runner: `golds/png.toml` is the PNG renderer contract.
#![cfg(feature = "png")]

use latex_rust::{
    latex_to_png, named_color, render_png, BoxContent, Dim, MathBox, MathFont, PngBackground,
    PngOptions,
};
use tiny_skia::Pixmap;

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    style: String,
    input: String,
    expect: String,
    dpi: String,
    width_px: String,
    height_px: String,
    sha256: String,
    background: String,
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
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/png.toml");
    let text = std::fs::read_to_string(path).expect("png.toml");
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
            "dpi" => rec.dpi = v,
            "width_px" => rec.width_px = v,
            "height_px" => rec.height_px = v,
            "sha256" => rec.sha256 = v,
            "background" => rec.background = v,
            _ => panic!("unknown gold field {k}"),
        }
    }
    if !rec.name.is_empty() {
        recs.push(rec);
    }
    recs
}

fn options_for(rec: &Rec) -> PngOptions {
    let mut opt = PngOptions::new();
    opt.display = rec.style == "display";
    if !rec.dpi.is_empty() {
        opt.dpi = Dim::parse(&rec.dpi);
    }
    opt.background = match rec.background.as_str() {
        "" | "transparent" => PngBackground::Transparent,
        "white" => PngBackground::White,
        name => {
            PngBackground::Color(named_color(name).unwrap_or_else(|e| panic!("{}: {e}", rec.name)))
        }
    };
    opt
}

fn png_for(font: &MathFont, rec: &Rec) -> Vec<u8> {
    latex_to_png(&rec.input, font, &options_for(rec))
        .unwrap_or_else(|e| panic!("{}: {e}", rec.name))
}

fn has_aa_edge(png: &[u8]) -> bool {
    let pm = Pixmap::decode_png(png).expect("decode png");
    pm.pixels().iter().any(|p| {
        let a = p.alpha();
        a > 0 && a < 255
    })
}

#[test]
fn png_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no png golds loaded");
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let mut dumps = Vec::new();
    for rec in recs {
        match rec.kind.as_str() {
            "png" => {
                let png = png_for(&font, &rec);
                let pm =
                    Pixmap::decode_png(&png).unwrap_or_else(|e| panic!("{}: decode {e}", rec.name));
                let hash = MathFont::sha256_hex(&png);
                if rec.sha256.is_empty() {
                    dumps.push(format!(
                        "{} width={} height={} sha256={}",
                        rec.name,
                        pm.width(),
                        pm.height(),
                        hash
                    ));
                    continue;
                }
                assert_eq!(pm.width().to_string(), rec.width_px, "{}: width", rec.name);
                assert_eq!(
                    pm.height().to_string(),
                    rec.height_px,
                    "{}: height",
                    rec.name
                );
                assert_eq!(hash, rec.sha256, "{}: sha256", rec.name);
            }
            "aa" => {
                let png = png_for(&font, &rec);
                assert!(
                    has_aa_edge(&png),
                    "{}: expected anti-aliased edge pixels",
                    rec.name
                );
            }
            "err" => {
                let err = latex_to_png(&rec.input, &font, &options_for(&rec)).expect_err(&rec.name);
                assert!(err.to_string().contains(&rec.expect), "{}: {err}", rec.name);
            }
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
    assert!(
        dumps.is_empty(),
        "png gold hashes not locked:\n{}",
        dumps.join("\n")
    );
}

#[test]
fn missing_glyph_id_is_err() {
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let bx = MathBox {
        width: Dim::one(),
        height: Dim::one(),
        depth: Dim::zero(),
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::glyph('x', 65535, Dim::one()),
    };
    let err = render_png(&bx, &font, &PngOptions::new()).expect_err("missing");
    assert!(err.to_string().contains("missing glyph"), "{err}");
}

#[test]
fn default_dpi_is_144() {
    let opt = PngOptions::new();
    assert_eq!(opt.dpi, Dim::from_i64(144));
    assert_eq!(opt.background, PngBackground::Transparent);
}
