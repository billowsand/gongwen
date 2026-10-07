#!/usr/bin/env python3
"""从固定矢量轮廓生成 MDEX 纸墨与夜墨图标，不依赖本机字体或设计稿截图。"""

from pathlib import Path

from PIL import Image, ImageDraw


OUT = Path(__file__).resolve().parents[1] / "assets/app-icon/themes/mdex"
SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
# 高对比衬线 M：细左竖、粗右竖，斜笔相交形成 TeX 式的字面。
M = (
    (196, 260), (335, 260), (495, 616), (656, 260),
    (800, 260), (800, 280), (751, 285), (751, 700),
    (800, 710), (800, 730), (601, 730), (601, 710),
    (650, 700), (650, 324), (469, 730), (447, 730),
    (262, 320), (262, 664), (269, 694), (291, 707),
    (317, 710), (317, 730), (196, 730), (196, 710),
    (222, 707), (239, 694), (244, 664), (244, 285),
    (196, 280),
)


def render_family(out: Path, tile: str, ink: str, cursor: str, border: str) -> None:
    out.mkdir(parents=True, exist_ok=True)
    points = " ".join(f"{x},{y}" for x, y in M)
    # 同一套几何同时保留可编辑 SVG 与原生窗口用的多尺寸 PNG/ICO。
    svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <rect x="56" y="56" width="912" height="912" rx="130" fill="{tile}" stroke="{border}" stroke-width="8"/>
  <polygon points="{points}" fill="{ink}"/>
  <path d="M756 770H866V880H756Z" fill="{cursor}"/>
</svg>
'''
    (out / "app-icon.svg").write_text(svg, encoding="utf-8")
    base = Image.new("RGBA", (2048, 2048))
    draw = ImageDraw.Draw(base)
    draw.rounded_rectangle((104, 104, 1944, 1944), radius=268, fill=border)
    draw.rounded_rectangle((120, 120, 1928, 1928), radius=252, fill=tile)
    draw.polygon([(x * 2, y * 2) for x, y in M], fill=ink)
    draw.rectangle((1512, 1540, 1732, 1760), fill=cursor)
    for size in SIZES:
        base.resize((size, size), Image.Resampling.LANCZOS).save(
            out / f"app-icon-{size}.png", optimize=True
        )
    base.save(out / "app-icon.ico", sizes=[(s, s) for s in SIZES if s <= 256])


def main() -> None:
    render_family(OUT, "#FAF8F2", "#20221F", "#216B45", "#20221F")
    render_family(OUT.with_name("mdex-dark"), "#191C19", "#E9E7DE", "#A9C98F", "#687762")


if __name__ == "__main__":
    main()
