//! 字体族名、系统字体探测与 egui 字体表构建。
//!
//! 由 src/theme.rs 拆分而来：本文件是模块 `theme::fonts`，与其它子模块共享
//! `theme` 根模块的私有可见性（结构体与根模块类型/常量仍在根文件中）。

use crate::models::{EditorFontFace, FontConfig, FontRole};
use eframe::egui;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 公文预览专用字体族的名字。与导出器一一对应：正文仿宋、一级标题黑体、
/// 二级标题楷体、文档标题小标宋。西文与数字的字面与 PDF 一致：仿宋、楷体是合成
/// 字体，自带仿宋_GB2312 / 楷体_GB2312 字面；页码前面挂宋体字面的拉丁子集；黑体、
/// 小标宋用自带字面。
pub const FONT_FANGSONG: &str = "gw-fangsong";

pub const FONT_HEITI: &str = "gw-heiti";

pub const FONT_KAITI: &str = "gw-kaiti";

pub const FONT_BIAOSONG: &str = "gw-biaosong";

/// 页码与纸面行号用的字体族，内置为宋体。纸上的非正文数字（页码）在公文里
/// 本来就有自己的一副字面，页边的行号沿用同一副，与正文仿宋一眼可辨。
pub const FONT_SONGTI: &str = "gw-songti";

/// 正文选「专用粗体字体」时使用的字面；合成加粗由预览在原字面上叠绘。
pub const FONT_BOLD: &str = "gw-bold";

/// 研究报告的字体族：中文同公文的方正系列，西文数字一律 TeX Gyre Termes——标题类
/// （黑体、小标宋）用 Bold，其余 Regular。与 PDF 一样钉死在随包字体上，不受设置页
/// 本机字体影响。研究报告的版面代码照旧写公文的族名，由 `Metrics::font` 换成这几支
/// （见 [`research_family`]）。
pub const FONT_RESEARCH_SONG: &str = "gw-research-song";

pub const FONT_RESEARCH_KAI: &str = "gw-research-kai";

pub const FONT_RESEARCH_TERMINAL: &str = "gw-research-terminal";

pub const FONT_RESEARCH_HEI: &str = "gw-research-hei";

/// 研究报告表头：黑体字形，西文仍是 Termes Regular（TeX 的 `\heiti` 不带 `\enhei`）。
pub const FONT_RESEARCH_HEI_PLAIN: &str = "gw-research-hei-plain";

pub const FONT_RESEARCH_BIAOSONG: &str = "gw-research-biaosong";

/// 研究报告版面里公文族名对应的研究报告字体族；没有对应的原样返回。
pub(crate) fn research_family(family: &str) -> &str {
    match family {
        FONT_SONGTI | FONT_FANGSONG => FONT_RESEARCH_SONG,
        FONT_KAITI => FONT_RESEARCH_KAI,
        FONT_HEITI => FONT_RESEARCH_HEI,
        FONT_BIAOSONG => FONT_RESEARCH_BIAOSONG,
        other => other,
    }
}

const DEDICATED_BOLD_PREVIEW_ID: &str = "gw-dedicated-bold-preview";

pub(crate) fn preview_uses_dedicated_bold(ctx: &egui::Context) -> bool {
    ctx.data(|data| {
        data.get_temp::<bool>(egui::Id::new(DEDICATED_BOLD_PREVIEW_ID))
            .unwrap_or(false)
    })
}

/// Markdown 源码编辑器专用的 egui 字体族名。单独注册一族而不是直接复用
/// `Proportional`，编辑器换字体就不必给高亮布局函数加参数。
pub const EDITOR_FONT_FAMILY: &str = "gongwen_editor";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UiFontCandidate {
    path: PathBuf,
    /// TTC/OTC 中的字面序号；普通 TTF/OTF 为 0。
    index: u32,
}

impl UiFontCandidate {
    pub(crate) fn new(path: impl Into<PathBuf>, index: u32) -> Self {
        Self {
            path: path.into(),
            index,
        }
    }
}

/// Windows 中文界面的系统默认字体。使用 WINDIR 而不是写死 C 盘。
#[cfg(target_os = "windows")]
pub(crate) fn platform_ui_font_candidates() -> Vec<UiFontCandidate> {
    let font_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    [
        ("msyh.ttc", 0),
        ("msyh.ttf", 0),
        ("NotoSansSC-VF.ttf", 0),
        ("SourceHanSansCN-Normal.ttf", 0),
        ("simsun.ttc", 0),
    ]
    .into_iter()
    .map(|(file, index)| UiFontCandidate::new(font_dir.join(file), index))
    .collect()
}

/// 让 fontconfig 返回字体文件和集合字面序号。Noto CJK 在常见 Linux 发行版里
/// 往往以 TTC 安装，只有同时使用这里返回的 index 才能选中简体中文字面。
#[cfg(target_os = "linux")]
pub(crate) fn fontconfig_candidate(pattern: &str) -> Option<UiFontCandidate> {
    let output = std::process::Command::new("fc-match")
        .arg("-f")
        .arg("%{file}\n%{index}\n")
        .arg(pattern)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let mut lines = text.lines();
    let path = lines.next()?.trim();
    if path.is_empty() {
        return None;
    }
    let index = lines
        .next()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    Some(UiFontCandidate::new(path, index))
}

/// Linux 优先使用 Noto Sans SC；后面的固定路径兼容没有 fontconfig 命令的精简
/// 环境，最后的 zh-cn 通用匹配只在 Noto 未安装时负责保证中文仍可显示。
#[cfg(target_os = "linux")]
pub(crate) fn platform_ui_font_candidates() -> Vec<UiFontCandidate> {
    let mut candidates = Vec::new();
    for pattern in [
        "Noto Sans SC:style=Regular",
        "Noto Sans CJK SC:style=Regular",
        "sans-serif:lang=zh-cn",
    ] {
        if let Some(candidate) = fontconfig_candidate(pattern)
            && !candidates.contains(&candidate)
        {
            candidates.push(candidate);
        }
    }
    for candidate in [
        UiFontCandidate::new("/usr/share/fonts/opentype/noto/NotoSansSC-Regular.otf", 0),
        UiFontCandidate::new("/usr/share/fonts/truetype/noto/NotoSansSC-Regular.ttf", 0),
        UiFontCandidate::new("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
        UiFontCandidate::new("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
        UiFontCandidate::new(
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
            2,
        ),
    ] {
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// macOS 自带中文界面字体。苹方等以 TTC 集合安装，字面序号随系统版本变化，
/// 因此按字面探测，优先选择真正包含简体字「国」的字面，避免误选繁体/日文字面。
#[cfg(target_os = "macos")]
pub(crate) fn platform_ui_font_candidates() -> Vec<UiFontCandidate> {
    let mut candidates = Vec::new();
    for path in [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ] {
        if let Some(candidate) = simplified_face(Path::new(path)) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// 在单个字体文件（含 TTC 集合）里定位简体中文字面：优先含「国」的字面，
/// 找不到再退回任一含「中」的字面，保证界面中文不显示成方框。
#[cfg(target_os = "macos")]
pub(crate) fn simplified_face(path: &Path) -> Option<UiFontCandidate> {
    let data = std::fs::read(path).ok()?;
    let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
    let mut fallback = None;
    for index in 0..count {
        let Ok(face) = ttf_parser::Face::parse(&data, index) else {
            continue;
        };
        if face.glyph_index('国').is_some() {
            return Some(UiFontCandidate::new(path, index));
        }
        if fallback.is_none() && face.glyph_index('中').is_some() {
            fallback = Some(index);
        }
    }
    fallback.map(|index| UiFontCandidate::new(path, index))
}

/// 其它平台暂不提供系统界面字体候选，退回 egui 默认字体。
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) fn platform_ui_font_candidates() -> Vec<UiFontCandidate> {
    Vec::new()
}

/// 设置页展示的当前平台默认界面字体名。
pub(crate) fn default_ui_font_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "微软雅黑"
    } else if cfg!(target_os = "linux") {
        "Noto Sans SC"
    } else if cfg!(target_os = "macos") {
        "苹方"
    } else {
        "系统字体"
    }
}

/// 加载第一个真正包含常用汉字的系统界面字体。校验字面可避免字体文件存在但
/// TTC 序号不正确时，egui 接受数据后仍把中文画成方框。
pub(crate) fn load_system_ui_font(fonts: &mut egui::FontDefinitions, key: &str) -> Option<String> {
    for candidate in platform_ui_font_candidates() {
        let Ok(data) = std::fs::read(&candidate.path) else {
            continue;
        };
        let Ok(face) = ttf_parser::Face::parse(&data, candidate.index) else {
            continue;
        };
        if face.glyph_index('中').is_none() {
            continue;
        }
        let mut font_data = egui::FontData::from_owned(data);
        font_data.index = candidate.index;
        center_line_gap(&mut font_data);
        fonts.font_data.insert(key.to_owned(), font_data.into());
        return Some(key.to_owned());
    }
    None
}

/// egui 排字时基线固定落在行顶往下 ascent 处，字体的 line gap 整段加在行底，
/// line gap 大的字体字面就整体偏上。冬青黑体（Hiragino Sans GB，新版 macOS 上
/// 苹方不在 /System/Library/Fonts，系统默认实际落到它）的 line gap 是半个字高，
/// 按钮、下拉框里的字明显靠上。把字形下移半个 line gap，上下各分一半，字面
/// 落回行正中；line gap 为 0 的字体不受影响。
pub(crate) fn center_line_gap(font_data: &mut egui::FontData) {
    font_data.tweak.y_offset_factor = line_gap_offset_factor(&font_data.font, font_data.index);
}

/// 半个 line gap 占字号的比例，即 `FontTweak::y_offset_factor` 该取的值。
/// ttf_parser 与 egui 所用的 skrifa 一样：OS/2 置了 USE_TYPO_METRICS 才用 typo
/// 度量，否则用 hhea，两边读到的 line gap 是同一个数。
pub(crate) fn line_gap_offset_factor(data: &[u8], index: u32) -> f32 {
    let Ok(face) = ttf_parser::Face::parse(data, index) else {
        return 0.0;
    };
    let gap = f32::from(face.line_gap().max(0));
    gap / 2.0 / f32::from(face.units_per_em())
}

/// 给已经装进 `font_data` 的用户所选字体补上 `center_line_gap`。
pub(crate) fn center_loaded_line_gap(fonts: &mut egui::FontDefinitions, key: &str) {
    if let Some(data) = fonts.font_data.get_mut(key) {
        center_line_gap(std::sync::Arc::make_mut(data));
    }
}

/// 取公文字体族。字体缺失时 `configure_fonts` 会使用独立的预览后备字体，不会报错。
pub(crate) fn official_family(name: &str) -> egui::FontFamily {
    egui::FontFamily::Name(name.into())
}

/// 源码编辑器里某一处元素该用的字体族。公文字面与预览、导出同源，
/// 因此编辑器上选了「仿宋」，看到的就是预览里那支仿宋。
pub(crate) fn editor_face_family(face: EditorFontFace) -> egui::FontFamily {
    match face {
        EditorFontFace::Editor => egui::FontFamily::Name(EDITOR_FONT_FAMILY.into()),
        EditorFontFace::Biaosong => official_family(FONT_BIAOSONG),
        EditorFontFace::Heiti => official_family(FONT_HEITI),
        EditorFontFace::Kaiti => official_family(FONT_KAITI),
        EditorFontFace::Fangsong => official_family(FONT_FANGSONG),
    }
}

/// 依次尝试候选路径，把第一个读到的字体以 `key` 存入 `font_data`。
///
/// `loaded` 记下已经读过的文件，同一份字体只在内存里留一份：加粗字体默认与
/// 一级标题同是黑体，兜底字体默认与页码同是宋体，中文字体动辄十几兆，重复
/// 载入纯属白占。命中已有文件时返回的是先前那个 key，字体族照样指得对。
pub(crate) fn load_font(
    fonts: &mut egui::FontDefinitions,
    loaded: &mut BTreeMap<PathBuf, String>,
    key: &str,
    candidates: &[PathBuf],
) -> Option<String> {
    for path in candidates {
        if let Some(existing) = loaded.get(path) {
            return Some(existing.clone());
        }
        if let Ok(data) = std::fs::read(path) {
            fonts
                .font_data
                .insert(key.to_owned(), egui::FontData::from_owned(data).into());
            loaded.insert(path.clone(), key.to_owned());
            return Some(key.to_owned());
        }
    }
    None
}

pub(crate) fn font_candidates(bundled: Option<&Path>, file: &str, system: &[&str]) -> Vec<PathBuf> {
    bundled
        .map(|dir| dir.join(file))
        .into_iter()
        .chain(system.iter().map(PathBuf::from))
        .collect()
}

/// 配置界面与公文预览的字体。`config` 里选了本机字体的位置，预览也跟着换，
/// 否则屏幕上看到的版式和编译出来的 PDF 对不上。
pub(crate) fn configure_fonts(ctx: &egui::Context, config: &FontConfig) {
    ctx.data_mut(|data| {
        data.insert_temp(
            egui::Id::new(DEDICATED_BOLD_PREVIEW_ID),
            config.uses_dedicated_bold_font(),
        );
    });
    ctx.set_fonts(font_definitions(config));
}

/// 装好全部字体族的 `FontDefinitions`。与 `configure_fonts` 分开是为了能在
/// 测试里直接问字体族「这个字排不排得出来」，不必起一个真窗口。
pub(crate) fn font_definitions(config: &FontConfig) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let bundled_fonts = crate::portable_runtime::find_font_dir();
    // 同一份字体文件只读一次，见 `load_font`。
    let mut loaded: BTreeMap<PathBuf, String> = BTreeMap::new();

    // 界面字体不再编进可执行文件：Windows 使用微软雅黑，Linux 使用 Noto Sans
    // SC，macOS 使用苹方。等宽文本保留 egui 自带的拉丁等宽字体，并把系统中文字体放在末尾兜底。
    let system_ui_font = load_system_ui_font(&mut fonts, "gw-system-ui");
    if let Some(font) = &system_ui_font {
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, font.clone());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push(font.clone());
    }

    // 用户在设置里指定的界面字体排在系统默认之前；文件缺失或读取失败时仍由
    // 上面的系统中文字体兜底，确保界面不会因一项失效配置而出现方框。
    let custom_ui_font = config.active_ui_font().and_then(|choice| {
        load_font(
            &mut fonts,
            &mut loaded,
            "gw-custom-ui",
            &[PathBuf::from(choice.path.trim())],
        )
    });
    if let Some(font) = custom_ui_font {
        center_loaded_line_gap(&mut fonts, &font);
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, font);
    }

    // Markdown 源码编辑器用独立字体族：用户指定的编辑器字体排在最前，
    // 后面整份接上 Proportional 的回退链——没设编辑器字体时行为与直接用
    // Proportional 完全一致，CJK 缺字也照常落到系统/界面字体上。
    let custom_editor_font = config.active_editor_font().and_then(|choice| {
        load_font(
            &mut fonts,
            &mut loaded,
            "gw-custom-editor",
            &[PathBuf::from(choice.path.trim())],
        )
    });
    let mut editor_family = Vec::new();
    if let Some(font) = custom_editor_font {
        center_loaded_line_gap(&mut fonts, &font);
        editor_family.push(font);
    }
    editor_family.extend(
        fonts
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    fonts.families.insert(
        egui::FontFamily::Name(EDITOR_FONT_FAMILY.into()),
        editor_family,
    );

    // 公文预览专用字体缺失时复用系统中文界面字体，避免重复载入同一份大字体。
    let preview_fallback = system_ui_font.or_else(|| {
        load_font(
            &mut fonts,
            &mut loaded,
            "gw-preview-fallback",
            &font_candidates(
                bundled_fonts.as_deref(),
                "FZShuSong.ttf",
                &[
                    r"C:\Windows\Fonts\msyh.ttc",
                    r"C:\Windows\Fonts\SourceHanSansCN-Normal.ttf",
                    r"C:\Windows\Fonts\NotoSansSC-VF.ttf",
                    r"C:\Windows\Fonts\simsun.ttc",
                ],
            ),
        )
    });
    // 公文专用字体缺失时也不回退到界面字体，保持预览与界面字体相互隔离。
    let mut fallback = Vec::new();
    if let Some(font) = &preview_fallback {
        fallback.push(font.clone());
    }
    fallback.extend(
        egui::FontDefinitions::default()
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );

    // 生僻字兜底：仿宋与楷体只有 GB2312 的 6763 个汉字，人名里的「喆」「赟」
    // 在它们上面没有字形。每个公文字体族后面都挂上兜底字体，egui 逐字回退，
    // 缺字落到兜底字体上——与 TeX 侧 \GwaSetFallbackFonts 看到的是同一支字体。
    let mut glyph_fallback = Vec::new();
    if let Some(key) = config.active(FontRole::Fallback).and_then(|choice| {
        load_font(
            &mut fonts,
            &mut loaded,
            "gw-glyph-fallback",
            &[PathBuf::from(choice.path.trim())],
        )
    }) {
        glyph_fallback.push(key);
    }
    // 内置宋体覆盖 GBK 全部汉字，始终留在链尾：用户另选的字体未必比它全，
    // 它漏掉的字还能由宋体接着兜。
    if let Some(key) = load_font(
        &mut fonts,
        &mut loaded,
        "gw-glyph-fallback-bundled",
        &font_candidates(
            bundled_fonts.as_deref(),
            FontRole::Fallback.bundled_file(),
            &[r"C:\Windows\Fonts\simsun.ttc"],
        ),
    ) {
        glyph_fallback.push(key);
    }

    // 各角色的中文字体（连同兜底链）与是否用内置字体；拉丁子集在下面按用途另挂。
    let mut bases: BTreeMap<&'static str, (Vec<String>, bool)> = BTreeMap::new();
    for (family, role, bundled_file, system_candidates) in [
        (
            FONT_FANGSONG,
            FontRole::Body,
            "GWFangSong.ttf",
            &[
                r"C:\Windows\Fonts\simfang.ttf",
                r"C:\Windows\Fonts\simsun.ttc",
            ][..],
        ),
        (
            FONT_HEITI,
            FontRole::Heading1,
            "FZHei.ttf",
            &[
                r"C:\Windows\Fonts\simhei.ttf",
                r"C:\Windows\Fonts\msyhbd.ttc",
            ][..],
        ),
        (
            FONT_KAITI,
            FontRole::Heading2,
            "GWKai.ttf",
            &[
                r"C:\Windows\Fonts\simkai.ttf",
                r"C:\Windows\Fonts\simfang.ttf",
            ][..],
        ),
        (
            FONT_BIAOSONG,
            FontRole::Title,
            "XiaoBiaoSong.ttf",
            &[
                r"C:\Windows\Fonts\方正小标宋简.TTF",
                r"C:\Windows\Fonts\FZXBSJW.TTF",
                r"C:\Windows\Fonts\STZHONGS.TTF",
                r"C:\Windows\Fonts\simhei.ttf",
            ][..],
        ),
        (
            FONT_SONGTI,
            FontRole::PageNumber,
            "FZShuSong.ttf",
            &[
                r"C:\Windows\Fonts\simsun.ttc",
                r"C:\Windows\Fonts\simfang.ttf",
            ][..],
        ),
        (
            FONT_BOLD,
            FontRole::Bold,
            "FZHei.ttf",
            &[
                r"C:\Windows\Fonts\simhei.ttf",
                r"C:\Windows\Fonts\msyhbd.ttc",
            ][..],
        ),
    ] {
        // 每个字体族只放对应的中文字体，不把 Times New Roman 放在最前作西文优先
        // （与国标一致）；内置字体时西文数字由拉丁子集接管，见下方。
        let mut candidates =
            font_candidates(bundled_fonts.as_deref(), bundled_file, system_candidates);
        // 设置里指定了本机字体就排在最前；读不出来（文件被删）时照旧回退。
        // 粗体字面只在「专用粗体字体」模式下生效；合成模式在原字面上叠绘。
        let selected = match role {
            FontRole::Bold if !config.uses_dedicated_bold_font() => None,
            _ => config.active(role),
        };
        if let Some(choice) = selected {
            candidates.insert(0, PathBuf::from(choice.path.trim()));
        }
        let mut list = match load_font(&mut fonts, &mut loaded, family, &candidates) {
            Some(key) => vec![key],
            // 一个都没装上时退回界面字体：预览的字体不对，但排版仍然成立。
            None => fallback.clone(),
        };
        // 兜底字体挂在这支字体后面，只接它排不出的字。
        let tail: Vec<String> = glyph_fallback
            .iter()
            .filter(|key| !list.contains(key))
            .cloned()
            .collect();
        list.extend(tail);
        bases.insert(family, (list, selected.is_none()));
    }

    // 西文数字与 PDF 一致：仿宋、楷体是合成字体，自带 GB2312 西文字面；页码前面挂
    // 宋体字面的拉丁子集，只在页码用内置字体时挂（另选了本机字体就用它自己的字面）。
    for (family, (mut list, bundled)) in bases.clone() {
        let subset = crate::portable_runtime::SIMSUN_LATIN_SUBSET_FILE;
        if family == FONT_SONGTI
            && bundled
            && let Some(dir) = bundled_fonts.as_deref()
            && let Some(key) = load_font(&mut fonts, &mut loaded, subset, &[dir.join(subset)])
        {
            list.insert(0, key);
        }
        fonts
            .families
            .insert(egui::FontFamily::Name(family.into()), list);
    }

    // 研究报告：Termes 只接 latin-in-cjk 的码位（与 PDF 一致），中文用随包方正字体；
    // 随包字体缺失时退回公文同角色的字体链。
    let termes = |fonts: &mut egui::FontDefinitions, key: &str, file: &str| {
        let data = std::fs::read(bundled_fonts.as_deref()?.join(file)).ok()?;
        let data =
            crate::font_cmap::restrict(&data, |ch| !crate::font_cmap::is_shared_with_cjk(ch))?;
        fonts
            .font_data
            .insert(key.to_owned(), egui::FontData::from_owned(data).into());
        Some(key.to_owned())
    };
    let termes_regular = termes(&mut fonts, "gw-termes-regular", "texgyretermes-regular.otf");
    let termes_bold = termes(&mut fonts, "gw-termes-bold", "texgyretermes-bold.otf");
    let terminal_mono = termes(&mut fonts, "gw-terminal-mono", "JetBrainsMono-Regular.ttf");
    for (family, base, bundled_file, latin) in [
        (
            FONT_RESEARCH_SONG,
            FONT_SONGTI,
            "FZShuSong.ttf",
            &termes_regular,
        ),
        (FONT_RESEARCH_KAI, FONT_KAITI, "GWKai.ttf", &termes_regular),
        (
            FONT_RESEARCH_TERMINAL,
            FONT_HEITI,
            "FZHei.ttf",
            &terminal_mono,
        ),
        (FONT_RESEARCH_HEI, FONT_HEITI, "FZHei.ttf", &termes_bold),
        (
            FONT_RESEARCH_HEI_PLAIN,
            FONT_HEITI,
            "FZHei.ttf",
            &termes_regular,
        ),
        (
            FONT_RESEARCH_BIAOSONG,
            FONT_BIAOSONG,
            "XiaoBiaoSong.ttf",
            &termes_bold,
        ),
    ] {
        let path: Vec<PathBuf> = bundled_fonts
            .as_deref()
            .map(|dir| dir.join(bundled_file))
            .into_iter()
            .collect();
        let mut list = match load_font(&mut fonts, &mut loaded, family, &path) {
            Some(key) => {
                let mut list = vec![key];
                for key in &glyph_fallback {
                    if !list.contains(key) {
                        list.push(key.clone());
                    }
                }
                list
            }
            None => bases[base].0.clone(),
        };
        if let Some(latin) = latin {
            list.insert(0, latin.clone());
        }
        fonts
            .families
            .insert(egui::FontFamily::Name(family.into()), list);
    }

    fonts
}
