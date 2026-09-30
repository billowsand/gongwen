//! `MathNode` → `MathBox`. All sizes are [`Dim`](crate::Dim).

use core::cell::Cell;
use core::cmp::Ordering;

use crate::atoms::symbol_atom_kind;
use crate::color::Color;
use crate::dim::Dim;
use crate::error::Error;
use crate::font::MathFont;
use crate::layout::metrics::MathParams;
use crate::layout::numbering::NumberingState;
use crate::layout::space::{atom_space_mu, convert_bin, space_width};
use crate::layout::style::MathStyle;
use crate::layout::{BoxContent, MathBox};
use crate::parser::DEFAULT_MAX_NESTING_DEPTH;
use crate::parser::{
    AccentKind, AtomKind, ColSpec, DelimSize, Delimiter, EnvRow, IntegralKind, MathNode,
    MatrixStyle, PhantomKind, SpaceKind, TextStyle,
};
use crate::style_map::styled_char;
use crate::symbols::lookup;

/// Lay out `node` in `style` using STIX Two Math metrics.
///
/// Every dimension on the returned [`MathBox`] is a [`Dim`](crate::Dim). Missing
/// glyphs and unsupported constructs are errors — never a substitute glyph.
///
/// # Arguments
///
/// * `node` — parsed math tree.
/// * `font` — face providing MATH constants and glyph metrics.
/// * `style` — TeX math style (`Display`, `Text`, scripts).
///
/// # Returns
///
/// A box tree ready for SVG, PNG, or egui emission.
///
/// # Errors
///
/// * [`Error::Font`] — glyph missing from the face.
/// * [`Error::Unsupported`] — construct or MATH table the engine will not fake.
/// * [`Error::Malformed`] — invalid structure discovered during layout.
///
/// # Examples
///
/// ```
/// use latex_rust::{layout, parse, MathFont, MathStyle};
///
/// let ast = parse(r"\frac{1}{2}").unwrap();
/// let font = MathFont::stix_two_math().unwrap();
/// let boxed = layout(&ast, &font, MathStyle::Text).unwrap();
/// assert!(!boxed.width.is_zero());
/// ```
pub fn layout(node: &MathNode, font: &MathFont, style: MathStyle) -> Result<MathBox, Error> {
    let mut state = NumberingState::default();
    layout_with_numbering(node, font, style, &mut state)
}

/// Lay out with a caller-owned equation counter and `\label` / `\ref` table.
///
/// # Arguments
///
/// * `node` — parsed math tree.
/// * `font` — face providing MATH constants and glyph metrics.
/// * `style` — TeX math style.
/// * `state` — counter and label map; survives across calls.
///
/// # Returns
///
/// A box tree. Numbers assigned for this tree are recorded in `state`.
///
/// # Errors
///
/// Same as [`layout`].
///
/// # Examples
///
/// ```
/// use latex_rust::{layout_with_numbering, parse, MathFont, MathStyle, NumberingState};
///
/// let ast = parse(r"\begin{equation}x\end{equation}").unwrap();
/// let font = MathFont::stix_two_math().unwrap();
/// let mut state = NumberingState::default();
/// let boxed = layout_with_numbering(&ast, &font, MathStyle::Display, &mut state).unwrap();
/// assert!(!boxed.width.is_zero());
/// ```
pub fn layout_with_numbering(
    node: &MathNode,
    font: &MathFont,
    style: MathStyle,
    state: &mut NumberingState,
) -> Result<MathBox, Error> {
    layout_impl(node, font, style, state, DEFAULT_MAX_NESTING_DEPTH)
}

/// Lay out with an explicit nesting limit instead of
/// [`DEFAULT_MAX_NESTING_DEPTH`](crate::DEFAULT_MAX_NESTING_DEPTH).
///
/// Use the same limit given to [`ParseOptions::with_max_depth`](crate::ParseOptions::with_max_depth),
/// so that every tree the parser accepts can also be laid out.
///
/// # Errors
///
/// Same as [`layout`]. A tree nesting deeper than `max_depth` returns
/// [`Error::Unsupported`].
///
/// # Examples
///
/// ```
/// use latex_rust::{layout_with_max_depth, parse, MathFont, MathStyle};
///
/// let ast = parse(r"\frac{1}{2}").unwrap();
/// let font = MathFont::stix_two_math().unwrap();
/// assert!(layout_with_max_depth(&ast, &font, MathStyle::Text, 64).is_ok());
/// assert!(layout_with_max_depth(&ast, &font, MathStyle::Text, 1).is_err());
/// ```
pub fn layout_with_max_depth(
    node: &MathNode,
    font: &MathFont,
    style: MathStyle,
    max_depth: usize,
) -> Result<MathBox, Error> {
    let mut state = NumberingState::default();
    layout_impl(node, font, style, &mut state, max_depth)
}

fn layout_impl(
    node: &MathNode,
    font: &MathFont,
    style: MathStyle,
    state: &mut NumberingState,
    max_depth: usize,
) -> Result<MathBox, Error> {
    let params = MathParams::from_font(font)?;
    let start = state.collect(node);
    Engine {
        font,
        params,
        numbers: state,
        idx: Cell::new(start),
        depth: Cell::new(0),
        max_depth,
    }
    .layout(node, style)
}

struct Engine<'a> {
    font: &'a MathFont,
    params: MathParams,
    numbers: &'a NumberingState,
    idx: Cell<usize>,
    /// Current nesting depth, bounded by `max_depth`.
    depth: Cell<usize>,
    /// Deepest nesting accepted.
    max_depth: usize,
}

struct Item {
    bx: MathBox,
    class: Option<AtomKind>,
}

impl Engine<'_> {
    fn layout(&self, node: &MathNode, style: MathStyle) -> Result<MathBox, Error> {
        Ok(self.item(node, style)?.bx)
    }

    /// Lay out `node` one level deeper, refusing to descend past `max_depth`.
    ///
    /// `parse` bounds the depth of any tree it builds, so this catches only a
    /// tree assembled by hand. It exists because `layout` is public and must
    /// not overflow the caller's stack whatever it is handed.
    fn item(&self, node: &MathNode, style: MathStyle) -> Result<Item, Error> {
        let depth = self.depth.get();
        if depth >= self.max_depth {
            return Err(Error::Unsupported {
                what: format!("tree nests deeper than {} levels", self.max_depth),
            });
        }
        self.depth.set(depth + 1);
        let out = self.item_inner(node, style);
        self.depth.set(depth);
        out
    }

    fn item_inner(&self, node: &MathNode, style: MathStyle) -> Result<Item, Error> {
        match node {
            MathNode::Atom(c, k) => {
                let bx = self.glyph(math_italic(math_char(*c)), style)?;
                Ok(Item {
                    bx,
                    class: Some(*k),
                })
            }
            MathNode::Symbol(name) => {
                let ch = math_italic(symbol_char(name)?);
                let bx = self.glyph(ch, style)?;
                Ok(Item {
                    bx,
                    class: Some(symbol_class(name)),
                })
            }
            MathNode::Row(items) => self.row(items, style),
            MathNode::Fraction(num, den) => self.fraction(num, den, style),
            MathNode::Radical(deg, rad) => self.radical(deg.as_deref(), rad, style),
            MathNode::Superscript(base, exp) => self.scripts(base, None, Some(exp), style),
            MathNode::Subscript(base, sub) => self.scripts(base, Some(sub), None, style),
            MathNode::SubSup(base, sub, exp) => self.scripts(base, Some(sub), Some(exp), style),
            MathNode::Delimited(open, body, close) => self.delimited(open, body, close, style),
            MathNode::SizedDelim(d, size, k) => {
                let needed = self.explicit_delim_span(*size, style);
                Ok(Item {
                    class: Some(*k),
                    bx: self.delim_box(d, &needed, style)?,
                })
            }
            MathNode::Space(kind) => Ok(Item {
                bx: MathBox::kern(self.space_dim(kind, style)),
                class: None,
            }),
            MathNode::Strut(h, d) => {
                let s = self.params.scale(style);
                Ok(Item {
                    bx: MathBox {
                        width: Dim::zero(),
                        height: h * &s,
                        depth: d * &s,
                        italic: Dim::zero(),
                        shift: Dim::zero(),
                        content: BoxContent::Empty,
                    },
                    class: Some(AtomKind::Ord),
                })
            }
            MathNode::Phantom(kind, inner) => {
                let mut bx = self.layout(inner, style)?;
                match kind {
                    PhantomKind::Full => bx.content = BoxContent::Empty,
                    PhantomKind::Vertical => {
                        bx.width = Dim::zero();
                        bx.content = BoxContent::Empty;
                    }
                    PhantomKind::Horizontal => {
                        bx.height = Dim::zero();
                        bx.depth = Dim::zero();
                        bx.content = BoxContent::Empty;
                    }
                }
                Ok(Item {
                    class: class_of(inner),
                    bx,
                })
            }
            MathNode::Text(s, ts) => self.text_run(s, *ts, style),
            MathNode::Operator(name, limits) => self.operator(name, *limits, style),
            MathNode::Sum(lo, hi) => self.large_op('∑', lo.as_deref(), hi.as_deref(), style, true),
            MathNode::Product(lo, hi) => {
                self.large_op('∏', lo.as_deref(), hi.as_deref(), style, true)
            }
            MathNode::Integral(k, lo, hi) => {
                let ch = match k {
                    IntegralKind::Int => '∫',
                    IntegralKind::Iint => '∬',
                    IntegralKind::Iiint => '∭',
                    IntegralKind::Oint => '∮',
                    IntegralKind::Oiint => '∯',
                };
                self.large_op(ch, lo.as_deref(), hi.as_deref(), style, false)
            }
            MathNode::Limit(sub) => {
                let op = self.text_run("lim", TextStyle::Rm, style)?;
                if let Some(s) = sub {
                    self.attach_limits(op.bx, None, Some(s), style, true)
                } else {
                    Ok(Item {
                        bx: op.bx,
                        class: Some(AtomKind::Op),
                    })
                }
            }
            MathNode::OverUnder(base, over, under) => {
                let mut b = self.layout(base, style)?;
                let mut needed = b.width.clone();
                if let Some(o) = over {
                    needed = needed.max(&self.layout(o, style.into_script())?.width);
                }
                if let Some(u) = under {
                    needed = needed.max(&self.layout(u, style.into_script())?.width);
                }
                b = self.stretch_h(b, &needed, style)?;
                self.attach_limits(b, over.as_deref(), under.as_deref(), style, true)
            }
            MathNode::Accent(base, kind) => self.accent(base, *kind, style),
            MathNode::CancelTo(value, expr) => self.cancelto(value, expr, style),
            MathNode::Matrix(ms, spec, rows) => self.matrix(*ms, spec, rows, style),
            MathNode::Substack(lines) => self.substack(lines, style),
            MathNode::Ref(key) => self.reference(key, style),
            MathNode::Tag { star, body } => self.tag_box(*star, body, style),
            MathNode::Label(_) | MathNode::NoNumber => Ok(Item {
                bx: MathBox::empty(),
                class: None,
            }),
            MathNode::Hline => Err(Error::Unsupported {
                what: "hline outside array".into(),
            }),
            MathNode::Intertext(n) => Ok(Item {
                class: Some(AtomKind::Ord),
                bx: self.layout(n, MathStyle::Text)?,
            }),
            MathNode::Color(c, body) | MathNode::TextColor(c, body) => {
                let inner = self.layout(body, style)?;
                Ok(Item {
                    class: class_of(body),
                    bx: color_wrap(*c, inner),
                })
            }
            MathNode::ColorBox(c, body) => {
                let inner = self.layout(body, style)?;
                let pad = self.params.mu(style) * Dim::from_i64(3);
                Ok(Item {
                    class: Some(AtomKind::Inner),
                    bx: back_color_wrap(*c, pad_box(inner, &pad)),
                })
            }
            MathNode::FColorBox(border, fill, body) => {
                let inner = self.layout(body, style)?;
                let pad = self.params.mu(style) * Dim::from_i64(3);
                let thick = self.params.fraction_rule_thickness.clone() * self.params.scale(style);
                Ok(Item {
                    class: Some(AtomKind::Inner),
                    bx: back_color_wrap(
                        *fill,
                        frame_wrap(thick, Some(*border), pad_box(inner, &pad)),
                    ),
                })
            }
        }
    }

    fn glyph(&self, ch: char, style: MathStyle) -> Result<MathBox, Error> {
        let g = self.font.glyph(ch)?;
        let s = self.params.scale(style);
        let italic = self.font.italic_correction(g.glyph_id);
        Ok(MathBox {
            width: &g.advance * &s,
            height: &g.height * &s,
            depth: &g.depth * &s,
            italic: &italic * &s,
            shift: Dim::zero(),
            content: BoxContent::glyph(ch, g.glyph_id, s),
        })
    }

    fn glyph_id(&self, ch: char, gid: u16, style: MathStyle) -> Result<MathBox, Error> {
        let g = self.font.glyph_id(ch, gid)?;
        let s = self.params.scale(style);
        let italic = self.font.italic_correction(gid);
        Ok(MathBox {
            width: &g.advance * &s,
            height: &g.height * &s,
            depth: &g.depth * &s,
            italic: &italic * &s,
            shift: Dim::zero(),
            content: BoxContent::glyph(ch, gid, s),
        })
    }

    fn space_dim(&self, kind: &SpaceKind, style: MathStyle) -> Dim {
        let mu = self.params.mu(style);
        match kind {
            SpaceKind::Thin => mu * Dim::from_i64(3),
            SpaceKind::Medium => mu * Dim::from_i64(4),
            SpaceKind::Thick => mu * Dim::from_i64(5),
            SpaceKind::NegThin => -(mu * Dim::from_i64(3)),
            SpaceKind::Quad => self.params.em(style),
            SpaceKind::Qquad => self.params.em(style) * Dim::from_i64(2),
            SpaceKind::ControlSpace => self.params.em(style) / Dim::from_i64(3),
            SpaceKind::Hspace(d) => d * &self.params.scale(style),
        }
    }

    fn text_run(&self, s: &str, ts: TextStyle, style: MathStyle) -> Result<Item, Error> {
        if ts == TextStyle::Pmb {
            return self.pmb(s, style);
        }
        let mut kids = Vec::new();
        for c in s.chars() {
            if c == ' ' {
                kids.push(MathBox::kern(self.params.mu(style) * Dim::from_i64(4)));
            } else {
                kids.push(self.glyph(styled_char(c, ts), style)?);
            }
        }
        Ok(Item {
            bx: MathBox::hpack(kids),
            class: Some(AtomKind::Ord),
        })
    }

    fn pmb(&self, s: &str, style: MathStyle) -> Result<Item, Error> {
        let base = self.text_run(s, TextStyle::Rm, style)?;
        let dx = self.params.em(style) / Dim::from_i64(25);
        let shifted = MathBox::hpack(vec![MathBox::kern(dx.clone()), base.bx.clone()]);
        Ok(Item {
            class: Some(AtomKind::Ord),
            bx: MathBox {
                width: &base.bx.width + &dx,
                height: base.bx.height.clone(),
                depth: base.bx.depth.clone(),
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::Overlap(vec![base.bx, shifted]),
            },
        })
    }

    fn operator(&self, name: &str, limits: bool, style: MathStyle) -> Result<Item, Error> {
        if let Some(ch) = single_glyph(name) {
            if !ch.is_ascii_alphabetic() {
                return self.large_op(ch, None, None, style, limits);
            }
        }
        self.text_run(name, TextStyle::Rm, style).map(|mut it| {
            it.class = Some(AtomKind::Op);
            it
        })
    }

    fn row(&self, items: &[MathNode], style: MathStyle) -> Result<Item, Error> {
        if items.is_empty() {
            return Ok(Item {
                bx: MathBox::empty(),
                class: Some(AtomKind::Ord),
            });
        }
        let mut laid = Vec::new();
        for n in items {
            laid.push(self.item(n, style)?);
        }
        let n = laid.len();
        let mut classes: Vec<Option<AtomKind>> = Vec::with_capacity(n);
        for i in 0..n {
            let prev = if i == 0 { None } else { laid[i - 1].class };
            let next = if i + 1 < n { laid[i + 1].class } else { None };
            classes.push(laid[i].class.map(|c| convert_bin(prev, c, next)));
        }
        let mut out = Vec::new();
        for i in 0..n {
            if i > 0 {
                if let (Some(l), Some(r)) = (classes[i - 1], classes[i]) {
                    let mu = atom_space_mu(l, r, style);
                    let w = space_width(mu, &self.params, style);
                    if !w.is_zero() {
                        out.push(MathBox::kern(w));
                    }
                }
            }
            out.push(laid[i].bx.clone());
        }
        let class = if n == 1 {
            classes[0]
        } else {
            Some(AtomKind::Ord)
        };
        Ok(Item {
            bx: MathBox::hpack(out),
            class,
        })
    }

    fn fraction(&self, num: &MathNode, den: &MathNode, style: MathStyle) -> Result<Item, Error> {
        let num_b = self.layout(num, style.numerator())?;
        let den_b = self.layout(den, style.denominator())?;
        let s = self.params.scale(style);
        let axis = &self.params.axis_height * &s;
        let thick = &self.params.fraction_rule_thickness * &s;
        let half = &thick / &Dim::from_i64(2);
        let (shift_up0, shift_dn0, gap_num, gap_den) = if style.is_display() {
            (
                &self.params.fraction_numerator_display_style_shift_up * &s,
                &self.params.fraction_denominator_display_style_shift_down * &s,
                &self.params.fraction_num_display_style_gap_min * &s,
                &self.params.fraction_denom_display_style_gap_min * &s,
            )
        } else {
            (
                &self.params.fraction_numerator_shift_up * &s,
                &self.params.fraction_denominator_shift_down * &s,
                &self.params.fraction_numerator_gap_min * &s,
                &self.params.fraction_denominator_gap_min * &s,
            )
        };
        let num_shift = shift_up0.max(&(&axis + &half + &gap_num + &num_b.depth));
        let den_shift = shift_dn0.max(&(&den_b.height + &gap_den + &half - &axis).clamp_nonneg());
        let width = num_b.width.max(&den_b.width);
        let num_c = center_in(num_b, &width);
        let den_c = center_in(den_b, &width);
        let num_h = num_c.height.clone();
        let den_d = den_c.depth.clone();
        let bar =
            MathBox::rule(width.clone(), thick.clone(), Dim::zero()).with_shift(&axis - &half);
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: MathBox {
                width,
                height: &num_shift + &num_h,
                depth: &den_shift + &den_d,
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::Overlap(vec![
                    num_c.with_shift(num_shift),
                    bar,
                    den_c.with_shift(-den_shift),
                ]),
            },
        })
    }

    fn radical(
        &self,
        deg: Option<&MathNode>,
        rad: &MathNode,
        style: MathStyle,
    ) -> Result<Item, Error> {
        let rad_b = self.layout(rad, style.cramp())?;
        let s = self.params.scale(style);
        let thick = &self.params.radical_rule_thickness * &s;
        let extra = &self.params.radical_extra_ascender * &s;
        let gap = if style.is_display() {
            &self.params.radical_display_style_vertical_gap * &s
        } else {
            &self.params.radical_vertical_gap * &s
        };
        let needed = &rad_b.height + &rad_b.depth + &gap + &thick + &extra;
        let surd = self.sized_glyph('√', &needed, style)?;
        let bar = MathBox::rule(rad_b.width.clone(), thick.clone(), Dim::zero())
            .with_shift(&rad_b.height + &gap);
        let rad_col = MathBox {
            width: rad_b.width.clone(),
            height: &rad_b.height + &gap + &thick + &extra,
            depth: rad_b.depth.clone(),
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::Overlap(vec![bar, rad_b.clone()]),
        };
        let mut kids = vec![surd, rad_col];
        let mut width = MathBox::hpack(vec![kids[0].clone(), kids[1].clone()]).width;
        let height = (&rad_b.height + &gap + &thick + &extra).max(&kids[0].height);
        let depth = rad_b.depth.max(&kids[0].depth);
        if let Some(d) = deg {
            let db = self.layout(d, MathStyle::ScriptScript)?;
            let before = &self.params.radical_kern_before_degree * &s;
            let after = &self.params.radical_kern_after_degree * &s;
            let pct = Dim::from_i64(i64::from(self.params.radical_degree_bottom_raise_percent))
                / Dim::from_i64(100);
            let raise = &height * &pct;
            let deg_box = db.with_shift(raise);
            kids = vec![
                MathBox::kern(before),
                deg_box,
                MathBox::kern(after),
                kids[0].clone(),
                kids[1].clone(),
            ];
            width = MathBox::hpack(kids.clone()).width;
        }
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: MathBox {
                width,
                height,
                depth,
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::HList(kids),
            },
        })
    }

    fn scripts(
        &self,
        base: &MathNode,
        sub: Option<&MathNode>,
        sup: Option<&MathNode>,
        style: MathStyle,
    ) -> Result<Item, Error> {
        let base_it = self.item(base, style)?;
        self.attach_scripts_to_box(base_it.bx, base_it.class, sub, sup, style)
    }

    fn attach_scripts_to_box(
        &self,
        base: MathBox,
        class: Option<AtomKind>,
        sub: Option<&MathNode>,
        sup: Option<&MathNode>,
        style: MathStyle,
    ) -> Result<Item, Error> {
        if sub.is_none() && sup.is_none() {
            return Ok(Item { bx: base, class });
        }
        let s = self.params.scale(style);
        let ss = style.into_script();
        let after = &self.params.space_after_script * &self.params.scale(ss);
        let mut sup_shift = Dim::zero();
        let mut sub_shift = Dim::zero();
        let sup_laid = if let Some(e) = sup {
            sup_shift = if style.is_cramped() {
                &self.params.superscript_shift_up_cramped * &s
            } else {
                &self.params.superscript_shift_up * &s
            };
            Some(self.layout(e, ss.cramp())?)
        } else {
            None
        };
        let sub_laid = if let Some(u) = sub {
            sub_shift = &self.params.subscript_shift_down * &s;
            Some(self.layout(u, ss)?)
        } else {
            None
        };
        if let (Some(sp), Some(sb)) = (&sup_laid, &sub_laid) {
            let gap = &sup_shift + &sub_shift - &sp.depth - &sb.height;
            let min_gap = &self.params.sub_superscript_gap_min * &s;
            if matches!(gap.cmp(&min_gap), Some(Ordering::Less)) {
                sub_shift = &sub_shift + (&min_gap - &gap);
            }
        }
        let mut kids = vec![base.clone()];
        let mut width = base.width.clone();
        if sup_laid.is_some() && !base.italic.is_zero() {
            kids.push(MathBox::kern(base.italic.clone()));
            width = &width + &base.italic;
        }
        let mut slot_w = Dim::zero();
        let mut slot_h = Dim::zero();
        let mut slot_d = Dim::zero();
        let mut slot_kids = Vec::new();
        if let Some(sp) = sup_laid {
            slot_w = slot_w.max(&sp.width);
            slot_h = slot_h.max(&(&sp.height + &sup_shift));
            slot_d = slot_d.max(&(&sp.depth - &sup_shift).clamp_nonneg());
            slot_kids.push(sp.with_shift(sup_shift));
        }
        if let Some(sb) = sub_laid {
            slot_w = slot_w.max(&sb.width);
            slot_h = slot_h.max(&(&sb.height - &sub_shift).clamp_nonneg());
            slot_d = slot_d.max(&(&sb.depth + &sub_shift));
            slot_kids.push(sb.with_shift(-sub_shift));
        }
        kids.push(MathBox {
            width: slot_w.clone(),
            height: slot_h.clone(),
            depth: slot_d.clone(),
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::Overlap(slot_kids),
        });
        if !after.is_zero() {
            kids.push(MathBox::kern(after.clone()));
        }
        width = &width + &slot_w + &after;
        Ok(Item {
            class,
            bx: MathBox {
                width,
                height: base.height.max(&slot_h),
                depth: base.depth.max(&slot_d),
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::HList(kids),
            },
        })
    }

    fn delimited(
        &self,
        open: &Delimiter,
        body: &MathNode,
        close: &Delimiter,
        style: MathStyle,
    ) -> Result<Item, Error> {
        let body_b = self.layout(body, style)?;
        let s = self.params.scale(style);
        let axis = &self.params.axis_height * &s;
        let above = (&body_b.height - &axis).clamp_nonneg();
        let below = &body_b.depth + &axis;
        let needed = above.max(&below) * Dim::from_i64(2);
        let left = self.delim_box(open, &needed, style)?;
        let right = self.delim_box(close, &needed, style)?;
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: MathBox::hpack(vec![left, body_b, right]),
        })
    }

    fn explicit_delim_span(&self, size: DelimSize, style: MathStyle) -> Dim {
        let em = self.params.em(style);
        let n = match size {
            DelimSize::Big => 12,
            DelimSize::Big2 => 18,
            DelimSize::Bigg => 24,
            DelimSize::Bigg2 => 30,
        };
        em * Dim::from_i64(n) / Dim::from_i64(10)
    }

    fn delim_box(&self, d: &Delimiter, needed: &Dim, style: MathStyle) -> Result<MathBox, Error> {
        match d {
            Delimiter::Empty => Ok(MathBox::empty()),
            Delimiter::Char(c) => self.sized_glyph(*c, needed, style),
            Delimiter::Named(n) => {
                let ch = named_delim(n)?;
                self.sized_glyph(ch, needed, style)
            }
        }
    }

    fn sized_glyph(&self, ch: char, needed: &Dim, style: MathStyle) -> Result<MathBox, Error> {
        let base = self.font.glyph(ch)?;
        let mut best = self.glyph(ch, style)?;
        let mut best_span = &best.height + &best.depth;
        for gid in self.font.vertical_variants(base.glyph_id) {
            let cand = self.glyph_id(ch, gid, style)?;
            let span = &cand.height + &cand.depth;
            let meets = span.cmp(needed).is_some_and(|o| o != Ordering::Less);
            let best_short = best_span.cmp(needed).is_some_and(|o| o == Ordering::Less);
            let tighter = span.cmp(&best_span).is_some_and(|o| o == Ordering::Less);
            let taller = span.cmp(&best_span).is_some_and(|o| o == Ordering::Greater);
            if (meets && (best_short || tighter)) || (best_short && taller) {
                best_span = span;
                best = cand;
            }
        }
        Ok(best)
    }

    fn stretch_h(&self, base: MathBox, needed: &Dim, style: MathStyle) -> Result<MathBox, Error> {
        let BoxContent::Glyph { ch, glyph_id, .. } = base.content else {
            return Ok(base);
        };
        let mut best = base;
        let mut best_w = best.width.clone();
        for gid in self.font.horizontal_variants(glyph_id) {
            let cand = self.glyph_id(ch, gid, style)?;
            let wider_needed = best_w.cmp(needed).is_some_and(|o| o == Ordering::Less);
            let fits = cand.width.cmp(needed).is_some_and(|o| o != Ordering::Less);
            let tighter = cand.width.cmp(&best_w).is_some_and(|o| o == Ordering::Less);
            let longer = cand
                .width
                .cmp(&best_w)
                .is_some_and(|o| o == Ordering::Greater);
            if (fits && (wider_needed || tighter)) || (wider_needed && longer) {
                best_w = cand.width.clone();
                best = cand;
            }
        }
        if best_w.cmp(needed).is_some_and(|o| o != Ordering::Less) {
            return Ok(best);
        }
        if let Some(asm) = self.assemble_h(ch, glyph_id, needed, style) {
            if asm
                .width
                .cmp(&best_w)
                .is_some_and(|o| o == Ordering::Greater)
            {
                return Ok(asm);
            }
        }
        Ok(best)
    }

    fn assemble_h(&self, ch: char, gid: u16, needed: &Dim, style: MathStyle) -> Option<MathBox> {
        let parts = self.font.horizontal_assembly_parts(gid);
        if parts.is_empty() {
            return None;
        }
        let s = self.params.scale(style);
        let fu = |n: u16| Dim::from_font_units(i64::from(n), self.params.units_per_em) * &s;
        let sequence = |copies: u32| {
            let mut v = Vec::new();
            for &(id, start, end, adv, ext) in &parts {
                if ext {
                    for _ in 0..copies {
                        v.push((id, start, end, adv));
                    }
                } else {
                    v.push((id, start, end, adv));
                }
            }
            v
        };
        let width_of = |seq: &[(u16, u16, u16, u16)]| {
            if seq.is_empty() {
                return Dim::zero();
            }
            let mut w = fu(seq[0].3);
            for i in 1..seq.len() {
                let overlap = seq[i - 1].2.min(seq[i].1);
                w = &w + &fu(seq[i].3) - &fu(overlap);
            }
            w
        };
        let mut copies = 0u32;
        while copies < 64 {
            if width_of(&sequence(copies))
                .cmp(needed)
                .is_some_and(|o| o != Ordering::Less)
            {
                break;
            }
            copies += 1;
        }
        let seq = sequence(copies);
        if seq.is_empty() {
            return None;
        }
        let mut kids = Vec::new();
        for (i, &(id, start, _, _)) in seq.iter().enumerate() {
            if i > 0 {
                let overlap = seq[i - 1].2.min(start);
                kids.push(MathBox::kern(-(fu(overlap))));
            }
            kids.push(self.glyph_id(ch, id, style).ok()?);
        }
        Some(MathBox::hpack(kids))
    }

    fn large_op(
        &self,
        ch: char,
        lo: Option<&MathNode>,
        hi: Option<&MathNode>,
        style: MathStyle,
        limits_in_display: bool,
    ) -> Result<Item, Error> {
        let min_h = if style.is_display() {
            self.params.display_operator_min_height.clone() * self.params.scale(style)
        } else {
            Dim::zero()
        };
        let op = self.sized_glyph(ch, &min_h, style)?;
        let use_limits = limits_in_display && style.is_display();
        self.attach_limits(op, hi, lo, style, use_limits)
    }

    fn attach_limits(
        &self,
        op: MathBox,
        over: Option<&MathNode>,
        under: Option<&MathNode>,
        style: MathStyle,
        as_limits: bool,
    ) -> Result<Item, Error> {
        if !as_limits {
            return self.attach_scripts_to_box(op, Some(AtomKind::Op), under, over, style);
        }
        let s = self.params.scale(style);
        let op_h = op.height.clone();
        let op_d = op.depth.clone();
        let over_b = match over {
            Some(o) => Some(self.layout(o, style.into_script())?),
            None => None,
        };
        let under_b = match under {
            Some(u) => Some(self.layout(u, style.into_script())?),
            None => None,
        };
        let mut width = op.width.clone();
        if let Some(ref o) = over_b {
            width = width.max(&o.width);
        }
        if let Some(ref u) = under_b {
            width = width.max(&u.width);
        }
        let mut height = op_h.clone();
        let mut depth = op_d.clone();
        let mut kids = vec![center_in(op, &width)];
        if let Some(ob) = over_b {
            let gap = self.params.upper_limit_gap_min.clone() * &s;
            let rise = self.params.upper_limit_baseline_rise_min.clone() * &s;
            let extra = gap.max(&rise);
            height = &height + &ob.height + &ob.depth + &extra;
            let sh = &op_h + &extra + &ob.depth;
            kids.push(center_in(ob, &width).with_shift(sh));
        }
        if let Some(ub) = under_b {
            let gap = self.params.lower_limit_gap_min.clone() * &s;
            let drop = self.params.lower_limit_baseline_drop_min.clone() * &s;
            let extra = gap.max(&drop);
            depth = &depth + &ub.height + &ub.depth + &extra;
            let sh = -(&op_d + &extra + &ub.height);
            kids.push(center_in(ub, &width).with_shift(sh));
        }
        Ok(Item {
            class: Some(AtomKind::Op),
            bx: MathBox {
                width,
                height,
                depth,
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::Overlap(kids),
            },
        })
    }

    fn not_overlay(&self, base: &MathNode, b: MathBox, style: MathStyle) -> Result<Item, Error> {
        let slash = match self.glyph('\u{0338}', style) {
            Ok(bx) => bx,
            Err(_) => self.glyph('/', style)?,
        };
        let width = b.width.max(&slash.width);
        let two = Dim::from_i64(2);
        let b_axis = &(&b.height - &b.depth) / &two;
        let s_axis = &(&slash.height - &slash.depth) / &two;
        let raise = &b_axis - &s_axis;
        let height = b.height.max(&(&slash.height + &raise).max(&slash.height));
        let depth = b.depth.max(&(&slash.depth - &raise).clamp_nonneg());
        Ok(Item {
            class: class_of(base),
            bx: MathBox {
                width: width.clone(),
                height,
                depth,
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::Overlap(vec![
                    center_in(b, &width),
                    center_in(slash, &width).with_shift(raise),
                ]),
            },
        })
    }

    fn accent(&self, base: &MathNode, kind: AccentKind, style: MathStyle) -> Result<Item, Error> {
        let nucleus_style = if is_tex_accent(kind) {
            style.cramp()
        } else {
            style
        };
        let b = self.layout(base, nucleus_style)?;
        if kind == AccentKind::Not {
            return self.not_overlay(base, b, style);
        }
        if kind == AccentKind::Boxed {
            return Ok(Item {
                class: Some(AtomKind::Ord),
                bx: self.boxed_frame(b, style),
            });
        }
        if matches!(
            kind,
            AccentKind::Cancel | AccentKind::BCancel | AccentKind::XCancel
        ) {
            let mut kids = vec![b.clone()];
            kids.extend(self.cancel_lines(&b, kind, style));
            return Ok(Item {
                class: Some(AtomKind::Ord),
                bx: MathBox {
                    width: b.width.clone(),
                    height: b.height.clone(),
                    depth: b.depth.clone(),
                    italic: Dim::zero(),
                    shift: Dim::zero(),
                    content: BoxContent::Overlap(kids),
                },
            });
        }
        if matches!(kind, AccentKind::Overline | AccentKind::Underline) {
            return self.bar_rule(b, kind == AccentKind::Underline, style);
        }
        let mut acc = self.accent_glyph(kind, style)?;
        let stretchy = is_stretchy_accent(kind);
        if stretchy {
            acc = self.stretch_h(acc, &b.width, style)?;
        }
        if is_under_accent(kind) {
            return Ok(Item {
                class: Some(AtomKind::Ord),
                bx: self.place_under(b, acc, style),
            });
        }
        let x_off = if stretchy {
            let extra = &b.width - &acc.width;
            &extra / &Dim::from_i64(2)
        } else {
            self.accent_x_off(&b, &acc, kind)
        };
        let raise = self.accent_raise(&b, kind, style);
        Ok(Item {
            class: Some(AtomKind::Ord),
            bx: overlay_accent(b, acc, x_off, raise),
        })
    }

    fn accent_glyph(&self, kind: AccentKind, style: MathStyle) -> Result<MathBox, Error> {
        for &ch in accent_candidates(kind) {
            if let Ok(bx) = self.glyph(ch, style) {
                return Ok(bx);
            }
        }
        Err(Error::Unsupported {
            what: format!("accent {}", kind.gold()),
        })
    }

    fn cancelto(&self, value: &MathNode, expr: &MathNode, style: MathStyle) -> Result<Item, Error> {
        let b = self.layout(expr, style)?;
        let val = self.layout(value, style.into_script())?;
        let mut kids = vec![b.clone()];
        kids.extend(self.cancel_lines(&b, AccentKind::Cancel, style));
        let gap = self.params.space_after_script.clone() * self.params.scale(style);
        let val_x = &b.width + &gap;
        let val_shift = &b.height + &val.depth;
        let val_w = val.width.clone();
        let val_h = val.height.clone();
        kids.push(shift_x(val, val_x.clone()).with_shift(val_shift.clone()));
        let width = &val_x + &val_w;
        let height = b.height.max(&(&val_shift + &val_h));
        Ok(Item {
            class: Some(AtomKind::Ord),
            bx: MathBox {
                width,
                height,
                depth: b.depth.clone(),
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::Overlap(kids),
            },
        })
    }

    fn boxed_frame(&self, inner: MathBox, style: MathStyle) -> MathBox {
        let pad = self.params.mu(style) * Dim::from_i64(3);
        let thick = self.params.fraction_rule_thickness.clone() * self.params.scale(style);
        frame_wrap(thick, None, pad_box(inner, &pad))
    }

    fn bar_rule(&self, b: MathBox, under: bool, style: MathStyle) -> Result<Item, Error> {
        let s = self.params.scale(style);
        let gap = if under {
            self.params.underbar_vertical_gap.clone() * &s
        } else {
            self.params.overbar_vertical_gap.clone() * &s
        };
        let thick = if under {
            self.params.underbar_rule_thickness.clone() * &s
        } else {
            self.params.overbar_rule_thickness.clone() * &s
        };
        let extra = if under {
            self.params.underbar_extra_descender.clone() * &s
        } else {
            self.params.overbar_extra_ascender.clone() * &s
        };
        let mut height = b.height.clone();
        let mut depth = b.depth.clone();
        let bar = if under {
            depth = &depth + &gap + &thick + &extra;
            MathBox::rule(b.width.clone(), thick, Dim::zero()).with_shift(-(&b.depth + &gap))
        } else {
            height = &height + &gap + &thick + &extra;
            MathBox::rule(b.width.clone(), thick, Dim::zero()).with_shift(&b.height + &gap)
        };
        Ok(Item {
            class: Some(AtomKind::Ord),
            bx: MathBox {
                width: b.width.clone(),
                height,
                depth,
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: BoxContent::Overlap(vec![b, bar]),
            },
        })
    }

    fn cancel_lines(&self, b: &MathBox, kind: AccentKind, style: MathStyle) -> Vec<MathBox> {
        let t = self.params.fraction_rule_thickness.clone() * self.params.scale(style);
        let w = b.width.clone();
        let h = b.height.clone();
        let d = b.depth.clone();
        let mk = |x1: Dim, y1: Dim, x2: Dim, y2: Dim| MathBox {
            width: w.clone(),
            height: h.clone(),
            depth: d.clone(),
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::Line {
                x1,
                y1,
                x2,
                y2,
                thickness: t.clone(),
            },
        };
        match kind {
            AccentKind::Cancel => vec![mk(Dim::zero(), -d.clone(), w.clone(), h.clone())],
            AccentKind::BCancel => vec![mk(Dim::zero(), h.clone(), w.clone(), -d.clone())],
            AccentKind::XCancel => vec![
                mk(Dim::zero(), -d.clone(), w.clone(), h.clone()),
                mk(Dim::zero(), h.clone(), w.clone(), -d.clone()),
            ],
            _ => Vec::new(),
        }
    }

    fn accent_x_off(&self, base: &MathBox, acc: &MathBox, kind: AccentKind) -> Dim {
        let two = Dim::from_i64(2);
        let base_att = first_glyph_id(base)
            .and_then(|id| self.font.top_accent_attachment(id))
            .unwrap_or_else(|| (&base.width + &base.italic) / &two);
        let acc_att = first_glyph_id(acc)
            .and_then(|id| self.font.top_accent_attachment(id))
            .unwrap_or_else(|| &acc.width / &two);
        match kind {
            AccentKind::Vec | AccentKind::Overrightarrow | AccentKind::Underrightarrow => {
                &base.width + &base.italic - &acc.width
            }
            AccentKind::Acute => &base_att - &acc_att + &(&acc.width / &Dim::from_i64(4)),
            AccentKind::Grave => &base_att - &acc_att - &(&acc.width / &Dim::from_i64(4)),
            _ => &base_att - &acc_att,
        }
    }

    /// Vertical shift of an over-accent glyph.
    ///
    /// Diacritic glyphs are drawn to sit on a base of `AccentBaseHeight`, so
    /// TeX's rule applies: lift only by how far the base rises above that
    /// height. Arrow and brace glyphs sit on the baseline and clear the base.
    /// The branch is chosen by kind, not glyph advance: STIX spacing
    /// diacritics (`ˆ`, `˙`, ...) have non-zero advance.
    fn accent_raise(&self, base: &MathBox, kind: AccentKind, style: MathStyle) -> Dim {
        let s = self.params.scale(style);
        let abh = if style.is_cramped() {
            &self.params.flattened_accent_base_height * &s
        } else {
            &self.params.accent_base_height * &s
        };
        if is_diacritic_accent(kind) {
            (&base.height - &abh).clamp_nonneg()
        } else {
            base.height.max(&abh)
        }
    }

    fn place_under(&self, base: MathBox, acc: MathBox, style: MathStyle) -> MathBox {
        let s = self.params.scale(style);
        let gap = self.params.underbar_vertical_gap.clone() * &s;
        let extra = self.params.underbar_extra_descender.clone() * &s;
        let raise = -(&base.depth + &gap + &acc.height);
        let width = base.width.max(&acc.width);
        let depth = &base.depth + &gap + &acc.height + &acc.depth + &extra;
        MathBox {
            width: width.clone(),
            height: base.height.clone(),
            depth,
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::Overlap(vec![
                center_in(base, &width),
                center_in(acc, &width).with_shift(raise),
            ]),
        }
    }

    fn take_number(&self) -> Option<String> {
        let i = self.idx.get();
        self.idx.set(i + 1);
        self.numbers.assigned(i).map(str::to_string)
    }

    fn number_box(&self, s: &str, style: MathStyle) -> Result<MathBox, Error> {
        Ok(self.text_run(s, TextStyle::Rm, style)?.bx)
    }

    fn attach_number(&self, body: MathBox, num: Option<MathBox>, style: MathStyle) -> MathBox {
        let Some(num) = num else {
            return body;
        };
        let gap = self.params.em(style);
        MathBox::hpack(vec![body, MathBox::kern(gap), num])
    }

    fn reference(&self, key: &str, style: MathStyle) -> Result<Item, Error> {
        let s = self.numbers.lookup(key).ok_or_else(|| Error::Unsupported {
            what: format!("undefined label {key}"),
        })?;
        self.text_run(s, TextStyle::Rm, style)
    }

    fn tag_box(&self, star: bool, body: &MathNode, style: MathStyle) -> Result<Item, Error> {
        let inner = self.layout(body, MathStyle::Text)?;
        let bx = if star {
            inner
        } else {
            let open = self.glyph('(', style)?;
            let close = self.glyph(')', style)?;
            MathBox::hpack(vec![open, inner, close])
        };
        Ok(Item {
            class: Some(AtomKind::Ord),
            bx,
        })
    }

    fn substack(&self, lines: &[MathNode], style: MathStyle) -> Result<Item, Error> {
        let ss = style.into_script();
        let sep = self.params.em(ss) / Dim::from_i64(5);
        let mut laid = Vec::new();
        for ln in lines {
            laid.push(self.layout(ln, ss)?);
        }
        let width = laid.iter().fold(Dim::zero(), |w, b| w.max(&b.width));
        let mut rows = Vec::new();
        for (i, b) in laid.into_iter().enumerate() {
            if i > 0 {
                rows.push(MathBox {
                    width: Dim::zero(),
                    height: Dim::zero(),
                    depth: sep.clone(),
                    italic: Dim::zero(),
                    shift: Dim::zero(),
                    content: BoxContent::Empty,
                });
            }
            rows.push(align_in(b, &width, ColSpec::Center));
        }
        Ok(Item {
            class: Some(AtomKind::Ord),
            bx: MathBox::vpack(rows),
        })
    }

    fn matrix(
        &self,
        style_m: MatrixStyle,
        spec: &[ColSpec],
        rows: &[EnvRow],
        style: MathStyle,
    ) -> Result<Item, Error> {
        if rows.is_empty() {
            return Ok(Item {
                bx: MathBox::empty(),
                class: Some(AtomKind::Inner),
            });
        }
        let body_style = if style_m.is_display_env() {
            MathStyle::Display
        } else {
            style
        };
        match style_m {
            MatrixStyle::Align => self.align_env(rows, body_style, true),
            MatrixStyle::Aligned | MatrixStyle::Split => self.align_env(rows, body_style, false),
            MatrixStyle::Gather => self.gather_env(rows, body_style),
            MatrixStyle::Multline => self.multline_env(rows, body_style),
            MatrixStyle::Equation => self.equation_env(rows, body_style),
            MatrixStyle::Array => self.array_env(spec, rows, body_style),
            MatrixStyle::Cases => self.cases_env(rows, body_style),
            _ => self.centered_matrix(style_m, rows, body_style),
        }
    }

    fn centered_matrix(
        &self,
        style_m: MatrixStyle,
        rows: &[EnvRow],
        style: MathStyle,
    ) -> Result<Item, Error> {
        let data = data_cells(rows)?;
        self.grid(
            &data,
            style,
            None,
            self.params.mu(style) * Dim::from_i64(10),
            ColSpec::Center,
            matrix_delims(style_m),
            false,
        )
    }

    fn cases_env(&self, rows: &[EnvRow], style: MathStyle) -> Result<Item, Error> {
        let data = data_cells(rows)?;
        self.grid(
            &data,
            style,
            None,
            self.params.em(style),
            ColSpec::Left,
            (Some('{'), None),
            false,
        )
    }

    fn align_env(&self, rows: &[EnvRow], style: MathStyle, numbered: bool) -> Result<Item, Error> {
        let mut math_rows: Vec<Vec<MathBox>> = Vec::new();
        let mut nums: Vec<Option<MathBox>> = Vec::new();
        let mut extras: Vec<RowKind> = Vec::new();
        let mut ncols = 0;
        for row in rows {
            match row {
                EnvRow::Hline => {
                    extras.push(RowKind::Hline);
                }
                EnvRow::Intertext(n) => {
                    extras.push(RowKind::Intertext(Box::new(
                        self.layout(n, MathStyle::Text)?,
                    )));
                }
                EnvRow::Cells { cells, .. } => {
                    let mut rboxes = Vec::new();
                    for c in cells {
                        rboxes.push(self.layout(c, style)?);
                    }
                    ncols = ncols.max(rboxes.len());
                    math_rows.push(rboxes);
                    extras.push(RowKind::Cells);
                    if numbered {
                        nums.push(match self.take_number() {
                            Some(s) => Some(self.number_box(&s, MathStyle::Text)?),
                            None => None,
                        });
                    }
                }
            }
        }
        let mut col_w = vec![Dim::zero(); ncols];
        for row in &math_rows {
            for (j, cell) in row.iter().enumerate() {
                col_w[j] = col_w[j].max(&cell.width);
            }
        }
        let pair_sep = self.params.em(style);
        let row_sep = self.params.em(style) / Dim::from_i64(5);
        let mut packed = Vec::new();
        let mut mi = 0;
        for extra in extras {
            match extra {
                RowKind::Hline => {
                    return Err(Error::Unsupported {
                        what: "hline in align".into(),
                    });
                }
                RowKind::Intertext(t) => {
                    if !packed.is_empty() {
                        packed.push(sep_row(row_sep.clone()));
                    }
                    packed.push(*t);
                }
                RowKind::Cells => {
                    if !packed.is_empty() {
                        packed.push(sep_row(row_sep.clone()));
                    }
                    let mut parts = Vec::new();
                    let row = pad_row(math_rows[mi].clone(), ncols);
                    for (j, cell) in row.into_iter().enumerate() {
                        if j > 0 && j % 2 == 0 {
                            parts.push(MathBox::kern(pair_sep.clone()));
                        }
                        let align = if j % 2 == 0 {
                            ColSpec::Right
                        } else {
                            ColSpec::Left
                        };
                        parts.push(align_in(cell, &col_w[j], align));
                    }
                    let mut body = MathBox::hpack(parts);
                    if numbered {
                        body = self.attach_number(body, nums[mi].clone(), style);
                    }
                    packed.push(body);
                    mi += 1;
                }
            }
        }
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: MathBox::vpack(packed),
        })
    }

    fn gather_env(&self, rows: &[EnvRow], style: MathStyle) -> Result<Item, Error> {
        let row_sep = self.params.em(style) / Dim::from_i64(5);
        let mut bodies = Vec::new();
        let mut kinds = Vec::new();
        for row in rows {
            match row {
                EnvRow::Hline => {
                    return Err(Error::Unsupported {
                        what: "hline in gather".into(),
                    });
                }
                EnvRow::Intertext(n) => {
                    kinds.push(RowKind::Intertext(Box::new(
                        self.layout(n, MathStyle::Text)?,
                    )));
                }
                EnvRow::Cells { cells, .. } => {
                    let node = if cells.len() == 1 {
                        cells[0].clone()
                    } else {
                        MathNode::Row(cells.clone())
                    };
                    bodies.push(self.layout(&node, style)?);
                    kinds.push(RowKind::Cells);
                }
            }
        }
        let max_w = bodies.iter().fold(Dim::zero(), |w, b| w.max(&b.width));
        let mut packed = Vec::new();
        let mut bi = 0;
        for kind in kinds {
            if !packed.is_empty() {
                packed.push(sep_row(row_sep.clone()));
            }
            match kind {
                RowKind::Intertext(t) => packed.push(*t),
                RowKind::Cells => {
                    let body = align_in(bodies[bi].clone(), &max_w, ColSpec::Center);
                    let num = match self.take_number() {
                        Some(s) => Some(self.number_box(&s, MathStyle::Text)?),
                        None => None,
                    };
                    packed.push(self.attach_number(body, num, style));
                    bi += 1;
                }
                RowKind::Hline => {}
            }
        }
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: MathBox::vpack(packed),
        })
    }

    fn multline_env(&self, rows: &[EnvRow], style: MathStyle) -> Result<Item, Error> {
        let data = data_cells(rows)?;
        if data.is_empty() {
            let _ = self.take_number();
            return Ok(Item {
                bx: MathBox::empty(),
                class: Some(AtomKind::Inner),
            });
        }
        let mut bodies = Vec::new();
        for row in &data {
            let node = if row.len() == 1 {
                row[0].clone()
            } else {
                MathNode::Row(row.clone())
            };
            bodies.push(self.layout(&node, style)?);
        }
        let max_w = bodies.iter().fold(Dim::zero(), |w, b| w.max(&b.width));
        let n = bodies.len();
        let row_sep = self.params.em(style) / Dim::from_i64(5);
        let mut packed = Vec::new();
        for (i, b) in bodies.into_iter().enumerate() {
            if i > 0 {
                packed.push(sep_row(row_sep.clone()));
            }
            let align = if i == 0 {
                ColSpec::Left
            } else if i + 1 == n {
                ColSpec::Right
            } else {
                ColSpec::Center
            };
            packed.push(align_in(b, &max_w, align));
        }
        let mut inner = MathBox::vpack(packed);
        let num = match self.take_number() {
            Some(s) => Some(self.number_box(&s, MathStyle::Text)?),
            None => None,
        };
        inner = self.attach_number(inner, num, style);
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: inner,
        })
    }

    fn equation_env(&self, rows: &[EnvRow], style: MathStyle) -> Result<Item, Error> {
        let data = data_cells(rows)?;
        let mut parts = Vec::new();
        for row in &data {
            for (i, c) in row.iter().enumerate() {
                if i > 0 {
                    parts.push(MathNode::Space(SpaceKind::Quad));
                }
                parts.push(c.clone());
            }
        }
        let node = wrap_nodes(parts);
        let body = self.layout(&node, style)?;
        let num = match self.take_number() {
            Some(s) => Some(self.number_box(&s, MathStyle::Text)?),
            None => None,
        };
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: self.attach_number(body, num, style),
        })
    }

    fn array_env(
        &self,
        spec: &[ColSpec],
        rows: &[EnvRow],
        style: MathStyle,
    ) -> Result<Item, Error> {
        let thick = self.params.fraction_rule_thickness.clone() * self.params.scale(style);
        let col_sep = self.params.mu(style) * Dim::from_i64(10);
        let row_sep = self.params.em(style) / Dim::from_i64(5);
        let mut kinds = Vec::new();
        let mut data: Vec<Vec<MathBox>> = Vec::new();
        let mut ncols_data = 0;
        for row in rows {
            match row {
                EnvRow::Hline => kinds.push(RowKind::Hline),
                EnvRow::Intertext(n) => {
                    kinds.push(RowKind::Intertext(Box::new(
                        self.layout(n, MathStyle::Text)?,
                    )));
                }
                EnvRow::Cells { cells, .. } => {
                    let mut rboxes = Vec::new();
                    for c in cells {
                        rboxes.push(self.layout(c, style)?);
                    }
                    ncols_data = ncols_data.max(rboxes.len());
                    data.push(rboxes);
                    kinds.push(RowKind::Cells);
                }
            }
        }
        let mut spec: Vec<ColSpec> = if spec.is_empty() {
            vec![ColSpec::Center; ncols_data]
        } else {
            spec.to_vec()
        };
        let mut ndata = spec.iter().filter(|c| !c.is_rule()).count();
        while ndata < ncols_data {
            spec.push(ColSpec::Center);
            ndata += 1;
        }
        for row in &mut data {
            while row.len() < ndata {
                row.push(MathBox::empty());
            }
        }
        let mut col_w: Vec<Dim> = spec
            .iter()
            .map(|c| {
                if c.is_rule() {
                    thick.clone()
                } else {
                    Dim::zero()
                }
            })
            .collect();
        for row in &data {
            let mut dj = 0;
            for (j, sp) in spec.iter().enumerate() {
                if sp.is_rule() {
                    continue;
                }
                if dj < row.len() {
                    col_w[j] = col_w[j].max(&row[dj].width);
                }
                dj += 1;
            }
        }
        let mut table_w = Dim::zero();
        for (j, _) in spec.iter().enumerate() {
            if j > 0 && !spec[j].is_rule() && !spec[j - 1].is_rule() {
                table_w = &table_w + &col_sep;
            }
            table_w = &table_w + &col_w[j];
        }
        let mut packed = Vec::new();
        let mut di = 0;
        for kind in kinds {
            match kind {
                RowKind::Hline => {
                    packed.push(MathBox::rule(table_w.clone(), thick.clone(), Dim::zero()));
                }
                RowKind::Intertext(t) => {
                    if !packed.is_empty() {
                        packed.push(sep_row(row_sep.clone()));
                    }
                    packed.push(*t);
                }
                RowKind::Cells => {
                    if !packed.is_empty()
                        && !matches!(packed.last().map(|b| &b.content), Some(BoxContent::Rule))
                    {
                        packed.push(sep_row(row_sep.clone()));
                    }
                    let row = &data[di];
                    let mut rh = Dim::zero();
                    let mut rd = Dim::zero();
                    for c in row {
                        rh = rh.max(&c.height);
                        rd = rd.max(&c.depth);
                    }
                    let mut parts = Vec::new();
                    let mut dj = 0;
                    for (j, sp) in spec.iter().enumerate() {
                        if j > 0 && !sp.is_rule() && !spec[j - 1].is_rule() {
                            parts.push(MathBox::kern(col_sep.clone()));
                        }
                        if sp.is_rule() {
                            parts.push(MathBox {
                                width: thick.clone(),
                                height: rh.clone(),
                                depth: rd.clone(),
                                italic: Dim::zero(),
                                shift: Dim::zero(),
                                content: BoxContent::Rule,
                            });
                        } else {
                            let cell = row.get(dj).cloned().unwrap_or_else(MathBox::empty);
                            parts.push(align_in(cell, &col_w[j], *sp));
                            dj += 1;
                        }
                    }
                    packed.push(MathBox::hpack(parts));
                    di += 1;
                }
            }
        }
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: MathBox::vpack(packed),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn grid(
        &self,
        rows: &[Vec<MathNode>],
        style: MathStyle,
        spec: Option<&[ColSpec]>,
        col_sep: Dim,
        default_align: ColSpec,
        delims: (Option<char>, Option<char>),
        numbered: bool,
    ) -> Result<Item, Error> {
        if rows.is_empty() {
            return Ok(Item {
                bx: MathBox::empty(),
                class: Some(AtomKind::Inner),
            });
        }
        let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut cells: Vec<Vec<MathBox>> = Vec::new();
        for row in rows {
            let mut rboxes = Vec::new();
            for c in 0..ncols {
                if c < row.len() {
                    rboxes.push(self.layout(&row[c], style)?);
                } else {
                    rboxes.push(MathBox::empty());
                }
            }
            cells.push(rboxes);
        }
        let mut col_w: Vec<Dim> = vec![Dim::zero(); ncols];
        for row in &cells {
            for (j, cell) in row.iter().enumerate() {
                col_w[j] = col_w[j].max(&cell.width);
            }
        }
        let row_sep = self.params.em(style) / Dim::from_i64(5);
        let mut row_boxes = Vec::new();
        for (ri, row) in cells.into_iter().enumerate() {
            let mut parts = Vec::new();
            for (j, cell) in row.into_iter().enumerate() {
                if j > 0 {
                    parts.push(MathBox::kern(col_sep.clone()));
                }
                let align = spec
                    .and_then(|s| s.get(j).copied())
                    .unwrap_or(default_align);
                parts.push(align_in(cell, &col_w[j], align));
            }
            if ri > 0 {
                row_boxes.push(sep_row(row_sep.clone()));
            }
            let mut packed = MathBox::hpack(parts);
            if numbered {
                let num = match self.take_number() {
                    Some(s) => Some(self.number_box(&s, MathStyle::Text)?),
                    None => None,
                };
                packed = self.attach_number(packed, num, style);
            }
            row_boxes.push(packed);
        }
        let mut inner = MathBox::vpack(row_boxes);
        let needed = &inner.height + &inner.depth;
        let (ld, rd) = delims;
        if let Some(l) = ld {
            let left = self.sized_glyph(l, &needed, style)?;
            let right = match rd {
                Some(r) => self.sized_glyph(r, &needed, style)?,
                None => MathBox::empty(),
            };
            inner = MathBox::hpack(vec![left, inner, right]);
        }
        Ok(Item {
            class: Some(AtomKind::Inner),
            bx: inner,
        })
    }
}

enum RowKind {
    Cells,
    Hline,
    Intertext(Box<MathBox>),
}

fn data_cells(rows: &[EnvRow]) -> Result<Vec<Vec<MathNode>>, Error> {
    let mut out = Vec::new();
    for r in rows {
        match r {
            EnvRow::Cells { cells, .. } => out.push(cells.clone()),
            EnvRow::Hline => {
                return Err(Error::Unsupported {
                    what: "hline in this environment".into(),
                });
            }
            EnvRow::Intertext(_) => {
                return Err(Error::Unsupported {
                    what: "intertext in this environment".into(),
                });
            }
        }
    }
    Ok(out)
}

fn sep_row(depth: Dim) -> MathBox {
    MathBox {
        width: Dim::zero(),
        height: Dim::zero(),
        depth,
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::Empty,
    }
}

fn pad_row(mut row: Vec<MathBox>, ncols: usize) -> Vec<MathBox> {
    while row.len() < ncols {
        row.push(MathBox::empty());
    }
    row
}

fn wrap_nodes(items: Vec<MathNode>) -> MathNode {
    if items.len() == 1 {
        items
            .into_iter()
            .next()
            .unwrap_or(MathNode::Row(Vec::new()))
    } else {
        MathNode::Row(items)
    }
}

fn align_in(inner: MathBox, width: &Dim, align: ColSpec) -> MathBox {
    if matches!(align, ColSpec::VRule) || inner.width.eq_dim(width) {
        return inner;
    }
    let extra = width - &inner.width;
    match align {
        ColSpec::Center => center_in(inner, width),
        ColSpec::Left => {
            let h = inner.height.clone();
            let d = inner.depth.clone();
            let packed = MathBox::hpack(vec![inner, MathBox::kern(extra)]);
            MathBox {
                width: packed.width,
                height: h,
                depth: d,
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: packed.content,
            }
        }
        ColSpec::Right => {
            let h = inner.height.clone();
            let d = inner.depth.clone();
            let packed = MathBox::hpack(vec![MathBox::kern(extra), inner]);
            MathBox {
                width: packed.width,
                height: h,
                depth: d,
                italic: Dim::zero(),
                shift: Dim::zero(),
                content: packed.content,
            }
        }
        ColSpec::VRule => inner,
    }
}

fn color_wrap(c: Color, inner: MathBox) -> MathBox {
    MathBox {
        width: inner.width.clone(),
        height: inner.height.clone(),
        depth: inner.depth.clone(),
        italic: inner.italic.clone(),
        shift: inner.shift.clone(),
        content: BoxContent::Color(c, Box::new(inner)),
    }
}

fn back_color_wrap(c: Color, inner: MathBox) -> MathBox {
    MathBox {
        width: inner.width.clone(),
        height: inner.height.clone(),
        depth: inner.depth.clone(),
        italic: inner.italic.clone(),
        shift: inner.shift.clone(),
        content: BoxContent::BackColor(c, Box::new(inner)),
    }
}

fn frame_wrap(thickness: Dim, stroke: Option<Color>, inner: MathBox) -> MathBox {
    MathBox {
        width: inner.width.clone(),
        height: inner.height.clone(),
        depth: inner.depth.clone(),
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::Frame {
            thickness,
            stroke,
            inner: Box::new(inner),
        },
    }
}

fn pad_box(inner: MathBox, pad: &Dim) -> MathBox {
    let w = &inner.width + pad + pad;
    let h = &inner.height + pad;
    let d = &inner.depth + pad;
    MathBox {
        width: w,
        height: h,
        depth: d,
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::HList(vec![
            MathBox::kern(pad.clone()),
            inner,
            MathBox::kern(pad.clone()),
        ]),
    }
}

fn center_in(inner: MathBox, width: &Dim) -> MathBox {
    if inner.width.eq_dim(width) {
        return inner;
    }
    let extra = width - &inner.width;
    let half = &extra / &Dim::from_i64(2);
    let h = inner.height.clone();
    let d = inner.depth.clone();
    let packed = MathBox::hpack(vec![
        MathBox::kern(half.clone()),
        inner,
        MathBox::kern(&extra - &half),
    ]);
    MathBox {
        width: packed.width,
        height: h,
        depth: d,
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: packed.content,
    }
}

fn shift_x(inner: MathBox, x: Dim) -> MathBox {
    if x.is_zero() {
        return inner;
    }
    let h = inner.height.clone();
    let d = inner.depth.clone();
    let packed = MathBox::hpack(vec![MathBox::kern(x), inner]);
    MathBox {
        width: packed.width,
        height: h,
        depth: d,
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: packed.content,
    }
}

fn overlay_accent(base: MathBox, acc: MathBox, x_off: Dim, raise: Dim) -> MathBox {
    let origin = (-x_off.clone()).clamp_nonneg();
    let base_x = origin.clone();
    let acc_x = &origin + &x_off;
    let width = (&base_x + &base.width).max(&(&acc_x + &acc.width));
    let acc_top = &acc.height + &raise;
    let acc_bot = &raise - &acc.depth;
    let height = base.height.max(&acc_top);
    let depth = base.depth.max(&(-acc_bot).clamp_nonneg());
    MathBox {
        width,
        height,
        depth,
        italic: Dim::zero(),
        shift: Dim::zero(),
        content: BoxContent::Overlap(vec![
            shift_x(base, base_x),
            shift_x(acc, acc_x).with_shift(raise),
        ]),
    }
}

fn first_glyph_id(b: &MathBox) -> Option<u16> {
    match &b.content {
        BoxContent::Glyph { glyph_id, .. } => Some(*glyph_id),
        BoxContent::HList(v) | BoxContent::VList(v) | BoxContent::Overlap(v) => {
            v.iter().find_map(first_glyph_id)
        }
        BoxContent::Color(_, inner)
        | BoxContent::BackColor(_, inner)
        | BoxContent::Frame { inner, .. } => first_glyph_id(inner),
        _ => None,
    }
}

fn is_tex_accent(kind: AccentKind) -> bool {
    matches!(
        kind,
        AccentKind::Hat
            | AccentKind::Check
            | AccentKind::Breve
            | AccentKind::Acute
            | AccentKind::Grave
            | AccentKind::Tilde
            | AccentKind::Bar
            | AccentKind::Vec
            | AccentKind::Dot
            | AccentKind::Ddot
            | AccentKind::Dddot
            | AccentKind::Ddddot
            | AccentKind::Ring
            | AccentKind::WideHat
            | AccentKind::WideTilde
            | AccentKind::Overleftarrow
            | AccentKind::Overrightarrow
            | AccentKind::Overleftrightarrow
            | AccentKind::Underleftarrow
            | AccentKind::Underrightarrow
            | AccentKind::Underleftrightarrow
            | AccentKind::Overbrace
            | AccentKind::Underbrace
    )
}

fn is_stretchy_accent(kind: AccentKind) -> bool {
    matches!(
        kind,
        AccentKind::WideHat
            | AccentKind::WideTilde
            | AccentKind::Overleftarrow
            | AccentKind::Overrightarrow
            | AccentKind::Overleftrightarrow
            | AccentKind::Underleftarrow
            | AccentKind::Underrightarrow
            | AccentKind::Underleftrightarrow
            | AccentKind::Overbrace
            | AccentKind::Underbrace
    )
}

fn is_diacritic_accent(kind: AccentKind) -> bool {
    matches!(
        kind,
        AccentKind::Hat
            | AccentKind::WideHat
            | AccentKind::Check
            | AccentKind::Breve
            | AccentKind::Acute
            | AccentKind::Grave
            | AccentKind::Tilde
            | AccentKind::WideTilde
            | AccentKind::Bar
            | AccentKind::Dot
            | AccentKind::Ddot
            | AccentKind::Dddot
            | AccentKind::Ddddot
            | AccentKind::Ring
    )
}

fn is_under_accent(kind: AccentKind) -> bool {
    matches!(
        kind,
        AccentKind::Underleftarrow
            | AccentKind::Underrightarrow
            | AccentKind::Underleftrightarrow
            | AccentKind::Underbrace
    )
}

fn accent_candidates(kind: AccentKind) -> &'static [char] {
    match kind {
        AccentKind::Hat | AccentKind::WideHat => &['ˆ', '\u{0302}'],
        AccentKind::Check => &['ˇ', '\u{030C}'],
        AccentKind::Breve => &['˘', '\u{0306}'],
        AccentKind::Acute => &['´', '\u{0301}'],
        AccentKind::Grave => &['`', '\u{0300}'],
        AccentKind::Tilde | AccentKind::WideTilde => &['˜', '\u{0303}'],
        AccentKind::Bar => &['¯', '\u{0304}'],
        AccentKind::Vec => &['→', '\u{20D7}'],
        AccentKind::Dot => &['˙', '\u{0307}'],
        AccentKind::Ddot => &['¨', '\u{0308}'],
        AccentKind::Dddot => &['\u{20DB}'],
        AccentKind::Ddddot => &['\u{20DC}'],
        AccentKind::Ring => &['˚', '\u{030A}'],
        AccentKind::Overleftarrow | AccentKind::Underleftarrow => &['←', '\u{27F5}'],
        AccentKind::Overrightarrow | AccentKind::Underrightarrow => &['→', '\u{27F6}'],
        AccentKind::Overleftrightarrow | AccentKind::Underleftrightarrow => &['↔', '\u{27F7}'],
        AccentKind::Overbrace => &['⏞'],
        AccentKind::Underbrace => &['⏟'],
        AccentKind::Not
        | AccentKind::Overline
        | AccentKind::Underline
        | AccentKind::Cancel
        | AccentKind::BCancel
        | AccentKind::XCancel
        | AccentKind::Boxed => &[],
    }
}

fn class_of(n: &MathNode) -> Option<AtomKind> {
    match n {
        MathNode::Atom(_, k) => Some(*k),
        MathNode::Symbol(s) => Some(symbol_class(s)),
        MathNode::Operator(_, _)
        | MathNode::Sum(_, _)
        | MathNode::Product(_, _)
        | MathNode::Integral(_, _, _)
        | MathNode::Limit(_) => Some(AtomKind::Op),
        MathNode::Fraction(_, _)
        | MathNode::Radical(_, _)
        | MathNode::Matrix(_, _, _)
        | MathNode::Substack(_)
        | MathNode::Delimited(_, _, _) => Some(AtomKind::Inner),
        MathNode::SizedDelim(_, _, k) => Some(*k),
        MathNode::Superscript(b, _)
        | MathNode::Subscript(b, _)
        | MathNode::SubSup(b, _, _)
        | MathNode::Accent(b, _)
        | MathNode::OverUnder(b, _, _)
        | MathNode::CancelTo(_, b)
        | MathNode::Tag { body: b, .. }
        | MathNode::Intertext(b) => class_of(b),
        MathNode::Text(_, _) | MathNode::Ref(_) => Some(AtomKind::Ord),
        MathNode::Color(_, b)
        | MathNode::TextColor(_, b)
        | MathNode::ColorBox(_, b)
        | MathNode::FColorBox(_, _, b)
        | MathNode::Phantom(_, b) => class_of(b),
        MathNode::Row(v) if v.len() == 1 => class_of(&v[0]),
        MathNode::Row(_) => Some(AtomKind::Ord),
        MathNode::Space(_)
        | MathNode::Strut(_, _)
        | MathNode::Label(_)
        | MathNode::NoNumber
        | MathNode::Hline => None,
    }
}

fn single_glyph(name: &str) -> Option<char> {
    let e = lookup(name)?;
    let mut chars = e.glyph.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// TeX's math-mode glyph for a bare source character.
///
/// `-` in math mode is `\mathchar"2200`, the minus sign, not the text hyphen.
/// The atom keeps its class, so binary-operator spacing is unchanged; `-`
/// inside `\text{...}` arrives as [`MathNode::Text`] and is not mapped.
fn math_char(c: char) -> char {
    match c {
        '-' => '\u{2212}',
        other => other,
    }
}

/// TeX's default math-italic face for a variable letter.
///
/// Latin letters and lowercase Greek (with its variant forms) are drawn from
/// the Mathematical Italic block, as plain TeX's `\fam1` does. Uppercase
/// Greek, digits and everything else stay upright. Explicit font commands
/// (`\mathrm`, `\text`, ...) arrive as [`MathNode::Text`] and bypass this.
fn math_italic(c: char) -> char {
    let lower_greek = ('\u{03B1}'..='\u{03C9}').contains(&c);
    if c.is_ascii_alphabetic() || lower_greek || matches!(c, 'ϵ' | 'ϑ' | 'ϰ' | 'ϕ' | 'ϱ' | 'ϖ')
    {
        styled_char(c, TextStyle::It)
    } else {
        c
    }
}

fn symbol_char(name: &str) -> Result<char, Error> {
    single_glyph(name).ok_or_else(|| Error::Unsupported {
        what: format!("symbol \\{name}"),
    })
}

fn symbol_class(name: &str) -> AtomKind {
    symbol_atom_kind(name)
}

fn named_delim(n: &str) -> Result<char, Error> {
    let c = match n {
        "{" => '{',
        "}" => '}',
        "|" | "Vert" | "lVert" | "rVert" => '‖',
        "vert" | "lvert" | "rvert" => '|',
        "langle" => '⟨',
        "rangle" => '⟩',
        "lfloor" => '⌊',
        "rfloor" => '⌋',
        "lceil" => '⌈',
        "rceil" => '⌉',
        "backslash" => '\\',
        "uparrow" => '↑',
        "downarrow" => '↓',
        "Uparrow" => '⇑',
        "Downarrow" => '⇓',
        "updownarrow" => '↕',
        "Updownarrow" => '⇕',
        other => {
            return Err(Error::Unsupported {
                what: format!("delimiter {other}"),
            });
        }
    };
    Ok(c)
}

fn matrix_delims(s: MatrixStyle) -> (Option<char>, Option<char>) {
    match s {
        MatrixStyle::Pmatrix => (Some('('), Some(')')),
        MatrixStyle::Bmatrix => (Some('['), Some(']')),
        MatrixStyle::Vmatrix => (Some('|'), Some('|')),
        MatrixStyle::VVmatrix => (Some('‖'), Some('‖')),
        MatrixStyle::BBmatrix => (Some('{'), Some('}')),
        MatrixStyle::Cases => (Some('{'), None),
        _ => (None, None),
    }
}
