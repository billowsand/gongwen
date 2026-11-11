#!/usr/bin/env python3
"""生成系统应用图标、主题图标和稿件包图标。"""

from __future__ import annotations

import argparse

from pathlib import Path
from typing import Iterable

from PIL import Image, ImageDraw, ImageFilter, ImageFont


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "assets" / "app-icon"
APP_SOURCE = OUT / "app-icon-source.png"
DOC_OUT = ROOT / "assets" / "doc-icon"
BRAND_MARK = ROOT / "assets" / "icons" / "brand-mark.svg"
SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
DOC_SIZES = (16, 24, 32, 48, 64, 128, 256, 512)
# 小于等于这个尺寸的文档图标不写「GWX」字样，字在任务栏尺寸下只会糊成一团。
DOC_COMPACT_MAX = 48
# 文档图标色带上的字：按顺序找第一个存在的粗体西文字体。
LABEL_FONTS = (
    "C:/Windows/Fonts/segoeuib.ttf",
    "C:/Windows/Fonts/arialbd.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
)
THEMES = {
    "claude": {
        "tile": ("#F7F6F3", "#DEDDD9"),
        "ink": ("#292928", "#101010"),
    },
    "nord": {
        "tile": ("#EEF2F7", "#CCD5E2"),
        "ink": ("#344354", "#161E2B"),
    },
    "gruvbox-dark": {
        "tile": ("#F4EBDA", "#DBCCB2"),
        "ink": ("#40352B", "#201915"),
    },
    "mdex": {
        "tile": ("#F8F7EF", "#DDE2D2"),
        "ink": ("#29352C", "#111A14"),
    },
    "mdex-dark": {
        "tile": ("#343B35", "#181D19"),
        "ink": ("#F2F2E9", "#C6CEBA"),
    },
}

# 轮廓坐标以 1024 为画布。COUNTER 是「文」字两笔交汇处留下的墨面。
PAGE = (
    ("M", 309, 203),
    ("L", 593, 203),
    ("C", 605, 203, 615, 207, 624, 215),
    ("L", 747, 333),
    ("C", 758, 343, 765, 357, 765, 373),
    ("L", 765, 746),
    ("C", 765, 781, 740, 806, 706, 806),
    ("L", 309, 806),
    ("C", 274, 806, 249, 781, 249, 746),
    ("L", 249, 263),
    ("C", 249, 228, 274, 203, 309, 203),
)
DOT = (
    ("M", 484, 380),
    ("L", 528, 367),
    ("L", 554, 414),
    ("L", 507, 429),
)
WRITING = (
    ("M", 332, 442),
    ("L", 682, 442),
    ("L", 682, 491),
    ("L", 625, 491),
    ("C", 611, 539, 586, 581, 549, 617),
    ("C", 584, 642, 631, 659, 689, 674),
    ("L", 659, 727),
    ("C", 597, 707, 545, 683, 504, 653),
    ("C", 461, 684, 409, 709, 352, 728),
    ("L", 323, 678),
    ("C", 376, 664, 423, 643, 462, 616),
    ("C", 426, 580, 401, 538, 386, 491),
    ("L", 332, 491),
)
COUNTER = (
    ("M", 442, 491),
    ("L", 569, 491),
    ("C", 555, 524, 534, 554, 506, 580),
    ("C", 477, 554, 456, 524, 442, 491),
)
FOLD = (
    ("M", 608, 205),
    ("C", 615, 208, 620, 211, 624, 215),
    ("L", 747, 333),
    ("C", 755, 341, 760, 350, 763, 360),
    ("L", 652, 360),
    ("C", 626, 360, 608, 342, 608, 316),
)


def hex_rgb(value: str) -> tuple[int, int, int]:
    value = value.removeprefix("#")
    return tuple(int(value[index : index + 2], 16) for index in (0, 2, 4))


def mix(a: tuple[int, int, int], b: tuple[int, int, int], t: float) -> tuple[int, int, int]:
    return tuple(round(left + (right - left) * t) for left, right in zip(a, b))


def cubic(
    start: tuple[float, float],
    control_a: tuple[float, float],
    control_b: tuple[float, float],
    end: tuple[float, float],
    steps: int = 16,
) -> list[tuple[float, float]]:
    points = []
    for step in range(1, steps + 1):
        t = step / steps
        inverse = 1.0 - t
        x = (
            inverse**3 * start[0]
            + 3 * inverse**2 * t * control_a[0]
            + 3 * inverse * t**2 * control_b[0]
            + t**3 * end[0]
        )
        y = (
            inverse**3 * start[1]
            + 3 * inverse**2 * t * control_a[1]
            + 3 * inverse * t**2 * control_b[1]
            + t**3 * end[1]
        )
        points.append((x, y))
    return points


def outline_points(outline: Iterable[tuple]) -> list[tuple[float, float]]:
    points = []
    for command, *coordinates in outline:
        if command == "C":
            ax, ay, bx, by, x, y = coordinates
            points.extend(cubic(points[-1], (ax, ay), (bx, by), (x, y)))
        else:
            x, y = coordinates
            points.append((x, y))
    return points


def paint_outline(draw: ImageDraw.ImageDraw, outline: Iterable[tuple], fill: int) -> None:
    draw.polygon([(2 * x, 2 * y) for x, y in outline_points(outline)], fill=fill)


def vertical_gradient(size: int, top: str, bottom: str) -> Image.Image:
    top_rgb = hex_rgb(top)
    bottom_rgb = hex_rgb(bottom)
    strip = Image.new("RGB", (1, size))
    strip.putdata([mix(top_rgb, bottom_rgb, y / (size - 1)) for y in range(size)])
    return strip.resize((size, size), Image.Resampling.NEAREST)


def over(image: Image.Image, fill: Image.Image | str, mask: Image.Image) -> None:
    layer = Image.new("RGBA", image.size, fill) if isinstance(fill, str) else fill.convert("RGBA")
    layer.putalpha(mask)
    image.alpha_composite(layer)


def shadow(
    image: Image.Image, mask: Image.Image, offset: tuple[int, int], blur: float, opacity: float
) -> None:
    shifted = Image.new("L", image.size)
    shifted.paste(mask, offset)
    blurred = shifted.filter(ImageFilter.GaussianBlur(blur))
    over(image, "#000000", blurred.point(lambda alpha: round(alpha * opacity)))


def writing_mask(size: int, *, cutout: bool) -> Image.Image:
    mask = Image.new("L", (size * 2, size * 2))
    draw = ImageDraw.Draw(mask)
    if cutout:
        paint_outline(draw, PAGE, 255)
    foreground = 0 if cutout else 255
    paint_outline(draw, DOT, foreground)
    paint_outline(draw, WRITING, foreground)
    paint_outline(draw, COUNTER, 255 - foreground)
    return mask.resize((size, size), Image.Resampling.LANCZOS)


def render_icon(tile_colors: tuple[str, str], ink_colors: tuple[str, str]) -> Image.Image:
    size = 1024
    tile = vertical_gradient(size, *tile_colors).convert("RGBA")
    mark = writing_mask(size, cutout=True)

    # 柔影定方向，贴边窄影让薄薄一层墨面有厚度。镂空里也保留投影。
    shadow(tile, mark, (13, 26), 24, 0.29)
    shadow(tile, mark, (2, 6), 4, 0.32)
    over(tile, vertical_gradient(size, *ink_colors), mark)

    fold = Image.new("L", (size * 2, size * 2))
    paint_outline(ImageDraw.Draw(fold), FOLD, 255)
    over(tile, ink_colors[0], fold.resize(tile.size, Image.Resampling.LANCZOS))
    tile_mask = Image.new("L", fold.size)
    ImageDraw.Draw(tile_mask).rounded_rectangle((112, 112, 1935, 1935), radius=436, fill=255)
    tile.putalpha(tile_mask.resize(tile.size, Image.Resampling.LANCZOS))
    return tile


def write_brand_mark() -> None:
    contours = (PAGE, DOT, WRITING, COUNTER)
    path = " ".join(
        " ".join(" ".join(map(str, command)) for command in outline) + " Z"
        for outline in contours
    )
    # 标题栏与菜单使用同一轮廓，以界面主题色着色。
    BRAND_MARK.write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" '
        'viewBox="163 160 688 688">\n'
        f'  <path fill="#ffffff" fill-rule="evenodd" d="{path}"/>\n'
        '</svg>\n',
        encoding="utf-8",
    )


def label_font(size: int) -> ImageFont.FreeTypeFont:
    for path in LABEL_FONTS:
        if Path(path).exists():
            return ImageFont.truetype(path, size)
    raise SystemExit("找不到粗体西文字体，请在 LABEL_FONTS 里补一个本机路径")


def render_document_icon(compact: bool) -> Image.Image:
    """`.gwx` 稿件包的文档图标：折角白纸、居中的「文」字与底部墨色色带。

    compact 版给 48 px 及以下用：不写后缀，「文」字和色带放大、页边描线加粗，
    任务栏和资源管理器小图标尺寸下也认得出纸的轮廓。
    """
    size = 1024
    image = Image.new("RGBA", (size, size))
    left, top, right, bottom = 176, 48, 848, 976
    fold = 220
    radius = 56

    page_mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(page_mask)
    draw.rounded_rectangle((left, top, right, bottom), radius=radius, fill=255)
    # 右上角切掉一个三角，留给折角。
    draw.polygon([(right - fold, top - 1), (right + 1, top - 1), (right + 1, top + fold)], fill=0)

    shadow(image, page_mask, (0, 14), 20, 0.27)
    over(image, vertical_gradient(size, "#FFFFFF", "#F1F2F6"), page_mask)

    # 页边一圈灰描线：白底资源管理器里也能看出纸的轮廓。
    outline_width = 44 if compact else 10
    outline_color = hex_rgb("#A9AFBB" if compact else "#C9CDD6")
    outline = Image.new("L", (size, size), 0)
    ImageDraw.Draw(outline).rounded_rectangle(
        (left, top, right, bottom), radius=radius, outline=255, width=outline_width
    )
    outline = Image.composite(outline, Image.new("L", (size, size), 0), page_mask)
    image.alpha_composite(
        Image.composite(Image.new("RGBA", (size, size), (*outline_color, 255)), Image.new("RGBA", (size, size)), outline)
    )
    edge = ImageDraw.Draw(image)
    edge.line([(right - fold, top), (right, top + fold)], fill=(*outline_color, 255), width=outline_width)

    # 折角贴合外边缘，内侧转角略圆。
    corner_x = right - fold
    corner_y = top + fold
    edge.polygon(
        [
            (corner_x, top), (right, corner_y), (corner_x + 36, corner_y),
            *cubic((corner_x + 36, corner_y), (corner_x + 14, corner_y),
                   (corner_x, corner_y - 14), (corner_x, corner_y - 36)),
        ],
        fill="#DDE0E7",
    )

    band_top = 700 if compact else 744
    band_bottom = 920 if compact else 892
    band_mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(band_mask).rectangle((left, band_top, right, band_bottom), fill=255)
    band_mask = Image.composite(band_mask, Image.new("L", (size, size), 0), page_mask)
    ink_colors = THEMES["claude"]["ink"]
    over(image, vertical_gradient(size, *ink_colors), band_mask)

    writing = writing_mask(size, cutout=False)
    writing = writing.crop(writing.getbbox())
    mark_width = 455 if compact else 350
    mark_height = round(mark_width * writing.height / writing.width)
    mark = Image.new("L", image.size)
    mark.paste(
        writing.resize((mark_width, mark_height), Image.Resampling.LANCZOS),
        ((size - mark_width) // 2, 239 if compact else 306),
    )
    shadow(image, mark, (8, 15), 14, 0.25)
    over(image, vertical_gradient(size, *ink_colors), mark)

    if not compact:
        font = label_font(118)
        text = "GWX"
        box = edge.textbbox((0, 0), text, font=font)
        width, height = box[2] - box[0], box[3] - box[1]
        x = (left + right - width) // 2 - box[0]
        y = (band_top + band_bottom - height) // 2 - box[1]
        edge.text((x, y), text, font=font, fill=(255, 255, 255, 255))
    return image


def write_document_icons(resampling: Image.Resampling) -> None:
    """稿件包文档图标：Windows 安装包登记用 ico，Linux 的 hicolor mimetypes 用 png。

    主题切换不影响系统里登记的图标，只出默认（claude）主题一套。
    """
    DOC_OUT.mkdir(parents=True, exist_ok=True)
    full = render_document_icon(compact=False)
    compact = render_document_icon(compact=True)
    icons = {
        size: (compact if size <= DOC_COMPACT_MAX else full).resize((size, size), resampling)
        for size in DOC_SIZES
    }
    for size, icon in icons.items():
        icon.save(DOC_OUT / f"gwx-document-{size}.png", optimize=True)
    icons[512].save(DOC_OUT / "gwx-document.png", optimize=True)
    write_ico(DOC_OUT / "gwx-document.ico", icons)


def write_ico(path: Path, icons: dict[int, Image.Image]) -> None:
    sizes = [size for size in icons if size <= 256]
    icons[256].save(
        path,
        format="ICO",
        sizes=[(size, size) for size in sizes],
        append_images=[icons[size] for size in sizes if size != 256],
    )


def write_app_icons() -> None:
    with Image.open(APP_SOURCE) as source:
        base = source.convert("RGBA")
    icons = {
        size: base.resize((size, size), Image.Resampling.LANCZOS)
        for size in SIZES
    }
    for size, icon in icons.items():
        icon.save(OUT / f"app-icon-{size}.png", optimize=True)
    icons[512].save(OUT / "app-icon.png", optimize=True)
    write_ico(OUT / "app-icon.ico", icons)
    icons[1024].save(
        OUT / "app-icon.icns",
        append_images=[icon for size, icon in icons.items() if size < 1024],
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app-only", action="store_true", help="仅重建系统应用图标")
    args = parser.parse_args()

    write_app_icons()
    if args.app_only:
        return

    resampling = Image.Resampling.LANCZOS
    for name, theme in THEMES.items():
        base = render_icon(theme["tile"], theme["ink"])
        theme_dir = OUT / "themes" / name
        theme_dir.mkdir(parents=True, exist_ok=True)
        icons = {size: base.resize((size, size), resampling) for size in SIZES}
        for size, icon in icons.items():
            icon.save(theme_dir / f"app-icon-{size}.png", optimize=True)

        if name in ("mdex", "mdex-dark"):
            write_ico(theme_dir / "app-icon.ico", icons)
    write_document_icons(resampling)
    write_brand_mark()


if __name__ == "__main__":
    main()
