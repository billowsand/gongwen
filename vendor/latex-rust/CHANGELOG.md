# Changelog

## [2.0.1] — 2026-09-27

A fix for the egui backend. No API changes; upgrade with
`cargo update -p latex-rust`.

### Fixed

- The egui backend renders every glyph. Before, formulas using `H`, math italic
  `t` (so `\partial_t u`), `π`, `σ`, `#`, `$`, `€`, `\dashv`, `\natural`, bold
  and blackboard-bold capitals and about 260 other glyphs failed with
  `Error::Unsupported { what: "glyph tessellation" }`. `B`, `k`, `τ`, `↦` and
  about 140 other glyphs were drawn with a stray triangle outside the glyph.
  The ear clipper that triangulates glyph outlines accepted ears whose closing
  diagonal ran through another vertex. Its result is now checked exactly, and
  outlines it cannot triangulate correctly are triangulated by exact scanline
  trapezoids under the non-zero rule, as the SVG and PNG backends fill them.
  Glyphs that were already drawn correctly get the same meshes as before.

## [2.0.0] — 2026-09-27

This release fixes the eight defects Tom Clark reported while integrating
latex-rust into IronLAB (issues #1–#7 and #11). Several of them change what the
renderer draws, so many golds were re-recorded, each in its own commit next to
the fix that moved it.

### Breaking changes

- `BoxContent::Glyph` has a new `scale: Dim` field, the factor the layout engine
  applied to the glyph's metrics. The variant is now `#[non_exhaustive]`, so
  match it with `BoxContent::Glyph { ch, glyph_id, .. }` and build it with the
  new `BoxContent::glyph(ch, glyph_id, scale)`. (#1)
- Parsing and layout now return `Err` for input nested deeper than
  `DEFAULT_MAX_NESTING_DEPTH` (32). Before, such input overflowed the stack and
  aborted the process. (#6)
- Output changes: variable letters are drawn in math italic, math-mode `-` is
  drawn as U+2212, script glyphs are drawn smaller, and diacritic accents sit
  lower. Code that pinned the old dimensions or images will see new values.

### Fixed

- Superscripts, subscripts and limits are drawn at script size in the SVG, PNG
  and egui renderers. Before, they were laid out at script size but drawn at
  full size. (#1)
- Unstyled Latin letters and lowercase Greek letters are set in math italic, as
  TeX does. Digits, uppercase Greek, `\mathrm`, `\text` and operator names stay
  upright. (#2)
- A math-mode `-` is drawn as U+2212 MINUS SIGN instead of the hyphen. It keeps
  the `Bin` class, and `-` inside `\text{...}` is still a hyphen. (#3)
- Font switches such as `\mathrm` reach nested subformulae (script bases,
  fractions, radicals). (#4)
- Diacritic accents (`\hat`, `\dot`, `\bar` and the rest) are placed by the TeX
  rule, lowering them by the font's accent base height. (#5)
- Deeply nested input returns `Err` instead of aborting the process. (#6)
- `STIX_TWO_MATH_OTF` is a `static`, so the font is linked into a binary once
  rather than once per use. (#11)

### Added

- `BoxContent::glyph(ch, glyph_id, scale)`. (#1)
- `Dim::as_ratio()` returns a `Dim`'s exact value as a numerator and
  denominator. (#7)
- `MathFont::face()` exposes the parsed OpenType face, and the crate re-exports
  `ttf_parser` so consumers can name its types. (#7)
- `ParseOptions`, `parse_with_options`, `layout_with_max_depth` and
  `DEFAULT_MAX_NESTING_DEPTH` let embedders choose the nesting limit. (#6)
- `tests/script_scale.rs` checks script glyph size in every backend, and new
  golds pin `a-b` to U+2212. (#1, #3)

### Thanks

Tom Clark (IronLAB) reported all eight issues with reproductions and proposed
fixes, and contributed the pull requests for #4, #7 and #11 and the first depth
limit for #6.

## [1.0.4] — 2026-09-20

Clippy debt clear (`-D warnings`): `RowKind::Intertext` boxed to shrink enum size.

## [1.0.3] — 2026-09-19

Coordinated patch with zenith-float, hdf5-rust, and redb-view (pure-Rust FOSS family adjacent to Accumath; Accumath itself stays proprietary).

## [1.0.2] — 2026-09-04

Standalone layout math. This crate does not depend on a numeric library.

- `Dim` is an exact rational (`num/den`). No hardware `f32`/`f64` in layout.
- SHA-256 of the embedded face is in-tree.
- Removed the numeric-library dependency from `Cargo.toml`.

## [1.0.0] — 2026-09-02

### Added

- Complete LaTeX math parser (`parse`, gold-stable `MathNode`)
- TeX-faithful layout engine (Appendix G style, Table 18 spacing, `Dim`)
- SVG renderer (`render_svg` / `latex_to_svg`) — self-contained SVG 1.1, no font embedding
- PNG renderer (feature `png`) via `tiny-skia` 0.11, DPI-aware, transparent background by default
- egui renderer (feature `egui`) — TrueType tessellation to `egui::Shape` meshes, no SVG intermediate
- Math-mode symbol catalog (Greek, AMS, arrows, operators, font styles) locked to `data/symbols.tsv`
- Full accent and decoration support (TeX placement, extensible hats/arrows/braces, cancel, boxed)
- Multiline environments — `align`, `aligned`, `split`, `gather`, `multline`, `equation`, `{array}`, `{cases}`
- Color support — named, rgb, RGB, HTML, cmyk, gray, `\definecolor`, group scope, `\fcolorbox` borders
- Exact-rational layout math (no hardware `f32`/`f64` in layout)

### Milestone notes

Milestones 1–10 landed as 0.1.0 development commits. This release packages that
surface as 1.0.0: rustdoc, README benchmarks, clippy/fmt, and crates.io metadata.
The crate is publish-ready; crates.io upload is a separate step.

### [1.0.1] - 2026-09-02

### Version to 1.0.1
### Fixed

Resolved an issue where the docs.rs documentation build was failing due to deprecated Rust nightly features (doc_auto_cfg).
Removed:  two temporary build documents Prompt and color addition
Corrected email address of author from jscarr@gmail.com to jscarr1964@gmail.com

### Architecture

Crate modules: `parser/`, `layout/`, `font/`, `render/svg`, `render/png`,
`render/egui`, plus `golds/` and `benches/` in the repository.
