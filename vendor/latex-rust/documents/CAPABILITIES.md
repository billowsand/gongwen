# latex-rust — Capabilities
**Version:** 1.0.2  
**Date:** 2026-09-04  
**License:** MIT OR Apache-2.0  
**Status:** Public crate 1.0.2 (SVG / PNG / egui; golds green; in-tree `Dim`)  

> Pure Rust LaTeX math renderer. No JavaScript. No webview. No runtime dependencies.  
> Surpasses MathJax and KaTeX in correctness, performance, and platform coverage.

---

## 1. Mission Statement

MathJax and KaTeX are JavaScript libraries. Every non-web application that needs LaTeX rendering must either bundle a JS runtime, shell out to a subprocess, or accept degraded output. latex-rust eliminates that dependency entirely.

latex-rust is a pure Rust crate that:
- Parses LaTeX math expressions into a typed AST
- Lays out that AST using a complete TeX-faithful box model
- Renders to SVG, PNG, or directly to egui primitives
- Runs identically on Windows, macOS, Linux, Android, and iOS
- Requires zero runtime dependencies beyond the Rust standard library

---

## 2. Where latex-rust Beats MathJax and KaTeX

| Capability | MathJax | KaTeX | latex-rust |
|---|---|---|---|
| Language | JavaScript | JavaScript | Pure Rust |
| Runtime dependency | Node / browser | Node / browser | None |
| Air-gap compatible | No | No | Yes |
| Exact-rational layout metrics | No | No | Yes (`Dim`) |
| Native desktop rendering | No | No | Yes |
| egui integration | No | No | Yes (native) |
| Android / iOS native | No | No | Yes |
| Startup time | Slow (JS parse) | Faster but JS | Instant |
| Memory footprint | Heavy | Moderate | Minimal |
| Honest error on unsupported input | Partial | Partial | Always |
| Hardware float in layout math | Yes | Yes | No (`Dim`) |
| Font metric precision | Limited | Limited | Full TrueType |

---

## 3. Architecture

```
latex-rust
├── src/parser/      LaTeX → AST
├── src/layout/      AST → Box model (TeX-faithful)
├── src/font/        Font metric tables, TrueType loader
├── src/render/
│   ├── svg/         Box model → SVG
│   ├── png/         Box model → PNG (via tiny-skia, feature `png`)
│   └── egui/        Box model → egui primitives (feature `egui`)
├── golds/           Gold test vectors
└── benches/         Layout and render benchmarks
```

These are public crate modules (`latex_rust::parser`, `layout`, `font`, `render`).
The same names are re-exported at the crate root. Embedded font *files* stay in
`fonts/` (STIX Two Math OFL); `src/font/` is the loader.

### 3.1 Parser

Input: LaTeX math string  
Output: `MathNode` AST  
Errors: `ParseError` — never a partial parse silently accepted

**Supported input forms:**
- Inline math: `$...$`
- Display math: `$$...$$` or `\[...\]`
- Raw expression (no delimiters, for programmatic use)

### 3.2 AST Nodes

Every node is a variant of `MathNode`. No implicit nodes. No surprises.

```
MathNode
├── Atom(char, AtomKind)           — single character/symbol
├── Fraction(num, den)             — \frac{}{} 
├── Radical(degree, radicand)      — \sqrt[n]{}
├── Superscript(base, exp)         — x^{}
├── Subscript(base, sub)           — x_{}
├── SubSup(base, sub, exp)         — x_{}^{}
├── Delimited(open, body, close)   — \left( \right)
├── Row(Vec<MathNode>)             — horizontal sequence
├── Matrix(style, colspec, rows)   — pmatrix, array, align, gather, …
├── Substack(Vec<MathNode>)        — \substack
├── Ref(key)                       — \ref
├── Tag / Label / NoNumber         — peeled into EnvRow inside environments
├── Hline / Intertext              — array / align rows
├── Sum(limits)                    — \sum
├── Integral(limits)               — \int
├── Product(limits)                — \prod
├── Limit(sub)                     — \lim
├── OverUnder(base, over, under)   — \overset \underset
├── Accent(base, accent)           — \hat \bar \vec \dot \ddot
├── Color(spec, body)              — \color after this point in a group
├── TextColor(spec, body)          — \textcolor{}{}
├── ColorBox(fill, body)           — \colorbox{}{}
├── FColorBox(border, fill, body)  — \fcolorbox{}{}{}
├── Text(string, style)            — \text{} \mathrm{} \mathbf{}
├── Space(SpaceKind)               — \, \; \quad \qquad
├── Operator(name, limits)         — \sin \cos \log named ops
├── Symbol(SymbolKind)             — Greek letters, arrows, relations
├── Strut(height, depth)           — vertical spacing
└── Phantom(MathNode)             — \phantom{}
```

### 3.3 Layout Engine (Box Model)

TeX-faithful box model. Every node produces a `Box`:

```rust
pub struct MathBox {
    pub width:   Dim,   // exact rational
    pub height:  Dim,   // above baseline
    pub depth:   Dim,   // below baseline
    pub italic:  Dim,   // italic correction
    pub content: BoxContent,
}
```

Layout rules follow Appendix G of The TeXbook exactly:
- Style propagation (display, text, script, scriptscript)
- Cramped style rules
- Mu (math unit) spacing table
- Delimiter sizing algorithm
- Radical vinculum height
- Fraction bar thickness
- Accent placement
- Operator limits placement

No approximations. No shortcuts. If TeX would render it one way latex-rust renders it that way.

### 3.4 Font System

TeX math requires specific font metrics. latex-rust ships with:
- **STIX Two Math** — full Unicode math coverage, OFL license, embeddable
- **Latin Modern Math** — TeX-native, OFL license, embeddable
- Font metric tables compiled to Rust at build time
- TrueType glyph outlines extracted at render time

Font metrics are stored as exact-rational `Dim` values — no hardware float in the layout path.

### 3.5 Renderers

**SVG renderer:**
- Self-contained SVG output
- Glyphs as `<path>` elements — no font embedding required
- Scales perfectly at any size
- Primary output format

**PNG renderer (`src/render/png`, feature `png`):**
- `tiny-skia` 0.11 rasterization; DPI-aware (`pixels = points × dpi / 72`)
- Without `features = ["png"]` every entry point is `Err(Unsupported)`
- Gold hashes lock PNG bytes at 72 / 144 / 300 DPI (`golds/png.toml`)
- On x86_64 with AVX2 — tiny-skia runs at peak performance automatically
- On Apple Silicon — tiny-skia uses the ARM NEON path, also fast
- No user configuration needed
- No compile flags required
- Works correctly on every supported architecture

**egui renderer (`src/render/egui`, feature `egui`):**
- TrueType outlines tessellated to `egui::Mesh`; rules and color boxes are `Shape::Rect`
- No SVG intermediate. Without `features = ["egui"]` every entry point is `Err(Unsupported)`
- Golds lock shape counts, mesh vertex/index counts, and bounding rects (`golds/egui.toml`)

---

## 4. Capability Inventory

### 4.1 Core Math Structures

| Capability | Status | Notes |
|---|---|---|
| Fractions `\frac` | ✅ | Display and text style (`golds/layout.toml`) |
| Binomial `\binom` | ✅ | Parsed as delimited fraction (`golds/parse.toml`) |
| Radicals `\sqrt` `\sqrt[n]` | ✅ | Vinculum + optional degree |
| Superscript / subscript | ✅ | Stacking, cramped style |
| Sub+superscript simultaneous | ✅ | `x_a^b` |
| Limits on operators | ✅ | Display vs inline position |
| Delimiters `\left \right` | ✅ | Sized to content; extensible variants |
| Fixed-size delimiters | ✅ | `\big \Big \bigg \Bigg` |
| Matrices | ✅ | pmatrix (and siblings) |
| Cases `\begin{cases}` | ✅ | Left brace; quad between columns (`golds/envs.toml`) |
| Arrays `\begin{array}` | ✅ | `l` `c` `r` `\|`, `\hline` (`golds/envs.toml`) |
| Aligned equations | ✅ | `\begin{aligned}` / `split` RL columns |

### 4.2 Symbols

| Capability | Status | Notes |
|---|---|---|
| Greek lowercase α β γ … | ✅ | Full set + variants; STIX glyphs |
| Greek uppercase Α Β Γ … | ✅ | `\Gamma` … `\Omega` |
| Hebrew ℵ ℶ ℷ ℸ | ✅ | |
| Binary operators + − × ÷ ± ∓ … | ✅ | Full TeX set; Bin atom class |
| Relations = < > ≤ ≥ ≠ ≈ ∼ … | ✅ | Full TeX set; Rel atom class |
| Arrows → ← ↔ ⇒ ⇔ … | ✅ | Short/long/harpoons; `\xrightarrow` stretches |
| Large operators ∑ ∏ ∫ ∮ ∯ … | ✅ | Display/text sizing |
| Dots ⋯ ⋮ ⋱ … | ✅ | `\ldots` `\cdots` `\vdots` `\ddots` `\iddots` |
| Set symbols ∈ ∉ ⊂ ⊃ ∪ ∩ … | ✅ | |
| Logic symbols ∀ ∃ ∧ ∨ ¬ … | ✅ | |
| Blackboard bold ℝ ℂ ℤ ℚ ℕ … | ✅ | `\mathbb` Unicode math alphabets |
| Calligraphic 𝒜 ℬ 𝒞 … | ✅ | `\mathcal` / `\mathscr` |
| Fraktur 𝔄 𝔅 ℭ … | ✅ | `\mathfrak` upper and lower |
| AMS symbols | ✅ | amssymb operators, relations, arrows, misc |

### 4.3 Accents and Decorations

| Capability | Status | Notes |
|---|---|---|
| `\hat \check \breve \acute \grave` | ✅ | TeX placement, MATH attachment |
| `\tilde \bar \vec \dot \ddot` | ✅ | `\vec` right-aligned |
| `\dddot \ddddot \mathring` | ✅ | Combining triple/quad dots from STIX |
| `\widehat \widetilde` | ✅ | Horizontal variants then assembly |
| `\overline \underline` | ✅ | Rule at exact base width |
| `\overbrace \underbrace` | ✅ | STIX brace glyphs + assembly; `^`/`_` as limits |
| `\overleftarrow \overrightarrow` | ✅ | Extensible; under-arrows too |
| `\overleftrightarrow` and under form | ✅ | Extensible |
| `\xrightarrow` / `\xleftarrow` | ✅ | Optional `[under]{over}` labels |
| `\overset \underset` | ✅ | Script-style, centered |
| `\stackrel` | ✅ | Alias for `\overset` |
| `\cancel \bcancel \xcancel \cancelto` | ✅ | SVG `<line>` diagonals |
| `\boxed` / `\fbox` | ✅ | Stroked frame, 3mu pad |

### 4.4 Text and Font Styles

| Capability | Status | Notes |
|---|---|---|
| `\mathrm \mathbf \mathit` | ✅ | Unicode math alphabets through STIX |
| `\mathsf \mathtt \mathbb` | ✅ | Full Latin; digits where Unicode defines them |
| `\mathcal \mathfrak \mathscr` | ✅ | Script exceptions (ℒ, ℋ, …); fraktur upper and lower |
| `\text{}` | ✅ | Text mode inside math |
| `\boldsymbol \pmb` | ✅ | Bold italic Greek/Latin; `\pmb` is a true overlay |
| `\operatorname{}` | ✅ | Named operators in `\mathrm` |

### 4.5 Spacing

| Capability | Status | Notes |
|---|---|---|
| Automatic inter-atom spacing | ✅ | TeXbook Table 18 (`golds/layout.toml` `ord-bin-text`, `ord-rel-text`, script drop) |
| `\,` thin space | ✅ | Parse + layout dims (`golds/parse.toml`, `golds/layout.toml`) |
| `\:` medium space | ✅ | `golds/parse.toml` `space_med`; 4 mu in layout |
| `\;` thick space | ✅ | `golds/parse.toml` `space_thick`; 5 mu in layout |
| `\!` negative thin space | ✅ | `golds/parse.toml` `space_neg`; −3 mu in layout |
| `\quad \qquad` | ✅ | 1 em / 2 em (`golds/layout.toml` `quad`, `golds/parse.toml`) |
| `\hspace{}` | ✅ | Em units (`golds/parse.toml` `hspace`) |
| `\phantom \vphantom \hphantom` | ✅ | Empty / height-only / width-only (`golds/parse.toml`) |

### 4.6 Named Operators

| Capability | Status | Notes |
|---|---|---|
| `\sin \cos \tan \cot \sec \csc` | ✅ | `\mathrm` operators (`golds/parse.toml` `op_*`) |
| `\arcsin \arccos \arctan` | ✅ | `golds/parse.toml` |
| `\sinh \cosh \tanh \coth` | ✅ | `golds/parse.toml` |
| `\log \ln \lg \exp` | ✅ | `golds/parse.toml` |
| `\lim \limsup \liminf` | ✅ | `\lim` is `MathNode::Limit`; others named ops (`golds/parse.toml`) |
| `\sup \inf \max \min` | ✅ | `golds/parse.toml` |
| `\det \dim \ker \deg` | ✅ | `golds/parse.toml` |
| `\gcd \lcm` | ✅ | `golds/parse.toml` |
| `\Pr \arg` | ✅ | `golds/parse.toml` |

### 4.7 Multiline and Display

| Capability | Status | Notes |
|---|---|---|
| Display math centering | ✅ | Display style; `gather` centers each row (`golds/envs.toml`). Snippet output is tight-boxed, not page-centered. |
| Equation numbering | ✅ | `\tag{}` / `\tag*{}`; `NumberingConfig` in `layout/` (`golds/envs.toml`) |
| `\begin{equation}` | ✅ | Display + number (or `\nonumber`) |
| `\begin{align}` | ✅ | RL alignment; per-row numbers |
| `\begin{gather}` | ✅ | Centered rows; per-row numbers |
| `\begin{multline}` | ✅ | First left, last right, middle center |
| `\begin{split}` | ✅ | Same alignment as `aligned`; number from enclosing `equation` |
| `\substack` | ✅ | Script-style centered stack |
| `\intertext` | ✅ | Full-width text row in `align` |
| `\nonumber \notag` | ✅ | |
| `\label \ref` | ✅ | Two-pass on one tree; state survives `layout_with_numbering` |

### 4.8 Color

Color is in scope for v1.0 (Milestone 8). Channel math uses `Dim` — never hardware `f32`/`f64`. An unsupported model or unknown name is `Err(Unsupported)`, never a silent default color.

| Capability | Status | Notes |
|---|---|---|
| `\color{name}` | ✅ | Named color — affects all following content in scope |
| `\textcolor{name}{expr}` | ✅ | Color applied to expression only |
| `\colorbox{name}{expr}` | ✅ | Background color box around expression |
| `\fcolorbox{border}{bg}{expr}` | ✅ | Framed color box (fill + border stroke) |
| `\definecolor{name}{model}{spec}` | ✅ | Custom color definition; forward refs are `Err` |
| Named colors (standard LaTeX set) | ✅ | black white red green blue cyan magenta yellow + dvipsnames set |
| RGB color model `{rgb}{r,g,b}` | ✅ | Values 0.0–1.0 via `Dim` |
| RGB integer model `{RGB}{R,G,B}` | ✅ | Values 0–255 via `Dim` |
| HTML hex color model `{HTML}{RRGGBB}` | ✅ | 6-digit hex |
| CMYK color model `{cmyk}{c,m,y,k}` | ✅ | Values 0.0–1.0 via `Dim`; naive `(1-c)(1-k)` to sRGB |
| Gray model `{gray}{g}` | ✅ | Value 0.0–1.0 via `Dim` |
| Color inheritance / scope | ✅ | Color scoped to current group `{}` |
| SVG fill / stroke emission | ✅ | `fill` and `stroke` on `<g>`; boxes as `<rect>` |
| PNG color pass-through | ✅ | tiny-skia `Paint` from `Color::to_rgba8` (`golds/png.toml`) |
| egui color pass-through | ✅ | `Color32` on meshes and rects (`golds/egui.toml`) |
| Unsupported color model | ✅ | Returns `Err(Unsupported)` — never a fake color |

---

## 5. What latex-rust Does Not Do

These are explicitly out of scope. They return `Err(Unsupported)` — never a fake render.

- Text mode LaTeX (document structure, `\section`, `\begin{document}`)
- TikZ / PGF graphics
- Chemistry notation (`\ce{}`) — separate crate
- BibTeX / bibliography
- PDF output (separate crate)
- Font substitution / fallback synthesis
- Right-to-left math

Unsupported color *models* (for example `spot`) and unknown color names return `Err(Unsupported)`. They are not out of scope as a feature — they are honest failures inside the color system (see §4.8).

---

## 6. Gold Test Standard

Every capability entry in §4 that is marked ✅ must have a corresponding gold test.

A gold test for latex-rust specifies:
- Input LaTeX string
- Expected SVG output hash OR pixel-level comparison
- Expected box dimensions (width, height, depth) at reference size
- Expected parse success or named error

Gold format:
```toml
[[gold]]
name = "frac_simple"
input = "\\frac{1}{2}"
kind = "svg"
box_width_mu  = "10.5"
box_height_mu = "12.3"
box_depth_mu  = "4.1"
svg_hash = "sha256:abc123..."
```

Color golds (hex is the contract until SVG lands; SVG golds then lock `fill`/`stroke`):

```toml
[[gold]]
name = "textcolor_red_x"
input = "\\textcolor{red}{x}"
kind = "svg"
notes = "x glyph path has fill:#ff0000"

[[gold]]
name = "color_scope"
input = "{\\color{blue} x + y} + z"
kind = "svg"
notes = "x and y blue, z default color"

[[gold]]
name = "definecolor_custom"
input = "\\definecolor{mygreen}{rgb}{0.0,0.6,0.0}\\textcolor{mygreen}{f(x)}"
kind = "svg"
notes = "custom rgb color applied correctly"

[[gold]]
name = "colorbox_expression"
input = "\\colorbox{yellow}{\\frac{1}{2}}"
kind = "svg"
notes = "yellow background box around fraction"

[[gold]]
name = "unsupported_color_model"
input = "\\definecolor{x}{spot}{0.5}"
kind = "error"
expect = "Unsupported"
notes = "spot color model not supported — never a fake color"
```

Rules:
- Gold is the contract. Code must pass gold. Gold is never changed to pass code.
- A render that differs by one pixel from gold is a failure.
- A parse that returns a different error than gold is a failure.
- No capability is marked ✅ without a passing gold.

---

## 7. Shipped surface

Each milestone ends with a full gold suite pass before the next begins.

### Milestone 1 — Core Infrastructure
- [x] Crate scaffolding (`latex-rust`, dual license, GitHub)
- [x] `Dim` type (exact rationals) for layout math
- [x] Font metric loader (STIX Two Math)
- [x] `MathBox` type and basic composition
- [x] Parser skeleton — tokenizer only
- [x] Gold runner infrastructure
- [x] Math symbol catalog (`data/symbols.tsv`)
- [x] Color model parser + named/dvipsnames table (spec in §4.8; SVG in Milestone 8)

### Milestone 2 — Parser Complete
- [x] Full tokenizer
- [x] `MathNode` AST complete
- [x] Parser produces correct AST for all §4 node types
- [x] `ParseError` for all unsupported input
- [x] Gold tests for every node type

### Milestone 3 — Layout Engine
- [x] Style machine (display / text / script / scriptscript / cramped)
- [x] Atom spacing table (TeX Appendix G)
- [x] Fraction layout
- [x] Radical layout (vinculum)
- [x] Sub/superscript layout
- [x] Delimiter sizing
- [x] Operator limits placement
- [x] Matrix layout
- [x] Gold tests for layout dimensions on every node type

### Milestone 4 — SVG Renderer
- [x] Glyph path extraction from TrueType
- [x] Box → SVG transform
- [x] Self-contained SVG output
- [x] Extensible delimiter rendering
- [x] Gold tests for SVG output on representative expressions

### Milestone 5 — Symbol Coverage
- [x] Full Greek alphabet
- [x] Full AMS symbol set
- [x] Blackboard bold, calligraphic, fraktur
- [x] All named operators
- [x] Gold tests for every symbol category

### Milestone 6 — Accents and Decorations
- [x] All single-char accents
- [x] Extensible accents
- [x] Overbrace / underbrace
- [x] Overline / underline
- [x] Gold tests for all accent types

### Milestone 7 — Advanced Structures
- [x] Multiline environments (align, gather, multline)
- [x] Equation numbering
- [x] Cases environment
- [x] Gold tests for all environments

### Milestone 8 — Color Support
- [x] Color model parser — named, rgb, RGB 0–255, HTML hex, cmyk, gray (`parse_color_spec`, golds)
- [x] Standard named color table — 8 LaTeX names + 68 dvipsnames (`data/dvipsnames.tsv`)
- [x] `\definecolor` custom color registration (`ColorTable::define`)
- [x] Color AST nodes — `Color`, `TextColor`, `ColorBox`, `FColorBox` (`golds/parse.toml`)
- [x] Color scope propagation through layout engine
- [x] SVG renderer — `fill` and `stroke` attribute emission on colored glyphs
- [x] PNG renderer — tiny-skia color pass-through (Milestone 9; `Color::to_rgba8`)
- [x] egui renderer — `Color32` emission (Milestone 10; `Color::to_rgba8`)
- [x] Gold tests for every color capability (hex / tokenize / parse AST / layout color boxes / SVG `fill`/`stroke` / PNG / egui locked)
- [x] Full corpus pass before Milestone 9

### Milestone 9 — PNG Renderer (feature = "png")
- [x] tiny-skia integration
- [x] DPI-aware rasterization
- [x] Gold tests for pixel output

### Milestone 10 — egui Renderer (feature = "egui")
- [x] egui Shape emission
- [x] Zero SVG intermediate
- [x] Integration test with egui test harness
- [x] Gold tests for shape output

### Milestone 11 — Polish and Release
- [x] Full documentation (rustdoc on every public item)
- [x] README with examples
- [x] CHANGELOG
- [x] Benchmarks vs KaTeX reference renders
- [x] crates.io

---

## 8. Dependencies

| Crate | Purpose | Optional |
|---|---|---|
| `ttf-parser` | TrueType glyph extraction | No |
| `tiny-skia` | PNG rasterization | Yes (feature = "png") |
| `egui` | egui primitive emission | Yes (feature = "egui") |

Color uses these crates only: CSS `fill`/`stroke` strings in SVG, `tiny-skia` color when the `png` feature is on, `egui::Color32` when the `egui` feature is on. No extra dependency.

No other dependencies. No JavaScript. No webview. No runtime.

---

## 9. Performance Targets

| Operation | Target |
|---|---|
| Parse `\frac{a+b}{c+d}` | < 50µs (measured ~2µs for `\frac{1}{2}`) |
| Layout full display equation | < 500µs (measured ~47µs) |
| SVG render full display equation | < 1ms (measured ~951µs) |
| PNG render at 144dpi | < 5ms (measured ~142µs) |
| egui inline cache miss | < 2ms (measured ~216µs) |

KaTeX cold start in a browser is typically 200-400ms. latex-rust target is < 10ms including font loading.

---

## 10. Correctness Standard

- No hardware float (`f32` / `f64`) in any layout calculation
- All dimension arithmetic uses in-tree exact-rational `Dim`
- Font metrics stored and computed in `Dim`
- TeX Appendix G spacing table implemented exactly
- Unsupported input always returns `Err(Unsupported)` — never a fake render
- Color channels are `Dim` (or integer hex). Unsupported color models never synthesize a color
- A wrong render is worse than `Err(Unsupported)`

---

## 11. Licensing

latex-rust is MIT OR Apache-2.0. It is a standalone crate.

The font data (STIX Two Math, Latin Modern Math) is OFL licensed and may be embedded in the compiled binary.

---

## 12. Contact and Development

latex-rust is developed as a standalone open source contribution to the Rust ecosystem.

Issues, PRs, and capability requests tracked via the repository issue tracker.

No alpha or beta period. Ship when the gold suite is complete and green.

---

*This document is the engineering source of truth. A capability not listed here is not in the crate. A capability listed here without a gold is not complete.*
