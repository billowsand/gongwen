//! LaTeX math string → [`MathNode`].
//!
//! Binding of `_` / `^` / primes is Pratt-style (tight postfix on the nucleus).
//! The math list itself is a TeX-style row of atoms, not an arithmetic tree.

use super::ast::{
    AccentKind, AtomKind, ColSpec, DelimSize, Delimiter, EnvRow, EqNumber, IntegralKind, MathNode,
    MatrixStyle, PhantomKind, SpaceKind, TextStyle,
};
use super::preproc::preprocess;
use super::token::{tokenize, Token};
use crate::color::{parse_color_spec, Color, ColorTable};
use crate::dim::Dim;
use crate::error::{Error, ParseError};
use crate::symbols::{lookup, SymbolKind as CatalogKind};

/// Default deepest nesting [`parse`] and [`layout`](crate::layout()) accept.
///
/// Both recurse over the structure of the input, so their stack use grows with
/// how deeply it nests. Past the limit they return an error rather than
/// consume unbounded stack: input this deep is pathological, and a crate that
/// refuses to invent a render should not abort the process either.
///
/// The count is of parser recursion levels rather than of LaTeX constructs, and
/// a braced argument costs two of them (one for the argument and one for the
/// group), so the default admits `\frac{1}{…}` nested about 15 deep. Real
/// mathematics rarely nests beyond five levels.
///
/// Measured on Linux x86-64 with the deepest input 32 admits in each nesting
/// shape, parsed, laid out, rendered to SVG and dropped: an optimised build
/// survives on a 1 MiB stack (the `wasm32` default) with about three times the
/// headroom needed, and an unoptimised build survives on the 2 MiB stack of a
/// default `std::thread` worker with about twice. An unoptimised build on a
/// 1 MiB stack is *not* covered: layout overflows there at about 14 nested
/// radicals or scripts. A caller that knows it has more stack can raise the
/// limit with [`ParseOptions::with_max_depth`] and
/// [`layout_with_max_depth`](crate::layout_with_max_depth); a caller on a
/// smaller stack should lower it.
pub const DEFAULT_MAX_NESTING_DEPTH: usize = 32;

/// Options for [`parse_with_options`].
///
/// # Examples
///
/// ```
/// use latex_rust::{parse_with_options, ParseOptions};
///
/// let deep = "{".repeat(40) + "x" + &"}".repeat(40);
/// assert!(parse_with_options(&deep, &ParseOptions::new()).is_err());
/// let roomy = ParseOptions::new().with_max_depth(128);
/// assert!(parse_with_options(&deep, &roomy).is_ok());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ParseOptions {
    /// Deepest parser recursion accepted. Defaults to [`DEFAULT_MAX_NESTING_DEPTH`].
    pub max_depth: usize,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_NESTING_DEPTH,
        }
    }
}

impl ParseOptions {
    /// Options with every field at its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the deepest parser recursion accepted.
    ///
    /// Raising it past the default is safe only if the calling thread has the
    /// stack for it; see [`DEFAULT_MAX_NESTING_DEPTH`].
    #[must_use]
    pub fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }
}

/// Parse a LaTeX math string into a [`MathNode`] using a fresh color table.
///
/// Accepts raw math, `$...$`, `$$...$$`, `\(...\)`, or `\[...\]`. Binding of
/// `_` / `^` / primes is Pratt-style (tight postfix on the nucleus).
///
/// # Arguments
///
/// * `input` — math source, with or without delimiter fences.
///
/// # Returns
///
/// The typed AST, or a [`ParseError`] naming the problem.
///
/// # Errors
///
/// * [`ParseError::Unknown`] — command is not in the catalog.
/// * [`ParseError::Unsupported`] — known construct this crate will not invent.
/// * [`ParseError::Malformed`] — syntactically invalid input.
/// * [`ParseError::UnmatchedDelimiter`] — `\left` without `\right` (or vice versa).
/// * [`ParseError::TrailingBackslash`] — input ended with a stray `\`.
///
/// # Examples
///
/// ```
/// use latex_rust::parse;
///
/// let ast = parse(r"\frac{1}{2}").unwrap();
/// assert_eq!(ast.gold(), r#"(frac (atom Ord "1") (atom Ord "2"))"#);
/// ```
pub fn parse(input: &str) -> Result<MathNode, ParseError> {
    parse_with_colors(input).map(|(n, _)| n)
}

/// Parse a math string, returning the AST and the color table after `\definecolor`.
///
/// # Arguments
///
/// * `input` — math source, with or without delimiter fences.
///
/// # Returns
///
/// The AST and the color table including any `\definecolor` names from `input`.
///
/// # Errors
///
/// Same as [`parse`].
///
/// # Examples
///
/// ```
/// use latex_rust::parse_with_colors;
///
/// let (ast, table) = parse_with_colors(r"\definecolor{ok}{named}{red}x").unwrap();
/// assert!(table.get("ok").is_ok());
/// let _ = ast;
/// ```
pub fn parse_with_colors(input: &str) -> Result<(MathNode, ColorTable), ParseError> {
    parse_with_options(input, &ParseOptions::default())
}

/// Parse a math string with explicit [`ParseOptions`], returning the AST and
/// the color table after `\definecolor`.
///
/// # Errors
///
/// Same as [`parse`]. Input nesting deeper than `options.max_depth` returns
/// [`ParseError::Malformed`].
///
/// # Examples
///
/// ```
/// use latex_rust::{parse_with_options, ParseOptions};
///
/// let tight = ParseOptions::new().with_max_depth(4);
/// assert!(parse_with_options(r"\frac{1}{\frac{1}{\frac{1}{2}}}", &tight).is_err());
/// ```
pub fn parse_with_options(
    input: &str,
    options: &ParseOptions,
) -> Result<(MathNode, ColorTable), ParseError> {
    let sanitized = preprocess(input);
    let tokens = tokenize(&sanitized)?;
    let tokens = strip_fences(&tokens)?;
    let mut p = Parser {
        tokens,
        pos: 0,
        depth: 0,
        max_depth: options.max_depth,
        colors: ColorTable::new(),
    };
    let node = p.parse_list(Stop::eof())?;
    p.skip_ws();
    if p.pos < p.tokens.len() {
        return Err(ParseError::Malformed(format!(
            "unexpected leftover token {}",
            p.tokens[p.pos]
        )));
    }
    Ok((node, p.colors))
}

#[derive(Clone, Copy)]
struct Stop {
    end_group: bool,
    amp: bool,
    cr: bool,
    right: bool,
    end_env: bool,
    rbracket: bool,
}

impl Stop {
    fn eof() -> Self {
        Self {
            end_group: false,
            amp: false,
            cr: false,
            right: false,
            end_env: false,
            rbracket: false,
        }
    }

    fn group() -> Self {
        Self {
            end_group: true,
            ..Self::eof()
        }
    }

    fn cell() -> Self {
        Self {
            amp: true,
            cr: true,
            end_env: true,
            ..Self::eof()
        }
    }

    fn delim() -> Self {
        Self {
            right: true,
            ..Self::eof()
        }
    }

    fn index() -> Self {
        Self {
            rbracket: true,
            ..Self::eof()
        }
    }

    fn substack_line() -> Self {
        Self {
            end_group: true,
            cr: true,
            ..Self::eof()
        }
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    colors: ColorTable,
    /// Current nesting depth, bounded by `max_depth`.
    depth: usize,
    /// Deepest nesting accepted, from [`ParseOptions::max_depth`].
    max_depth: usize,
}

impl Parser {
    fn skip_ws(&mut self) {
        while matches!(self.tokens.get(self.pos), Some(Token::Space)) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn peek_ws(&mut self) -> Option<&Token> {
        self.skip_ws();
        self.peek()
    }

    fn bump(&mut self) -> Option<Token> {
        self.skip_ws();
        self.bump_raw()
    }

    fn bump_raw(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned()?;
        self.pos += 1;
        Some(t)
    }

    fn is_stop(&self, tok: &Token, stop: Stop) -> bool {
        match tok {
            Token::EndGroup if stop.end_group => true,
            Token::AlignmentTab if stop.amp => true,
            Token::Command(s) if s == "\\" && stop.cr => true,
            Token::Command(s) if s == "cr" && stop.cr => true,
            Token::Command(s) if s == "right" && stop.right => true,
            Token::Command(s) if s == "end" && stop.end_env => true,
            Token::Char(']') if stop.rbracket => true,
            _ => false,
        }
    }

    /// Run `f` one level deeper, refusing to descend past `max_depth`.
    fn nested<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<T, ParseError>,
    ) -> Result<T, ParseError> {
        if self.depth >= self.max_depth {
            return Err(ParseError::Malformed(format!(
                "input nests deeper than {} levels",
                self.max_depth
            )));
        }
        self.depth += 1;
        let out = f(self);
        self.depth -= 1;
        out
    }

    fn parse_list(&mut self, stop: Stop) -> Result<MathNode, ParseError> {
        self.nested(|p| p.parse_list_inner(stop))
    }

    fn parse_list_inner(&mut self, stop: Stop) -> Result<MathNode, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            let Some(tok) = self.peek().cloned() else {
                break;
            };
            if self.is_stop(&tok, stop) {
                break;
            }
            match &tok {
                Token::MathShift | Token::DisplayShift => {
                    return Err(ParseError::Malformed("unexpected math shift".into()));
                }
                Token::Command(n) if n == "color" => {
                    self.bump();
                    let c = self.parse_color_from_cmd()?;
                    let rest = self.parse_list(stop)?;
                    items.push(MathNode::Color(c, Box::new(rest)));
                    break;
                }
                Token::Command(n) if n == "definecolor" => {
                    self.bump();
                    self.parse_definecolor()?;
                    continue;
                }
                _ => {}
            }
            items.push(self.parse_atom()?);
        }
        Ok(wrap_row(items))
    }

    fn parse_atom(&mut self) -> Result<MathNode, ParseError> {
        let mut nucleus = self.parse_nucleus()?;
        let mut limits = None;
        loop {
            match self.peek_ws() {
                Some(Token::Command(n)) if n == "limits" => {
                    self.bump();
                    limits = Some(true);
                }
                Some(Token::Command(n)) if n == "nolimits" => {
                    self.bump();
                    limits = Some(false);
                }
                _ => break,
            }
        }
        if let Some(flag) = limits {
            if let MathNode::Operator(name, _) = nucleus {
                nucleus = MathNode::Operator(name, flag);
            }
        }
        self.bind_scripts(nucleus)
    }

    fn parse_nucleus(&mut self) -> Result<MathNode, ParseError> {
        self.skip_ws();
        match self.peek().cloned() {
            None => Err(ParseError::Malformed("unexpected end of input".into())),
            Some(Token::Superscript | Token::Subscript | Token::Char('\'')) => {
                Ok(MathNode::Row(Vec::new()))
            }
            Some(Token::BeginGroup) => self.parse_group(),
            Some(Token::Char(c)) => {
                self.bump();
                Ok(MathNode::Atom(c, atom_kind(c)))
            }
            Some(Token::Command(name)) => {
                self.bump();
                self.parse_command(&name)
            }
            Some(other) => Err(ParseError::Malformed(format!("unexpected token {other}"))),
        }
    }

    fn parse_group(&mut self) -> Result<MathNode, ParseError> {
        match self.bump() {
            Some(Token::BeginGroup) => {}
            _ => return Err(ParseError::Malformed("expected '{'".into())),
        }
        let inner = self.parse_list(Stop::group())?;
        match self.bump() {
            Some(Token::EndGroup) => Ok(inner),
            _ => Err(ParseError::Malformed("unmatched '{'".into())),
        }
    }

    fn parse_arg(&mut self) -> Result<MathNode, ParseError> {
        self.nested(Self::parse_arg_inner)
    }

    fn parse_arg_inner(&mut self) -> Result<MathNode, ParseError> {
        self.skip_ws();
        match self.peek() {
            Some(Token::BeginGroup) => self.parse_group(),
            Some(_) => self.parse_nucleus(),
            None => Err(ParseError::Malformed("missing argument".into())),
        }
    }

    fn parse_script(&mut self) -> Result<MathNode, ParseError> {
        self.parse_arg()
    }

    fn bind_scripts(&mut self, nucleus: MathNode) -> Result<MathNode, ParseError> {
        let mut sub: Option<MathNode> = None;
        let mut sup: Option<MathNode> = None;
        let mut sup_from_prime = false;
        loop {
            match self.peek_ws() {
                Some(Token::Subscript) => {
                    self.bump();
                    if sub.is_some() {
                        return Err(ParseError::Malformed("double subscript".into()));
                    }
                    sub = Some(self.parse_script()?);
                }
                Some(Token::Superscript) => {
                    self.bump();
                    let s = self.parse_script()?;
                    if let Some(prev) = sup.take() {
                        if !sup_from_prime {
                            return Err(ParseError::Malformed("double superscript".into()));
                        }
                        sup = Some(wrap_row(vec![prev, s]));
                        sup_from_prime = false;
                    } else {
                        sup = Some(s);
                    }
                }
                Some(Token::Char('\'')) => {
                    self.bump();
                    let prime = MathNode::Atom('′', AtomKind::Ord);
                    sup = Some(match sup.take() {
                        None => prime,
                        Some(prev) => wrap_row(vec![prev, prime]),
                    });
                    sup_from_prime = true;
                }
                _ => break,
            }
        }
        Ok(apply_scripts(nucleus, sub, sup))
    }

    fn parse_command(&mut self, name: &str) -> Result<MathNode, ParseError> {
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let n = self.parse_arg()?;
                let d = self.parse_arg()?;
                Ok(MathNode::Fraction(Box::new(n), Box::new(d)))
            }
            "binom" | "dbinom" | "tbinom" => {
                let n = self.parse_arg()?;
                let k = self.parse_arg()?;
                Ok(MathNode::Delimited(
                    Delimiter::Char('('),
                    Box::new(MathNode::Fraction(Box::new(n), Box::new(k))),
                    Delimiter::Char(')'),
                ))
            }
            "genfrac" => self.parse_genfrac(),
            "sqrt" => {
                let index = if matches!(self.peek_ws(), Some(Token::Char('['))) {
                    self.bump();
                    let idx = self.parse_list(Stop::index())?;
                    match self.bump() {
                        Some(Token::Char(']')) => {}
                        _ => {
                            return Err(ParseError::Malformed(
                                "expected ']' after \\sqrt index".into(),
                            ))
                        }
                    }
                    Some(Box::new(idx))
                } else {
                    None
                };
                let rad = self.parse_arg()?;
                Ok(MathNode::Radical(index, Box::new(rad)))
            }
            "left" => self.parse_delimited(),
            "right" => Err(ParseError::UnmatchedDelimiter),
            "begin" => self.parse_begin(),
            "end" => Err(ParseError::Malformed("unexpected \\end".into())),
            "over" => Err(ParseError::Malformed("\\over outside a group".into())),
            "choose" => Err(ParseError::Malformed("\\choose outside a group".into())),
            "hat" => self.accent(AccentKind::Hat),
            "check" => self.accent(AccentKind::Check),
            "breve" => self.accent(AccentKind::Breve),
            "acute" => self.accent(AccentKind::Acute),
            "grave" => self.accent(AccentKind::Grave),
            "tilde" => self.accent(AccentKind::Tilde),
            "bar" => self.accent(AccentKind::Bar),
            "vec" => self.accent(AccentKind::Vec),
            "dot" => self.accent(AccentKind::Dot),
            "ddot" => self.accent(AccentKind::Ddot),
            "dddot" => self.accent(AccentKind::Dddot),
            "ddddot" => self.accent(AccentKind::Ddddot),
            "widehat" => self.accent(AccentKind::WideHat),
            "widetilde" => self.accent(AccentKind::WideTilde),
            "overline" => self.accent(AccentKind::Overline),
            "underline" => self.accent(AccentKind::Underline),
            "overbrace" => self.accent(AccentKind::Overbrace),
            "underbrace" => self.accent(AccentKind::Underbrace),
            "overleftarrow" => self.accent(AccentKind::Overleftarrow),
            "overrightarrow" => self.accent(AccentKind::Overrightarrow),
            "overleftrightarrow" => self.accent(AccentKind::Overleftrightarrow),
            "underleftarrow" => self.accent(AccentKind::Underleftarrow),
            "underrightarrow" => self.accent(AccentKind::Underrightarrow),
            "underleftrightarrow" => self.accent(AccentKind::Underleftrightarrow),
            "cancel" => self.accent(AccentKind::Cancel),
            "bcancel" => self.accent(AccentKind::BCancel),
            "xcancel" => self.accent(AccentKind::XCancel),
            "boxed" | "fbox" => self.accent(AccentKind::Boxed),
            "mathring" => self.accent(AccentKind::Ring),
            "cancelto" => {
                let value = self.parse_arg()?;
                let expr = self.parse_arg()?;
                if is_empty_node(&expr) {
                    return Err(ParseError::Malformed("empty accent base".into()));
                }
                Ok(MathNode::CancelTo(Box::new(value), Box::new(expr)))
            }
            "not" => {
                let body = self.parse_nucleus()?;
                Ok(MathNode::Accent(Box::new(body), AccentKind::Not))
            }
            "overset" => {
                let over = self.parse_arg()?;
                let base = self.parse_arg()?;
                Ok(MathNode::OverUnder(
                    Box::new(base),
                    Some(Box::new(over)),
                    None,
                ))
            }
            "underset" => {
                let under = self.parse_arg()?;
                let base = self.parse_arg()?;
                Ok(MathNode::OverUnder(
                    Box::new(base),
                    None,
                    Some(Box::new(under)),
                ))
            }
            "stackrel" => {
                let over = self.parse_arg()?;
                let base = self.parse_arg()?;
                Ok(MathNode::OverUnder(
                    Box::new(base),
                    Some(Box::new(over)),
                    None,
                ))
            }
            "mathrm" | "textrm" => self.font(TextStyle::Rm),
            "mathbf" | "textbf" => self.font(TextStyle::Bf),
            "mathit" | "textit" => self.font(TextStyle::It),
            "mathsf" | "textsf" => self.font(TextStyle::Sf),
            "mathtt" | "texttt" => self.font(TextStyle::Tt),
            "mathbb" => self.font(TextStyle::Bb),
            "mathcal" => self.font(TextStyle::Cal),
            "mathfrak" => self.font(TextStyle::Frak),
            "mathscr" => self.font(TextStyle::Scr),
            "boldsymbol" => self.font(TextStyle::Boldsymbol),
            "pmb" => self.font(TextStyle::Pmb),
            "xrightarrow" => self.parse_xarrow("longrightarrow"),
            "xleftarrow" => self.parse_xarrow("longleftarrow"),
            "text" | "mbox" => self.parse_text(TextStyle::Text),
            "operatorname" => {
                let name = self.collect_group_text()?;
                Ok(MathNode::Operator(name, false))
            }
            "," => Ok(MathNode::Space(SpaceKind::Thin)),
            ":" | ">" => Ok(MathNode::Space(SpaceKind::Medium)),
            ";" => Ok(MathNode::Space(SpaceKind::Thick)),
            "!" => Ok(MathNode::Space(SpaceKind::NegThin)),
            "quad" => Ok(MathNode::Space(SpaceKind::Quad)),
            "qquad" => Ok(MathNode::Space(SpaceKind::Qquad)),
            " " => Ok(MathNode::Space(SpaceKind::ControlSpace)),
            "hspace" => {
                let spec = self.collect_group_text()?;
                let d = parse_tex_dim(&spec)?;
                Ok(MathNode::Space(SpaceKind::Hspace(d)))
            }
            "phantom" => {
                let b = self.parse_arg()?;
                Ok(MathNode::Phantom(PhantomKind::Full, Box::new(b)))
            }
            "vphantom" => {
                let b = self.parse_arg()?;
                Ok(MathNode::Phantom(PhantomKind::Vertical, Box::new(b)))
            }
            "hphantom" => {
                let b = self.parse_arg()?;
                Ok(MathNode::Phantom(PhantomKind::Horizontal, Box::new(b)))
            }
            "strut" => Ok(MathNode::Strut(Dim::ratio(7, 10), Dim::ratio(3, 10))),
            "rule" => {
                let _w = parse_tex_dim(&self.collect_group_text()?)?;
                let h = parse_tex_dim(&self.collect_group_text()?)?;
                Ok(MathNode::Strut(h, Dim::zero()))
            }
            "textcolor" => {
                let c = self.parse_color_from_cmd()?;
                let body = self.parse_arg()?;
                Ok(MathNode::TextColor(c, Box::new(body)))
            }
            "colorbox" => {
                let c = self.parse_color_from_cmd()?;
                let body = self.parse_arg()?;
                Ok(MathNode::ColorBox(c, Box::new(body)))
            }
            "fcolorbox" => {
                let border = self.parse_color_from_cmd()?;
                let fill = self.parse_color_from_cmd()?;
                let body = self.parse_arg()?;
                Ok(MathNode::FColorBox(border, fill, Box::new(body)))
            }
            "sum" => Ok(MathNode::Sum(None, None)),
            "prod" => Ok(MathNode::Product(None, None)),
            "int" => Ok(MathNode::Integral(IntegralKind::Int, None, None)),
            "iint" => Ok(MathNode::Integral(IntegralKind::Iint, None, None)),
            "iiint" => Ok(MathNode::Integral(IntegralKind::Iiint, None, None)),
            "oint" => Ok(MathNode::Integral(IntegralKind::Oint, None, None)),
            "oiint" => Ok(MathNode::Integral(IntegralKind::Oiint, None, None)),
            "lim" => Ok(MathNode::Limit(None)),
            "sin" | "cos" | "tan" | "cot" | "sec" | "csc" | "arcsin" | "arccos" | "arctan"
            | "sinh" | "cosh" | "tanh" | "coth" | "log" | "ln" | "lg" | "exp" | "limsup"
            | "liminf" | "sup" | "inf" | "max" | "min" | "det" | "dim" | "ker" | "deg" | "gcd"
            | "lcm" | "Pr" | "arg" => Ok(MathNode::Operator(name.to_string(), false)),
            "coprod" | "bigcup" | "bigcap" | "bigsqcup" | "bigvee" | "bigwedge" | "bigoplus"
            | "bigotimes" | "biguplus" => Ok(MathNode::Operator(name.to_string(), true)),
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr" | "biggl"
            | "biggr" | "Biggl" | "Biggr" | "bigm" | "Bigm" | "biggm" | "Biggm" => {
                self.parse_sized_delim(name)
            }
            "tag" => {
                let star = matches!(self.peek_ws(), Some(Token::Char('*')));
                if star {
                    self.bump();
                }
                let body = self.parse_arg()?;
                Ok(MathNode::Tag {
                    star,
                    body: Box::new(body),
                })
            }
            "label" => {
                let key = self.collect_group_text()?;
                Ok(MathNode::Label(key))
            }
            "ref" => {
                let key = self.collect_group_text()?;
                Ok(MathNode::Ref(key))
            }
            "nonumber" | "notag" => Ok(MathNode::NoNumber),
            "hline" => Ok(MathNode::Hline),
            "intertext" => {
                let s = self.collect_group_text()?;
                Ok(MathNode::Intertext(Box::new(MathNode::Text(
                    s,
                    TextStyle::Text,
                ))))
            }
            "substack" => self.parse_substack(),
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "limits"
            | "nolimits" => self.parse_nucleus(),
            "{" | "}" => {
                let c = name.chars().next().unwrap_or('{');
                Ok(MathNode::Atom(
                    c,
                    if name == "{" {
                        AtomKind::Open
                    } else {
                        AtomKind::Close
                    },
                ))
            }
            "|" => Ok(MathNode::Symbol("Vert".into())),
            "backslash" => Ok(MathNode::Symbol("backslash".into())),
            _ => {
                if name.starts_with("math")
                    && name.len() > 4
                    && name.chars().all(|c| c.is_ascii_alphabetic())
                {
                    return Err(ParseError::Unsupported(format!("font style {name}")));
                }
                if name.starts_with("wide") {
                    return Err(ParseError::Unsupported(format!("accent {name}")));
                }
                self.parse_symbol_or_unknown(name)
            }
        }
    }

    fn accent(&mut self, kind: AccentKind) -> Result<MathNode, ParseError> {
        let body = self.parse_arg()?;
        if is_empty_node(&body) {
            return Err(ParseError::Malformed("empty accent base".into()));
        }
        Ok(MathNode::Accent(Box::new(body), kind))
    }

    fn parse_xarrow(&mut self, arrow: &str) -> Result<MathNode, ParseError> {
        let under = if matches!(self.peek_ws(), Some(Token::Char('['))) {
            self.bump();
            let u = self.parse_list(Stop::index())?;
            match self.bump() {
                Some(Token::Char(']')) => Some(Box::new(u)),
                _ => {
                    return Err(ParseError::Malformed(
                        "expected ']' after x-arrow optional argument".into(),
                    ))
                }
            }
        } else {
            None
        };
        let over = self.parse_arg()?;
        Ok(MathNode::OverUnder(
            Box::new(MathNode::Symbol(arrow.to_string())),
            Some(Box::new(over)),
            under,
        ))
    }

    fn font(&mut self, style: TextStyle) -> Result<MathNode, ParseError> {
        let inner = self.parse_arg()?;
        Ok(collapse_text(apply_text_style(inner, style)))
    }

    fn parse_text(&mut self, style: TextStyle) -> Result<MathNode, ParseError> {
        let s = self.collect_group_text()?;
        Ok(MathNode::Text(s, style))
    }

    fn parse_delimited(&mut self) -> Result<MathNode, ParseError> {
        let open = self.parse_delimiter()?;
        let body = self.parse_list(Stop::delim())?;
        match self.bump() {
            Some(Token::Command(n)) if n == "right" => {}
            _ => return Err(ParseError::UnmatchedDelimiter),
        }
        let close = self.parse_delimiter()?;
        Ok(MathNode::Delimited(open, Box::new(body), close))
    }

    fn parse_sized_delim(&mut self, name: &str) -> Result<MathNode, ParseError> {
        let size = DelimSize::from_command(name)
            .ok_or_else(|| ParseError::Malformed(format!("unknown delimiter size \\{name}")))?;
        let d = self.parse_delimiter()?;
        let class = DelimSize::class_from_command(name).unwrap_or_else(|| match &d {
            Delimiter::Char(c) => atom_kind(*c),
            Delimiter::Named(n) if n == "{" => AtomKind::Open,
            Delimiter::Named(n) if n == "}" => AtomKind::Close,
            _ => AtomKind::Open,
        });
        Ok(MathNode::SizedDelim(d, size, class))
    }

    fn parse_delimiter(&mut self) -> Result<Delimiter, ParseError> {
        self.skip_ws();
        match self.bump() {
            Some(Token::Char('.')) => Ok(Delimiter::Empty),
            Some(Token::Char(c)) if matches!(c, '(' | ')' | '[' | ']' | '|' | '/' | '<' | '>') => {
                Ok(Delimiter::Char(c))
            }
            Some(Token::Command(n)) => match n.as_str() {
                "." => Ok(Delimiter::Empty),
                "{" | "}" | "|" => Ok(Delimiter::Named(n)),
                "langle" | "rangle" | "lfloor" | "rfloor" | "lceil" | "rceil" | "lvert"
                | "rvert" | "lVert" | "rVert" | "vert" | "Vert" | "uparrow" | "downarrow"
                | "Uparrow" | "Downarrow" | "updownarrow" | "Updownarrow" | "backslash"
                | "lgroup" | "rgroup" | "lmoustache" | "rmoustache" => Ok(Delimiter::Named(n)),
                other => Err(ParseError::Malformed(format!(
                    "unknown delimiter \\{other}"
                ))),
            },
            Some(other) => Err(ParseError::Malformed(format!(
                "expected delimiter, found {other}"
            ))),
            None => Err(ParseError::Malformed("expected delimiter".into())),
        }
    }

    fn parse_begin(&mut self) -> Result<MathNode, ParseError> {
        let name = self.collect_group_text()?;
        let colspec = if name == "array" {
            let preamble = self.collect_group_text()?;
            parse_colspec(&preamble)?
        } else {
            Vec::new()
        };
        let style = match name.as_str() {
            "matrix" => MatrixStyle::Matrix,
            "pmatrix" => MatrixStyle::Pmatrix,
            "bmatrix" => MatrixStyle::Bmatrix,
            "vmatrix" => MatrixStyle::Vmatrix,
            "Vmatrix" => MatrixStyle::VVmatrix,
            "Bmatrix" => MatrixStyle::BBmatrix,
            "cases" => MatrixStyle::Cases,
            "array" => MatrixStyle::Array,
            "aligned" => MatrixStyle::Aligned,
            "align" => MatrixStyle::Align,
            "gather" => MatrixStyle::Gather,
            "multline" => MatrixStyle::Multline,
            "equation" => MatrixStyle::Equation,
            "split" => MatrixStyle::Split,
            other => {
                return Err(ParseError::Unsupported(format!("environment {other}")));
            }
        };
        let rows = self.parse_rows()?;
        self.expect_end(&name)?;
        Ok(MathNode::Matrix(style, colspec, rows))
    }

    fn parse_substack(&mut self) -> Result<MathNode, ParseError> {
        self.skip_ws();
        match self.bump() {
            Some(Token::BeginGroup) => {}
            _ => {
                return Err(ParseError::Malformed(
                    "expected '{' after \\substack".into(),
                ))
            }
        }
        let mut lines = Vec::new();
        loop {
            self.skip_ws();
            if matches!(self.peek(), Some(Token::EndGroup)) {
                self.bump();
                break;
            }
            let line = self.parse_list(Stop::substack_line())?;
            lines.push(line);
            self.skip_ws();
            match self.peek() {
                Some(Token::Command(n)) if n == "\\" || n == "cr" => {
                    self.bump();
                }
                Some(Token::EndGroup) => {
                    self.bump();
                    break;
                }
                None => return Err(ParseError::Malformed("unmatched '{' in \\substack".into())),
                Some(other) => {
                    return Err(ParseError::Malformed(format!(
                        "unexpected token {other} in \\substack"
                    )))
                }
            }
        }
        if lines.is_empty() {
            return Err(ParseError::Malformed("empty \\substack".into()));
        }
        Ok(MathNode::Substack(lines))
    }

    fn parse_rows(&mut self) -> Result<Vec<EnvRow>, ParseError> {
        self.skip_ws();
        if matches!(self.peek(), Some(Token::Command(n)) if n == "end") {
            return Ok(Vec::new());
        }
        let mut rows = Vec::new();
        loop {
            self.skip_ws();
            if matches!(self.peek(), Some(Token::Command(n)) if n == "end") {
                return Ok(rows);
            }
            if matches!(self.peek(), Some(Token::Command(n)) if n == "hline") {
                self.bump();
                rows.push(EnvRow::Hline);
                continue;
            }
            if matches!(self.peek(), Some(Token::Command(n)) if n == "intertext") {
                self.bump();
                let s = self.collect_group_text()?;
                rows.push(EnvRow::Intertext(Box::new(MathNode::Text(
                    s,
                    TextStyle::Text,
                ))));
                continue;
            }
            let mut cells = Vec::new();
            let mut number = EqNumber::Default;
            let mut labels = Vec::new();
            loop {
                let cell = self.parse_list(Stop::cell())?;
                let cell = peel_row_meta(cell, &mut number, &mut labels);
                cells.push(cell);
                self.skip_ws();
                match self.peek() {
                    Some(Token::AlignmentTab) => {
                        self.bump();
                    }
                    Some(Token::Command(n)) if n == "\\" || n == "cr" => {
                        self.bump();
                        rows.push(finish_env_row(cells, number, labels));
                        self.skip_ws();
                        if matches!(self.peek(), Some(Token::Command(e)) if e == "end") {
                            return Ok(rows);
                        }
                        break;
                    }
                    Some(Token::Command(n)) if n == "end" => {
                        rows.push(finish_env_row(cells, number, labels));
                        return Ok(rows);
                    }
                    None => {
                        return Err(ParseError::Malformed(
                            "unmatched \\begin (missing \\end)".into(),
                        ));
                    }
                    Some(other) => {
                        return Err(ParseError::Malformed(format!(
                            "unexpected token {other} in environment body"
                        )));
                    }
                }
            }
        }
    }

    fn expect_end(&mut self, name: &str) -> Result<(), ParseError> {
        match self.bump() {
            Some(Token::Command(n)) if n == "end" => {}
            _ => return Err(ParseError::Malformed(format!("expected \\end{{{name}}}"))),
        }
        let got = self.collect_group_text()?;
        if got != name {
            return Err(ParseError::Malformed(format!(
                "\\begin{{{name}}} closed by \\end{{{got}}}"
            )));
        }
        Ok(())
    }

    fn parse_genfrac(&mut self) -> Result<MathNode, ParseError> {
        let ldel = self.collect_group_text()?;
        let rdel = self.collect_group_text()?;
        let _thickness = self.collect_group_text()?;
        let _style = self.collect_group_text()?;
        let num = self.parse_arg()?;
        let den = self.parse_arg()?;
        let frac = MathNode::Fraction(Box::new(num), Box::new(den));
        if ldel.is_empty() && rdel.is_empty() {
            return Ok(frac);
        }
        Ok(MathNode::Delimited(
            delim_from_text(&ldel)?,
            Box::new(frac),
            delim_from_text(&rdel)?,
        ))
    }

    fn parse_color_from_cmd(&mut self) -> Result<Color, ParseError> {
        self.skip_ws();
        let model = if matches!(self.peek(), Some(Token::Char('['))) {
            self.bump();
            let m = self.collect_until_char(']')?;
            match self.bump() {
                Some(Token::Char(']')) => {}
                _ => {
                    return Err(ParseError::Malformed(
                        "expected ']' after color model".into(),
                    ))
                }
            }
            m
        } else {
            "named".into()
        };
        let spec = self.collect_group_text()?;
        parse_color_spec(&model, &spec, Some(&self.colors)).map_err(color_err)
    }

    fn parse_definecolor(&mut self) -> Result<(), ParseError> {
        let name = self.collect_group_text()?;
        let model = self.collect_group_text()?;
        let spec = self.collect_group_text()?;
        self.colors
            .define(&name, &model, &spec)
            .map_err(color_err)?;
        Ok(())
    }

    fn collect_group_text(&mut self) -> Result<String, ParseError> {
        self.skip_ws();
        match self.bump_raw() {
            Some(Token::Space) => {
                self.pos -= 1;
                self.skip_ws();
                return self.collect_group_text();
            }
            Some(Token::BeginGroup) => {}
            _ => return Err(ParseError::Malformed("expected '{'".into())),
        }
        let mut s = String::new();
        let mut depth = 1;
        while depth > 0 {
            match self.bump_raw() {
                None => return Err(ParseError::Malformed("unmatched '{'".into())),
                Some(Token::BeginGroup) => {
                    depth += 1;
                    s.push('{');
                }
                Some(Token::EndGroup) => {
                    depth -= 1;
                    if depth > 0 {
                        s.push('}');
                    }
                }
                Some(Token::Space) => s.push(' '),
                Some(Token::Char(c)) => s.push(c),
                Some(Token::Command(n)) => {
                    if n.len() == 1 {
                        s.push(n.chars().next().unwrap_or('\\'));
                    } else {
                        s.push('\\');
                        s.push_str(&n);
                    }
                }
                Some(other) => {
                    return Err(ParseError::Malformed(format!(
                        "unexpected token {other} in group text"
                    )))
                }
            }
        }
        Ok(s)
    }

    fn collect_until_char(&mut self, end: char) -> Result<String, ParseError> {
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(ParseError::Malformed(format!("expected '{end}'"))),
                Some(Token::Char(c)) if *c == end => break,
                Some(Token::Char(c)) => {
                    s.push(*c);
                    self.bump_raw();
                }
                Some(Token::Space) => {
                    s.push(' ');
                    self.bump_raw();
                }
                Some(other) => {
                    return Err(ParseError::Malformed(format!(
                        "unexpected token {other} in optional argument"
                    )))
                }
            }
        }
        Ok(s)
    }

    fn parse_symbol_or_unknown(&self, name: &str) -> Result<MathNode, ParseError> {
        let canon = alias(name);
        if let Some(e) = lookup(canon).or_else(|| lookup(name)) {
            match e.kind {
                CatalogKind::Container | CatalogKind::Modifier => {
                    return Err(ParseError::Unsupported(format!("\\{name}")));
                }
                CatalogKind::Symbol | CatalogKind::Operator => {
                    return Ok(MathNode::Symbol(canon.to_string()));
                }
            }
        }
        if is_extra_symbol(canon) {
            return Ok(MathNode::Symbol(canon.to_string()));
        }
        Err(ParseError::Unknown(format!("\\{name}")))
    }
}

fn strip_fences(tokens: &[Token]) -> Result<Vec<Token>, ParseError> {
    let t = trim_spaces(tokens);
    if t.len() >= 2 {
        let inner = match (&t[0], &t[t.len() - 1]) {
            (Token::MathShift, Token::MathShift) => Some(&t[1..t.len() - 1]),
            (Token::DisplayShift, Token::DisplayShift) => Some(&t[1..t.len() - 1]),
            (Token::Command(a), Token::Command(b)) if a == "[" && b == "]" => {
                Some(&t[1..t.len() - 1])
            }
            (Token::Command(a), Token::Command(b)) if a == "(" && b == ")" => {
                Some(&t[1..t.len() - 1])
            }
            _ => None,
        };
        if let Some(inner) = inner {
            return Ok(trim_spaces(inner).to_vec());
        }
        if matches!(t[0], Token::MathShift | Token::DisplayShift)
            || matches!(&t[0], Token::Command(s) if s == "[" || s == "(")
        {
            return Err(ParseError::Malformed("unmatched math delimiter".into()));
        }
    }
    Ok(t.to_vec())
}

fn trim_spaces(tokens: &[Token]) -> &[Token] {
    let mut a = 0;
    let mut b = tokens.len();
    while a < b && tokens[a] == Token::Space {
        a += 1;
    }
    while b > a && tokens[b - 1] == Token::Space {
        b -= 1;
    }
    &tokens[a..b]
}

fn wrap_row(mut items: Vec<MathNode>) -> MathNode {
    if items.len() == 1 {
        items.remove(0)
    } else {
        MathNode::Row(items)
    }
}

fn parse_colspec(s: &str) -> Result<Vec<ColSpec>, ParseError> {
    let mut out = Vec::new();
    for c in s.chars() {
        match c {
            'l' => out.push(ColSpec::Left),
            'c' => out.push(ColSpec::Center),
            'r' => out.push(ColSpec::Right),
            '|' => out.push(ColSpec::VRule),
            ' ' | '\t' => {}
            '@' | '!' | '>' | '<' | 'p' | 'm' | 'b' | '*' => {
                return Err(ParseError::Unsupported(format!("array preamble `{c}`")))
            }
            other => return Err(ParseError::Malformed(format!("array preamble `{other}`"))),
        }
    }
    if out.is_empty() {
        return Err(ParseError::Malformed("empty array preamble".into()));
    }
    Ok(out)
}

fn peel_row_meta(node: MathNode, number: &mut EqNumber, labels: &mut Vec<String>) -> MathNode {
    match node {
        MathNode::NoNumber => {
            *number = EqNumber::Suppress;
            MathNode::Row(Vec::new())
        }
        MathNode::Tag { star, body } => {
            *number = EqNumber::Tag { star, body };
            MathNode::Row(Vec::new())
        }
        MathNode::Label(k) => {
            labels.push(k);
            MathNode::Row(Vec::new())
        }
        MathNode::Hline => MathNode::Hline,
        MathNode::Intertext(n) => MathNode::Intertext(n),
        MathNode::Row(items) => {
            let mut kept = Vec::new();
            for it in items {
                let p = peel_row_meta(it, number, labels);
                if !is_empty_node(&p) {
                    kept.push(p);
                }
            }
            wrap_row(kept)
        }
        other => other,
    }
}

fn finish_env_row(cells: Vec<MathNode>, number: EqNumber, labels: Vec<String>) -> EnvRow {
    if cells.len() == 1 && matches!(cells[0], MathNode::Hline) {
        return EnvRow::Hline;
    }
    if cells.len() == 1 {
        if let MathNode::Intertext(n) = &cells[0] {
            return EnvRow::Intertext(n.clone());
        }
    }
    EnvRow::Cells {
        cells,
        number,
        labels,
    }
}

fn is_empty_node(n: &MathNode) -> bool {
    match n {
        MathNode::Row(v) => v.is_empty() || v.iter().all(is_empty_node),
        MathNode::Space(_) | MathNode::NoNumber | MathNode::Label(_) => true,
        _ => false,
    }
}

fn apply_scripts(nucleus: MathNode, sub: Option<MathNode>, sup: Option<MathNode>) -> MathNode {
    match nucleus {
        MathNode::Sum(None, None) => MathNode::Sum(sub.map(Box::new), sup.map(Box::new)),
        MathNode::Product(None, None) => MathNode::Product(sub.map(Box::new), sup.map(Box::new)),
        MathNode::Integral(k, None, None) => {
            MathNode::Integral(k, sub.map(Box::new), sup.map(Box::new))
        }
        MathNode::Limit(None) => {
            let lim = MathNode::Limit(sub.map(Box::new));
            match sup {
                Some(s) => MathNode::Superscript(Box::new(lim), Box::new(s)),
                None => lim,
            }
        }
        MathNode::Accent(b, k @ (AccentKind::Overbrace | AccentKind::Underbrace)) => {
            match (sub, sup) {
                (None, None) => MathNode::Accent(b, k),
                (s, e) => MathNode::OverUnder(
                    Box::new(MathNode::Accent(b, k)),
                    e.map(Box::new),
                    s.map(Box::new),
                ),
            }
        }
        other => match (sub, sup) {
            (None, None) => other,
            (Some(s), None) => MathNode::Subscript(Box::new(other), Box::new(s)),
            (None, Some(e)) => MathNode::Superscript(Box::new(other), Box::new(e)),
            (Some(s), Some(e)) => MathNode::SubSup(Box::new(other), Box::new(s), Box::new(e)),
        },
    }
}

/// Apply a math font style to a whole subformula.
///
/// A LaTeX font command such as `\mathrm` switches the font for everything in
/// its argument, not only for the characters at the top of the argument's list,
/// so this descends into scripts, fractions, radicals, accents and delimited
/// bodies as well as rows. Delimiters themselves are `Delimiter` values rather
/// than nodes and are left alone, as they are in LaTeX.
fn apply_text_style(node: MathNode, style: TextStyle) -> MathNode {
    match node {
        MathNode::Atom(c, _) if crate::style_map::is_stylable(c) => {
            MathNode::Text(c.to_string(), style)
        }
        MathNode::Text(s, _) => MathNode::Text(s, style),
        MathNode::Symbol(name) => {
            if let Some(ch) = crate::symbols::glyph_char(&name) {
                if crate::style_map::is_stylable(ch) {
                    MathNode::Text(ch.to_string(), style)
                } else {
                    MathNode::Symbol(name)
                }
            } else {
                MathNode::Symbol(name)
            }
        }
        MathNode::Row(v) => collapse_text(MathNode::Row(
            v.into_iter().map(|n| apply_text_style(n, style)).collect(),
        )),
        MathNode::Substack(v) => {
            MathNode::Substack(v.into_iter().map(|n| apply_text_style(n, style)).collect())
        }
        MathNode::Superscript(b, sup) => MathNode::Superscript(
            Box::new(apply_text_style(*b, style)),
            Box::new(apply_text_style(*sup, style)),
        ),
        MathNode::Subscript(b, sub) => MathNode::Subscript(
            Box::new(apply_text_style(*b, style)),
            Box::new(apply_text_style(*sub, style)),
        ),
        MathNode::SubSup(b, sub, sup) => MathNode::SubSup(
            Box::new(apply_text_style(*b, style)),
            Box::new(apply_text_style(*sub, style)),
            Box::new(apply_text_style(*sup, style)),
        ),
        MathNode::Fraction(num, den) => MathNode::Fraction(
            Box::new(apply_text_style(*num, style)),
            Box::new(apply_text_style(*den, style)),
        ),
        MathNode::Radical(index, body) => MathNode::Radical(
            index.map(|i| Box::new(apply_text_style(*i, style))),
            Box::new(apply_text_style(*body, style)),
        ),
        MathNode::Accent(body, kind) => {
            MathNode::Accent(Box::new(apply_text_style(*body, style)), kind)
        }
        MathNode::Delimited(open, body, close) => {
            MathNode::Delimited(open, Box::new(apply_text_style(*body, style)), close)
        }
        other => other,
    }
}

fn collapse_text(node: MathNode) -> MathNode {
    let MathNode::Row(v) = node else {
        return node;
    };
    let mut out: Vec<MathNode> = Vec::new();
    for n in v {
        match (out.last_mut(), &n) {
            (Some(MathNode::Text(a, sa)), MathNode::Text(b, sb)) if sa == sb => {
                a.push_str(b);
            }
            _ => out.push(n),
        }
    }
    wrap_row(out)
}

fn atom_kind(c: char) -> AtomKind {
    match c {
        '+' | '-' | '*' | '±' | '∓' | '·' | '×' | '÷' => AtomKind::Bin,
        '=' | '<' | '>' | '≠' | '≤' | '≥' | '≈' | '≡' => AtomKind::Rel,
        '(' | '[' | '{' => AtomKind::Open,
        ')' | ']' | '}' => AtomKind::Close,
        ',' | ';' | '!' | '?' | ':' => AtomKind::Punct,
        _ => AtomKind::Ord,
    }
}

fn delim_from_text(s: &str) -> Result<Delimiter, ParseError> {
    let s = s.trim();
    if s.is_empty() || s == "." {
        return Ok(Delimiter::Empty);
    }
    if s.chars().count() == 1 {
        let c = s.chars().next().unwrap();
        return Ok(Delimiter::Char(c));
    }
    let name = s.strip_prefix('\\').unwrap_or(s);
    Ok(Delimiter::Named(name.to_string()))
}

fn parse_tex_dim(s: &str) -> Result<Dim, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseError::Malformed("empty dimension".into()));
    }
    let mut i = 0;
    let b = s.as_bytes();
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
        i += 1;
    }
    if i == 0 || (i == 1 && (b[0] == b'+' || b[0] == b'-')) {
        return Err(ParseError::Malformed(format!("invalid dimension `{s}`")));
    }
    let num = Dim::parse(&s[..i]);
    let unit = s[i..].trim();
    match unit {
        "" | "em" => Ok(num),
        "mu" => Ok(Dim::from_mu(&num)),
        "pt" | "bp" => Ok(num / Dim::from_i64(10)),
        other => Err(ParseError::Unsupported(format!("dimension unit {other}"))),
    }
}

fn color_err(e: Error) -> ParseError {
    match e {
        Error::Unsupported { what } => ParseError::Unsupported(what),
        Error::Parse(p) => p,
        Error::Malformed { what } => ParseError::Malformed(what),
        Error::InvalidOption { what } => ParseError::Malformed(what),
        other => ParseError::Malformed(other.to_string()),
    }
}

fn alias(name: &str) -> &str {
    match name {
        "le" => "leq",
        "ge" => "geq",
        "ne" => "neq",
        "dots" => "ldots",
        "lnot" => "neg",
        "dag" => "dagger",
        "ddag" => "ddagger",
        "owns" => "ni",
        _ => name,
    }
}

fn is_extra_symbol(name: &str) -> bool {
    matches!(
        name,
        "Gamma"
            | "Delta"
            | "Theta"
            | "Lambda"
            | "Xi"
            | "Pi"
            | "Sigma"
            | "Upsilon"
            | "Phi"
            | "Psi"
            | "Omega"
            | "varepsilon"
            | "vartheta"
            | "varpi"
            | "varrho"
            | "varsigma"
            | "varphi"
            | "ldots"
            | "cdots"
            | "vdots"
            | "ddots"
            | "colon"
            | "mid"
            | "lvert"
            | "rvert"
            | "lVert"
            | "rVert"
            | "vert"
            | "Vert"
            | "implies"
            | "iff"
            | "to"
            | "gets"
            | "neq"
            | "leq"
            | "geq"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frac_gold() {
        let n = parse(r"\frac{1}{2}").unwrap();
        assert_eq!(n.gold(), r#"(frac (atom Ord "1") (atom Ord "2"))"#);
    }
}
