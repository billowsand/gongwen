//! TeX-faithful math box model. Dimensions are [`Dim`](crate::Dim).

mod engine;
mod metrics;
mod numbering;
mod space;
mod style;

pub use engine::{layout, layout_with_max_depth, layout_with_numbering};
pub use metrics::MathParams;
pub use numbering::{NumberFormat, NumberStyle, NumberingConfig, NumberingState};
pub use style::MathStyle;

use crate::color::Color;
use crate::dim::Dim;
use crate::error::Error;
use crate::font::MathFont;

/// What a box contains.
///
/// Glyphs carry an OpenType id from the math face. Lists compose children.
/// Color wrappers do not change dimensions. [`BoxContent::Line`] and
/// [`BoxContent::Frame`] are decorations from cancel / boxed constructs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoxContent {
    /// Empty box (strut or placeholder).
    Empty,
    /// Solid rule (fraction bar, vinculum). No glyph.
    Rule,
    /// A single character whose metrics came from the math font.
    ///
    /// Build with [`BoxContent::glyph`]; the variant is non-exhaustive.
    #[non_exhaustive]
    Glyph {
        /// Character.
        ch: char,
        /// OpenType glyph id.
        glyph_id: u16,
        /// Outline scale relative to the em: 1 at text size, smaller at script
        /// and scriptscript size. Box dimensions are already scaled; renderers
        /// draw the outline at this factor.
        scale: Dim,
    },
    /// Horizontal list. Width is the sum of children.
    HList(Vec<MathBox>),
    /// Vertical list. Height/depth stack on the baseline of the first box.
    VList(Vec<MathBox>),
    /// Horizontal kern (atom spacing, `\hspace`).
    Kern(Dim),
    /// Color wrapper. Dimensions match the inner box. Sets SVG `fill`.
    Color(Color, Box<MathBox>),
    /// Background color (`\colorbox`). Inner glyphs keep the default fill.
    BackColor(Color, Box<MathBox>),
    /// Children share the left edge; each child's [`MathBox::shift`] is its baseline.
    Overlap(Vec<MathBox>),
    /// Diagonal or free line in em, relative to the box left and baseline (`y` up).
    Line {
        /// Start x (em from left).
        x1: Dim,
        /// Start y (em above baseline).
        y1: Dim,
        /// End x.
        x2: Dim,
        /// End y.
        y2: Dim,
        /// Stroke thickness (em).
        thickness: Dim,
    },
    /// Stroked rectangle around the inner box. Inner is laid out at the same origin.
    Frame {
        /// Rule thickness (em).
        thickness: Dim,
        /// Border color. `None` inherits the current fill (`\boxed`).
        stroke: Option<Color>,
        /// Contents inside the frame.
        inner: Box<MathBox>,
    },
}

/// TeX-style box: width, height above baseline, depth below, italic correction.
///
/// # Examples
///
/// ```
/// use latex_rust::{Dim, MathBox};
///
/// let packed = MathBox::hpack(vec![
///     MathBox::rule(Dim::one(), Dim::zero(), Dim::zero()),
///     MathBox::rule(Dim::ratio(1, 2), Dim::zero(), Dim::zero()),
/// ]);
/// assert_eq!(packed.width, Dim::ratio(3, 2));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathBox {
    /// Width.
    pub width: Dim,
    /// Height above the baseline.
    pub height: Dim,
    /// Depth below the baseline.
    pub depth: Dim,
    /// Italic correction.
    pub italic: Dim,
    /// Baseline raise relative to the parent list (positive is up).
    pub shift: Dim,
    /// Payload.
    pub content: BoxContent,
}

impl BoxContent {
    /// Glyph content drawn at `scale` times the em (1 for text size).
    #[must_use]
    pub fn glyph(ch: char, glyph_id: u16, scale: Dim) -> Self {
        Self::Glyph {
            ch,
            glyph_id,
            scale,
        }
    }
}

impl MathBox {
    /// Zero-size empty box.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            width: Dim::zero(),
            height: Dim::zero(),
            depth: Dim::zero(),
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::Empty,
        }
    }

    /// Rule with explicit dimensions (fraction bar, strut).
    #[must_use]
    pub fn rule(width: Dim, height: Dim, depth: Dim) -> Self {
        Self {
            width,
            height,
            depth,
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::Rule,
        }
    }

    /// Horizontal kern of `width` (zero height and depth).
    #[must_use]
    pub fn kern(width: Dim) -> Self {
        Self {
            width: width.clone(),
            height: Dim::zero(),
            depth: Dim::zero(),
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::Kern(width),
        }
    }

    /// Box from a font glyph. Errors if the face has no glyph for `ch`.
    pub fn from_glyph(font: &MathFont, ch: char) -> Result<Self, Error> {
        let g = font.glyph(ch)?;
        Ok(Self {
            width: g.advance,
            height: g.height,
            depth: g.depth,
            italic: font.italic_correction(g.glyph_id),
            shift: Dim::zero(),
            content: BoxContent::glyph(ch, g.glyph_id, Dim::one()),
        })
    }

    /// Pack boxes in a row. Width sums; height and depth are maxima.
    #[must_use]
    pub fn hpack(children: Vec<Self>) -> Self {
        let mut width = Dim::zero();
        let mut height = Dim::zero();
        let mut depth = Dim::zero();
        for c in &children {
            width = &width + &c.width;
            height = height.max(&c.height);
            depth = depth.max(&c.depth);
        }
        Self {
            width,
            height,
            depth,
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::HList(children),
        }
    }

    /// Pack boxes in a column, first child on the baseline.
    ///
    /// Subsequent children sit below the previous (height + depth stacked).
    #[must_use]
    pub fn vpack(children: Vec<Self>) -> Self {
        if children.is_empty() {
            return Self::empty();
        }
        let mut width = Dim::zero();
        let height = children[0].height.clone();
        let mut depth = children[0].depth.clone();
        for c in children.iter().skip(1) {
            width = width.max(&c.width);
            depth = &depth + &c.height;
            depth = &depth + &c.depth;
        }
        width = width.max(&children[0].width);
        Self {
            width,
            height,
            depth,
            italic: Dim::zero(),
            shift: Dim::zero(),
            content: BoxContent::VList(children),
        }
    }

    /// Raise this box's baseline by `shift` (positive is up).
    #[must_use]
    pub fn with_shift(mut self, shift: Dim) -> Self {
        self.shift = shift;
        self
    }

    /// Gold-stable width/height/depth decimal string.
    #[must_use]
    pub fn dim_gold(&self) -> String {
        format!(
            "w={} h={} d={}",
            self.width.to_dec_string(),
            self.height.to_dec_string(),
            self.depth.to_dec_string()
        )
    }
}
