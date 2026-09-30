//! Equation counters, `\tag`, and `\label` / `\ref` (two-pass).

use std::collections::HashMap;

use crate::parser::{EnvRow, EqNumber, MathNode, MatrixStyle};

/// How auto equation numbers are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberStyle {
    /// `(1)`, `(2)`, …
    Arabic,
    /// `(i)`, `(ii)`, …
    Roman,
    /// `(a)`, `(b)`, …
    Alphabetic,
}

/// Wrapper around the number body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberFormat {
    /// `(1)`
    Parenthesized,
    /// `[1]`
    Bracketed,
    /// `1`
    Plain,
}

/// Counter style for a render (or a sequence of [`layout_with_numbering`](super::layout_with_numbering) calls).
///
/// # Examples
///
/// ```
/// use latex_rust::{NumberFormat, NumberStyle, NumberingConfig};
///
/// let cfg = NumberingConfig::new();
/// assert_eq!(cfg.style, NumberStyle::Arabic);
/// assert_eq!(cfg.start, 1);
/// assert_eq!(cfg.format, NumberFormat::Parenthesized);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumberingConfig {
    /// Digit / roman / letter.
    pub style: NumberStyle,
    /// First auto number (usually 1).
    pub start: usize,
    /// Parentheses, brackets, or none.
    pub format: NumberFormat,
}

impl Default for NumberingConfig {
    fn default() -> Self {
        Self {
            style: NumberStyle::Arabic,
            start: 1,
            format: NumberFormat::Parenthesized,
        }
    }
}

impl NumberingConfig {
    /// Arabic, start at 1, parenthesized.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Mutable numbering / label table. Survives across `layout_with_numbering` calls.
///
/// # Examples
///
/// ```
/// use latex_rust::{NumberingConfig, NumberingState};
///
/// let state = NumberingState::new(NumberingConfig::new());
/// assert!(state.label("eq:1").is_none());
/// ```
#[derive(Clone, Debug)]
pub struct NumberingState {
    config: NumberingConfig,
    next: usize,
    labels: HashMap<String, String>,
    assigned: Vec<Option<String>>,
}

impl Default for NumberingState {
    fn default() -> Self {
        Self::new(NumberingConfig::default())
    }
}

impl NumberingState {
    /// Counter starts at `config.start`.
    #[must_use]
    pub fn new(config: NumberingConfig) -> Self {
        let next = config.start;
        Self {
            config,
            next,
            labels: HashMap::new(),
            assigned: Vec::new(),
        }
    }

    /// Formatted number bound to `key`, if `\label{key}` was seen.
    #[must_use]
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }

    pub(crate) fn lookup(&self, key: &str) -> Option<&str> {
        self.label(key)
    }

    pub(crate) fn assigned(&self, i: usize) -> Option<&str> {
        self.assigned.get(i).and_then(|o| o.as_deref())
    }

    /// Walk `node`, assign numbers, fill labels. Returns the index of the first
    /// new assignment (for this tree).
    pub fn collect(&mut self, node: &MathNode) -> usize {
        let start = self.assigned.len();
        collect_node(node, self);
        start
    }

    fn wrap(&self, body: &str) -> String {
        match self.config.format {
            NumberFormat::Parenthesized => format!("({body})"),
            NumberFormat::Bracketed => format!("[{body}]"),
            NumberFormat::Plain => body.to_string(),
        }
    }

    fn auto_body(&self, n: usize) -> String {
        match self.config.style {
            NumberStyle::Arabic => n.to_string(),
            NumberStyle::Roman => to_roman(n),
            NumberStyle::Alphabetic => to_alpha(n),
        }
    }

    fn auto_display(&mut self) -> String {
        let n = self.next;
        self.next = self.next.saturating_add(1);
        self.wrap(&self.auto_body(n))
    }

    fn bind(&mut self, labels: &[String], display: &str) {
        for k in labels {
            self.labels.insert(k.clone(), display.to_string());
        }
    }
}

fn collect_node(node: &MathNode, st: &mut NumberingState) {
    match node {
        MathNode::Matrix(style, _, rows) => collect_matrix(*style, rows, st),
        MathNode::Row(v) | MathNode::Substack(v) => {
            for n in v {
                collect_node(n, st);
            }
        }
        MathNode::Fraction(a, b)
        | MathNode::Superscript(a, b)
        | MathNode::Subscript(a, b)
        | MathNode::CancelTo(a, b) => {
            collect_node(a, st);
            collect_node(b, st);
        }
        MathNode::SubSup(a, b, c) => {
            collect_node(a, st);
            collect_node(b, st);
            collect_node(c, st);
        }
        MathNode::Radical(deg, r) => {
            if let Some(d) = deg {
                collect_node(d, st);
            }
            collect_node(r, st);
        }
        MathNode::Delimited(_, b, _)
        | MathNode::Accent(b, _)
        | MathNode::Color(_, b)
        | MathNode::TextColor(_, b)
        | MathNode::ColorBox(_, b)
        | MathNode::Phantom(_, b)
        | MathNode::Intertext(b)
        | MathNode::Tag { body: b, .. } => collect_node(b, st),
        MathNode::FColorBox(_, _, b) => collect_node(b, st),
        MathNode::Sum(lo, hi) | MathNode::Product(lo, hi) | MathNode::Integral(_, lo, hi) => {
            if let Some(n) = lo {
                collect_node(n, st);
            }
            if let Some(n) = hi {
                collect_node(n, st);
            }
        }
        MathNode::Limit(lo) => {
            if let Some(n) = lo {
                collect_node(n, st);
            }
        }
        MathNode::OverUnder(b, over, under) => {
            collect_node(b, st);
            if let Some(n) = over {
                collect_node(n, st);
            }
            if let Some(n) = under {
                collect_node(n, st);
            }
        }
        MathNode::Atom(_, _)
        | MathNode::SizedDelim(_, _, _)
        | MathNode::Text(_, _)
        | MathNode::Space(_)
        | MathNode::Operator(_, _)
        | MathNode::Symbol(_)
        | MathNode::Strut(_, _)
        | MathNode::Ref(_)
        | MathNode::Label(_)
        | MathNode::NoNumber
        | MathNode::Hline => {}
    }
}

fn collect_matrix(style: MatrixStyle, rows: &[EnvRow], st: &mut NumberingState) {
    for row in rows {
        match row {
            EnvRow::Cells { cells, .. } => {
                for c in cells {
                    collect_node(c, st);
                }
            }
            EnvRow::Intertext(n) => collect_node(n, st),
            EnvRow::Hline => {}
        }
    }
    if style.numbers_rows() {
        for row in rows {
            match row {
                EnvRow::Intertext(_) | EnvRow::Hline => {}
                EnvRow::Cells { number, labels, .. } => {
                    let display = assign(number, st);
                    if let Some(d) = &display {
                        st.bind(labels, d);
                    }
                    st.assigned.push(display);
                }
            }
        }
    } else if style.numbers_once() {
        let mut number = EqNumber::Default;
        let mut labels = Vec::new();
        for row in rows {
            if let EnvRow::Cells {
                number: n,
                labels: l,
                ..
            } = row
            {
                match n {
                    EqNumber::Default => {}
                    other => number = other.clone(),
                }
                labels.extend(l.iter().cloned());
            }
        }
        let display = assign(&number, st);
        if let Some(d) = &display {
            st.bind(&labels, d);
        }
        st.assigned.push(display);
    }
}

fn assign(number: &EqNumber, st: &mut NumberingState) -> Option<String> {
    match number {
        EqNumber::Suppress => None,
        EqNumber::Tag { star, body } => {
            let plain = node_plain(body);
            let display = if *star { plain } else { st.wrap(&plain) };
            Some(display)
        }
        EqNumber::Default => Some(st.auto_display()),
    }
}

fn node_plain(n: &MathNode) -> String {
    match n {
        MathNode::Atom(c, _) => c.to_string(),
        MathNode::Text(s, _) => s.clone(),
        MathNode::Symbol(name) => name.clone(),
        MathNode::Row(v) => v.iter().map(node_plain).collect(),
        MathNode::Tag { body, .. } => node_plain(body),
        other => other.gold(),
    }
}

fn to_roman(mut n: usize) -> String {
    if n == 0 {
        return "0".into();
    }
    let pairs: [(usize, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, g) in pairs {
        while n >= v {
            s.push_str(g);
            n -= v;
        }
    }
    s
}

fn to_alpha(mut n: usize) -> String {
    if n == 0 {
        return "0".into();
    }
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        s.insert(0, char::from(b'a' + (n % 26) as u8));
        n /= 26;
    }
    s
}
