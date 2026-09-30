//! Nesting-depth limit: the default is safe on a 1 MiB stack, and callers can move it.

use latex_rust::{
    layout, layout_with_max_depth, parse, parse_with_options, render_svg, MathFont, MathStyle,
    ParseOptions, SvgOptions, DEFAULT_MAX_NESTING_DEPTH,
};

/// A named nesting shape that builds input `n` levels deep.
type Shape = (&'static str, fn(usize) -> String);

/// Nesting shapes, each building input `n` levels deep.
fn shapes() -> Vec<Shape> {
    vec![
        ("braces", |n| "{".repeat(n) + "x" + &"}".repeat(n)),
        ("frac", |n| "\\frac{1}{".repeat(n) + "2" + &"}".repeat(n)),
        ("sqrt", |n| "\\sqrt{".repeat(n) + "x" + &"}".repeat(n)),
        ("sup", |n| "x^{".repeat(n) + "y" + &"}".repeat(n)),
        ("left", |n| {
            "\\left(".repeat(n) + "x" + &"\\right)".repeat(n)
        }),
        ("mathrm", |n| "\\mathrm{".repeat(n) + "x" + &"}".repeat(n)),
    ]
}

/// Deepest `n` for which `make(n)` parses under `opts`.
fn deepest(make: fn(usize) -> String, opts: &ParseOptions) -> usize {
    let mut n = 0;
    while parse_with_options(&make(n + 1), opts).is_ok() {
        n += 1;
        assert!(n < 10_000, "limit never reached");
    }
    n
}

/// The smallest stack the default limit is documented to be safe on: 1 MiB in an
/// optimised build (the `wasm32` default), 2 MiB unoptimised (the `std::thread` default).
const SMALL_STACK: usize = if cfg!(debug_assertions) {
    2 << 20
} else {
    1 << 20
};

fn on_small_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("worker panicked");
}

#[test]
fn deepest_default_input_parses_lays_out_and_renders_on_small_stack() {
    on_small_stack(|| {
        let font = MathFont::stix_two_math().expect("font");
        let opts = ParseOptions::default();
        for (name, make) in shapes() {
            let n = deepest(make, &opts);
            assert!(n >= 5, "{name}: default admits only {n} levels");
            eprintln!("{name}: default admits {n} levels");
            let ast = parse(&make(n)).unwrap_or_else(|e| panic!("{name}@{n}: {e}"));
            let bx = layout(&ast, &font, MathStyle::Display)
                .unwrap_or_else(|e| panic!("{name}@{n}: parse accepted but layout refused: {e}"));
            render_svg(&bx, &font, &SvgOptions::default())
                .unwrap_or_else(|e| panic!("{name}@{n}: render: {e}"));
        }
    });
}

#[test]
fn pathological_input_errs_instead_of_aborting_on_small_stack() {
    on_small_stack(|| {
        for (name, make) in shapes() {
            for n in [DEFAULT_MAX_NESTING_DEPTH + 1, 1_000, 100_000] {
                assert!(parse(&make(n)).is_err(), "{name}@{n} should be refused");
            }
        }
    });
}

#[test]
fn caller_can_raise_and_lower_the_limit() {
    // A raised limit needs a caller that has the stack for it, as the docs say.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| {
            let font = MathFont::stix_two_math().expect("font");
            let tight = ParseOptions::new().with_max_depth(8);
            let roomy = ParseOptions::new().with_max_depth(128);
            for (name, make) in shapes() {
                let d_tight = deepest(make, &tight);
                let d_default = deepest(make, &ParseOptions::default());
                let d_roomy = deepest(make, &roomy);
                assert!(
                    d_tight < d_default && d_default < d_roomy,
                    "{name}: {d_tight} {d_default} {d_roomy}"
                );
                let (ast, _) = parse_with_options(&make(d_roomy), &roomy).expect("roomy parse");
                assert!(
                    layout_with_max_depth(&ast, &font, MathStyle::Display, 128).is_ok(),
                    "{name}@{d_roomy}: roomy layout"
                );
            }
        })
        .expect("spawn")
        .join()
        .expect("worker panicked");
}

#[test]
fn default_is_32() {
    assert_eq!(DEFAULT_MAX_NESTING_DEPTH, 32);
    assert_eq!(ParseOptions::default().max_depth, 32);
}
