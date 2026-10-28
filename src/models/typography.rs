//! 主题、纸张、字体方案、Ribbon 与编号、输入法外观配置。
//!
//! 由 src/models.rs 拆分而来：本文件是模块 `models::typography`，与其它子模块共享
//! `models` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::TemplateKind;
use serde::{Deserialize, Serialize};

/// 界面主题。与 `ThemeName::ALL` 的预设一一对应，缺省为默认的 Claude 奶油。
///
/// 前三项是明色，后三项是深色。深色主题只影响界面外壳，
/// 公文纸面另由 [`PaperMode`] 决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeName {
    #[default]
    // 已移除的主题兼容旧配置，统一回退奶油主题。
    #[serde(
        alias = "sky",
        alias = "lilac",
        alias = "green",
        alias = "everforest",
        alias = "solarized_light",
        alias = "latte",
        alias = "gruvbox_light",
        alias = "dracula",
        alias = "tokyo_night"
    )]
    Claude,
    /// MDEX 纸墨：米白底、墨黑字与终端绿。
    Mdex,
    /// 锤子稿纸：灰棕拟物工具栏、浅木纹底板与横线稿纸。
    Smartisan,
    /// Nord：冷灰蓝，饱和度最低。
    Nord,
    /// Gruvbox Dark：暖褐底 + 橙强调，默认主题的深色亲戚。
    GruvboxDark,
    /// MDEX 夜墨：纸墨的深色版，暖墨底与鼠尾草绿。
    MdexDark,
    /// 跟随 Omarchy：读 Omarchy 当前主题的 `colors.toml` 现算配色，明暗随它。
    /// 只在 Omarchy 上出现，不在 [`ThemeName::ALL`] 里。
    Omarchy,
}

impl ThemeName {
    /// 全部固定预设，顺序与设置页展示一致：先明色后深色。
    /// 「跟随 Omarchy」随桌面而定，单列在外。
    pub const ALL: [ThemeName; 6] = [
        Self::Claude,
        Self::Mdex,
        Self::Smartisan,
        Self::Nord,
        Self::GruvboxDark,
        Self::MdexDark,
    ];
}

/// 屏幕上公文纸面的显示模式。**只影响预览显示**，导出的 DOCX/PDF 一律仍是
/// 白纸黑字红头，不受这里的选择影响。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaperMode {
    /// 跟随界面主题：按主题冷暖与明暗选用本色、宣纸白、夜墨蓝或檀黑棕。
    #[default]
    Follow,
    /// 始终白纸黑字，与打印稿一致。
    Light,
    /// 始终深色纸，长时间盯屏更省眼。
    Dark,
}

impl PaperMode {
    /// 全部可选项，顺序与设置页展示一致。
    pub const ALL: [PaperMode; 3] = [Self::Follow, Self::Light, Self::Dark];

    /// 设置页展示的名字。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Follow => "跟随主题",
            Self::Light => "白纸黑字",
            Self::Dark => "深色纸面",
        }
    }
}

/// Mermaid 图表（流程、泳道、序列、甘特、饼、桑基、时间线、雷达）的配色与线型。
/// 字体、字号不在这里：它们随文种走，
/// 公文用仿宋、研究报告用黑体，并按正文字号定出图内字号。预览与导出同一套。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagramTheme {
    /// 公文用「墨线」，研究报告用「藏青」。
    #[default]
    Auto,
    /// 白底黑线、不填色：最接近手绘的公文流程图，黑白打印、复印都不走样。
    Ink,
    /// 浅灰填色、深灰线条，判断框略深：层次比墨线多一级，打印仍是灰阶。
    Gray,
    /// 藏青线条、雾蓝填色，判断框用米黄：研究报告里克制的一点颜色。
    Navy,
    /// 青瓷绿线条、淡青填色，判断框用米白：比藏青更柔和，适合长报告。
    Celadon,
}

impl DiagramTheme {
    /// 全部可选项，顺序与菜单、设置页展示一致。
    pub const ALL: [DiagramTheme; 5] =
        [Self::Auto, Self::Ink, Self::Gray, Self::Navy, Self::Celadon];

    /// 菜单与设置页展示的名字。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Auto => "按文种自动",
            Self::Ink => "墨线",
            Self::Gray => "素灰",
            Self::Navy => "藏青",
            Self::Celadon => "青瓷",
        }
    }

    /// 悬停说明。
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Auto => "公文用「墨线」，研究报告用「藏青」",
            Self::Ink => "白底黑线、不填色，黑白打印与复印不走样，适合红头公文",
            Self::Gray => "浅灰填色、深灰线条，层次清楚，打印仍是灰阶",
            Self::Navy => "藏青线条、雾蓝填色、米黄判断框，研究报告里克制的一点颜色",
            Self::Celadon => "青瓷绿线条、淡青填色，比藏青柔和，适合长报告",
        }
    }
}

/// Markdown 源码编辑器里某一处可以用的字面。
///
/// 「编辑器字体」是设置里那支通用编辑器字体（没单独选就跟随界面字体）；其余四项
/// 是公文预览与导出用的同一批字面，选了它们，源码里看到的标题、正文就与预览、
/// 打印稿一个模样——不熟悉 Markdown 的人不必在脑子里做一次「标记→版式」的换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorFontFace {
    /// 跟随编辑器字体，即改动此项之前的一贯行为。
    #[default]
    Editor,
    /// 方正小标宋：公文大标题。
    Biaosong,
    /// 黑体：一级标题「一、」。
    Heiti,
    /// 楷体：二级标题「（一）」。
    Kaiti,
    /// 仿宋：正文与三级以下标题。
    Fangsong,
}

impl EditorFontFace {
    /// 全部可选项，顺序与设置页展示一致。
    pub const ALL: [EditorFontFace; 5] = [
        Self::Editor,
        Self::Biaosong,
        Self::Heiti,
        Self::Kaiti,
        Self::Fangsong,
    ];

    /// 设置页展示的名字。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Editor => "编辑器字体",
            Self::Biaosong => "小标宋",
            Self::Heiti => "黑体",
            Self::Kaiti => "楷体",
            Self::Fangsong => "仿宋",
        }
    }
}

/// 源码编辑器里可以分别指定字面的六处 Markdown 元素。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditorFontSlot {
    /// `#` 文档标题。
    Title,
    /// `##` 一级标题。
    Heading1,
    /// `###` 二级标题。
    Heading2,
    /// `####` 及更深的标题。
    Heading3,
    /// 正文、列表与表格单元。
    Body,
    /// `#`、`|`、`**` 等结构标记，以及行内代码与 HTML 注释。
    Mark,
}

impl EditorFontSlot {
    /// 全部元素，顺序与设置页展示一致。
    pub const ALL: [EditorFontSlot; 6] = [
        Self::Title,
        Self::Heading1,
        Self::Heading2,
        Self::Heading3,
        Self::Body,
        Self::Mark,
    ];

    /// 设置页展示的名字。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Title => "文档标题（#）",
            Self::Heading1 => "一级标题（##）",
            Self::Heading2 => "二级标题（###）",
            Self::Heading3 => "三级及以下标题",
            Self::Body => "正文、列表与表格",
            Self::Mark => "标记与行内代码",
        }
    }

    /// 设置页的悬停说明。
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Title => "公文大标题，预览里是方正小标宋",
            Self::Heading1 => "一级标题「一、」，预览里是黑体",
            Self::Heading2 => "二级标题「（一）」，预览里是楷体",
            Self::Heading3 => "三级、四级标题，预览里随正文用仿宋",
            Self::Body => "正文段落、列表项与表格单元，预览里是仿宋",
            Self::Mark => {
                "`#`、`|`、`**` 这些只在源码里出现的符号，以及行内代码、HTML 注释。\
                          它们在成稿里没有对应物，留在编辑器字体上更容易一眼认出"
            }
        }
    }
}

/// 源码编辑器的 Markdown 字面方案：每处元素各用哪支字体。
///
/// 默认六处全是「编辑器字体」，与加入本设置之前的显示完全一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorFontScheme {
    pub title: EditorFontFace,
    pub heading1: EditorFontFace,
    pub heading2: EditorFontFace,
    pub heading3: EditorFontFace,
    pub body: EditorFontFace,
    pub mark: EditorFontFace,
}

impl EditorFontScheme {
    /// 取某一处元素当前用的字面。
    pub(crate) fn face(&self, slot: EditorFontSlot) -> EditorFontFace {
        match slot {
            EditorFontSlot::Title => self.title,
            EditorFontSlot::Heading1 => self.heading1,
            EditorFontSlot::Heading2 => self.heading2,
            EditorFontSlot::Heading3 => self.heading3,
            EditorFontSlot::Body => self.body,
            EditorFontSlot::Mark => self.mark,
        }
    }

    /// 可写引用，供设置页逐项改。
    pub(crate) fn face_mut(&mut self, slot: EditorFontSlot) -> &mut EditorFontFace {
        match slot {
            EditorFontSlot::Title => &mut self.title,
            EditorFontSlot::Heading1 => &mut self.heading1,
            EditorFontSlot::Heading2 => &mut self.heading2,
            EditorFontSlot::Heading3 => &mut self.heading3,
            EditorFontSlot::Body => &mut self.body,
            EditorFontSlot::Mark => &mut self.mark,
        }
    }

    /// 标题按公文字面走，正文与标记仍留在编辑器字体上。
    pub(crate) fn official_headings() -> Self {
        Self {
            title: EditorFontFace::Biaosong,
            heading1: EditorFontFace::Heiti,
            heading2: EditorFontFace::Kaiti,
            heading3: EditorFontFace::Fangsong,
            body: EditorFontFace::Editor,
            mark: EditorFontFace::Editor,
        }
    }

    /// 正文也换成仿宋：源码看起来最接近成稿。
    pub(crate) fn official() -> Self {
        Self {
            body: EditorFontFace::Fangsong,
            ..Self::official_headings()
        }
    }
}

/// 设置页预置的几套字面方案。挑一套就够用，不想逐项配的人不必逐项配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum EditorFontPreset {
    /// 全部用编辑器字体：一眼能看出哪些是 Markdown 标记，是默认。
    #[default]
    Editor,
    /// 只有标题换成公文字面，正文仍是编辑器字体。
    OfficialHeadings,
    /// 标题与正文都用公文字面，最像成稿。
    Official,
}

impl EditorFontPreset {
    /// 全部模板，顺序与设置页展示一致。
    pub const ALL: [EditorFontPreset; 3] = [Self::Editor, Self::OfficialHeadings, Self::Official];

    /// 设置页展示的名字。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Editor => "编辑器字体",
            Self::OfficialHeadings => "标题随公文",
            Self::Official => "与公文一致",
        }
    }

    /// 设置页的悬停说明。
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Editor => "六处全用编辑器字体，Markdown 标记最醒目（默认）",
            Self::OfficialHeadings => {
                "标题用小标宋/黑体/楷体，正文仍用编辑器字体，兼顾像成稿与好编辑"
            }
            Self::Official => "标题与正文都按公文字面排，源码看着最接近打印稿",
        }
    }

    /// 该模板对应的方案。
    pub(crate) fn scheme(self) -> EditorFontScheme {
        match self {
            Self::Editor => EditorFontScheme::default(),
            Self::OfficialHeadings => EditorFontScheme::official_headings(),
            Self::Official => EditorFontScheme::official(),
        }
    }

    /// 方案正好等于哪个模板；逐项改出来的组合返回 `None`（设置页显示「自定义」）。
    pub(crate) fn matching(scheme: &EditorFontScheme) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.scheme() == *scheme)
    }
}

/// 起草页功能区的分区卡。仿 Word：第一行选分区，第二行才是该分区的按钮。
///
/// 记在配置里而不是每篇稿件各记各的——切分区是「我现在要干哪一类活」，
/// 换一篇稿子通常还在干同一类活，每开一篇都要重新切反而累。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RibbonTab {
    /// 常用：存盘、版本、查找、AI、校验。
    #[default]
    Home,
    /// 往光标处插入表格、图片、公文构件、词库词条与符号。
    Insert,
    /// 研究报告专用：mdx research 的区段标记与行内标记。只在研究报告下出现。
    Research,
    /// 改已有文字的 Markdown 标记：标题层级、加粗、列表、清理。
    Format,
    /// 公文编号与 Markdown 编号互转，以及空格、标点、表格等格式清理。
    Clean,
    /// 校验、版本比对、查找、字数。
    Review,
    /// 显示方式、缩放与各个面板的开关。
    View,
    /// 导出与打开成品。
    Output,
}

impl RibbonTab {
    /// 分区卡顺序即屏幕上从左到右的顺序。「研报」紧挨着「插入」：两者干的是
    /// 同一类活（往稿子里放构件），只是一个通用、一个只有研究报告用得上。
    pub const ALL: [Self; 8] = [
        Self::Home,
        Self::Insert,
        Self::Research,
        Self::Format,
        Self::Clean,
        Self::Review,
        Self::View,
        Self::Output,
    ];

    /// 这个分区卡在该文种下出不出现。研报构件（区段标记、锚点、交叉引用、
    /// 文献引用、脚注、表题）只有研究报告认，公文下摆出来纯是干扰。
    pub(crate) fn shown_for(self, kind: TemplateKind) -> bool {
        self != Self::Research || kind.is_research()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Home => "开始",
            Self::Insert => "插入",
            Self::Research => "研报",
            Self::Format => "格式",
            Self::Clean => "清洗",
            Self::Review => "审校",
            Self::View => "视图",
            Self::Output => "输出",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Home => "保存、版本、查找替换、AI 起草与优化、规则校验",
            Self::Insert => "表格、图片、正文与附件标记、词库词条、日期与符号",
            Self::Research => "研究报告的区段标记、锚点、交叉引用、文献引用、脚注与表题",
            Self::Format => "标题层级、加粗、项目符号，以及引号与空行的规范化",
            Self::Clean => "公文编号与 Markdown 编号互转、空格/标点/表格/目录等格式清理",
            Self::Review => "重新校验、版本对照、查找替换、字数统计",
            Self::View => "显示方式、预览缩放、各个面板的开关",
            Self::Output => "按格式导出、打开输出目录与最近一次的成品",
        }
    }
}

/// 源码编辑器字号允许的上下限（px）。
pub const EDITOR_FONT_SIZE_MIN: f32 = 10.0;

pub const EDITOR_FONT_SIZE_MAX: f32 = 24.0;

/// 公文各级标题的编号样式。默认编号链为「一、→（一）→ 1. →（1）」，与
/// GB/T 9704 的四级标题层级一致；导出的 PDF、Word 与界面预览共用同一套选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HeadingNumbering {
    /// 一、二、三
    #[default]
    Chinese,
    /// （一）（二）（三）
    ChineseParen,
    /// （1）（2）（3）
    FullDigitParen,
    /// (1)(2)(3)
    HalfDigitParen,
    /// 1. 2. 3.（正文无空格）
    DecimalDot,
    /// 第1章 第2章
    ChapterDigit,
    /// 第一章 第二章
    ChapterChinese,
    /// 第一部分 第二部分
    Part,
    /// 第1节 第2节
    SectionDigit,
}

impl HeadingNumbering {
    /// 全部可选样式，顺序与设置页下拉展示一致。
    pub const ALL: [HeadingNumbering; 9] = [
        Self::Chinese,
        Self::ChineseParen,
        Self::FullDigitParen,
        Self::HalfDigitParen,
        Self::DecimalDot,
        Self::ChapterDigit,
        Self::ChapterChinese,
        Self::Part,
        Self::SectionDigit,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Chinese => "一、二、三",
            Self::ChineseParen => "（一）（二）（三）",
            Self::FullDigitParen => "（1）（2）（3）",
            Self::HalfDigitParen => "(1)(2)(3)",
            Self::DecimalDot => "1. 2. 3.",
            Self::ChapterDigit => "第1章 第2章",
            Self::ChapterChinese => "第一章 第二章",
            Self::Part => "第一部分 第二部分",
            Self::SectionDigit => "第1节 第2节",
        }
    }
}

/// 列表项的编号样式。`list1` 对应段内列表（圈号；独立列表项下缩进写的子项也
/// 并成段内列表，用同一种编号），`list2` 对应独立列表（数字），二者可分别配置。
/// 字段名里的一、二是历史命名，不是嵌套层级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ListNumbering {
    /// ①②③
    #[default]
    Circled,
    /// (1)(2)(3)
    HalfParen,
    /// （1）（2）（3）
    FullParen,
    /// 1. 2. 3.
    DecimalDot,
}

impl ListNumbering {
    /// 全部可选样式，顺序与设置页下拉展示一致。
    pub const ALL: [ListNumbering; 4] = [
        Self::Circled,
        Self::HalfParen,
        Self::FullParen,
        Self::DecimalDot,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Circled => "①②③",
            Self::HalfParen => "(1)(2)(3)",
            Self::FullParen => "（1）（2）（3）",
            Self::DecimalDot => "1. 2. 3.",
        }
    }
}

/// 标题与列表的编号样式设置。旧配置没有该字段时按默认值补齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct NumberingConfig {
    /// 一级标题（Markdown `##`）编号样式。
    pub heading1: HeadingNumbering,
    /// 二级标题（`###`）。
    pub heading2: HeadingNumbering,
    /// 三级标题（`####`）。
    pub heading3: HeadingNumbering,
    /// 四级标题（`#####`）。
    pub heading4: HeadingNumbering,
    /// 段内列表编号样式；独立列表项下缩进写的子项也用它。
    pub list1: ListNumbering,
    /// 独立列表编号样式。
    pub list2: ListNumbering,
    /// 序号表（表前一行 `<!-- [序号表] -->`）里分组行的编号样式。
    pub table_group: HeadingNumbering,
}

impl Default for NumberingConfig {
    fn default() -> Self {
        Self {
            heading1: HeadingNumbering::Chinese,
            heading2: HeadingNumbering::ChineseParen,
            heading3: HeadingNumbering::DecimalDot,
            heading4: HeadingNumbering::HalfDigitParen,
            list1: ListNumbering::Circled,
            list2: ListNumbering::DecimalDot,
            table_group: HeadingNumbering::ChineseParen,
        }
    }
}

impl NumberingConfig {
    /// 指定 Markdown 标题层级对应的编号样式；层级 2–5 分别对应一至四级标题。
    pub(crate) fn heading(self, level: u8) -> Option<HeadingNumbering> {
        match level {
            2 => Some(self.heading1),
            3 => Some(self.heading2),
            4 => Some(self.heading3),
            5 => Some(self.heading4),
            _ => None,
        }
    }
}

/// 公文排版里可以单独换字体的位置。
///
/// 与 `gonghan-gwa.cls` 的字体族一一对应，且互不复用：换其中一项不会牵动另一项，
/// 唯一的例外是二级标题字体同时充当正文的 `ItalicFont`（类文件历来如此）。
/// 末一项 [`FontRole::Fallback`] 不对应字体族，而是所有字体族共用的后备字体。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// 公文大标题与附件标题，内置为方正小标宋。
    Title,
    /// `##` 一级标题「一、」，内置为黑体。
    Heading1,
    /// `###` 二级标题「（一）」，内置为楷体。
    Heading2,
    /// 正文与表格，内置为仿宋；「1.」「(1)」两级标题在它上面加粗。
    Body,
    /// 页脚页码，内置为宋体。
    PageNumber,
    /// 正文里的 Markdown 加粗文字，仅在「专用粗体字体」模式下生效，内置为黑体。
    Bold,
    /// 上面各支字体都没有的生僻字改用它，内置为宋体。
    Fallback,
}

impl FontRole {
    pub const ALL: [FontRole; 7] = [
        Self::Title,
        Self::Heading1,
        Self::Heading2,
        Self::Body,
        Self::PageNumber,
        Self::Bold,
        Self::Fallback,
    ];

    /// 决定版式的那几支字体，不含只在缺字时顶上的兜底字体。
    pub const TYPESETTING: [FontRole; 6] = [
        Self::Title,
        Self::Heading1,
        Self::Heading2,
        Self::Body,
        Self::PageNumber,
        Self::Bold,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Title => "标题字体",
            Self::Heading1 => "一级标题字体",
            Self::Heading2 => "二级标题字体",
            Self::Body => "正文字体",
            Self::PageNumber => "页码字体",
            Self::Bold => "加粗字体",
            Self::Fallback => "兜底字体",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Title => "公文大标题与附件标题，内置为方正小标宋",
            Self::Heading1 => "一级标题「一、」、表头与附件名，内置为方正黑体",
            Self::Heading2 => {
                "二级标题「（一）」，内置为方正楷体（西文、数字与符号取楷体_GB2312 字面）；同时用作主送机关、正文括号与斜体的字面"
            }
            Self::Body => {
                "正文与表格，内置为方正仿宋（西文、数字与符号取仿宋_GB2312 字面）；「1.」「(1)」两级标题在它上面加粗"
            }
            Self::PageNumber => "页脚页码，内置为方正书宋（数字保持宋体字面）",
            Self::Bold => {
                "正文里的 Markdown 加粗文字改用专门的粗体字体时用它，内置为方正黑体；选「当前字体直接加粗」时不生效，标题始终用仿宋合成加粗"
            }
            Self::Fallback => {
                "上面几支字体排不出的生僻字（「喆」「赟」等）改用它，内置为方正书宋，覆盖 GBK 全部汉字；\
                 想排 GBK 以外的字（如「𠮟」）才需要另选一支字库更大的本机字体"
            }
        }
    }

    /// 内置字体文件名，未配置本机字体时使用。必须是
    /// `portable_runtime::OFFICIAL_FONT_FILES` 里的一项，由单元测试守住。
    ///
    /// 中文一律用覆盖 GBK 的方正系列。仿宋、楷体是合成字体：非汉字字形取
    /// 仿宋_GB2312 / 楷体_GB2312（见 `portable_runtime::OFFICIAL_FONT_FILES`）；
    /// 页码数字另由宋体字面的拉丁子集 GWSimSunLatin 接管。
    pub(crate) fn bundled_file(self) -> &'static str {
        match self {
            Self::Title => "XiaoBiaoSong.ttf",
            Self::Heading1 => "FZHei.ttf",
            Self::Heading2 => "GWKai.ttf",
            Self::Body => "GWFangSong.ttf",
            Self::PageNumber => "FZShuSong.ttf",
            Self::Bold => "FZHei.ttf",
            Self::Fallback => "FZShuSong.ttf",
        }
    }

    /// 内置字体的中文名，用于界面上说明“不选就用什么”。
    pub(crate) fn bundled_label(self) -> &'static str {
        match self {
            Self::Title => "方正小标宋",
            Self::Heading1 => "方正黑体",
            Self::Heading2 => "方正楷体",
            Self::Body => "方正仿宋",
            Self::PageNumber => "方正书宋",
            Self::Bold => "方正黑体",
            Self::Fallback => "方正书宋",
        }
    }

    /// 配置键与临时字体文件名里用的短标识。
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Heading1 => "heading1",
            Self::Heading2 => "heading2",
            Self::Body => "body",
            Self::PageNumber => "pagenumber",
            Self::Bold => "bold",
            Self::Fallback => "fallback",
        }
    }
}

/// 正文里 Markdown 加粗（`**文字**`）怎么排成粗体；四、五级标题始终用仿宋合成加粗。
///
/// 中文字体大多没有配套的粗体字面。默认沿用排版器的合成加粗（TeX 的
/// `AutoFakeBold`、Word 的合成粗体），字形还是当前这套字体，只是加重；
/// 想要真正的粗体字面时改选专用字体，三端（预览、Word、PDF）一起换。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoldStyle {
    /// 用当前字体直接加粗。
    #[default]
    Synthetic,
    /// 换用专门的粗体字体（见 [`FontRole::Bold`]）。
    DedicatedFont,
}

impl BoldStyle {
    pub const ALL: [BoldStyle; 2] = [Self::Synthetic, Self::DedicatedFont];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Synthetic => "当前字体直接加粗",
            Self::DedicatedFont => "使用专门的粗体字体",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Synthetic => {
                "字体不变（正文仍是仿宋），只加重笔画：Word 里相当于点了加粗按钮，PDF 里描边加粗"
            }
            Self::DedicatedFont => {
                "改用专门的粗体字面；没挑字面、或没开「使用本机字体编译」时用内置黑体"
            }
        }
    }
}

/// 某一个位置选定的本机字体。家族名与文件路径都要留：前者写进 Word 按名字找字体，
/// 后者给 PDF 排版按文件加载。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontChoice {
    /// 英文家族名，Word 按名字找字体用。
    pub family: String,
    /// 本地化字体名，只用于界面显示。
    pub display: String,
    /// 字体文件绝对路径。
    pub path: String,
}

impl FontChoice {
    pub(crate) fn is_set(&self) -> bool {
        !self.family.trim().is_empty() && !self.path.trim().is_empty()
    }

    /// 界面上显示的名字：优先本地化名。
    pub(crate) fn label(&self) -> &str {
        let display = self.display.trim();
        if display.is_empty() {
            self.family.trim()
        } else {
            display
        }
    }
}

/// 应用内词表输入法。基础表与公文四码表合并查询。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImeConfig {
    pub enabled: bool,
    pub full_width_punctuation: bool,
    pub page_size: usize,
    pub page_keys: String,
    pub candidate_vertical: bool,
    pub candidate_font_percent: u16,
    /// 唯一四码候选自动上屏，默认关闭。
    pub auto_commit: bool,
    /// 第五码先上屏当前候选，再开始下一编码；无候选时保留原编码。
    pub fifth_commit: bool,
    /// 简码提示：候选还有更短的码时在词后标出。
    pub code_hint: bool,
    /// 词组提示：逐字打出表里已有的词时，在状态栏提示它的编码。
    pub phrase_hint: bool,
    /// 逐码提示：一至三码时在精确候选后补出以它开头的候选，并标出还要打的码。
    pub prefix_hint: bool,
    /// 旧配置短语迁移入口；迁移后也保留原配置以便回退。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub phrases: Vec<ImePhrase>,
}

/// 输入法的一条自定义短语：敲 `code` 时在候选第 `position` 位出 `text`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImePhrase {
    /// 输入码：1–32 个小写英文字母。
    pub code: String,

    /// 上屏的文字，可以换行。
    pub text: String,

    /// 候选位置，1–9。
    pub position: usize,

    /// 停用的短语留着配置、不出候选。
    pub enabled: bool,
}

impl Default for ImePhrase {
    fn default() -> Self {
        Self {
            code: String::new(),
            text: String::new(),
            position: 1,
            enabled: true,
        }
    }
}

impl Default for ImeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            full_width_punctuation: true,
            page_size: 5,
            page_keys: "[]".into(),
            candidate_vertical: false,
            candidate_font_percent: 100,
            auto_commit: false,
            fifth_commit: true,
            code_hint: true,
            phrase_hint: true,
            prefix_hint: false,
            phrases: Vec::new(),
        }
    }
}

/// 编译公文时使用的字体。字段全为空即表示沿用内置字体。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// 总开关。关掉时下面五项即使填了也不生效，方便临时对照内置版式。
    pub use_system_fonts: bool,
    /// 应用界面（窗口、菜单、列表）字体。留空时使用当前平台的系统默认中文字体；
    /// 不受 `use_system_fonts` 编译开关控制。
    pub ui_font: FontChoice,
    /// Markdown 源码编辑器字体。留空时跟随界面字体；
    /// 与界面字体一样不受 `use_system_fonts` 编译开关控制。
    pub editor_font: FontChoice,
    pub title: FontChoice,
    pub heading1: FontChoice,
    pub heading2: FontChoice,
    pub body: FontChoice,
    pub page_number: FontChoice,
    /// 加粗文字改用专门粗体字体时的字面，仅在 `bold_style` 为
    /// [`BoldStyle::DedicatedFont`] 时生效。
    pub bold: FontChoice,
    /// 上面几支字体排不出的生僻字改用它。留空时使用内置宋体（覆盖 GBK 全部汉字）；
    /// 与界面字体一样不受 `use_system_fonts` 编译开关控制——它只在缺字时顶上，
    /// 关掉没有「对照内置版式」的意义，只会让字重新丢掉。
    pub fallback: FontChoice,
    /// 加粗文字怎么排：用当前字体直接加粗，还是换用专门的粗体字体。
    pub bold_style: BoldStyle,
}

impl FontConfig {
    pub(crate) fn choice(&self, role: FontRole) -> &FontChoice {
        match role {
            FontRole::Title => &self.title,
            FontRole::Heading1 => &self.heading1,
            FontRole::Heading2 => &self.heading2,
            FontRole::Body => &self.body,
            FontRole::PageNumber => &self.page_number,
            FontRole::Bold => &self.bold,
            FontRole::Fallback => &self.fallback,
        }
    }

    pub(crate) fn choice_mut(&mut self, role: FontRole) -> &mut FontChoice {
        match role {
            FontRole::Title => &mut self.title,
            FontRole::Heading1 => &mut self.heading1,
            FontRole::Heading2 => &mut self.heading2,
            FontRole::Body => &mut self.body,
            FontRole::PageNumber => &mut self.page_number,
            FontRole::Bold => &mut self.bold,
            FontRole::Fallback => &mut self.fallback,
        }
    }

    /// 真正生效的选择：总开关关掉或者这一项没配就返回 `None`，调用方据此
    /// 回退到内置字体。
    ///
    /// 兜底字体是唯一不看总开关的位置：它决定的不是版式，而是一个字排不排得
    /// 出来，关掉只会让生僻字重新从纸面上消失。
    pub(crate) fn active(&self, role: FontRole) -> Option<&FontChoice> {
        if !self.use_system_fonts && role != FontRole::Fallback {
            return None;
        }
        let choice = self.choice(role);
        choice.is_set().then_some(choice)
    }

    /// 界面字体不受公文编译字体总开关影响，配置后立即生效。
    pub(crate) fn active_ui_font(&self) -> Option<&FontChoice> {
        self.ui_font.is_set().then_some(&self.ui_font)
    }

    /// 编辑器字体同样不受编译字体总开关影响；留空时返回 `None`，
    /// 由主题层回落到界面字体的回退链。
    pub(crate) fn active_editor_font(&self) -> Option<&FontChoice> {
        self.editor_font.is_set().then_some(&self.editor_font)
    }

    /// 加粗文字是不是换用专门的粗体字面（而不是让排版器就着当前字体合成加粗）。
    pub(crate) fn uses_dedicated_bold_font(&self) -> bool {
        self.bold_style == BoldStyle::DedicatedFont
    }

    /// 加粗文字要换用的字体家族名（Word 用）。
    ///
    /// `None` 表示沿用当前字体直接加粗：Word 写 `w:b` 让它自己合成。选了「专用
    /// 粗体字体」但没挑字面时回落「黑体」——DOCX 是给别人机器看的交换格式，字体名
    /// 始终写收文方机器上常见的中易字体名，不随内置 PDF 字体（方正系列）变化。
    pub(crate) fn bold_family_docx(&self) -> Option<&str> {
        if !self.uses_dedicated_bold_font() {
            return None;
        }
        match self.active(FontRole::Bold) {
            Some(choice) => Some(choice.family.trim()),
            None => Some("黑体"),
        }
    }
}
