#!/usr/bin/env python3
"""Generate the theme-aware raster app icon family from one shared geometry."""

from __future__ import annotations

from pathlib import Path
from typing import Iterable

from PIL import Image, ImageDraw, ImageFilter, ImageFont


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "assets" / "app-icon"
DOC_OUT = ROOT / "assets" / "doc-icon"
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
        "tile_top": "#2A2927",
        "tile_bottom": "#171614",
        "accent": "#C96442",
        "accent_light": "#D47450",
    },
    # Terminal-inspired schemes. The tile stays dark in every theme so the icon
    # reads at taskbar size; only the accent tracks the theme's accent colour.
    "nord": {
        "tile_top": "#3B4252",
        "tile_bottom": "#2E3440",
        "accent": "#88C0D0",
        "accent_light": "#9BD0DE",
    },
    "gruvbox-dark": {
        "tile_top": "#282828",
        "tile_bottom": "#1D2021",
        "accent": "#FE8019",
        "accent_light": "#FF9645",
    },
}


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


def polygon_mask(size: int, points: Iterable[tuple[float, float]]) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).polygon(list(points), fill=255)
    return mask


def vertical_gradient(size: int, top: str, bottom: str) -> Image.Image:
    top_rgb = hex_rgb(top)
    bottom_rgb = hex_rgb(bottom)
    image = Image.new("RGBA", (size, size))
    pixels = image.load()
    for y in range(size):
        color = mix(top_rgb, bottom_rgb, y / max(size - 1, 1))
        for x in range(size):
            pixels[x, y] = (*color, 255)
    return image


def diagonal_gradient(size: int, start: str, end: str) -> Image.Image:
    start_rgb = hex_rgb(start)
    end_rgb = hex_rgb(end)
    image = Image.new("RGBA", (size, size))
    pixels = image.load()
    for y in range(size):
        for x in range(size):
            t = min(max((x + 0.35 * y) / (1.35 * (size - 1)), 0), 1)
            pixels[x, y] = (*mix(start_rgb, end_rgb, t), 255)
    return image


def render_icon(theme: dict[str, str], size: int = 1024) -> Image.Image:
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    tile_mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(tile_mask).rounded_rectangle((56, 56, 968, 968), radius=218, fill=255)
    image.alpha_composite(Image.composite(vertical_gradient(size, theme["tile_top"], theme["tile_bottom"]), Image.new("RGBA", (size, size)), tile_mask))

    # A restrained shadow separates the white sheet from the dark tile without becoming skeuomorphic.
    shadow_mask = Image.new("L", (size, size), 0)
    paper = [
        (235, 520),
        (353, 276),
        *cubic((353, 276), (370, 241), (402, 230), (440, 237)),
        (614, 270),
        *cubic((614, 270), (648, 276), (670, 298), (681, 329)),
        (772, 590),
        (516, 714),
    ]
    ImageDraw.Draw(shadow_mask).polygon([(x + 8, y + 16) for x, y in paper], fill=135)
    shadow_mask = shadow_mask.filter(ImageFilter.GaussianBlur(18))
    shadow = Image.new("RGBA", (size, size), (0, 0, 0, 92))
    image.alpha_composite(Image.composite(shadow, Image.new("RGBA", (size, size)), shadow_mask))

    paper_mask = polygon_mask(size, paper)
    paper_fill = vertical_gradient(size, "#FFFFFF", "#F2F3F7")
    image.alpha_composite(Image.composite(paper_fill, Image.new("RGBA", (size, size)), paper_mask))

    left = [(235, 520)]
    left += cubic((235, 520), (218, 548), (225, 579), (246, 616))
    left += [(354, 801)]
    left += cubic((354, 801), (377, 840), (421, 845), (449, 812))
    left += [(524, 724), (453, 654), (380, 741)]
    left += cubic((380, 741), (363, 761), (344, 755), (331, 734))
    left += [(235, 570)]
    left += cubic((235, 570), (225, 553), (224, 536), (235, 520))

    right = [(453, 654), (524, 724), (602, 650)]
    right += cubic((602, 650), (627, 628), (653, 617), (686, 614))
    right += [(828, 601)]
    right += cubic((828, 601), (858, 598), (878, 618), (884, 647))
    right += [(910, 770)]
    right += cubic((910, 770), (916, 800), (897, 824), (868, 830))
    right += [(681, 866)]
    right += cubic((681, 866), (643, 873), (610, 860), (585, 832))
    right += [(453, 680)]

    ribbon = diagonal_gradient(size, theme["accent"], theme["accent_light"])
    image.alpha_composite(Image.composite(ribbon, Image.new("RGBA", (size, size)), polygon_mask(size, left)))
    image.alpha_composite(Image.composite(ribbon, Image.new("RGBA", (size, size)), polygon_mask(size, right)))

    # The narrow darker fold makes the continuous ribbon readable down to taskbar size.
    fold = [(524, 724), (602, 650), (625, 631), (552, 754), (585, 832), (548, 790)]
    fold_color = mix(hex_rgb(theme["accent"]), hex_rgb(theme["tile_bottom"]), 0.22)
    ImageDraw.Draw(image).polygon(fold, fill=(*fold_color, 205))
    return image


def label_font(size: int) -> ImageFont.FreeTypeFont:
    for path in LABEL_FONTS:
        if Path(path).exists():
            return ImageFont.truetype(path, size)
    raise SystemExit("找不到粗体西文字体，请在 LABEL_FONTS 里补一个本机路径")


def render_document_icon(theme: dict[str, str], compact: bool, size: int = 1024) -> Image.Image:
    """`.gwx` 稿件包的文档图标：折角白纸 + 居中的程序图标 + 底部强调色色带。

    compact 版给 48 px 及以下用：不写字，程序图标和色带放大、页边描线加粗，
    任务栏和资源管理器小图标尺寸下也认得出纸的轮廓。
    """
    image = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    left, top, right, bottom = 176, 48, 848, 976
    fold = 220
    radius = 56

    page_mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(page_mask)
    draw.rounded_rectangle((left, top, right, bottom), radius=radius, fill=255)
    # 右上角切掉一个三角，留给折角。
    draw.polygon([(right - fold, top - 1), (right + 1, top - 1), (right + 1, top + fold)], fill=0)

    shadow_mask = page_mask.filter(ImageFilter.GaussianBlur(20))
    shadow = Image.new("RGBA", (size, size), (0, 0, 0, 70))
    image.alpha_composite(
        Image.composite(shadow, Image.new("RGBA", (size, size)), shadow_mask),
        dest=(0, 14),
    )
    page_fill = vertical_gradient(size, "#FFFFFF", "#F1F2F6")
    image.alpha_composite(Image.composite(page_fill, Image.new("RGBA", (size, size)), page_mask))

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

    # 折角：一块略深的三角，带圆角的那一侧朝里。
    edge.polygon(
        [(right - fold, top), (right - fold, top + fold - 36), (right - 36, top + fold)],
        fill=(*hex_rgb("#DDE0E7"), 255),
    )
    edge.rounded_rectangle(
        (right - fold, top + fold - 72, right - fold + 72, top + fold),
        radius=36,
        fill=(*hex_rgb("#DDE0E7"), 255),
    )

    band_top = 700 if compact else 744
    band_bottom = 920 if compact else 892
    band_mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(band_mask).rectangle((left, band_top, right, band_bottom), fill=255)
    band_mask = Image.composite(band_mask, Image.new("L", (size, size), 0), page_mask)
    band = diagonal_gradient(size, theme["accent"], theme["accent_light"])
    image.alpha_composite(Image.composite(band, Image.new("RGBA", (size, size)), band_mask))

    mark_size = 560 if compact else 470
    mark_top = 150 if compact else 230
    mark = render_icon(theme).resize((mark_size, mark_size), Image.Resampling.LANCZOS)
    image.alpha_composite(mark, dest=((left + right - mark_size) // 2, mark_top))

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
    theme = THEMES["claude"]
    full = render_document_icon(theme, compact=False)
    compact = render_document_icon(theme, compact=True)
    icons = {
        size: (compact if size <= DOC_COMPACT_MAX else full).resize((size, size), resampling)
        for size in DOC_SIZES
    }
    for size, icon in icons.items():
        icon.save(DOC_OUT / f"gwx-document-{size}.png", optimize=True)
    icons[512].save(DOC_OUT / "gwx-document.png", optimize=True)
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    # Pillow 的 ICO 按 sizes 从一张底图缩放；不同尺寸用不同底图要走 append_images。
    icons[256].save(
        DOC_OUT / "gwx-document.ico",
        format="ICO",
        sizes=[(size, size) for size in ico_sizes],
        append_images=[icons[size] for size in ico_sizes if size != 256],
    )


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    resampling = Image.Resampling.LANCZOS
    rendered: dict[str, dict[int, Image.Image]] = {}
    for name, theme in THEMES.items():
        base = render_icon(theme)
        theme_dir = OUT / "themes" / name
        theme_dir.mkdir(parents=True, exist_ok=True)
        rendered[name] = {}
        for size in SIZES:
            icon = base.resize((size, size), resampling)
            rendered[name][size] = icon
            icon.save(theme_dir / f"app-icon-{size}.png", optimize=True)

    # Claude is the stable installer/launcher identity; the runtime window icon follows the theme.
    for size in SIZES:
        rendered["claude"][size].save(OUT / f"app-icon-{size}.png", optimize=True)
    rendered["claude"][512].save(OUT / "app-icon.png", optimize=True)
    rendered["claude"][256].save(
        OUT / "app-icon.ico",
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    write_document_icons(resampling)


if __name__ == "__main__":
    main()
