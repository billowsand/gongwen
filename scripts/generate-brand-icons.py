#!/usr/bin/env python3
"""Generate the theme-aware raster app icon family from one shared geometry."""

from __future__ import annotations

from pathlib import Path
from typing import Iterable

from PIL import Image, ImageDraw, ImageFilter


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "assets" / "app-icon"
SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
THEMES = {
    "claude": {
        "tile_top": "#2A2927",
        "tile_bottom": "#171614",
        "accent": "#C96442",
        "accent_light": "#D47450",
    },
    "sky": {
        "tile_top": "#17242E",
        "tile_bottom": "#0B1B2A",
        "accent": "#1F8AC0",
        "accent_light": "#37A0D6",
    },
    "lilac": {
        "tile_top": "#26213A",
        "tile_bottom": "#171129",
        "accent": "#8A63C9",
        "accent_light": "#9C78D8",
    },
    "green": {
        "tile_top": "#1C2A1E",
        "tile_bottom": "#0F2014",
        "accent": "#3E8E4E",
        "accent_light": "#56A262",
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


if __name__ == "__main__":
    main()
