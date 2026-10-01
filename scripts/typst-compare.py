"""Typst / TeX 双引擎版式对照。

先跑 `cargo test --locked typst_tex_compare -- --ignored`，它把每个用例的
`tex.pdf` 与 `typst.pdf` 写到 `tmp/typst-compare/<用例>/`。本脚本逐页取出两份 PDF
的文字行（基线 y、左右 x，单位毫米），按文字内容配对后报告：

- 页数是否一致；
- 每一行的基线差、左边界差（超过阈值的标出来）；
- 只出现在一边的行（多半是断行不同）。

并在每个用例目录下生成 `compare-<页>.png`（左 TeX、右 Typst）供目视检查。

依赖 PyMuPDF（`pip install pymupdf`）与 Pillow；生成对比图另需 poppler 的 pdftoppm。

用法：
    python scripts/typst-compare.py [用例名 ...] [--tol 0.3] [--no-images]
"""

import argparse
import shutil
import subprocess
import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
try:
    import pymupdf as fitz
except ImportError:  # 旧版 PyMuPDF
    import fitz

MM = 25.4 / 72
ROOT = Path(__file__).resolve().parent.parent / "tmp" / "typst-compare"


def lines_of(path):
    """返回 [(页, 基线mm, 左mm, 右mm, 文字)]；同一基线的片段合并成一行。"""
    doc = fitz.open(path)
    out = []
    for pno, page in enumerate(doc, 1):
        # 横页（TeX 的 /Rotate 90 与 Typst 的横向页面）统一按显示方向取坐标。
        rot = page.rotation_matrix
        raw = []
        for block in page.get_text("dict")["blocks"]:
            for line in block.get("lines", []):
                spans = [s for s in line["spans"] if s["text"].strip()]
                if not spans:
                    continue
                origin = fitz.Point(spans[0]["origin"]) * rot
                x0 = min(fitz.Rect(s["bbox"]).transform(rot).x0 for s in spans)
                x1 = max(fitz.Rect(s["bbox"]).transform(rot).x1 for s in spans)
                text = "".join(s["text"] for s in spans).replace(" ", "").replace("\u00a0", "")
                raw.append((origin.y * MM, x0 * MM, x1 * MM, text))
        raw.sort(key=lambda r: (round(r[0], 1), r[1]))
        merged = []
        for y, x0, x1, text in raw:
            if merged and abs(merged[-1][1] - y) < 0.6:
                p, my, mx0, mx1, mt = merged[-1]
                merged[-1] = (p, my, min(mx0, x0), max(mx1, x1), mt + text)
            else:
                merged.append((pno, y, x0, x1, text))
        out.extend(merged)
    return out, len(doc)


def compare(case_dir, tol, images):
    tex_pdf, typ_pdf = case_dir / "tex.pdf", case_dir / "typst.pdf"
    if not tex_pdf.exists() or not typ_pdf.exists():
        print(f"== {case_dir.name}：缺少 PDF，跳过")
        return False
    a, pages_a = lines_of(tex_pdf)
    b, pages_b = lines_of(typ_pdf)
    print(f"== {case_dir.name}：页数 TeX {pages_a} / Typst {pages_b}"
          + ("" if pages_a == pages_b else "  ← 页数不同"))
    unmatched_b = list(b)
    worst = 0.0
    only_tex = []
    for p, y, x0, x1, text in a:
        hit = next((r for r in unmatched_b if r[4] == text and r[0] == p), None)
        if hit is None:
            hit = next((r for r in unmatched_b if r[4] == text), None)
        if hit is None:
            only_tex.append((p, y, text))
            continue
        unmatched_b.remove(hit)
        dy, dx = hit[1] - y, hit[2] - x0
        same_page = hit[0] == p
        if same_page:
            worst = max(worst, abs(dy))
        if not same_page or abs(dy) > tol or abs(dx) > tol:
            where = f"第{p}页" if same_page else f"TeX 第{p}页 → Typst 第{hit[0]}页"
            print(f"   {where} Δy={dy:+.2f} Δx={dx:+.2f}  {text[:28]}")
    for p, y, text in only_tex:
        print(f"   仅 TeX  第{p}页 y={y:6.2f}  {text[:32]}")
    for p, y, x0, x1, text in unmatched_b:
        print(f"   仅 Typst 第{p}页 y={y:6.2f}  {text[:32]}")
    print(f"   同文行最大基线差 {worst:.2f}mm；仅一边出现的行 TeX {len(only_tex)} / Typst {len(unmatched_b)}")
    if images and shutil.which("pdftoppm"):
        render_pairs(case_dir, max(pages_a, pages_b))
    return pages_a == pages_b


def render_pairs(case_dir, pages):
    from PIL import Image, ImageDraw

    for old in case_dir.glob("_r*.png"):
        old.unlink()
    for name in ("tex", "typst"):
        subprocess.run(["pdftoppm", "-r", "60", "-png", str(case_dir / f"{name}.pdf"),
                        str(case_dir / f"_r{name}")], check=True)
    for i in range(1, pages + 1):
        def load(name):
            for pattern in (f"_r{name}-{i}.png", f"_r{name}-{i:02d}.png"):
                path = case_dir / pattern
                if path.exists():
                    return Image.open(path).convert("RGB")
            return None
        left, right = load("tex"), load("typst")
        w = max(im.size[0] for im in (left, right) if im) if (left or right) else 0
        h = max(im.size[1] for im in (left, right) if im) if (left or right) else 0
        canvas = Image.new("RGB", (w * 2 + 10, h), "gray")
        if left:
            canvas.paste(left, (0, 0))
        if right:
            canvas.paste(right, (w + 10, 0))
        draw = ImageDraw.Draw(canvas)
        draw.text((6, 4), "TeX", fill="blue")
        draw.text((w + 16, 4), "Typst", fill="blue")
        canvas.save(case_dir / f"compare-{i}.png")
    for old in case_dir.glob("_r*.png"):
        old.unlink()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("cases", nargs="*")
    parser.add_argument("--tol", type=float, default=0.3, help="基线 / 左边界差的容差（毫米）")
    parser.add_argument("--no-images", action="store_true")
    args = parser.parse_args()
    sys.stdout.reconfigure(encoding="utf-8")
    dirs = [ROOT / c for c in args.cases] if args.cases else sorted(p for p in ROOT.iterdir() if p.is_dir())
    for d in dirs:
        compare(d, args.tol, not args.no_images)


if __name__ == "__main__":
    main()
