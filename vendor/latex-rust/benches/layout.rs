//! Parse, layout, SVG, PNG, and egui timings (run with `cargo bench`).
//!
//! PNG and egui rows require `--features png,egui`. A miss panics so a slow
//! regression fails the bench.

use std::time::{Duration, Instant};

use latex_rust::{layout, parse, render_svg, MathFont, MathStyle, SvgOptions};

#[cfg(feature = "png")]
use latex_rust::{render_png, PngOptions};
#[cfg(feature = "egui")]
use latex_rust::{shapes, EguiOptions};

const FRAC: &str = r"\frac{1}{2}";
const DISPLAY: &str = r"\sum_{n=1}^{N}\frac{1}{n^{2}}=\frac{\pi^{2}}{6}";

fn avg(iters: u32, warmup: u32, mut f: impl FnMut()) -> Duration {
    for _ in 0..warmup {
        f();
    }
    let t0 = Instant::now();
    for _ in 0..iters {
        f();
    }
    t0.elapsed() / iters
}

fn report(name: &str, got: Duration, target: Duration) {
    let ok = got <= target;
    println!(
        "{name:42} {got:>10?}  target {target:?}  {}",
        if ok { "OK" } else { "MISS" }
    );
    assert!(ok, "{name} {got:?} exceeds target {target:?}");
}

fn main() {
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let svg_opt = SvgOptions::new();

    let parse_frac = avg(8_000, 64, || {
        let _ = parse(FRAC).expect("parse frac");
    });
    report("parse \\frac{1}{2}", parse_frac, Duration::from_micros(50));

    let parse_disp = avg(2_000, 32, || {
        let _ = parse(DISPLAY).expect("parse display");
    });
    report(
        "parse full display equation",
        parse_disp,
        Duration::from_micros(200),
    );

    let ast_frac = parse(FRAC).expect("parse");
    let ast_disp = parse(DISPLAY).expect("parse");

    let lay_frac = avg(2_000, 32, || {
        let _ = layout(&ast_frac, &font, MathStyle::Text).expect("layout frac");
    });
    report("layout \\frac{1}{2}", lay_frac, Duration::from_micros(100));

    let lay_disp = avg(400, 16, || {
        let _ = layout(&ast_disp, &font, MathStyle::Display).expect("layout display");
    });
    report(
        "layout full display equation",
        lay_disp,
        Duration::from_micros(500),
    );

    let tree_frac = layout(&ast_frac, &font, MathStyle::Text).expect("layout frac");
    let tree_disp = layout(&ast_disp, &font, MathStyle::Display).expect("layout display");

    let svg_frac = avg(400, 16, || {
        let _ = render_svg(&tree_frac, &font, &svg_opt).expect("svg frac");
    });
    report("SVG \\frac{1}{2}", svg_frac, Duration::from_micros(500));

    let mut disp_opt = SvgOptions::new();
    disp_opt.display = true;
    let svg_disp = avg(200, 8, || {
        let _ = render_svg(&tree_disp, &font, &disp_opt).expect("svg display");
    });
    report(
        "SVG full display equation",
        svg_disp,
        Duration::from_millis(1),
    );

    #[cfg(feature = "png")]
    {
        let mut png144 = PngOptions::new();
        png144.dpi = latex_rust::Dim::from_i64(144);
        png144.display = true;
        let png_144 = avg(80, 4, || {
            let _ = render_png(&tree_disp, &font, &png144).expect("png 144");
        });
        report("PNG 144 DPI (display)", png_144, Duration::from_millis(5));

        let mut png300 = PngOptions::new();
        png300.dpi = latex_rust::Dim::from_i64(300);
        png300.display = true;
        let png_300 = avg(40, 2, || {
            let _ = render_png(&tree_disp, &font, &png300).expect("png 300");
        });
        report("PNG 300 DPI (display)", png_300, Duration::from_millis(15));
    }
    #[cfg(not(feature = "png"))]
    println!("PNG benches skipped (enable --features png)");

    #[cfg(feature = "egui")]
    {
        let tree = layout(&ast_frac, &font, MathStyle::Text).expect("layout");
        let opt = EguiOptions::new();
        // First call tessellates glyphs (cache miss). Later calls reuse triangles.
        let t0 = Instant::now();
        let _ = shapes(&tree, &font, &opt, egui::Pos2::ZERO, 1.0).expect("egui miss");
        report(
            "egui inline (cache miss)",
            t0.elapsed(),
            Duration::from_millis(2),
        );

        let hit = avg(2_000, 32, || {
            let _ = shapes(&tree, &font, &opt, egui::Pos2::ZERO, 1.0).expect("egui hit");
        });
        report("egui inline (cache hit)", hit, Duration::from_micros(100));
    }
    #[cfg(not(feature = "egui"))]
    println!("egui benches skipped (enable --features egui)");
}
