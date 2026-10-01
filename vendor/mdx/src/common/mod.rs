//! 公共工具：被 parser、docx_research、docx_official、typst_research 共享的纯逻辑。

#![allow(dead_code)]

pub mod ast;
pub mod citation;
pub mod crossref;
pub mod docx_image;
pub mod figure_size;
pub mod front_matter;
pub mod heading;
pub mod images;
pub mod inline;
pub mod markers;
pub mod numbering;
pub mod parts;
pub mod quote;
pub mod quotes;
pub mod table;
pub mod table_layout;
