//! 富文本粘贴的样例测试：HTML 照真实剪贴板的样子手写。
//!
//! 真实来源的格式差异（Word / WPS 的版本、各家浏览器与网站的标记）只能由用户在真机上
//! 验收，见 `docs/rich-paste-design.md` 第十一节；这里的样本都是合成的（不含真实单位
//! 与人名），按设计文档第四、六节逐条核对规则。

use super::html::{self, HtmlError};
use super::*;

fn context(kind: crate::models::TemplateKind) -> Context {
    Context {
        kind,
        draft_empty: true,
        has_title: false,
        section_level: None,
    }
}

/// 公文（非研究报告）的常见上下文。
fn notice() -> Context {
    context(crate::models::TemplateKind::OfficialLetter)
}

fn convert_html(markup: &str, ctx: &Context) -> Outcome {
    convert(
        &Source::Html {
            html: markup.to_string(),
            plain: String::new(),
        },
        ctx,
    )
    .expect("HTML 应能转换")
}

fn markdown_of(markup: &str) -> String {
    convert_html(markup, &notice()).markdown
}

fn markdown_for(markup: &str, research: bool) -> String {
    let kind = if research {
        crate::models::TemplateKind::ResearchReport
    } else {
        crate::models::TemplateKind::OfficialLetter
    };
    convert_html(markup, &context(kind)).markdown
}

/// Word 列表段落：编号在 `mso-list:Ignore` 的 span 里，外面套着条件注释。
fn word_item(level: u8, marker: &str, text: &str) -> String {
    format!(
        "<p class=MsoListParagraph style='margin-left:21.0pt;text-indent:-21.0pt;\
         mso-list:l0 level{level} lfo1'>\
         <!--[if !supportLists]--><span style='mso-list:Ignore'>{marker}\
         <span style='font:7.0pt \"Times New Roman\"'>&nbsp;&nbsp;</span></span>\
         <!--[endif]-->{text}</p>"
    )
}

/// 1. Word 的三级自动编号：编号被去掉，层级由 `structure` 按编号重新定。
#[test]
fn word_mso_list_numbering_sets_three_heading_levels() {
    let html = format!(
        "<html><body>{}{}{}{}</body></html>",
        word_item(1, "一、", "总体要求"),
        word_item(2, "（一）", "指导思想"),
        word_item(
            3,
            "1.",
            "提高认识。各单位要压实责任，细化任务分工，确保各项措施落地见效，形成闭环。"
        ),
        word_item(1, "二、", "主要任务"),
    );
    let markdown = markdown_of(&html);
    assert!(markdown.contains("## 总体要求\n"), "{markdown}");
    assert!(markdown.contains("### 指导思想\n"), "{markdown}");
    assert!(markdown.contains("#### 提高认识\n"), "{markdown}");
    assert!(markdown.contains("## 主要任务"), "{markdown}");
    // 读起来要像正常公文：编号由程序重新生成，正文里不留手打的编号。
    assert!(!markdown.contains("一、总体要求"), "{markdown}");
    assert!(!markdown.contains("（一）指导思想"), "{markdown}");
    assert!(!markdown.contains("1.提高认识"), "{markdown}");
}

/// 2. Word 标题样式与 `MsoTitle`：标题层级照搬标签。
#[test]
fn word_heading_styles_and_the_title_class_are_kept() {
    let html = "<html><body>\
        <p class=MsoTitle>关于做好冬季森林防火工作的通知</p>\
        <h1>一、总体要求</h1><p>各单位加强巡查。</p>\
        <h2>（一）指导思想</h2><p>坚持安全第一。</p>\
        </body></html>";
    let markdown = markdown_of(html);
    assert!(
        markdown.starts_with("# 关于做好冬季森林防火工作的通知\n"),
        "{markdown}"
    );
    assert!(markdown.contains("## 总体要求\n"), "{markdown}");
    assert!(markdown.contains("### 指导思想\n"), "{markdown}");
}

/// 3. 表格：跨行跨列与表头照原样，只有一格的表格拆回段落。
#[test]
fn tables_keep_spans_and_single_cell_tables_become_paragraphs() {
    let html = "<html><body><table>\
        <tr><th>序号</th><th colspan=2>事项</th></tr>\
        <tr><td>1</td><td rowspan=2>材料报送</td><td>甲</td></tr>\
        <tr><td>2</td><td>乙</td></tr>\
        </table></body></html>";
    let outcome = convert_html(html, &notice());
    assert!(
        outcome.markdown.contains("| 序号 | 事项 |"),
        "{}",
        outcome.markdown
    );
    assert!(
        outcome.markdown.contains("| 1 | 材料报送 | 甲 |"),
        "{}",
        outcome.markdown
    );
    assert!(
        outcome.markdown.contains("| 2 |  | 乙 |"),
        "{}",
        outcome.markdown
    );
    assert_eq!(outcome.stats.tables, 1);

    // Word 的文本框表格常被复制成一格，不能留下只有一格的管道表。
    let html = "<html><body><table><tr><td>\
        <p>一、总体要求</p><p>各单位加强巡查。</p>\
        </td></tr></table></body></html>";
    let outcome = convert_html(html, &notice());
    assert!(!outcome.markdown.contains('|'), "{}", outcome.markdown);
    assert!(
        outcome.markdown.contains("## 总体要求"),
        "{}",
        outcome.markdown
    );
    assert_eq!(outcome.stats.tables, 0);
}

/// 4. 政府网站：`text-indent:2em` 加全角缩进、`<strong>` 包住整行编号标题。
#[test]
fn government_page_indentation_and_bold_titles() {
    let html = "<html><body>\
        <p style=\"text-indent:2em\">　　一、总体要求</p>\
        <p style='text-indent:2em'>　　（一）指导思想</p>\
        <p><strong>二、主要任务</strong></p>\
        <p style='text-indent:2em'>　　各单位要压实责任。</p>\
        </body></html>";
    let markdown = markdown_of(html);
    assert!(markdown.contains("## 总体要求\n"), "{markdown}");
    assert!(markdown.contains("### 指导思想\n"), "{markdown}");
    assert!(markdown.contains("## 主要任务\n"), "{markdown}");
    // 段首缩进（`&nbsp;` 与全角空格）不能留在正文里。
    assert!(!markdown.contains('\u{3000}'), "{markdown}");
    assert!(!markdown.contains('\u{a0}'), "{markdown}");
}

/// 5. 引用块：研究报告保留，其余文种拆成普通段落。
#[test]
fn blockquotes_are_kept_for_research_only() {
    let html = "<html><body><blockquote><p>这是引用的一段材料。</p></blockquote></body></html>";
    assert!(markdown_for(html, true).starts_with("> 这是引用的一段材料。"));
    let notice = markdown_for(html, false);
    assert!(!notice.contains('>'), "{notice}");
    assert!(notice.contains("这是引用的一段材料。"), "{notice}");
}

/// 6. 行内语法：链接留字、斜体去标记、删除线连字丢、图片丢且计数。
#[test]
fn inline_syntax_follows_the_rules() {
    let html = "<html><body><p>详见<a href='http://example.com'>这里的说明</a>，\
        <i>斜体</i>不该留，<del>删掉的旧内容</del>连字一起丢。\
        <img src='a.png'><img src='b.png' alt='配图'></p></body></html>";
    let outcome = convert_html(html, &notice());
    assert!(
        outcome
            .markdown
            .contains("详见这里的说明，斜体不该留，连字一起丢。"),
        "{}",
        outcome.markdown
    );
    assert!(
        !outcome.markdown.contains("example.com"),
        "{}",
        outcome.markdown
    );
    assert!(!outcome.markdown.contains("斜体*"), "{}", outcome.markdown);
    assert!(
        !outcome.markdown.contains("删掉的旧内容"),
        "{}",
        outcome.markdown
    );
    assert!(!outcome.markdown.contains("配图"), "{}", outcome.markdown);
    assert_eq!(outcome.stats.skipped_images, 2);
}

/// 7. 嵌套列表与段内换行（`<br>`）。
#[test]
fn nested_lists_and_line_breaks() {
    let html = "<html><body>\
        <ul><li>第一项<ul><li>子项</li></ul></li><li>第二项</li></ul>\
        <p>上一行<br>下一行</p>\
        </body></html>";
    let markdown = markdown_of(html);
    assert!(markdown.contains("- 第一项"), "{markdown}");
    // 子项保持缩进，没被压平成同级。
    assert!(markdown.contains("\n  - 子项"), "{markdown}");
    assert!(markdown.contains("- 第二项"), "{markdown}");
    // `<br>` 拆成两段：接回一行会把「张某<br>联系电话」接成「张某联系电话」。
    assert!(markdown.contains("上一行\n\n下一行"), "{markdown}");
}

/// 有序列表嵌套：改成 `- ` 列表，子项不被结构还原当成段首编号压平。
#[test]
fn ordered_lists_keep_their_nesting() {
    let markdown =
        markdown_of("<ol><li>甲项<ol><li>子项一</li><li>子项二</li></ol></li><li>乙项</li></ol>");
    assert!(markdown.contains("- 甲项"), "{markdown}");
    assert!(markdown.contains("\n  - 子项一"), "{markdown}");
    assert!(markdown.contains("\n  - 子项二"), "{markdown}");
    assert!(markdown.contains("- 乙项"), "{markdown}");
}

/// 只装着行内内容的 `div` 各自成段（网页与编辑器常这样分段）。
#[test]
fn inline_only_divs_are_separate_paragraphs() {
    let markdown =
        markdown_of("<div><div><span>第一段。</span></div><div><span>第二段。</span></div></div>");
    assert_eq!(markdown.trim(), "第一段。\n\n第二段。");
    let markdown = markdown_of("<div>引子文字<div>第二段。</div></div>");
    assert_eq!(markdown.trim(), "引子文字\n\n第二段。");
}

/// 字段之间的全角空格与 `<br>` 都不能让两个字段接成一串。
#[test]
fn field_separators_survive() {
    for html in [
        "<p>联系人：张某　联系电话：12345678</p>",
        "<p>联系人：张某&nbsp;&nbsp;联系电话：12345678</p>",
    ] {
        let markdown = markdown_of(html);
        assert_eq!(
            markdown.trim(),
            "联系人：张某　联系电话：12345678",
            "{html}"
        );
    }
    let markdown = markdown_of("<p>联系人：张某<br>联系电话：12345678</p>");
    assert_eq!(markdown.trim(), "联系人：张某\n\n联系电话：12345678");
    // 标题里的换行接成一行。
    let markdown = markdown_of("<h1>关于做好冬季<br>森林防火工作的通知</h1><p>正文。</p>");
    assert!(
        markdown.starts_with("# 关于做好冬季森林防火工作的通知"),
        "{markdown}"
    );
}

/// 从 VS Code 这类编辑器复制的 Markdown 带着一份 HTML，不能走 HTML 转换。
#[test]
fn markdown_copied_with_html_is_pasted_verbatim() {
    let outcome = convert(
        &Source::Html {
            html: "<div><div><span>## 一级标题</span></div><div><span>正文**加粗**。</span></div></div>"
                .to_string(),
            plain: "## 一级标题\n正文**加粗**。".to_string(),
        },
        &notice(),
    );
    assert!(outcome.is_none());
}

/// 8. 微信公众号的层层 `section` / `span`，以及 Excel 的表格 HTML。
#[test]
fn wechat_sections_and_spreadsheet_tables() {
    let html = "<html><body><section><section><span><span>\
        <p><span><span>一、总体要求</span></span></p>\
        <p><span>各单位加强巡查。</span></p>\
        <p><br></p>\
        </span></span></section></section></body></html>";
    let markdown = markdown_of(html);
    assert!(markdown.contains("## 总体要求\n"), "{markdown}");
    assert!(markdown.contains("各单位加强巡查。"), "{markdown}");

    // Excel 复制出来的是带表头与合并单元格的表格。
    let html = "<html><body><table border=0 cellpadding=0 cellspacing=0>\
        <tr><td class=xl65>序号</td><td class=xl65>事项</td><td class=xl65>备注</td></tr>\
        <tr><td class=xl65>1</td><td class=xl65 colspan=2>材料报送</td></tr>\
        </table></body></html>";
    let outcome = convert_html(html, &notice());
    assert!(
        outcome.markdown.contains("| 序号 | 事项 | 备注 |"),
        "{}",
        outcome.markdown
    );
    assert!(
        outcome.markdown.contains("| 1 | 材料报送 |  |"),
        "{}",
        outcome.markdown
    );
    assert_eq!(outcome.stats.tables, 1);
}

/// 9. `script` / `style` / 注释丢掉；整页 HTML 里 `<style>` 的类规则能生效。
#[test]
fn scripts_styles_and_comments_are_dropped_while_stylesheets_apply() {
    let html = "<html><head><style>.strong { font-weight: bold }</style></head><body>\
        <script>var secret = 1;</script>\
        <!-- 隐藏的注释 -->\
        <p><o:p>&nbsp;</o:p></p>\
        <p><span class='strong'>不得转录</span>内部材料。</p>\
        </body></html>";
    let markdown = markdown_of(html);
    assert!(!markdown.contains("var secret"), "{markdown}");
    assert!(!markdown.contains("隐藏的注释"), "{markdown}");
    assert!(!markdown.contains("font-weight"), "{markdown}");
    assert!(markdown.contains("**不得转录**内部材料。"), "{markdown}");
}

/// 10. 畸形 HTML 不 panic；超过节点数或深度上限时退回纯文本。
#[test]
fn malformed_html_is_repaired_and_limits_fall_back_to_plain_text() {
    // 不闭合、乱嵌套：html5ever 按规范修复，转换成正常结构。
    let html = "<html><body><p>一、总体要求<p>各单位加强巡查<div>二、主要任务</p></span></div></body></html>";
    let markdown = markdown_of(html);
    assert!(markdown.contains("## 总体要求"), "{markdown}");
    assert!(markdown.contains("## 主要任务"), "{markdown}");

    // 上限：单元测试用小额度，业务代码用模块里的常量。
    let html = "<html><body><div><div><p>正文</p></div></div></body></html>";
    assert_eq!(
        html::parse_with_limits(html, MAX_TEST_NODES, MAX_TEST_DEPTH).err(),
        Some(HtmlError::TooManyNodes)
    );
    assert_eq!(
        html::parse_with_limits(html, 1000, 2).err(),
        Some(HtmlError::TooDeep)
    );

    // 深度超限的整页：HTML 这条路失败，自动退回纯文本（这里纯文本也带编号）。
    let deep = format!(
        "<html><body>{}{}各单位加强巡查。{}</body></html>",
        "<div>".repeat(200),
        "一、总体要求",
        "</div>".repeat(200)
    );
    let outcome = convert(
        &Source::Html {
            html: deep,
            plain: "一、总体要求\n（一）指导思想\n各单位加强巡查。\n".to_string(),
        },
        &notice(),
    )
    .expect("HTML 超限时应退回纯文本");
    assert!(
        outcome.markdown.contains("## 总体要求"),
        "{}",
        outcome.markdown
    );
}

/// 11. 纯文本：微信式多行编号能识别；已是 Markdown、只差空白都原样。
#[test]
fn plain_text_numbering_and_markdown_detection() {
    let wechat = "　　一、总体要求\n　　（一）指导思想\n　　各单位加强巡查。\n";
    let outcome =
        convert(&Source::Plain(wechat.to_string()), &notice()).expect("多行纯文本应被识别");
    assert!(
        outcome.markdown.contains("## 总体要求"),
        "{}",
        outcome.markdown
    );
    assert!(
        outcome.markdown.contains("### 指导思想"),
        "{}",
        outcome.markdown
    );

    assert!(convert(&Source::Plain("# 标题\n\n正文".into()), &notice()).is_none());
    assert!(
        convert(
            &Source::Plain("各单位加强巡查。\n\n确保安全。".into()),
            &notice()
        )
        .is_none()
    );
}

/// 12. 粘贴选项随稿子状态变化：已有标题时不认标题，稿子不为空时不剥版头版记。
#[test]
fn paste_options_follow_the_draft_state() {
    let html =
        "<html><body><p>关于做好冬季森林防火工作的通知</p><p>各单位加强巡查。</p></body></html>";
    // 空稿子：开头那行像公文标题，升成 `# `。
    assert!(markdown_of(html).starts_with("# 关于做好冬季森林防火工作的通知"));

    // 稿子里已经有 `# ` 标题：不能再来一个。
    let ctx = Context {
        kind: crate::models::TemplateKind::OfficialLetter,
        draft_empty: false,
        has_title: true,
        section_level: None,
    };
    let markdown = convert_html(html, &ctx).markdown;
    assert!(!markdown.starts_with("# "), "{markdown}");
    assert!(
        markdown.starts_with("关于做好冬季森林防火工作的通知"),
        "{markdown}"
    );

    // 稿子不为空：文末的落款与成文日期是作者刚写的，不能被当成版记剥掉。
    let html = "<html><body><p>一、总体要求</p><p>各单位加强巡查。</p>\
        <p>某某市人民政府办公室</p><p>2026年10月1日</p></body></html>";
    let html_markdown = |draft_empty| {
        convert_html(
            html,
            &Context {
                kind: crate::models::TemplateKind::OfficialLetter,
                draft_empty,
                has_title: true,
                section_level: None,
            },
        )
        .markdown
    };
    assert!(
        html_markdown(false).contains("2026年10月1日"),
        "稿子不为空时不该剥落款"
    );
    assert!(
        !html_markdown(true).contains("2026年10月1日"),
        "空稿子按整篇导入剥版记"
    );
}

/// 节点与深度的测试额度（业务代码用 `html.rs` 里的常量）。
const MAX_TEST_NODES: usize = 3;
const MAX_TEST_DEPTH: usize = 64;
