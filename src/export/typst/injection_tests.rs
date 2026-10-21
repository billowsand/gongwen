//! 注入防线：用户写的正文只是**数据**，不会变成 Typst / LaTeX 代码。
//!
//! 公文正文会被原样写进模板数据（JSON），模板从数据里取值排版；公式交给
//! 进程内的纯排版器解析。这两条一旦被绕过，用户正文里的 `#let`、`\input`
//! 就能读本机文件或执行命令。这里把「正文只当数据」钉成测试，改动模板与
//! 数据通道时会被测出来。

use super::frame;
use crate::models::{DraftInput, NumberingConfig};
use crate::typst_engine;
use crate::units::UnitDisplay;
use crate::visual_diff::ElementMarks;

/// 一段「若被当成代码就会出事」的正文。
const EVIL: &str = "#let evil = read(\"/etc/passwd\")";

fn fonts() -> typst_engine::FontFamilies {
    typst_engine::FontFamilies {
        title: String::new(),
        heading1: String::new(),
        heading2: String::new(),
        body: String::new(),
        page_number: String::new(),
        bold: None,
        page_number_latin: None,
        fallback: Vec::new(),
    }
}

#[test]
fn 正文里的_typst_代码只作为数据出现() {
    let markdown = format!("# 测试标题\n\n{EVIL}\n");
    let doc = frame::document(
        &DraftInput::default(),
        &markdown,
        &UnitDisplay::new(&[]),
        &NumberingConfig::default(),
        &ElementMarks::default(),
        fonts(),
    );
    let json = serde_json::to_string(&doc).expect("序列化模板数据");
    // 数据里带着这段文字（排出来就是这行字）。行内解析会把它拆成几段，
    // 所以按两处关键片段核对。
    assert!(
        json.contains("#let evil = read") && json.contains("/etc/passwd"),
        "正文应当原样进入模板数据：{json}"
    );
    // 模板源码里没有它：模板是固定的那一份，不掺用户内容。
    assert!(
        !typst_engine::TEMPLATE.contains("evil"),
        "公文模板里不该出现用户正文"
    );
    assert!(
        !typst_engine::RESEARCH_TEMPLATE.contains("evil"),
        "研究报告模板里不该出现用户正文"
    );
}

#[test]
fn 公式里的_tex_命令不被排版器执行() {
    // latex-rust 是纯排版器：没有文件 IO，`\input` 之类的命令只会解析失败。
    let source = "\\input{/etc/passwd}";
    let result = latex_rust::parse(source);
    assert!(
        result.is_err(),
        "\\input 不该被当成可执行命令解析：{result:?}"
    );
}
