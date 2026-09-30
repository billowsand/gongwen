//! Issue #1: script glyphs are drawn at script size by every backend, not
//! only laid out at script size.

use latex_rust::{
    latex_to_svg, layout, parse, BoxContent, Dim, MathBox, MathFont, MathStyle, SvgOptions,
};

fn glyphs(b: &MathBox, out: &mut Vec<(char, Dim)>) {
    match &b.content {
        BoxContent::Glyph { ch, scale, .. } => out.push((*ch, scale.clone())),
        BoxContent::HList(v) | BoxContent::VList(v) | BoxContent::Overlap(v) => {
            v.iter().for_each(|k| glyphs(k, out));
        }
        BoxContent::Color(_, k) | BoxContent::BackColor(_, k) => glyphs(k, out),
        _ => {}
    }
}

fn scales(latex: &str) -> Vec<(char, Dim)> {
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let bx = layout(&parse(latex).expect("parse"), &font, MathStyle::Text).expect("layout");
    let mut out = Vec::new();
    glyphs(&bx, &mut out);
    out
}

/// Script scale from the font's MATH table (`ScriptPercentScaleDown`).
fn script_scale() -> Dim {
    scales("x^2")[1].1.clone()
}

#[test]
fn layout_records_script_and_scriptscript_scale() {
    let s = scales("x^{2^3}");
    assert_eq!(s.len(), 3);
    assert_eq!(s[0].1, Dim::one(), "base at text size");
    assert_eq!(
        s[1].1,
        Dim::from_i64(7) / Dim::from_i64(10),
        "STIX ScriptPercentScaleDown = 70"
    );
    assert_eq!(
        s[2].1,
        Dim::from_i64(11) / Dim::from_i64(20),
        "STIX ScriptScriptPercentScaleDown = 55"
    );
}

#[test]
fn svg_script_glyph_transform_is_scaled() {
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let svg = latex_to_svg("x^2", &font, &SvgOptions::new()).expect("svg");
    let fu_pt = Dim::from_i64(12) / Dim::from_i64(i64::from(font.units_per_em()));
    let text = format!(
        "scale({} {})",
        fu_pt.to_svg_string(),
        (-fu_pt.clone()).to_svg_string()
    );
    let k = &fu_pt * &script_scale();
    let script = format!(
        "scale({} {})",
        k.to_svg_string(),
        (-k.clone()).to_svg_string()
    );
    assert_eq!(svg.matches(&text).count(), 1, "base x at text size\n{svg}");
    assert_eq!(
        svg.matches(&script).count(),
        1,
        "superscript 2 at script size\n{svg}"
    );
}

#[cfg(feature = "png")]
#[test]
fn png_glyph_ink_follows_glyph_scale() {
    use latex_rust::{render_png, PngOptions};
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let two = layout(&parse("2").expect("parse"), &font, MathStyle::Text).expect("layout");
    let gid = match &two.content {
        BoxContent::HList(v) => match &v[0].content {
            BoxContent::Glyph { glyph_id, .. } => *glyph_id,
            other => panic!("{other:?}"),
        },
        BoxContent::Glyph { glyph_id, .. } => *glyph_id,
        other => panic!("{other:?}"),
    };
    // A roomy box so the unscaled glyph is never clipped by the canvas.
    let ink_rows = |scale: Dim| {
        let bx = MathBox {
            width: Dim::from_i64(2),
            height: Dim::from_i64(2),
            depth: Dim::one(),
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::glyph('2', gid, scale),
        };
        let mut opt = PngOptions::new();
        opt.dpi = Dim::from_i64(600);
        let pm = tiny_skia::Pixmap::decode_png(&render_png(&bx, &font, &opt).expect("png"))
            .expect("decode");
        let w = pm.width() as usize;
        let rows: Vec<usize> = pm
            .pixels()
            .chunks(w)
            .enumerate()
            .filter(|(_, r)| r.iter().any(|p| p.alpha() > 127))
            .map(|(i, _)| i)
            .collect();
        (rows.last().expect("ink") - rows[0] + 1) as f64
    };
    let text = ink_rows(Dim::one());
    let script = ink_rows(script_scale());
    let want = 0.7 * text;
    assert!(
        (script - want).abs() <= 2.0,
        "script ink {script}px, text {text}px, want about {want}px"
    );
}

#[cfg(feature = "egui")]
#[test]
fn egui_script_glyph_mesh_is_scaled() {
    use egui::{Pos2, Shape};
    use latex_rust::{latex_to_shapes, EguiOptions};
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let mesh_h = |latex: &str| {
        let (shapes, _) =
            latex_to_shapes(latex, &font, &EguiOptions::new(), Pos2::ZERO, 1.0).expect("shapes");
        let ys: Vec<f32> = shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Mesh(m) => Some(m.vertices.iter().map(|v| v.pos.y).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect();
        let lo = ys.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        hi - lo
    };
    let ratio = mesh_h("{}^{2}") / mesh_h("2");
    assert!(
        (ratio - 0.7).abs() < 1e-3,
        "script/text mesh height ratio {ratio}"
    );
}
