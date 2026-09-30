//! Typed math AST produced by the parser.

use crate::color::Color;
use crate::dim::Dim;

/// TeX math atom class (Appendix G).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomKind {
    /// Ordinary (`x`, `1`, `\alpha` as a letter-like glyph).
    Ord,
    /// Large operator class.
    Op,
    /// Binary operator (`+`, `\times`).
    Bin,
    /// Relation (`=`, `\leq`).
    Rel,
    /// Opening delimiter.
    Open,
    /// Closing delimiter.
    Close,
    /// Punctuation (`,`).
    Punct,
    /// Inner (fraction-like).
    Inner,
}

impl AtomKind {
    fn gold(self) -> &'static str {
        match self {
            Self::Ord => "Ord",
            Self::Op => "Op",
            Self::Bin => "Bin",
            Self::Rel => "Rel",
            Self::Open => "Open",
            Self::Close => "Close",
            Self::Punct => "Punct",
            Self::Inner => "Inner",
        }
    }
}

/// Accent or decoration applied to a nucleus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccentKind {
    /// `\hat`
    Hat,
    /// `\check`
    Check,
    /// `\breve`
    Breve,
    /// `\acute`
    Acute,
    /// `\grave`
    Grave,
    /// `\tilde`
    Tilde,
    /// `\bar`
    Bar,
    /// `\vec`
    Vec,
    /// `\dot`
    Dot,
    /// `\ddot`
    Ddot,
    /// `\dddot`
    Dddot,
    /// `\ddddot`
    Ddddot,
    /// `\widehat`
    WideHat,
    /// `\widetilde`
    WideTilde,
    /// `\overline`
    Overline,
    /// `\underline`
    Underline,
    /// `\overbrace`
    Overbrace,
    /// `\underbrace`
    Underbrace,
    /// `\overleftarrow`
    Overleftarrow,
    /// `\overrightarrow`
    Overrightarrow,
    /// `\overleftrightarrow`
    Overleftrightarrow,
    /// `\underleftarrow`
    Underleftarrow,
    /// `\underrightarrow`
    Underrightarrow,
    /// `\underleftrightarrow`
    Underleftrightarrow,
    /// `\cancel`
    Cancel,
    /// `\bcancel`
    BCancel,
    /// `\xcancel`
    XCancel,
    /// `\boxed`
    Boxed,
    /// `\mathring`
    Ring,
    /// `\not`
    Not,
}

impl AccentKind {
    pub(crate) fn gold(self) -> &'static str {
        match self {
            Self::Hat => "hat",
            Self::Check => "check",
            Self::Breve => "breve",
            Self::Acute => "acute",
            Self::Grave => "grave",
            Self::Tilde => "tilde",
            Self::Bar => "bar",
            Self::Vec => "vec",
            Self::Dot => "dot",
            Self::Ddot => "ddot",
            Self::Dddot => "dddot",
            Self::Ddddot => "ddddot",
            Self::WideHat => "widehat",
            Self::WideTilde => "widetilde",
            Self::Overline => "overline",
            Self::Underline => "underline",
            Self::Overbrace => "overbrace",
            Self::Underbrace => "underbrace",
            Self::Overleftarrow => "overleftarrow",
            Self::Overrightarrow => "overrightarrow",
            Self::Overleftrightarrow => "overleftrightarrow",
            Self::Underleftarrow => "underleftarrow",
            Self::Underrightarrow => "underrightarrow",
            Self::Underleftrightarrow => "underleftrightarrow",
            Self::Cancel => "cancel",
            Self::BCancel => "bcancel",
            Self::XCancel => "xcancel",
            Self::Boxed => "boxed",
            Self::Ring => "mathring",
            Self::Not => "not",
        }
    }
}

/// Font / text style for a run of characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    /// `\mathrm`
    Rm,
    /// `\mathbf`
    Bf,
    /// `\mathit`
    It,
    /// `\mathsf`
    Sf,
    /// `\mathtt`
    Tt,
    /// `\mathbb`
    Bb,
    /// `\mathcal`
    Cal,
    /// `\mathfrak`
    Frak,
    /// `\mathscr`
    Scr,
    /// `\boldsymbol`
    Boldsymbol,
    /// `\pmb`
    Pmb,
    /// `\text`
    Text,
}

impl TextStyle {
    fn gold(self) -> &'static str {
        match self {
            Self::Rm => "rm",
            Self::Bf => "bf",
            Self::It => "it",
            Self::Sf => "sf",
            Self::Tt => "tt",
            Self::Bb => "bb",
            Self::Cal => "cal",
            Self::Frak => "frak",
            Self::Scr => "scr",
            Self::Boldsymbol => "boldsymbol",
            Self::Pmb => "pmb",
            Self::Text => "text",
        }
    }
}

/// Horizontal skip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpaceKind {
    /// `\,`
    Thin,
    /// `\:` or `\>`
    Medium,
    /// `\;`
    Thick,
    /// `\!`
    NegThin,
    /// `\quad`
    Quad,
    /// `\qquad`
    Qquad,
    /// `\ ` (control space)
    ControlSpace,
    /// `\hspace{...}` in em.
    Hspace(Dim),
}

/// Matrix / alignment environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixStyle {
    /// `{matrix}`
    Matrix,
    /// `{pmatrix}`
    Pmatrix,
    /// `{bmatrix}`
    Bmatrix,
    /// `{vmatrix}`
    Vmatrix,
    /// `{Vmatrix}`
    VVmatrix,
    /// `{Bmatrix}`
    BBmatrix,
    /// `{cases}`
    Cases,
    /// `{array}`
    Array,
    /// `{aligned}`
    Aligned,
    /// `{align}`
    Align,
    /// `{gather}`
    Gather,
    /// `{multline}`
    Multline,
    /// `{equation}`
    Equation,
    /// `{split}`
    Split,
}

impl MatrixStyle {
    fn gold(self) -> &'static str {
        match self {
            Self::Matrix => "matrix",
            Self::Pmatrix => "pmatrix",
            Self::Bmatrix => "bmatrix",
            Self::Vmatrix => "vmatrix",
            Self::VVmatrix => "Vmatrix",
            Self::BBmatrix => "Bmatrix",
            Self::Cases => "cases",
            Self::Array => "array",
            Self::Aligned => "aligned",
            Self::Align => "align",
            Self::Gather => "gather",
            Self::Multline => "multline",
            Self::Equation => "equation",
            Self::Split => "split",
        }
    }

    /// `align` / `gather` / `multline` / `equation` take display style and numbers.
    #[must_use]
    pub fn is_display_env(self) -> bool {
        matches!(
            self,
            Self::Align | Self::Gather | Self::Multline | Self::Equation
        )
    }

    /// Rows are numbered unless `\nonumber` / `\notag` / `\tag` says otherwise.
    #[must_use]
    pub fn numbers_rows(self) -> bool {
        matches!(self, Self::Align | Self::Gather)
    }

    /// One equation number for the whole environment (last line for `multline`).
    #[must_use]
    pub fn numbers_once(self) -> bool {
        matches!(self, Self::Equation | Self::Multline)
    }
}

/// One column of an `{array}` preamble (`l`, `c`, `r`, `|`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColSpec {
    /// `l`
    Left,
    /// `c`
    Center,
    /// `r`
    Right,
    /// `|`
    VRule,
}

impl ColSpec {
    fn gold(self) -> char {
        match self {
            Self::Left => 'l',
            Self::Center => 'c',
            Self::Right => 'r',
            Self::VRule => '|',
        }
    }

    /// True for `|`.
    #[must_use]
    pub fn is_rule(self) -> bool {
        matches!(self, Self::VRule)
    }
}

/// Per-row equation number in numbered environments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EqNumber {
    /// Auto number when the environment numbers rows; none otherwise.
    Default,
    /// `\nonumber` / `\notag`
    Suppress,
    /// `\tag{...}` or `\tag*{...}`
    Tag {
        /// `\tag*` — no parentheses around the tag.
        star: bool,
        /// Tag body (text style at layout).
        body: Box<MathNode>,
    },
}

/// One row of a matrix / alignment environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvRow {
    /// Alignment cells (`&`-separated).
    Cells {
        /// Column entries.
        cells: Vec<MathNode>,
        /// Number / tag for this row.
        number: EqNumber,
        /// `\label{...}` keys bound to this row's number.
        labels: Vec<String>,
    },
    /// `\hline`
    Hline,
    /// `\intertext{...}`
    Intertext(Box<MathNode>),
}

impl EnvRow {
    /// A data row with default numbering and no labels.
    #[must_use]
    pub fn cells(cells: Vec<MathNode>) -> Self {
        Self::Cells {
            cells,
            number: EqNumber::Default,
            labels: Vec::new(),
        }
    }

    fn gold(&self) -> String {
        match self {
            Self::Hline => "(hline)".into(),
            Self::Intertext(n) => format!("(intertext {})", n.gold()),
            Self::Cells {
                cells,
                number,
                labels,
            } => {
                let mut s = String::from("(");
                for (i, c) in cells.iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    s.push_str(&c.gold());
                }
                match number {
                    EqNumber::Default => {}
                    EqNumber::Suppress => s.push_str(" (nonumber)"),
                    EqNumber::Tag { star: false, body } => {
                        s.push_str(&format!(" (tag {})", body.gold()));
                    }
                    EqNumber::Tag { star: true, body } => {
                        s.push_str(&format!(" (tagstar {})", body.gold()));
                    }
                }
                for lab in labels {
                    s.push_str(&format!(" (label {lab})"));
                }
                s.push(')');
                s
            }
        }
    }
}

/// Which integral glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntegralKind {
    /// `\int`
    Int,
    /// `\iint`
    Iint,
    /// `\iiint`
    Iiint,
    /// `\oint`
    Oint,
    /// `\oiint`
    Oiint,
}

impl IntegralKind {
    fn gold(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Iint => "iint",
            Self::Iiint => "iiint",
            Self::Oint => "oint",
            Self::Oiint => "oiint",
        }
    }
}

/// `\phantom` / `\vphantom` / `\hphantom`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhantomKind {
    /// `\phantom`
    Full,
    /// `\vphantom`
    Vertical,
    /// `\hphantom`
    Horizontal,
}

impl PhantomKind {
    fn gold(self) -> &'static str {
        match self {
            Self::Full => "phantom",
            Self::Vertical => "vphantom",
            Self::Horizontal => "hphantom",
        }
    }
}

/// A `\left` / `\right` delimiter (or `.` for empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delimiter {
    /// `\left.` / `\right.`
    Empty,
    /// Literal character (`(`, `[`, `|`).
    Char(char),
    /// Named delimiter (`langle`, `{` from `\{`).
    Named(String),
}

impl Delimiter {
    fn gold(&self) -> String {
        match self {
            Self::Empty => ".".into(),
            Self::Char(c) => c.to_string(),
            Self::Named(n) => {
                if n == "{" || n == "}" || n == "|" {
                    format!("\\{n}")
                } else {
                    n.clone()
                }
            }
        }
    }
}

/// `\big` / `\Big` / `\bigg` / `\Bigg` (and `l`/`r`/`m` siblings).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelimSize {
    /// `\big` — about 1.2 em.
    Big,
    /// `\Big` — about 1.8 em.
    Big2,
    /// `\bigg` — about 2.4 em.
    Bigg,
    /// `\Bigg` — about 3.0 em.
    Bigg2,
}

impl DelimSize {
    fn gold(self) -> &'static str {
        match self {
            Self::Big => "big",
            Self::Big2 => "Big",
            Self::Bigg => "bigg",
            Self::Bigg2 => "Bigg",
        }
    }

    /// Map a control sequence to a size. `None` if it is not a `\big` family command.
    #[must_use]
    pub fn from_command(name: &str) -> Option<Self> {
        match name {
            "big" | "bigl" | "bigr" | "bigm" => Some(Self::Big),
            "Big" | "Bigl" | "Bigr" | "Bigm" => Some(Self::Big2),
            "bigg" | "biggl" | "biggr" | "biggm" => Some(Self::Bigg),
            "Bigg" | "Biggl" | "Biggr" | "Biggm" => Some(Self::Bigg2),
            _ => None,
        }
    }

    /// Open / Close / Rel from the `l` / `r` / `m` suffix; `None` for unsuffixed `\big`.
    #[must_use]
    pub fn class_from_command(name: &str) -> Option<AtomKind> {
        if name.ends_with('l') {
            Some(AtomKind::Open)
        } else if name.ends_with('r') {
            Some(AtomKind::Close)
        } else if name.ends_with('m') {
            Some(AtomKind::Rel)
        } else {
            None
        }
    }
}

/// Typed math-mode syntax tree.
///
/// Produced by [`crate::parse()`]. [`MathNode::gold`] is the gold-stable debug form.
///
/// # Examples
///
/// ```
/// use latex_rust::parse;
///
/// let n = parse("x^2").unwrap();
/// assert!(n.gold().contains("sup"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MathNode {
    /// Single character with a TeX atom class.
    Atom(char, AtomKind),
    /// `\frac` / `\dfrac` / `\tfrac` / `\cfrac` / `{a \over b}`.
    Fraction(Box<MathNode>, Box<MathNode>),
    /// `\sqrt` / `\sqrt[n]`. Index `None` is a square root.
    Radical(Option<Box<MathNode>>, Box<MathNode>),
    /// `x^{}`
    Superscript(Box<MathNode>, Box<MathNode>),
    /// `x_{}`
    Subscript(Box<MathNode>, Box<MathNode>),
    /// `x_{}^{}`
    SubSup(Box<MathNode>, Box<MathNode>, Box<MathNode>),
    /// `\left ... \right`
    Delimited(Delimiter, Box<MathNode>, Delimiter),
    /// `\big` / `\Big` / `\bigg` / `\Bigg` (and `l`/`r`/`m` forms).
    SizedDelim(Delimiter, DelimSize, AtomKind),
    /// Horizontal list of nodes.
    Row(Vec<MathNode>),
    /// Matrix or multiline environment. `colspec` is the `{array}` preamble (empty otherwise).
    Matrix(MatrixStyle, Vec<ColSpec>, Vec<EnvRow>),
    /// `\substack{...}` — stacked script-style lines.
    Substack(Vec<MathNode>),
    /// `\ref{key}`
    Ref(String),
    /// `\tag{...}` / `\tag*{...}` (peeled into [`EnvRow`] when inside an environment).
    Tag {
        /// `\tag*`
        star: bool,
        /// Tag body.
        body: Box<MathNode>,
    },
    /// `\label{key}`
    Label(String),
    /// `\nonumber` / `\notag`
    NoNumber,
    /// `\hline` (peeled into [`EnvRow::Hline`] in environments).
    Hline,
    /// `\intertext{...}`
    Intertext(Box<MathNode>),
    /// `\sum` with optional lower / upper limits.
    Sum(Option<Box<MathNode>>, Option<Box<MathNode>>),
    /// `\int` family with optional limits.
    Integral(IntegralKind, Option<Box<MathNode>>, Option<Box<MathNode>>),
    /// `\prod` with optional limits.
    Product(Option<Box<MathNode>>, Option<Box<MathNode>>),
    /// `\lim` with optional subscript.
    Limit(Option<Box<MathNode>>),
    /// `\overset` / `\underset` / `\stackrel` (base, over, under).
    OverUnder(Box<MathNode>, Option<Box<MathNode>>, Option<Box<MathNode>>),
    /// Accent or decoration on a nucleus.
    Accent(Box<MathNode>, AccentKind),
    /// `\cancelto{value}{expr}`
    CancelTo(Box<MathNode>, Box<MathNode>),
    /// Styled character run (`\mathrm`, `\text`, …).
    Text(String, TextStyle),
    /// Explicit math skip.
    Space(SpaceKind),
    /// Named operator (`\sin`). The flag is `\limits` vs `\nolimits`.
    Operator(String, bool),
    /// Named glyph (`\alpha`, `\times`). The string is the control-sequence name.
    Symbol(String),
    /// `\color` applied to a following math list.
    Color(Color, Box<MathNode>),
    /// `\textcolor{...}{...}`
    TextColor(Color, Box<MathNode>),
    /// `\colorbox{...}{...}`
    ColorBox(Color, Box<MathNode>),
    /// `\fcolorbox{border}{fill}{body}`
    FColorBox(Color, Color, Box<MathNode>),
    /// Vertical strut (height, depth) in em.
    Strut(Dim, Dim),
    /// `\phantom` family.
    Phantom(PhantomKind, Box<MathNode>),
}

impl MathNode {
    /// Gold-stable S-expression. One space between items; no trailing space.
    #[must_use]
    pub fn gold(&self) -> String {
        match self {
            Self::Atom(c, k) => format!("(atom {} {})", k.gold(), quote_atom(*c)),
            Self::Fraction(n, d) => format!("(frac {} {})", n.gold(), d.gold()),
            Self::Radical(None, r) => format!("(sqrt {})", r.gold()),
            Self::Radical(Some(i), r) => format!("(sqrtn {} {})", i.gold(), r.gold()),
            Self::Superscript(b, e) => format!("(sup {} {})", b.gold(), e.gold()),
            Self::Subscript(b, s) => format!("(sub {} {})", b.gold(), s.gold()),
            Self::SubSup(b, s, e) => format!("(subsup {} {} {})", b.gold(), s.gold(), e.gold()),
            Self::Delimited(l, b, r) => {
                format!("(delim {} {} {})", l.gold(), b.gold(), r.gold())
            }
            Self::SizedDelim(d, sz, k) => {
                format!("(big {} {} {})", sz.gold(), k.gold(), quote_delim(d))
            }
            Self::Row(items) => {
                if items.is_empty() {
                    "(row)".into()
                } else {
                    let mut s = String::from("(row");
                    for it in items {
                        s.push(' ');
                        s.push_str(&it.gold());
                    }
                    s.push(')');
                    s
                }
            }
            Self::Matrix(style, spec, rows) => {
                let mut s = format!("(matrix {}", style.gold());
                if !spec.is_empty() {
                    s.push(' ');
                    for c in spec {
                        s.push(c.gold());
                    }
                }
                for row in rows {
                    s.push(' ');
                    s.push_str(&row.gold());
                }
                s.push(')');
                s
            }
            Self::Substack(lines) => {
                let mut s = String::from("(substack");
                for ln in lines {
                    s.push(' ');
                    s.push_str(&ln.gold());
                }
                s.push(')');
                s
            }
            Self::Ref(k) => format!("(ref {k})"),
            Self::Tag { star: false, body } => format!("(tag {})", body.gold()),
            Self::Tag { star: true, body } => format!("(tagstar {})", body.gold()),
            Self::Label(k) => format!("(label {k})"),
            Self::NoNumber => "(nonumber)".into(),
            Self::Hline => "(hline)".into(),
            Self::Intertext(n) => format!("(intertext {})", n.gold()),
            Self::Sum(lo, hi) => format!("(sum {} {})", opt(lo), opt(hi)),
            Self::Integral(k, lo, hi) => {
                format!("({} {} {})", k.gold(), opt(lo), opt(hi))
            }
            Self::Product(lo, hi) => format!("(prod {} {})", opt(lo), opt(hi)),
            Self::Limit(lo) => format!("(lim {})", opt(lo)),
            Self::OverUnder(b, over, under) => {
                format!("(overunder {} {} {})", b.gold(), opt(over), opt(under))
            }
            Self::Accent(b, a) => format!("(accent {} {})", a.gold(), b.gold()),
            Self::CancelTo(v, e) => format!("(cancelto {} {})", v.gold(), e.gold()),
            Self::Text(t, st) => format!("(text {} {})", st.gold(), quote_text(t)),
            Self::Space(SpaceKind::Thin) => "(space thin)".into(),
            Self::Space(SpaceKind::Medium) => "(space medium)".into(),
            Self::Space(SpaceKind::Thick) => "(space thick)".into(),
            Self::Space(SpaceKind::NegThin) => "(space negthin)".into(),
            Self::Space(SpaceKind::Quad) => "(space quad)".into(),
            Self::Space(SpaceKind::Qquad) => "(space qquad)".into(),
            Self::Space(SpaceKind::ControlSpace) => "(space control)".into(),
            Self::Space(SpaceKind::Hspace(d)) => format!("(space hspace {})", dim_gold(d)),
            Self::Operator(name, false) => format!("(op {name})"),
            Self::Operator(name, true) => format!("(op {name} limits)"),
            Self::Symbol(name) => format!("(symbol {name})"),
            Self::Color(c, b) => format!("(color {} {})", c.css_hex(), b.gold()),
            Self::TextColor(c, b) => format!("(textcolor {} {})", c.css_hex(), b.gold()),
            Self::ColorBox(c, b) => format!("(colorbox {} {})", c.css_hex(), b.gold()),
            Self::FColorBox(border, fill, b) => {
                format!(
                    "(fcolorbox {} {} {})",
                    border.css_hex(),
                    fill.css_hex(),
                    b.gold()
                )
            }
            Self::Strut(h, d) => format!("(strut {} {})", dim_gold(h), dim_gold(d)),
            Self::Phantom(k, b) => format!("({} {})", k.gold(), b.gold()),
        }
    }
}

fn opt(n: &Option<Box<MathNode>>) -> String {
    match n {
        None => "_".into(),
        Some(x) => x.gold(),
    }
}

fn quote_delim(d: &Delimiter) -> String {
    match d {
        Delimiter::Empty => ".".into(),
        Delimiter::Char(c) => quote_atom(*c),
        Delimiter::Named(n) => n.clone(),
    }
}

fn quote_atom(c: char) -> String {
    match c {
        '"' => "'\"'".into(),
        '\'' => "\"'\"".into(),
        _ => format!("\"{c}\""),
    }
}

fn quote_text(t: &str) -> String {
    format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
}

fn dim_gold(d: &Dim) -> String {
    const RATIOS: [(i64, i64); 10] = [
        (0, 1),
        (1, 1),
        (2, 1),
        (1, 2),
        (1, 18),
        (2, 18),
        (3, 18),
        (7, 10),
        (3, 10),
        (1, 10),
    ];
    for (n, den) in RATIOS {
        if d.eq_dim(&Dim::ratio(n, den)) {
            if den == 1 {
                return n.to_string();
            }
            return format!("{n}/{den}");
        }
    }
    for i in -64i64..65 {
        if d.eq_dim(&Dim::from_i64(i)) {
            return i.to_string();
        }
    }
    d.to_dec_string()
}
