//! OpenType math font metrics. Integer font units → [`Dim`](crate::Dim).

use ttf_parser::Face;

use crate::dim::Dim;
use crate::error::{Error, FontError};

/// Embedded STIX Two Math Regular 2.13 (SIL OFL 1.1).
///
/// A `static` rather than a `const`, so that the font's bytes are placed in the binary once and every use refers
/// to that one copy. A `const` is inlined at each use site, and a crate that reads these bytes as well as calling
/// [`MathFont::stix_two_math`] carries the 839 KB font twice.
pub static STIX_TWO_MATH_OTF: &[u8] =
    include_bytes!("../../fonts/stix-two-math/STIXTwoMath-Regular.otf");

/// SHA-256 (hex) of [`STIX_TWO_MATH_OTF`]. Locked by gold.
pub const STIX_TWO_MATH_SHA256: &str =
    "f2076b9f1676438439dd41e23676f5ab99056e83d6b8f8c27841591ef2ccfa72";

/// Face name as shipped.
pub const STIX_TWO_MATH_NAME: &str = "STIX Two Math";

/// Horizontal glyph metrics in font units and em.
///
/// # Examples
///
/// ```
/// use latex_rust::MathFont;
///
/// let font = MathFont::stix_two_math().unwrap();
/// let g = font.glyph('x').unwrap();
/// assert_eq!(g.ch, 'x');
/// assert!(!g.advance.is_zero());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphMetrics {
    /// Character requested.
    pub ch: char,
    /// OpenType glyph id.
    pub glyph_id: u16,
    /// Horizontal advance, font units.
    pub advance_fu: u16,
    /// Advance in em.
    pub advance: Dim,
    /// Height above baseline in em (`max(y_max, 0)`).
    pub height: Dim,
    /// Depth below baseline in em (`max(-y_min, 0)`).
    pub depth: Dim,
}

/// Loaded math face.
///
/// # Examples
///
/// ```
/// use latex_rust::MathFont;
///
/// let font = MathFont::stix_two_math().unwrap();
/// assert_eq!(font.units_per_em(), 1000);
/// ```
pub struct MathFont {
    raw: &'static [u8],
    face: Face<'static>,
    fallback: Option<Face<'static>>,
    units_per_em: u16,
    ascender_fu: i16,
    descender_fu: i16,
}

/// 回退字形的 glyph-id 标记位：`MathFont` 设了回退字体时，回退 face 的
/// glyph id 记上这一位，光栅化据此换用回退 face。两个 face 的字形数都
/// 必须低于这个值。
pub const FALLBACK_GLYPH_FLAG: u16 = 0x8000;

/// `glyph_id` 是否来自回退字体（STIX 缺字形时用来兜 CJK 的那支）。
#[must_use]
pub fn is_fallback_glyph(glyph_id: u16) -> bool {
    glyph_id & FALLBACK_GLYPH_FLAG != 0
}

impl MathFont {
    /// Load the embedded STIX Two Math Regular face.
    ///
    /// # Errors
    ///
    /// [`crate::FontError::InvalidFace`] if the embedded bytes are not a usable OpenType face.
    ///
    /// # Examples
    ///
    /// ```
    /// use latex_rust::MathFont;
    /// assert!(MathFont::stix_two_math().is_ok());
    /// ```
    pub fn stix_two_math() -> Result<Self, Error> {
        Self::from_bytes(STIX_TWO_MATH_OTF)
    }

    /// Load the embedded STIX Two Math Regular face with a fallback face that
    /// is consulted for characters STIX lacks (CJK, for example). The
    /// fallback face needs no MATH table: only its glyph advances and
    /// bounding boxes are read, accents and glyph variants simply do not
    /// apply to its glyphs.
    ///
    /// # Errors
    ///
    /// [`crate::FontError::InvalidFace`] if either buffer is not a usable
    /// OpenType face, or a face has too many glyphs for the fallback
    /// glyph-id flag.
    pub fn stix_two_math_with_fallback(fallback: &'static [u8]) -> Result<Self, Error> {
        Self::from_bytes_with_fallback(STIX_TWO_MATH_OTF, Some(fallback))
    }

    /// Parse OpenType bytes. Lifetime is `'static` for the embedded font only;
    /// this constructor requires a static buffer so the face can be rebuilt.
    pub fn from_bytes(raw: &'static [u8]) -> Result<Self, Error> {
        Self::from_bytes_with_fallback(raw, None)
    }

    /// Parse OpenType bytes with an optional fallback face, see
    /// [`Self::stix_two_math_with_fallback`].
    ///
    /// # Errors
    ///
    /// [`crate::FontError::InvalidFace`] on unusable bytes or too many glyphs.
    pub fn from_bytes_with_fallback(
        raw: &'static [u8],
        fallback: Option<&'static [u8]>,
    ) -> Result<Self, Error> {
        let face = Face::parse(raw, 0).map_err(|_| FontError::InvalidFace)?;
        let units_per_em = face.units_per_em();
        if units_per_em == 0 {
            return Err(FontError::InvalidFace.into());
        }
        let fallback = fallback
            .map(|bytes| Face::parse(bytes, 0).map_err(|_| FontError::InvalidFace))
            .transpose()?;
        if face.number_of_glyphs() >= FALLBACK_GLYPH_FLAG
            || fallback
                .as_ref()
                .is_some_and(|fb| fb.number_of_glyphs() >= FALLBACK_GLYPH_FLAG)
        {
            return Err(FontError::InvalidFace.into());
        }
        let ascender_fu = face.ascender();
        let descender_fu = face.descender();
        Ok(Self {
            raw,
            face,
            fallback,
            units_per_em,
            ascender_fu,
            descender_fu,
        })
    }

    /// 回退 face（主 face 缺字形时兜底用），未设置时为 None。
    #[must_use]
    pub fn fallback_face(&self) -> Option<&Face<'static>> {
        self.fallback.as_ref()
    }

    /// The parsed OpenType face.
    ///
    /// An external render backend needs glyph outlines and bounding boxes,
    /// which this crate does not otherwise expose. Reaching the face here
    /// rather than re-parsing [`Self::bytes`] guarantees that the glyph ids in
    /// [`BoxContent::Glyph`](crate::BoxContent::Glyph) are resolved against the
    /// same face, parsed by the same version of `ttf-parser`, that produced
    /// them. The crate re-exports [`ttf_parser`] so that a
    /// consumer can name this type without pinning the version itself.
    ///
    /// # Examples
    ///
    /// ```
    /// use latex_rust::{ttf_parser, MathFont};
    ///
    /// let font = MathFont::stix_two_math().expect("STIX Two Math");
    /// let metrics = font.glyph('x').expect("x");
    /// let id = ttf_parser::GlyphId(metrics.glyph_id);
    /// assert!(font.face().glyph_bounding_box(id).is_some());
    /// ```
    #[must_use]
    pub fn face(&self) -> &Face<'static> {
        &self.face
    }

    /// OpenType bytes this face was parsed from.
    #[must_use]
    pub fn bytes(&self) -> &'static [u8] {
        self.raw
    }

    /// `unitsPerEm` from the `head` table.
    #[must_use]
    pub fn units_per_em(&self) -> u16 {
        self.units_per_em
    }

    /// `hhea` ascender in font units.
    #[must_use]
    pub fn ascender_fu(&self) -> i16 {
        self.ascender_fu
    }

    /// `hhea` descender in font units (typically negative).
    #[must_use]
    pub fn descender_fu(&self) -> i16 {
        self.descender_fu
    }

    /// Ascender in em.
    #[must_use]
    pub fn ascender(&self) -> Dim {
        Dim::from_font_units(i64::from(self.ascender_fu), self.units_per_em)
    }

    /// Depth below baseline from `hhea` descender, in em (non-negative).
    #[must_use]
    pub fn descender(&self) -> Dim {
        let d = i64::from(self.descender_fu);
        Dim::from_font_units(-d, self.units_per_em)
    }

    /// Metrics for `ch`, or [`FontError::MissingGlyph`].
    ///
    /// 主 face 没有这个字时查回退 face：回退字形的度量按回退 face 的
    /// unitsPerEm 折算成 em，glyph id 记上 [`FALLBACK_GLYPH_FLAG`]。
    pub fn glyph(&self, ch: char) -> Result<GlyphMetrics, Error> {
        if let Some(gid) = self.face.glyph_index(ch) {
            return self.metrics(ch, gid.0, &self.face, self.units_per_em);
        }
        if let Some(fb) = &self.fallback {
            if let Some(gid) = fb.glyph_index(ch) {
                return self.metrics(ch, gid.0 | FALLBACK_GLYPH_FLAG, fb, fb.units_per_em());
            }
        }
        Err(FontError::MissingGlyph { ch }.into())
    }

    /// `face` 上 `gid`（已带标记位）的度量，font units 与 em 两套。
    fn metrics(
        &self,
        ch: char,
        glyph_id: u16,
        face: &Face<'static>,
        upem: u16,
    ) -> Result<GlyphMetrics, Error> {
        let gid = ttf_parser::GlyphId(glyph_id & !FALLBACK_GLYPH_FLAG);
        let advance_fu = face
            .glyph_hor_advance(gid)
            .ok_or(FontError::MissingGlyph { ch })?;
        let mut height_fu = 0i64;
        let mut depth_fu = 0i64;
        if let Some(bbox) = face.glyph_bounding_box(gid) {
            height_fu = i64::from(bbox.y_max).max(0);
            depth_fu = i64::from(-bbox.y_min).max(0);
        }
        Ok(GlyphMetrics {
            ch,
            glyph_id,
            advance_fu,
            advance: Dim::from_font_units(i64::from(advance_fu), upem),
            height: Dim::from_font_units(height_fu, upem),
            depth: Dim::from_font_units(depth_fu, upem),
        })
    }

    /// Metrics for OpenType glyph id `gid`, tagged with `ch` for the box payload.
    pub fn glyph_id(&self, ch: char, gid: u16) -> Result<GlyphMetrics, Error> {
        let face = self.face();
        let gid = ttf_parser::GlyphId(gid);
        let advance_fu = face
            .glyph_hor_advance(gid)
            .ok_or(FontError::MissingGlyph { ch })?;
        let mut height_fu = 0i64;
        let mut depth_fu = 0i64;
        if let Some(bbox) = face.glyph_bounding_box(gid) {
            height_fu = i64::from(bbox.y_max).max(0);
            depth_fu = i64::from(-bbox.y_min).max(0);
        }
        let upem = self.units_per_em;
        Ok(GlyphMetrics {
            ch,
            glyph_id: gid.0,
            advance_fu,
            advance: Dim::from_font_units(i64::from(advance_fu), upem),
            height: Dim::from_font_units(height_fu, upem),
            depth: Dim::from_font_units(depth_fu, upem),
        })
    }

    /// MATH italic correction for `glyph_id`, or zero.
    pub fn italic_correction(&self, glyph_id: u16) -> Dim {
        if is_fallback_glyph(glyph_id) {
            return Dim::zero();
        }
        let face = self.face();
        let Some(math) = face.tables().math else {
            return Dim::zero();
        };
        let Some(info) = math.glyph_info else {
            return Dim::zero();
        };
        let Some(table) = info.italic_corrections else {
            return Dim::zero();
        };
        match table.get(ttf_parser::GlyphId(glyph_id)) {
            Some(v) => Dim::from_font_units(i64::from(v.value), self.units_per_em),
            None => Dim::zero(),
        }
    }

    /// MATH top-accent attachment (em from glyph left), if present.
    pub fn top_accent_attachment(&self, glyph_id: u16) -> Option<Dim> {
        if is_fallback_glyph(glyph_id) {
            return None;
        }
        let face = self.face();
        let math = face.tables().math?;
        let info = math.glyph_info?;
        let table = info.top_accent_attachments?;
        let v = table.get(ttf_parser::GlyphId(glyph_id))?;
        Some(Dim::from_font_units(i64::from(v.value), self.units_per_em))
    }

    /// Horizontal glyph-assembly parts: `(gid, start_connector, end_connector, advance, extender)`.
    /// Lengths are font units.
    pub fn horizontal_assembly_parts(&self, glyph_id: u16) -> Vec<(u16, u16, u16, u16, bool)> {
        let mut out = Vec::new();
        if is_fallback_glyph(glyph_id) {
            return out;
        }
        let face = self.face();
        let Some(math) = face.tables().math else {
            return out;
        };
        let Some(variants) = math.variants else {
            return out;
        };
        let Some(cons) = variants
            .horizontal_constructions
            .get(ttf_parser::GlyphId(glyph_id))
        else {
            return out;
        };
        let Some(assembly) = cons.assembly else {
            return out;
        };
        for i in 0..assembly.parts.len() {
            if let Some(p) = assembly.parts.get(i) {
                out.push((
                    p.glyph_id.0,
                    p.start_connector_length,
                    p.end_connector_length,
                    p.full_advance,
                    p.part_flags.extender(),
                ));
            }
        }
        out
    }

    /// Horizontal MATH variants of `glyph_id`, including the base glyph first.
    pub fn horizontal_variants(&self, glyph_id: u16) -> Vec<u16> {
        if is_fallback_glyph(glyph_id) {
            return Vec::new();
        }
        let mut out = vec![glyph_id];
        let face = self.face();
        let Some(math) = face.tables().math else {
            return out;
        };
        let Some(variants) = math.variants else {
            return out;
        };
        let Some(cons) = variants
            .horizontal_constructions
            .get(ttf_parser::GlyphId(glyph_id))
        else {
            return out;
        };
        for i in 0..cons.variants.len() {
            if let Some(v) = cons.variants.get(i) {
                out.push(v.variant_glyph.0);
            }
        }
        out
    }

    /// Vertical MATH variants of `glyph_id`, including the base glyph first.
    pub fn vertical_variants(&self, glyph_id: u16) -> Vec<u16> {
        if is_fallback_glyph(glyph_id) {
            return Vec::new();
        }
        let mut out = vec![glyph_id];
        let face = self.face();
        let Some(math) = face.tables().math else {
            return out;
        };
        let Some(variants) = math.variants else {
            return out;
        };
        let Some(cons) = variants
            .vertical_constructions
            .get(ttf_parser::GlyphId(glyph_id))
        else {
            return out;
        };
        for i in 0..cons.variants.len() {
            if let Some(v) = cons.variants.get(i) {
                out.push(v.variant_glyph.0);
            }
        }
        out
    }

    /// SHA-256 hex of the raw face bytes.
    #[must_use]
    pub fn sha256_hex(bytes: &[u8]) -> String {
        let d = crate::hash::sha256(bytes);
        let mut s = String::with_capacity(64);
        for b in d {
            s.push_str(&hex_byte(b));
        }
        s
    }
}

fn hex_byte(b: u8) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let hi = H[(b >> 4) as usize];
    let lo = H[(b & 0xf) as usize];
    let mut out = String::with_capacity(2);
    out.push(hi as char);
    out.push(lo as char);
    out
}
