#!/usr/bin/env python3
"""给「方正 + GB2312 非汉字」合成字体改名，生成随包的公文仿宋 / 公文楷体。

合成字体：汉字与全角标点取方正仿宋_GBK / 方正楷体_GBK（覆盖 GBK），西文、数字、
带圈数字、弯引号、破折号、省略号等非汉字字形取仿宋_GB2312 / 楷体_GB2312，
国标字面一支字体内就齐了，不再需要另挂拉丁子集。

合成后的文件名字表仍是方正原名（FZFangSong-Z02 等），与原版方正字体撞名：用户
另选原版方正字体当本机字体时，排版引擎分不清两支。这里只改名字表（家族名、全名、
唯一 ID、PostScript 名，中英文记录都改），字形与度量一律不动；版权（nameID 0）、
商标（7）等出处记录保留。

源文件是 runtime/fonts 下手工合成的两支（不入库），产物写到 runtime/fonts/ 与
font/（开发资产目录），SHA-256 由脚本打印，人工登记到 runtime/SHA256SUMS.*.txt。

用法：python scripts/rename-merged-fonts.py
"""

import hashlib
import shutil
from pathlib import Path

from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
RUNTIME_FONTS = ROOT / "runtime" / "fonts"
DEV_FONTS = ROOT / "font"

# (源文件, 产物文件, 英文家族名, 中文家族名, PostScript 名)
JOBS = [
    (
        "FZFangSong+FS-GB2312_non-cjk.ttf",
        "GWFangSong.ttf",
        "GW FangSong",
        "公文仿宋",
        "GWFangSong",
    ),
    (
        "FZKai+KT-GB2312_non-cjk.ttf",
        "GWKai.ttf",
        "GW Kai",
        "公文楷体",
        "GWKai",
    ),
]

CHINESE_LANG_IDS = {0x804, 0x404, 0xC04, 0x1004, 0x1404}


def rename(font: TTFont, family: str, family_zh: str, ps_name: str) -> None:
    for record in font["name"].names:
        chinese = record.platformID == 3 and record.langID in CHINESE_LANG_IDS
        name = family_zh if chinese else family
        if record.nameID in (1, 4, 16):
            value = name
        elif record.nameID == 3:
            value = f"{ps_name};gongwen-merged"
        elif record.nameID == 6:
            value = ps_name
        else:
            continue
        record.string = value


def main() -> None:
    for src_name, dst_name, family, family_zh, ps_name in JOBS:
        src = RUNTIME_FONTS / src_name
        if not src.is_file():
            raise SystemExit(f"源字体缺失：{src}")
        font = TTFont(src)
        rename(font, family, family_zh, ps_name)
        dst = RUNTIME_FONTS / dst_name
        font.save(dst)
        check = TTFont(dst)
        assert check["name"].getDebugName(1) == family, dst_name
        assert check["name"].getDebugName(6) == ps_name, dst_name
        if DEV_FONTS.is_dir():
            shutil.copyfile(dst, DEV_FONTS / dst_name)
        digest = hashlib.sha256(dst.read_bytes()).hexdigest()
        print(f"{digest}  fonts/{dst_name}  ({dst.stat().st_size} bytes, family={family})")


if __name__ == "__main__":
    main()
