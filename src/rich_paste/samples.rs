//! 剪贴板样本的夹具测试：`tests/fixtures/paste/<名字>.html` + `<名字>.expected.md`。
//!
//! 样本都是合成的（Word 片段、Word 整页、政府网站、微信公众号、Excel、点分编号、跳号、
//! 网页文章、白头件列表、研究报告文框），不含真实单位与人名；采样真实剪贴板的工具与脱敏
//! 流程见同目录的 README.md。

use super::*;
use std::path::{Path, PathBuf};

fn context() -> Context {
    Context {
        kind: crate::models::TemplateKind::OfficialLetter,
        draft_empty: true,
        has_title: false,
        section_level: None,
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/paste")
}

fn sample_paths() -> Vec<PathBuf> {
    let root = root();
    let mut paths = std::fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("读不到样本目录 {}：{error}", root.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn actual_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/paste-samples")
        .join(format!("{stem}.actual.md"))
}

/// HTML 样本 → 空稿、非研究报告下的 Markdown。
fn convert_sample(path: &Path) -> String {
    let html = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()));
    convert(
        &Source::Html {
            html,
            plain: String::new(),
        },
        &context(),
    )
    .unwrap_or_else(|| panic!("{} 没有转换结果", path.display()))
    .markdown
}

/// 同一份样本按研究报告的上下文再转一遍：文框只在这条路上出现。
fn convert_research(path: &Path) -> String {
    convert(
        &Source::Html {
            html: std::fs::read_to_string(path).expect("读样本"),
            plain: String::new(),
        },
        &Context {
            kind: crate::models::TemplateKind::ResearchReport,
            draft_empty: true,
            has_title: false,
            section_level: None,
        },
    )
    .expect("研究报告上下文应能转换")
    .markdown
}

/// 读一份样本的 HTML。
fn read_sample(name: &str) -> String {
    let path = root().join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()))
}

/// 同一份网页文章粘到稿子 `##` 一节下面：样式标题整体往下平移，不再出现 `# `。
#[test]
fn web_headings_shift_under_the_draft_section() {
    let markdown = convert(
        &Source::Html {
            html: read_sample("web-headings.html"),
            plain: String::new(),
        },
        &Context {
            kind: crate::models::TemplateKind::OfficialLetter,
            draft_empty: false,
            has_title: true,
            section_level: Some(2),
        },
    )
    .expect("网页文章应能转换")
    .markdown;
    assert!(markdown.contains("### 背景"), "{markdown}");
    assert!(markdown.contains("#### 现状"), "{markdown}");
    assert!(markdown.contains("### 建议"), "{markdown}");
    assert!(!markdown.contains("\n# "), "{markdown}");
}

/// 白头件里粘贴列表：列表项转成各自独立的普通段落，不自动补「一是二是」。
#[test]
fn whitepaper_paste_turns_lists_into_paragraphs() {
    let markdown = convert(
        &Source::Html {
            html: read_sample("whitepaper-list.html"),
            plain: String::new(),
        },
        &Context {
            kind: crate::models::TemplateKind::WhitePaper,
            draft_empty: false,
            has_title: true,
            section_level: Some(2),
        },
    )
    .expect("白头件内容应能转换")
    .markdown;
    assert!(!markdown.contains("- "), "{markdown}");
    assert!(markdown.contains("第一项工作：完善制度"), "{markdown}");
    assert!(markdown.contains("每月通报一次进展"), "{markdown}");
    assert!(markdown.contains("第三项工作：严格问责"), "{markdown}");
}

/// 研究报告里的文框：`<aside>`、单格表格、有框线或底纹的容器，只要第一段是整段加粗
/// 的短标题就写成 `> [!名称] 标题`；名称从标题前缀里取，取不到写「专栏」。
#[test]
fn research_boxes_come_from_aside_tables_and_styled_containers() {
    let markdown = convert_research(&root().join("research-box.html"));
    assert!(
        markdown.contains("> [!案例] 某市“场景牵引”授权运营做法"),
        "{markdown}"
    );
    assert!(
        markdown.contains("> [!专栏] 新加坡的数据治理经验"),
        "{markdown}"
    );
    assert!(
        markdown.contains("> [!做法]\n>\n> 先把台账建起来"),
        "{markdown}"
    );
    assert!(
        markdown.contains("> [!延伸阅读] 数据要素流通的三条路径"),
        "{markdown}"
    );
    // 没有整段加粗短标题的候选不是文框。
    assert!(
        !markdown.contains("[!专栏] 这一段的标题没有加粗"),
        "{markdown}"
    );
    assert!(!markdown.contains("这是加粗标题]"), "{markdown}");
}

/// 合成样本（Word 片段、Word 整页、政府网站、微信公众号、Excel、点分编号、跳号、
/// 网页文章、白头件列表、研究报告文框）逐个比对期望输出。
#[test]
fn paste_samples_match_expected_markdown() {
    let paths = sample_paths();
    assert!(
        paths.len() >= 5,
        "样本目录至少要 5 份 HTML，实际 {} 份",
        paths.len()
    );
    for path in paths {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let actual = convert_sample(&path);
        let output = actual_path(&path);
        std::fs::create_dir_all(output.parent().expect("有上级目录")).expect("建输出目录");
        std::fs::write(&output, &actual).expect("写实际输出");
        let expected_path = path.with_file_name(format!("{stem}.expected.md"));
        let expected = std::fs::read_to_string(&expected_path).unwrap_or_else(|_| {
            panic!(
                "缺少期望文件 {}；先跑 cargo test --locked dump_paste_sample_outputs -- --ignored 审阅实际输出",
                expected_path.display()
            )
        });
        assert_eq!(
            actual.trim_end(),
            expected.trim_end(),
            "样本 {stem} 与期望不一致；实际输出见 {}",
            output.display()
        );
    }
}

/// 只导出样本的实际转换结果，供人工审阅后写成 `.expected.md`；不参与普通测试。
#[test]
#[ignore = "用 --ignored 导出样本实际输出，便于人工审阅后更新期望文件"]
fn dump_paste_sample_outputs() {
    for path in sample_paths() {
        let actual = convert_sample(&path);
        let output = actual_path(&path);
        std::fs::create_dir_all(output.parent().expect("有上级目录")).expect("建输出目录");
        std::fs::write(&output, &actual).expect("写实际输出");
        eprintln!("{} -> {}", path.display(), output.display());
    }
}

/// 采样工具：把当前剪贴板里的 HTML 存成样本。用法见 `tests/fixtures/paste/README.md`。
#[test]
#[ignore = "读当前剪贴板 HTML 写成样本，用 GONGWEN_PASTE_SAMPLE 指定文件名"]
fn capture_clipboard_html() {
    let name = std::env::var("GONGWEN_PASTE_SAMPLE").expect(
        "请用 GONGWEN_PASTE_SAMPLE=样本名 指定输出文件名，例如 \
         GONGWEN_PASTE_SAMPLE=word-notice-wps；先复制要采样的内容",
    );
    let html = crate::rich_paste::clipboard::read_platform_html()
        .expect("剪贴板里没有 HTML（先在 Word / 浏览器里复制一段内容）");
    let path = root().join(format!("{name}.html"));
    std::fs::write(&path, html).expect("写样本文件");
    eprintln!("已写入 {}；记得脱敏后再补 .expected.md", path.display());
}
