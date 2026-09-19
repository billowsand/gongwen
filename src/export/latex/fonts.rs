//! 字体：字体名清理与注入 \XeTeX 字体钩子。
//!
//! 由 src/export/latex.rs 拆分而来：本文件是模块 `export::latex::fonts`，与其它子模块共享
//! `export::latex` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{FontConfig, FontRole};

/// 判断 `\GwaFontPath` 是否为空的条件式开头，钩子内部据此在按名字、按文件两条
/// 加载方式之间分流。
///
/// 不能写成 `\ifx\GwaFontPath\@empty`：类文件用 `\providecommand` 声明的空宏是
/// long macro，与 `\@empty` 的定义属性不同，内容虽为空也会被判成非空——导出的
/// `.tex` 拿到别的机器上编译时会误入按文件那条分支，去找一个并不存在的字体文件。
/// 与 `gonghan-gwa.cls` 里的判断保持同一种写法。
const EMPTY_FONT_PATH: &str = r"\if\relax\detokenize\expandafter{\GwaFontPath}\relax";

/// 「专用粗体字体」模式下把 `\GwBold` 换成真正的粗体字面。
///
/// 与字体钩子分开发：加粗排法不受「使用本机字体编译」总开关约束，没选任何本机
/// 字体时也要生效（此时回落内置黑体）。选「当前字体直接加粗」时返回 `None`，
/// 产出的 TeX 与从前逐字节一致。
pub(crate) fn bold_setup_hook(fonts: &FontConfig) -> Option<String> {
    let family = sanitize_font_name(fonts.bold_family()?);
    if family.is_empty() {
        return None;
    }
    // 中西文一起换：加粗的西文与数字若留在正文字体上，粗细会和汉字对不齐。
    //
    // 先 `\def` 再 `\AtBeginDocument` 调用，不把这段直接塞进 `\AtBeginDocument`
    // 的参数里：LaTeX 2020 起的钩子按 token 原样存参数，不做 `##` → `#` 的归并，
    // `\renewcommand{\GwBold}[1]{...##1}` 存进去再执行就变成了一个裸的 `#`，
    // 编译直接死在「macro parameter character # in horizontal mode」。`\def` 的
    // 宏体照常归并，两种 LaTeX 上都对。
    Some(format!(
        r"\makeatletter
\def\GwaBoldSetup{{%
    {EMPTY_FONT_PATH}
        \setCJKfamilyfont{{gwabold}}{{{family}}}%
        \newfontfamily\engwabold{{{family}}}%
    \else
        \setCJKfamilyfont{{gwabold}}[Path={{\GwaFontPath}}]{{{file}}}%
        \newfontfamily\engwabold[Path={{\GwaFontPath}}]{{{file}}}%
    \fi
    \renewcommand{{\GwBold}}[1]{{{{\CJKfamily{{gwabold}}\engwabold ##1}}}}%
}}
\AtBeginDocument{{\GwaBoldSetup}}
\makeatother
",
        family = family,
        file = match fonts.active(FontRole::Bold) {
            Some(choice) => choice.compiled_file_name(FontRole::Bold),
            None => FontRole::Bold.bundled_file().to_string(),
        },
    ))
}

/// 设置里另选了兜底字体时，顶替类文件默认的内置宋体。
///
/// 与字体钩子一样不受「使用本机字体编译」总开关约束：兜底决定的是一个字排不
/// 排得出来，不是版式。没选时返回 `None`，由类文件用内置宋体兜底，产出的 TeX
/// 与从前逐字节一致。
///
/// 选定的字体排在内置宋体前面，宋体仍留在链尾：用户挑的字库未必比宋体全，
/// 它漏掉的字还能由宋体接着兜。
pub(crate) fn fallback_setup_hook(fonts: &FontConfig) -> Option<String> {
    let choice = fonts.active(FontRole::Fallback)?;
    let family = sanitize_font_name(&choice.family);
    if family.is_empty() {
        return None;
    }
    let file = choice.compiled_file_name(FontRole::Fallback);
    let bundled_name = FontRole::Fallback.bundled_family();
    let bundled_file = FontRole::Fallback.bundled_file();
    Some(format!(
        r"\makeatletter
\def\GwaFontFallbackHook{{%
    {EMPTY_FONT_PATH}
        \GwaSetFallbackFonts{{}}{{{family},{bundled_name}}}%
    \else
        \GwaSetFallbackFonts{{Path={{\GwaFontPath}}}}{{{file},{bundled_file}}}%
    \fi
}}
\makeatother
"
    ))
}

/// 字体名里可能出现的、会被 TeX 当成控制字符的符号。字体名本身不该有这些，
/// 出现了也只可能是配置被手工改坏，直接剔除而不是让编译在别处报错。
pub(crate) fn sanitize_font_name(name: &str) -> String {
    name.chars()
        .filter(|ch| {
            !matches!(
                ch,
                '\\' | '{' | '}' | '%' | '#' | '$' | '&' | '~' | '^' | '_'
            )
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// 生成注入到 `\documentclass` 之前的字体设置钩子。类文件见到
/// `\GwaFontSetupHook` 有定义就改用它，顶替内置字体那段默认设置。
///
/// 钩子内部保留按名字、按文件两条分支：内置 Tectonic 编译时用拷进临时目录的
/// 字体文件，导出的 `.tex` 拿到别的机器上编译时则按字体名加载。没有配置的位置
/// 沿用内置字体，行为与不注入钩子时一致。
pub(crate) fn font_setup_hook(fonts: &FontConfig) -> Option<String> {
    if !fonts.any_active() {
        return None;
    }
    let file = |role: FontRole| match fonts.active(role) {
        Some(choice) => choice.compiled_file_name(role),
        None => role.bundled_file().to_string(),
    };
    let name = |role: FontRole| match fonts.active(role) {
        Some(choice) => sanitize_font_name(&choice.family),
        None => role.bundled_family().to_string(),
    };

    let (title_name, title_file) = (name(FontRole::Title), file(FontRole::Title));
    let (heiti_name, heiti_file) = (name(FontRole::Heading1), file(FontRole::Heading1));
    let (kai_name, kai_file) = (name(FontRole::Heading2), file(FontRole::Heading2));
    let (body_name, body_file) = (name(FontRole::Body), file(FontRole::Body));
    let (song_name, song_file) = (name(FontRole::PageNumber), file(FontRole::PageNumber));

    // 方正小标宋没有统一的字体名，未指定标题字体时沿用类文件的两段探测。
    let title_by_name = if title_name.is_empty() {
        r"        \IfFontExistsTF{FZXiaoBiaoSong-B05}{%
            \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05}%
            \newfontfamily\enbt{FZXiaoBiaoSong-B05}%
        }{%
            \IfFontExistsTF{FZXiaoBiaoSong-B05S}{%
                \setCJKfamilyfont{xbs}{FZXiaoBiaoSong-B05S}%
                \newfontfamily\enbt{FZXiaoBiaoSong-B05S}%
            }{%
                \ClassError{gonghan-gwa}{未找到方正小标宋字体}{请安装 FZXiaoBiaoSong-B05 或 FZXiaoBiaoSong-B05S}%
            }%
        }%"
            .to_string()
    } else {
        format!(
            r"        \setCJKfamilyfont{{xbs}}{{{title_name}}}%
        \newfontfamily\enbt{{{title_name}}}%"
        )
    };

    Some(format!(
        r"\makeatletter
\def\GwaFontSetupHook{{%
    {EMPTY_FONT_PATH}
        \setCJKmainfont[ItalicFont={{{kai_name}}}, AutoFakeBold=true]{{{body_name}}}%
{title_by_name}
        \setCJKfamilyfont{{kaiti}}[AutoFakeBold=true]{{{kai_name}}}%
        \newfontfamily\enkai{{{kai_name}}}%
        \setCJKfamilyfont{{songti}}{{{song_name}}}%
        \newfontfamily\ennumber{{{song_name}}}%
        \newfontfamily\ensong{{{song_name}}}%
        \newfontfamily\enheiti{{{heiti_name}}}%
        \setCJKfamilyfont{{heiti}}{{{heiti_name}}}%
        \setCJKfamilyfont{{fangsong}}[AutoFakeBold=true]{{{body_name}}}%
        \setmainfont[ItalicFont={{{kai_name}}}]{{{body_name}}}%
        \setCJKmonofont{{{heiti_name}}}%
        \setmonofont{{{heiti_name}}}%
    \else
        \setCJKmainfont[Path={{\GwaFontPath}}, ItalicFont={{{kai_file}}}, AutoFakeBold=true]{{{body_file}}}%
        \setCJKfamilyfont{{xbs}}[Path={{\GwaFontPath}}]{{{title_file}}}%
        \newfontfamily\enbt[Path={{\GwaFontPath}}]{{{title_file}}}%
        \setCJKfamilyfont{{kaiti}}[Path={{\GwaFontPath}}, AutoFakeBold=true]{{{kai_file}}}%
        \newfontfamily\enkai[Path={{\GwaFontPath}}]{{{kai_file}}}%
        \setCJKfamilyfont{{songti}}[Path={{\GwaFontPath}}]{{{song_file}}}%
        \newfontfamily\ennumber[Path={{\GwaFontPath}}]{{{song_file}}}%
        \newfontfamily\ensong[Path={{\GwaFontPath}}]{{{song_file}}}%
        \newfontfamily\enheiti[Path={{\GwaFontPath}}]{{{heiti_file}}}%
        \setCJKfamilyfont{{heiti}}[Path={{\GwaFontPath}}]{{{heiti_file}}}%
        \setCJKfamilyfont{{fangsong}}[Path={{\GwaFontPath}}, AutoFakeBold=true]{{{body_file}}}%
        \setmainfont[Path={{\GwaFontPath}}, ItalicFont={{{kai_file}}}]{{{body_file}}}%
        \setCJKmonofont[Path={{\GwaFontPath}}]{{{heiti_file}}}%
        \setmonofont[Path={{\GwaFontPath}}]{{{heiti_file}}}%
    \fi
}}
\makeatother
"
    ))
}
