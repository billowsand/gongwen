//! Gold runner: `golds/egui.toml` is the egui renderer contract.
#![cfg(feature = "egui")]

use egui::{Pos2, Shape};
use latex_rust::{
    latex_to_shapes, render_egui, shapes, BoxContent, Color, Dim, EguiOptions, MathBox, MathFont,
};

#[derive(Default)]
struct Rec {
    name: String,
    kind: String,
    style: String,
    input: String,
    expect: String,
    font_size_pt: String,
    pixels_per_point: String,
    shape_count: String,
    mesh_count: String,
    rect_count: String,
    rect_width_px: String,
    rect_height_px: String,
    vertex_count: String,
    index_count: String,
    color: String,
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
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/golds/egui.toml");
    let text = std::fs::read_to_string(path).expect("egui.toml");
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
            "font_size_pt" => rec.font_size_pt = v,
            "pixels_per_point" => rec.pixels_per_point = v,
            "shape_count" => rec.shape_count = v,
            "mesh_count" => rec.mesh_count = v,
            "rect_count" => rec.rect_count = v,
            "rect_width_px" => rec.rect_width_px = v,
            "rect_height_px" => rec.rect_height_px = v,
            "vertex_count" => rec.vertex_count = v,
            "index_count" => rec.index_count = v,
            "color" => rec.color = v,
            _ => panic!("unknown gold field {k}"),
        }
    }
    if !rec.name.is_empty() {
        recs.push(rec);
    }
    recs
}

fn options_for(rec: &Rec) -> (EguiOptions, f32) {
    let mut opt = EguiOptions::new();
    opt.display = rec.style == "display";
    if !rec.font_size_pt.is_empty() {
        opt.font_size_pt = Dim::parse(&rec.font_size_pt);
    }
    if rec.color == "blue" {
        opt.color = Color::rgb(0, 0, 255);
    }
    let ppp = if rec.pixels_per_point.is_empty() {
        1.0
    } else {
        f32::from_bits(Dim::parse(&rec.pixels_per_point).to_ieee32_bits())
    };
    (opt, ppp)
}

fn emit_for(font: &MathFont, rec: &Rec) -> (Vec<Shape>, egui::Rect) {
    let (opt, ppp) = options_for(rec);
    latex_to_shapes(&rec.input, font, &opt, Pos2::ZERO, ppp)
        .unwrap_or_else(|e| panic!("{}: {e}", rec.name))
}

fn mesh_stats(shapes: &[Shape]) -> (usize, usize) {
    let mut vertices = 0usize;
    let mut indices = 0usize;
    for s in shapes {
        if let Shape::Mesh(m) = s {
            assert!(
                m.vertices.len() >= 3,
                "mesh must tessellate to at least one triangle's vertices"
            );
            assert!(
                m.indices.len() >= 3 && m.indices.len() % 3 == 0,
                "mesh must have triangle indices"
            );
            vertices += m.vertices.len();
            indices += m.indices.len();
        }
    }
    (vertices, indices)
}

fn count_kind(shapes: &[Shape]) -> (usize, usize, usize) {
    let mut mesh = 0;
    let mut rect = 0;
    let mut other = 0;
    for s in shapes {
        match s {
            Shape::Mesh(_) => mesh += 1,
            Shape::Rect(_) => rect += 1,
            _ => other += 1,
        }
    }
    (mesh, rect, other)
}

fn first_mesh_hex(shapes: &[Shape]) -> Option<String> {
    for s in shapes {
        if let Shape::Mesh(m) = s {
            if let Some(v) = m.vertices.first() {
                return Some(format!(
                    "#{:02x}{:02x}{:02x}",
                    v.color.r(),
                    v.color.g(),
                    v.color.b()
                ));
            }
        }
    }
    None
}

fn first_rect_hex(shapes: &[Shape]) -> Option<String> {
    for s in shapes {
        if let Shape::Rect(r) = s {
            let c = r.fill;
            return Some(format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()));
        }
    }
    None
}

fn fmt_px(v: f32) -> String {
    Dim::from_ieee32_bits(v.to_bits()).to_dec_string()
}

#[test]
fn egui_golds() {
    let recs = load_golds();
    assert!(!recs.is_empty(), "no egui golds loaded");
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let mut dumps = Vec::new();
    for rec in recs {
        match rec.kind.as_str() {
            "egui" => {
                let (shapes, rect) = emit_for(&font, &rec);
                let (mesh, rct, other) = count_kind(&shapes);
                let (vertices, indices) = if mesh > 0 {
                    mesh_stats(&shapes)
                } else {
                    (0, 0)
                };
                assert_eq!(other, 0, "{}: unexpected non-mesh/rect shapes", rec.name);
                let dump = format!(
                    "{} shapes={} mesh={} rect={} verts={} idxs={} w={} h={}",
                    rec.name,
                    shapes.len(),
                    mesh,
                    rct,
                    vertices,
                    indices,
                    fmt_px(rect.width()),
                    fmt_px(rect.height())
                );
                if rec.shape_count.is_empty()
                    || rec.mesh_count.is_empty()
                    || rec.rect_count.is_empty()
                    || rec.rect_width_px.is_empty()
                    || rec.rect_height_px.is_empty()
                    || rec.vertex_count.is_empty()
                    || rec.index_count.is_empty()
                {
                    dumps.push(dump);
                    continue;
                }
                assert_eq!(
                    shapes.len().to_string(),
                    rec.shape_count,
                    "{}: shape_count",
                    rec.name
                );
                assert_eq!(mesh.to_string(), rec.mesh_count, "{}: mesh_count", rec.name);
                assert_eq!(rct.to_string(), rec.rect_count, "{}: rect_count", rec.name);
                assert_eq!(
                    vertices.to_string(),
                    rec.vertex_count,
                    "{}: vertex_count",
                    rec.name
                );
                assert_eq!(
                    indices.to_string(),
                    rec.index_count,
                    "{}: index_count",
                    rec.name
                );
                assert_eq!(
                    fmt_px(rect.width()),
                    rec.rect_width_px,
                    "{}: rect_width",
                    rec.name
                );
                assert_eq!(
                    fmt_px(rect.height()),
                    rec.rect_height_px,
                    "{}: rect_height",
                    rec.name
                );
                if rec.color == "red" {
                    assert_eq!(
                        first_mesh_hex(&shapes).as_deref(),
                        Some("#ff0000"),
                        "{}: mesh color",
                        rec.name
                    );
                }
                if rec.color == "yellow-rect" {
                    assert_eq!(
                        first_rect_hex(&shapes).as_deref(),
                        Some("#ffff00"),
                        "{}: rect color",
                        rec.name
                    );
                }
                if rec.color == "blue" {
                    assert_eq!(
                        first_mesh_hex(&shapes).as_deref(),
                        Some("#0000ff"),
                        "{}: theme color",
                        rec.name
                    );
                }
            }
            "err" => {
                let (opt, ppp) = options_for(&rec);
                let err =
                    latex_to_shapes(&rec.input, &font, &opt, Pos2::ZERO, ppp).expect_err(&rec.name);
                assert!(err.to_string().contains(&rec.expect), "{}: {err}", rec.name);
            }
            other => panic!("{}: unknown kind {other}", rec.name),
        }
    }
    assert!(
        dumps.is_empty(),
        "egui golds not locked:\n{}",
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
    let err = shapes(&bx, &font, &EguiOptions::new(), Pos2::ZERO, 1.0).expect_err("missing");
    assert!(err.to_string().contains("missing glyph"), "{err}");
}

#[test]
fn zero_ppp_is_invalid() {
    let font = MathFont::stix_two_math().expect("STIX");
    let err = shapes(
        &MathBox::empty(),
        &font,
        &EguiOptions::new(),
        Pos2::ZERO,
        0.0,
    )
    .expect_err("ppp");
    assert!(err.to_string().contains("invalid option"), "{err}");
}

#[test]
fn render_egui_empty_ok() {
    let font = MathFont::stix_two_math().expect("STIX");
    render_egui(&MathBox::empty(), &font).expect("empty");
}

#[test]
fn hidpi_scales() {
    let font = MathFont::stix_two_math().expect("STIX");
    let opt = EguiOptions::new();
    let (_, r1) = latex_to_shapes("x", &font, &opt, Pos2::ZERO, 1.0).expect("1");
    let (_, r2) = latex_to_shapes("x", &font, &opt, Pos2::ZERO, 2.0).expect("2");
    let (_, r15) = latex_to_shapes("x", &font, &opt, Pos2::ZERO, 1.5).expect("1.5");
    let w1 = Dim::from_ieee32_bits(r1.width().to_bits());
    let w2 = Dim::from_ieee32_bits(r2.width().to_bits());
    let w15 = Dim::from_ieee32_bits(r15.width().to_bits());
    let two = Dim::from_ieee32_bits((2.0f32).to_bits());
    assert!(w2.eq_dim(&(w1.clone() * two)), "2x width");
    assert!(
        matches!(w15.cmp(&w1), Some(core::cmp::Ordering::Greater)),
        "1.5x wider than 1x"
    );
    assert!(
        matches!(w2.cmp(&w15), Some(core::cmp::Ordering::Greater)),
        "2x wider than 1.5x"
    );
}
