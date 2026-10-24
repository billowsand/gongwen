#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""把 assets/fuma/quan/ 下的分表合并成随包的基础词表 assets/fuma/quan.txt。

分表（UTF-8、LF、每行「编码,位置=文字」）：
    danzi.txt        单字
    shuangzi.txt     双字
    sanzi.txt        三字
    sizi.txt         四字
    siziyishang.txt  四字以上

合并结果仍是输入法运行时唯一的 quan.txt：UTF-16LE + BOM + CRLF，开头是版本说明行。
行序按（编码, 位置, 字数, 文字）排，与分表里的书写顺序无关；同编码同位置的字数是
往短词在前，候选列表据此显示。删词或加词后跑一遍：

    python scripts/merge-ime-tables.py            # 写回 assets/fuma/quan.txt
    python scripts/merge-ime-tables.py --check    # 只校验，不写文件

只用标准库。
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TABLE_DIR = ROOT / "assets" / "fuma" / "quan"
OUT = ROOT / "assets" / "fuma" / "quan.txt"

# 分表文件名 -> 该表的字数（None 表示四字以上）
TABLES = [
    ("danzi.txt", 1),
    ("shuangzi.txt", 2),
    ("sanzi.txt", 3),
    ("sizi.txt", 4),
    ("siziyishang.txt", None),
]

# quan.txt 的说明行，只在本脚本里维护。
HEADER = [
    "#本表贴入搜狗拼音自定义短语使用",
    "#词表来源 flypy.cc；自 2026-10-24 起由本项目自行维护，维护约定见 assets/fuma/README.md",
    "#2026-10-24 清理口语句词条（聊天应答、方言、亲属称谓、拟声象声、网络俚语、表情符号），详见 docs/table-ime.md",
    "#2026-10-24 拆分为 assets/fuma/quan/ 下五个分表（单字、双字、三字、四字、四字以上），"
    "由 scripts/merge-ime-tables.py 合并；同时按公文口径清理一批无关词条，详见 docs/table-ime.md",
]


def char_count(text: str) -> int:
    """词的字数：按码点计，与 Rust 的 chars().count() 一致。"""
    return len(text)


def valid_code(code: str) -> bool:
    return 1 <= len(code) <= 4 and code.isascii() and code.islower() and code.isalpha()


def parse_table(path: Path, want_chars):
    """读一个分表，返回 [(code, pos, word, where)]；校验编码、位置与字数。"""
    rows = []
    errors = []
    for line_no, raw in enumerate(path.read_text(encoding="utf-8-sig").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        where = f"{path.name}:{line_no}"
        left, sep, word = line.partition("=")
        if not sep or not left or not word:
            errors.append(f"{where} 不是「编码,位置=文字」：{raw[:40]}")
            continue
        code, sep2, pos_text = left.partition(",")
        if not sep2:
            code, pos_text = left, "1"
        code, pos_text = code.strip(), pos_text.strip()
        if not valid_code(code):
            errors.append(f"{where} 编码不合法：{code}")
            continue
        if not pos_text.isdigit() or int(pos_text) < 1:
            errors.append(f"{where} 位置不是正整数：{pos_text}")
            continue
        if "\t" in word or "\r" in word or "\n" in word:
            errors.append(f"{where} 词里有分隔符：{word[:40]}")
            continue
        chars = char_count(word)
        if want_chars is None and chars < 5:
            errors.append(f"{where} 四字以上表里出现 {chars} 字词：{word}")
            continue
        if want_chars is not None and chars != want_chars:
            errors.append(f"{where} {path.name} 里出现 {chars} 字词：{word}")
            continue
        rows.append((code, int(pos_text), word, where))
    return rows, errors


def sort_key(row):
    code, pos, word, _ = row
    return (code, pos, char_count(word), word)


def main() -> int:
    ap = argparse.ArgumentParser(description="合并输入法基础词表分表")
    ap.add_argument("--check", action="store_true", help="只校验分表，不写 quan.txt")
    args = ap.parse_args()

    all_rows = []
    errors = []
    for name, want in TABLES:
        path = TABLE_DIR / name
        if not path.exists():
            print(f"缺少分表：{path}", file=sys.stderr)
            return 1
        rows, errs = parse_table(path, want)
        errors.extend(errs)
        all_rows.extend(rows)
        print(f"{name}: {len(rows)} 条")

    seen = {}
    duplicates = []
    for row in all_rows:
        key = (row[0], row[2])
        if key in seen:
            duplicates.append(f"{row[3]} 与 {seen[key]} 重复：{row[0]},{row[2]}")
        else:
            seen[key] = row[3]
    errors.extend(duplicates)

    if errors:
        print(f"\n共 {len(errors)} 处问题：", file=sys.stderr)
        for err in errors[:40]:
            print("  " + err, file=sys.stderr)
        if len(errors) > 40:
            print(f"  ……另有 {len(errors) - 40} 处", file=sys.stderr)
        return 1

    all_rows.sort(key=sort_key)
    out_lines = list(HEADER) + [f"{code},{pos}={word}" for code, pos, word, _ in all_rows]
    text = "\r\n".join(out_lines)
    payload = b"\xff\xfe" + text.encode("utf-16-le")

    if args.check:
        if OUT.exists() and OUT.read_bytes() == payload:
            print(f"\n{OUT.name} 与分表一致（{len(all_rows)} 条）")
            return 0
        print(f"\n{OUT.name} 与分表不一致，需要重新合并", file=sys.stderr)
        return 1

    OUT.write_bytes(payload)
    print(f"\n已写出 {OUT}（{len(all_rows)} 条，{len(payload)} 字节）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
