//! 帮助中心的静态内容：章节正文与配图都用 `include_*` 打进二进制，离线可读。
//!
//! 正文以 `assets/help/` 为准；`docs/help/` 下另有一份同名副本，配图也重复了
//! 一整套（6.4 MB）。新增或改动章节时三处都要同步：`assets/help/*.md`、本文件的
//! [`CHAPTERS`]、`docs/help/`——副本靠手工维护，漏一处两边就不一致了。
//!
//! 图的可编辑源在 `docs/help/diagrams/`（drawio + 生成脚本），改图从那里出。

/// 章节所属的部分。顺序即手册的阅读顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    /// 走进公文助手。
    Intro,
    /// 写一篇公文。
    Writing,
    /// AI 能力。
    Ai,
    /// 词表与知识。
    Lexicon,
    /// 稿件与设置。
    Ops,
    /// 附录。
    Appendix,
}

impl Part {
    pub(crate) const ALL: [Part; 6] = [
        Part::Intro,
        Part::Writing,
        Part::Ai,
        Part::Lexicon,
        Part::Ops,
        Part::Appendix,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Part::Intro => "第一部分 走进公文助手",
            Part::Writing => "第二部分 写一篇公文",
            Part::Ai => "第三部分 AI 能力",
            Part::Lexicon => "第四部分 词表与知识",
            Part::Ops => "第五部分 稿件与设置",
            Part::Appendix => "附录",
        }
    }
}

/// 一章帮助正文。`id` 同时是文件名主干，跨章链接用它寻址。
pub(crate) struct Chapter {
    pub(crate) id: &'static str,
    pub(crate) part: Part,
    pub(crate) title: &'static str,
    /// 目录里标题下的一句话摘要。
    pub(crate) summary: &'static str,
    pub(crate) body: &'static str,
}

/// 配图：markdown 里写 `![图注](images/xxx.png)`，`key` 带 `images/` 前缀。
pub(crate) struct HelpImage {
    pub(crate) key: &'static str,
    pub(crate) bytes: &'static [u8],
}

macro_rules! chapter {
    ($id:literal, $part:expr, $title:literal, $summary:literal) => {
        Chapter {
            id: $id,
            part: $part,
            title: $title,
            summary: $summary,
            body: include_str!(concat!("../../assets/help/", $id, ".md")),
        }
    };
}

pub(crate) const CHAPTERS: &[Chapter] = &[
    chapter!(
        "01-intro",
        Part::Intro,
        "公文助手是什么",
        "定位、设计理念与 AI 三条红线"
    ),
    chapter!(
        "02-ui",
        Part::Intro,
        "界面总览",
        "五区布局、标签模型与十二套主题"
    ),
    chapter!(
        "03-quickstart",
        Part::Intro,
        "五分钟出第一份稿子",
        "从接模型到导出签发稿的最短路径"
    ),
    chapter!(
        "04-workflow",
        Part::Intro,
        "办文全流程与 SOP",
        "六步办文与七阶段办理进度"
    ),
    chapter!(
        "05-profiles",
        Part::Writing,
        "立稿与要素填报",
        "七种文种、版头主体版记与密级规则"
    ),
    chapter!(
        "06-editor",
        Part::Writing,
        "拟稿：源码编辑与五种视图",
        "编辑器、视图模式与查找替换"
    ),
    chapter!(
        "07-ribbon",
        Part::Writing,
        "功能区与插入构件",
        "七个分区与公文构件的插入"
    ),
    chapter!(
        "08-research",
        Part::Writing,
        "研究报告专章",
        "区段、交叉引用、文献与数学公式"
    ),
    chapter!(
        "09-proofread",
        Part::Writing,
        "核稿：审校与校验",
        "三档提示、要素校验与存疑清零"
    ),
    chapter!(
        "10-versions",
        Part::Writing,
        "版本、对照与花脸稿",
        "提交版本、两版对照与改稿痕迹"
    ),
    chapter!(
        "11-export",
        Part::Writing,
        "导出、打印与文件命名",
        "三种格式、PDF 编译与成品入口"
    ),
    chapter!(
        "12-ai-guard",
        Part::Ai,
        "AI 的边界与保障",
        "三条红线、事实闸门与采纳自检"
    ),
    chapter!(
        "13-ai-workbench",
        Part::Ai,
        "AI 起草工作台",
        "五种工作流与事实单确认流程"
    ),
    chapter!(
        "14-ai-polish",
        Part::Ai,
        "AI 优化、提示词与文字复核",
        "提示词管理与小模型逐句复核"
    ),
    chapter!(
        "15-vocabulary",
        Part::Lexicon,
        "标准词库",
        "单位树、人员与 Excel 导入导出"
    ),
    chapter!(
        "16-proofread-table",
        Part::Lexicon,
        "校对词表",
        "六组分类、三级判定与命中条件"
    ),
    chapter!(
        "17-lexicon",
        Part::Lexicon,
        "公文词表与输入法",
        "扫描收词、省键数与小鹤码表"
    ),
    chapter!(
        "18-knowledge",
        Part::Lexicon,
        "知识库与问答",
        "导入、索引、检索测试与知识库问答"
    ),
    chapter!(
        "19-manuscripts",
        Part::Ops,
        "稿件管理",
        "状态机、盖章附件与 ZIP 迁移"
    ),
    chapter!("20-settings", Part::Ops, "设置全解", "十二个分区逐项说明"),
    chapter!(
        "21-data",
        Part::Ops,
        "数据、备份与迁移",
        "数据存放路径与备份建议"
    ),
    chapter!(
        "a-shortcuts",
        Part::Appendix,
        "附录 A 快捷键一览",
        "全部快捷键速查"
    ),
    chapter!(
        "b-faq",
        Part::Appendix,
        "附录 B 常见问题",
        "高频问题与排查思路"
    ),
    chapter!(
        "c-glossary",
        Part::Appendix,
        "附录 C 术语表",
        "文种与办文用语速查"
    ),
];

/// 配图清单。`include_bytes!` 不能进宏展开，只能逐条列出；新增配图时
/// 在这里补一行，并保证文件已放在 `assets/help/images/`。
pub(crate) const IMAGES: &[HelpImage] = &[
    HelpImage {
        key: "images/ui-hero.png",
        bytes: include_bytes!("../../assets/help/images/ui-hero.png"),
    },
    HelpImage {
        key: "images/ui-elements.png",
        bytes: include_bytes!("../../assets/help/images/ui-elements.png"),
    },
    HelpImage {
        key: "images/ui-revision.png",
        bytes: include_bytes!("../../assets/help/images/ui-revision.png"),
    },
    HelpImage {
        key: "images/ui-preview-nav.png",
        bytes: include_bytes!("../../assets/help/images/ui-preview-nav.png"),
    },
    HelpImage {
        key: "images/ui-versions.png",
        bytes: include_bytes!("../../assets/help/images/ui-versions.png"),
    },
    HelpImage {
        key: "images/ui-lexicon.png",
        bytes: include_bytes!("../../assets/help/images/ui-lexicon.png"),
    },
    HelpImage {
        key: "images/ui-settings.png",
        bytes: include_bytes!("../../assets/help/images/ui-settings.png"),
    },
    HelpImage {
        key: "images/ui-themes.png",
        bytes: include_bytes!("../../assets/help/images/ui-themes.png"),
    },
    HelpImage {
        key: "images/diag-concept.png",
        bytes: include_bytes!("../../assets/help/images/diag-concept.png"),
    },
    HelpImage {
        key: "images/diag-layout.png",
        bytes: include_bytes!("../../assets/help/images/diag-layout.png"),
    },
    HelpImage {
        key: "images/diag-quickstart.png",
        bytes: include_bytes!("../../assets/help/images/diag-quickstart.png"),
    },
    HelpImage {
        key: "images/diag-sop.png",
        bytes: include_bytes!("../../assets/help/images/diag-sop.png"),
    },
    HelpImage {
        key: "images/diag-profile.png",
        bytes: include_bytes!("../../assets/help/images/diag-profile.png"),
    },
    HelpImage {
        key: "images/diag-ribbon.png",
        bytes: include_bytes!("../../assets/help/images/diag-ribbon.png"),
    },
    HelpImage {
        key: "images/diag-research.png",
        bytes: include_bytes!("../../assets/help/images/diag-research.png"),
    },
    HelpImage {
        key: "images/diag-export.png",
        bytes: include_bytes!("../../assets/help/images/diag-export.png"),
    },
    HelpImage {
        key: "images/diag-ai-flow.png",
        bytes: include_bytes!("../../assets/help/images/diag-ai-flow.png"),
    },
    HelpImage {
        key: "images/diag-ai-workbench.png",
        bytes: include_bytes!("../../assets/help/images/diag-ai-workbench.png"),
    },
    HelpImage {
        key: "images/diag-manuscript.png",
        bytes: include_bytes!("../../assets/help/images/diag-manuscript.png"),
    },
    HelpImage {
        key: "images/diag-data.png",
        bytes: include_bytes!("../../assets/help/images/diag-data.png"),
    },
    HelpImage {
        key: "images/diag-editor-views.png",
        bytes: include_bytes!("../../assets/help/images/diag-editor-views.png"),
    },
];

/// 按 `key` 取配图字节。markdown 里的相对路径原样作 key。
pub(crate) fn image_bytes(key: &str) -> Option<&'static [u8]> {
    IMAGES
        .iter()
        .find(|img| img.key == key)
        .map(|img| img.bytes)
}

/// 按章节 id 找下标。
pub(crate) fn index_of(id: &str) -> Option<usize> {
    CHAPTERS.iter().position(|ch| ch.id == id)
}

/// 按章节 id 取章名，供跨章链接的悬停提示用。
pub(crate) fn title_of(id: &str) -> Option<&'static str> {
    CHAPTERS.iter().find(|ch| ch.id == id).map(|ch| ch.title)
}
