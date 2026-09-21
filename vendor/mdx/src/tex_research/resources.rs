use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub const TEMPLATE_TEX: &str = include_str!("../../resources/research/template.tex");
pub const MD2TEX_CLS: &str = include_str!("../../resources/research/md2tex.cls");

/// Extract runtime resources required next to the generated `.tex` file.
pub fn extract(output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| "创建输出目录失败")?;
    fs::write(output_dir.join("md2tex.cls"), MD2TEX_CLS).with_context(|| "释放 md2tex.cls 失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注入 \MdxFontPath 的宿主按文件名提供字体，文件名是双方的协议：
    /// 改名等于改协议，宿主的字体清单必须同步，所以在这里钉住。
    #[test]
    fn research_class_loads_every_font_by_file_when_a_path_is_injected() {
        assert!(MD2TEX_CLS.contains("\\providecommand{\\MdxFontPath}{}"));
        assert!(
            MD2TEX_CLS.contains("\\if\\relax\\detokenize\\expandafter{\\MdxFontPath}\\relax"),
            "空判断必须用 detokenize；\\ifx 与 \\@empty 比较会把空宏误判成非空"
        );
        for file in [
            "FZShuSong.ttf",
            "FZXiaoBiaoSong.ttf",
            "FZHei.ttf",
            "FZKai.ttf",
            "JetBrainsMono-Regular.ttf",
            "texgyretermes-regular.otf",
            "texgyretermes-bold.otf",
            "texgyretermes-italic.otf",
            "texgyretermes-bolditalic.otf",
        ] {
            assert!(MD2TEX_CLS.contains(file), "路径分支缺少字体文件：{file}");
        }
    }

    /// 两条分支必须覆盖同一组 family，否则按路径排出来的版面和命令行对不上。
    #[test]
    fn both_font_branches_define_the_same_families() {
        // 资源文件用 CRLF 保存，按行切分才与行尾无关。
        let lines = MD2TEX_CLS.lines().collect::<Vec<_>>();
        let start = lines
            .iter()
            .position(|line| line.contains("\\detokenize\\expandafter{\\MdxFontPath}"))
            .expect("字体段应当以 \\MdxFontPath 判空开头");
        let split = start
            + lines[start..]
                .iter()
                .position(|line| line.trim() == "\\else")
                .expect("字体段应当有 \\else 分支");
        let end = split
            + lines[split..]
                .iter()
                .position(|line| line.trim() == "\\fi")
                .expect("字体段应当闭合");
        let by_name = lines[start..split].join("\n");
        let by_path = lines[split..end].join("\n");
        for family in [
            "\\setCJKmainfont",
            "\\setmainfont",
            "\\setmonofont",
            "\\setCJKmonofont",
            "\\setCJKfamilyfont{xbs}",
            "\\setCJKfamilyfont{kaiti}",
            "\\setCJKfamilyfont{songti}",
            "\\setCJKfamilyfont{fangsong}",
            "\\setCJKfamilyfont{heiti}",
            "\\setCJKfamilyfont{code}",
            "\\newfontfamily\\encircle",
            "\\newfontfamily\\enxbs",
            "\\newfontfamily\\ennumber",
            "\\newfontfamily\\enhei",
            "\\newfontfamily\\codefamily",
        ] {
            assert!(by_name.contains(family), "family name 分支缺少 {family}");
            assert!(by_path.contains(family), "路径分支缺少 {family}");
        }
    }

    /// ctex 的平台探测会让同一份稿子在三个系统上排出三种字形，因此钉死
    /// fontset=none；本类唯一用到的 ctex 字体命令 \heiti 自己补定义。
    #[test]
    fn research_class_pins_the_ctex_fontset_and_defines_heiti() {
        assert!(MD2TEX_CLS.contains("fontset=none"));
        assert!(MD2TEX_CLS.contains("\\renewcommand{\\heiti}{\\CJKfamily{heiti}}"));
    }

    #[test]
    fn research_class_uses_separate_latin_and_cjk_code_fonts() {
        assert!(MD2TEX_CLS.contains("\\newcommand{\\codefont}{JetBrains Mono}"));
        assert!(MD2TEX_CLS.contains("\\newcommand{\\codeCJKfont}{FZKai-Z03}"));
        assert!(MD2TEX_CLS.contains("\\newcommand{\\maintitle}{FZXiaoBiaoSong-B05S}"));
        assert!(MD2TEX_CLS.contains("\\setmonofont{\\codefont}"));
        assert!(MD2TEX_CLS.contains("\\setCJKfamilyfont{code}{\\codeCJKfont}"));
        assert!(!MD2TEX_CLS.contains("LXGW Bright Code"));
    }

    #[test]
    fn research_class_loads_math_packages() {
        // 行内 \(...\) 与独立 \[...\] 公式依赖 amsmath / mathtools
        assert!(MD2TEX_CLS.contains("\\RequirePackage{amsmath}"));
        assert!(MD2TEX_CLS.contains("\\RequirePackage{mathtools}"));
    }
}
