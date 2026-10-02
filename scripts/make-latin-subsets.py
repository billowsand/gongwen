#!/usr/bin/env python3
"""从宋体提取拉丁字面，生成随包的页码数字子集字体 GWSimSunLatin。

页码数字保持宋体字面，页码的中文字形由方正书宋承担（见 src/portable_runtime.rs）。
Typst 按字体列表逐字回退，子集排在页码字体之前即可接管拉丁。正文与楷体的国标
西文字面改由合成字体 GWFangSong / GWKai 自带（scripts/rename-merged-fonts.py），
不再生成 GWFangSongLatin。

子集只含 U+0020–U+007E：全角标点（——、……等）必须落回中文字体，
进了子集就会被当成半角字面，反而出错。

源字体是 font/ 下的开发资产（不入库）；产物写到 runtime/fonts/，
SHA-256 由各平台 runtime/SHA256SUMS.*.txt 校验（脚本顺手打印，人工登记）。

用法：python scripts/make-latin-subsets.py
"""

from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
FONT_DIR = ROOT / "font"
OUT_DIR = ROOT / "runtime" / "fonts"

# 只保留可打印 ASCII（含空格）。不含 U+2013/U+2014 等破折号与 ……：
# 它们在公文里是全角字面，必须从中文字体出。
ASCII_PRINTABLE = list(range(0x20, 0x7F))

# (源文件, 产物文件, 新家族名, PostScript 名)
JOBS = [
    ("SimSun.ttf", "GWSimSunLatin.ttf", "GW SimSun Latin", "GWSimSunLatin"),
]


def rename(font: TTFont, family: str, ps_name: str) -> None:
    """改写家族相关 name 记录，避免与系统里的同名中文字体撞家族名。

    保留 copyright（nameID 0）等出处记录，只动家族名（1/16）、全名（4）、
    唯一 ID（3）与 PostScript 名（6）。
    """
    for record in font["name"].names:
        if record.nameID in (1, 4, 16):
            value = family
        elif record.nameID == 6:
            value = ps_name
        elif record.nameID == 3:
            value = f"{ps_name};gongwen-latin-subset"
        else:
            continue
        record.string = value.encode(record.getEncoding())


def make_subset(src: Path, dst: Path, family: str, ps_name: str) -> None:
    options = subset.Options()
    options.hinting = False  # 桌面排版不走屏幕小字号渲染，hint 是纯体积
    options.layout_features = []  # 拉丁字面不需要任何 OpenType 特性
    options.glyph_names = False  # post 表不带字形名
    options.notdef_outline = True
    options.recalc_bounds = True
    options.name_IDs = ["*"]  # 名字先全留，rename 里精确改写
    font = subset.load_font(str(src), options)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=ASCII_PRINTABLE)
    subsetter.subset(font)
    rename(font, family, ps_name)
    font.save(dst)


def main() -> None:
    for src_name, dst_name, family, ps_name in JOBS:
        src = FONT_DIR / src_name
        if not src.is_file():
            raise SystemExit(f"源字体缺失：{src}（font/ 是开发资产，见 runtime/README.md）")
        dst = OUT_DIR / dst_name
        make_subset(src, dst, family, ps_name)
        check = TTFont(dst)
        covered = check.getBestCmap().keys()
        assert set(ASCII_PRINTABLE) <= set(covered), f"{dst_name} 覆盖不完整"
        actual = check["name"].getDebugName(1)
        assert actual == family, f"{dst_name} 家族名异常：{actual!r}"
        print(f"{dst_name}: {dst.stat().st_size} bytes, family={actual}")


if __name__ == "__main__":
    main()
