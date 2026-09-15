//! 公共工具：被 parser、tex_official、docx_research、docx_official 共享的纯逻辑。

#![allow(dead_code)]

pub mod ast;
pub mod citation;
pub mod crossref;
pub mod docx_image;
pub mod front_matter;
pub mod heading;
pub mod images;
pub mod inline;
pub mod markers;
pub mod numbering;
pub mod parts;
pub mod quotes;
pub mod table;
pub mod table_layout;
pub mod table_to_longtblr;
