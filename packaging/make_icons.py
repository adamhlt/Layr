"""Renders the app icon (assets/logo/1-overlap.svg on a dark tile) to the files the builds use.

    python packaging/make_icons.py

Writes assets/icon/icon.png (1024 px) and icon.ico (16 to 256 px), whose tile fills the square
as Windows and Linux icons do, and icon-macos.png and AppIcon.icns, whose tile sits on Apple's
grid with a margin around it. Needs Pillow.
The shapes are drawn here rather than rasterised from the SVG, so no SVG renderer is needed;
keep them in step with the SVG.
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets" / "icon"

SIZE = 1024
SS = 4  # supersampling
TILE = (0x16, 0x17, 0x1C, 255)
INK = (0xF4, 0xF4, 0xF5, 255)


def render(margin: float, glyph: float) -> Image.Image:
    """`margin`: the space around the tile, `glyph`: the logo's width, both of the canvas."""
    n = SIZE * SS
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    m = round(n * margin)
    d.rounded_rectangle([m, m, n - m, n - m], radius=round((n - 2 * m) * 0.225), fill=TILE)
    # The glyph spans 40..216 of the SVG's 256 viewBox.
    span = n * glyph
    s = span / 176.0
    o = (n - span) / 2.0 - 40 * s

    def box(x, y, w, h):
        return [o + x * s, o + y * s, o + (x + w) * s, o + (y + h) * s]

    d.rounded_rectangle(box(40, 40, 120, 120), radius=28 * s, fill=INK)
    # The outlined square: an 16-unit stroke centred on its edge.
    half = 8
    d.rounded_rectangle(box(96 - half, 96 - half, 120 + 2 * half, 120 + 2 * half), radius=(28 + half) * s, outline=INK, width=round(16 * s))
    return img.resize((SIZE, SIZE), Image.LANCZOS)


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    # Windows and Linux: the tile fills the square and the logo most of the tile.
    full = render(margin=0.0, glyph=0.68)
    full.save(OUT / "icon.png")
    full.save(OUT / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    # macOS: Apple's grid leaves about a tenth of the canvas around the tile.
    mac = render(margin=0.09, glyph=0.56)
    mac.save(OUT / "icon-macos.png")
    mac.save(OUT / "AppIcon.icns")
    print("wrote", ", ".join(p.name for p in sorted(OUT.iterdir())))


if __name__ == "__main__":
    main()
