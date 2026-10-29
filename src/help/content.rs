//! 帮助正文：`assets/help/` 下的一组研究报告 Markdown，按文件名升序拼成一篇。
//!
//! 这一组文件就是一篇普通的研究报告，写法与起草页的研究报告完全一样（见技能包
//! `research-report.md`）：`00-front.md` 放报告题名、目录与摘要，其后每章一个文件，
//! 部分用 `# 部分名`、章用 `## 章名 {#chap:…}`，附录前写 `<!-- [附录] -->`，跨章引用写
//! `第{@chap:…}章`。示意图一律写成 Mermaid 围栏；只有界面截图是位图，放在
//! `assets/help/images/`，以 `help-` 打头。整个文件夹也能用「从文件夹新建研究报告」
//! 原样导入成稿件，在起草页边看预览边改。
//!
//! 正文与截图都编译进二进制，离线可读。新增一章：在 `assets/help/` 放好文件，再在
//! [`FILES`] 里按文件名顺序补一行——测试会核对两边一致。

use crate::models::{DraftInput, ResearchMetadata, TemplateKind};
use anyhow::{Context, Result};
use std::sync::OnceLock;

macro_rules! help_file {
    ($name:literal) => {
        ($name, include_str!(concat!("../../assets/help/", $name)))
    };
}

/// 帮助正文的各个文件，顺序即拼接顺序（文件名升序，与「从文件夹新建」一致）。
pub(crate) const FILES: &[(&str, &str)] = &[
    help_file!("00-front.md"),
    help_file!("01-intro.md"),
    help_file!("02-ui.md"),
    help_file!("03-quickstart.md"),
    help_file!("04-workflow.md"),
    help_file!("05-profiles.md"),
    help_file!("06-editor.md"),
    help_file!("07-ribbon.md"),
    help_file!("08-research.md"),
    help_file!("09-proofread.md"),
    help_file!("10-versions.md"),
    help_file!("11-export.md"),
    help_file!("12-ai-guard.md"),
    help_file!("13-ai-workbench.md"),
    help_file!("14-ai-polish.md"),
    help_file!("15-vocabulary.md"),
    help_file!("16-proofread-table.md"),
    help_file!("17-lexicon.md"),
    help_file!("18-knowledge.md"),
    help_file!("19-manuscripts.md"),
    help_file!("20-settings.md"),
    help_file!("21-data.md"),
    help_file!("22-send-package.md"),
    help_file!("23-task-assistant.md"),
    help_file!("a-shortcuts.md"),
    help_file!("b-faq.md"),
    help_file!("c-glossary.md"),
];

macro_rules! help_image {
    ($name:literal) => {
        (
            $name,
            include_bytes!(concat!("../../assets/help/images/", $name)),
        )
    };
}

/// 界面截图。正文里写 `![图题](images/help-xxx.png)`；看帮助时先释放到用户配置目录的
/// `images/` 下（[`install_images`]），预览与导出 PDF 就和普通稿件的插图同一条路。
pub(crate) const IMAGES: &[(&str, &[u8])] = &[
    help_image!("help-ui-elements.png"),
    help_image!("help-ui-hero.png"),
    help_image!("help-ui-lexicon.png"),
    help_image!("help-ui-preview-nav.png"),
    help_image!("help-ui-revision.png"),
    help_image!("help-ui-versions.png"),
];

/// 拼好的整篇正文。文件之间空一行，免得前一篇末段与后一篇首行粘成一段。
pub(crate) fn markdown() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        FILES
            .iter()
            .map(|(_, text)| text.trim_end())
            .collect::<Vec<_>>()
            .join("\n\n")
            + "\n"
    })
}

/// 帮助这篇研究报告的文档要素：封面上印的那几样。题名取正文里的 `#`。
pub(crate) fn draft() -> DraftInput {
    DraftInput {
        kind: TemplateKind::ResearchReport,
        research: ResearchMetadata {
            security: "公开".into(),
            file_type: "使用手册".into(),
            version: format!("V{}", env!("CARGO_PKG_VERSION")),
            institution: "公文助手".into(),
            ..ResearchMetadata::default()
        },
        ..DraftInput::default()
    }
}

/// 把截图写进用户配置目录的 `images/`。已有同样内容的文件就不动；升级后截图换了，
/// 按字节比对会覆盖成新的。
pub(crate) fn install_images() -> Result<()> {
    let dir = crate::images::image_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("无法创建图片目录 {}", dir.display()))?;
    for (name, bytes) in IMAGES {
        let path = dir.join(name);
        if std::fs::read(&path).is_ok_and(|existing| existing == *bytes) {
            continue;
        }
        std::fs::write(&path, bytes)
            .with_context(|| format!("无法写入帮助截图 {}", path.display()))?;
    }
    Ok(())
}
