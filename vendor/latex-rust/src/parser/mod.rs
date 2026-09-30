//! LaTeX math → [`MathNode`] AST.
//!
//! Pipeline: [`preprocess`] → [`tokenize`] → typed parse. Unknown or unsupported
//! input is [`ParseError`](crate::error::ParseError), never a silent partial tree.

mod ast;
mod parse;
mod preproc;
mod token;

pub use ast::{
    AccentKind, AtomKind, ColSpec, DelimSize, Delimiter, EnvRow, EqNumber, IntegralKind, MathNode,
    MatrixStyle, PhantomKind, SpaceKind, TextStyle,
};
pub use parse::{
    parse, parse_with_colors, parse_with_options, ParseOptions, DEFAULT_MAX_NESTING_DEPTH,
};
pub use preproc::preprocess;
pub use token::{format_tokens, tokenize, Token};
