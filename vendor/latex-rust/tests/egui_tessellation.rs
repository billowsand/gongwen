//! Every glyph the egui backend is asked for tessellates.
//!
//! Before 1.0.5 / 2.0.1 the egui backend's ear clipper got stuck on glyphs
//! such as `H`, math italic `t`, `π` and `σ` and returned
//! `Error::Unsupported { what: "glyph tessellation" }` for any formula that
//! used them, and it drew ink outside a few others (`B`, `k`, `τ`, `↦`).
#![cfg(feature = "egui")]

use egui::{Pos2, Shape};
use latex_rust::{latex_to_shapes, symbols, EguiOptions, MathFont, SymbolKind};

fn meshes(latex: &str) -> usize {
    let font = MathFont::stix_two_math().expect("STIX Two Math");
    let (shapes, _) = latex_to_shapes(latex, &font, &EguiOptions::new(), Pos2::ZERO, 1.0)
        .unwrap_or_else(|e| panic!("{latex}: {e:?}"));
    shapes
        .iter()
        .filter(|s| matches!(s, Shape::Mesh(m) if !m.indices.is_empty()))
        .count()
}

#[test]
fn formulas_that_failed_before_render() {
    for latex in [
        r"\partial_t u = \Delta u",
        r"\begin{equation} e^{i\pi} + 1 = 0 \end{equation}",
        r"e^{-\frac{x^2}{2\sigma^2}}",
        r"\det(A) = \sum_{\sigma \in S_n} \operatorname{sgn}(\sigma) \prod_{i=1}^n a_{i,\sigma(i)}",
        r"\mathrm{d}x \quad \mathbf{v} \quad \mathit{text} \quad \mathcal{L} \quad \mathbb{R} \quad \mathfrak{g} \quad \mathsf{S} \quad \mathtt{t}",
        r"H",
        r"t",
        r"\mathrm{H}",
        r"\mathbf{H}",
        r"\mathbf{\pi} + \mathbf{\sigma} + \mathbf{\Phi}",
        r"\mathbb{B} \mathbb{T} \mathbb{g} \mathbb{h} \mathbb{m} \mathbb{n} \mathbb{u}",
        r"\mathtt{B} \mathtt{H} \mathtt{u}",
        r"\dashv \natural \boxtimes \nvDash \# \$",
        r"\text{€ ¥}",
        r"B + k + \tau + \mapsto",
    ] {
        assert!(meshes(latex) > 0, "{latex}");
    }
}

#[test]
fn every_letter_in_every_math_alphabet_renders() {
    let latin: String = ('a'..='z').chain('A'..='Z').collect();
    let digits = "0123456789";
    assert!(meshes(&latin) >= 52);
    for font in [
        "mathrm", "mathbf", "mathit", "mathcal", "mathbb", "mathfrak", "mathsf", "mathtt",
    ] {
        meshes(&format!(r"\{font}{{{latin}}}"));
        meshes(&format!(r"\{font}{{{digits}}}"));
    }
    for g in [
        "alpha",
        "beta",
        "gamma",
        "delta",
        "epsilon",
        "varepsilon",
        "zeta",
        "eta",
        "theta",
        "vartheta",
        "iota",
        "kappa",
        "lambda",
        "mu",
        "nu",
        "xi",
        "pi",
        "varpi",
        "rho",
        "varrho",
        "sigma",
        "varsigma",
        "tau",
        "upsilon",
        "phi",
        "varphi",
        "chi",
        "psi",
        "omega",
        "Gamma",
        "Delta",
        "Theta",
        "Lambda",
        "Xi",
        "Pi",
        "Sigma",
        "Upsilon",
        "Phi",
        "Psi",
        "Omega",
    ] {
        meshes(&format!(r"\{g} \mathbf{{\{g}}} x_{{\{g}}}^{{\{g}}}"));
    }
}

#[test]
fn every_catalog_symbol_renders_in_egui() {
    for e in symbols() {
        let latex = match e.kind {
            SymbolKind::Symbol | SymbolKind::Operator if !e.latex.contains("{}") => {
                e.latex.to_string()
            }
            _ => e.latex.replace("{}", "{x}"),
        };
        if latex_rust::parse(&latex).is_err() {
            continue;
        }
        meshes(&latex);
        meshes(&format!("x_{{{latex}}}^{{{latex}}}"));
    }
}
