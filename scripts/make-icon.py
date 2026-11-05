"""Draws crates/app/ui/icon.ico: an @ built from overlapping red ribbons on a white tile. Also
draws icon-unread.ico, the same at the sizes the notification area uses, with a dot for unread mail.

Needs numpy and Pillow. Run from anywhere:  python scripts/make-icon.py [preview.png]

Each ribbon multiplies with what is under it, so a darker red appears wherever two overlap. Below
64px the overlaps stop reading, so the strokes thicken, and from 24px down the @ is one solid red.
"""

import math
import sys
from dataclasses import dataclass
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

OUT = Path(__file__).resolve().parent.parent / "crates" / "app" / "ui" / "icon.ico"
UNREAD = OUT.with_name("icon-unread.ico")

N = 1024  # drawings are made at this size and scaled down
SOLID_RED = "#d81e2c"
DOT_RED = (0xD8, 0x0D, 0x20, 255)  # the same red as the unread count on the taskbar button
DOT = 0.42  # the dot's width, as a fraction of the icon
DOT_GAP = 0.08  # the clear ring around it, which keeps it apart from the @ on any taskbar


def arc(cx, cy, r, start, end):
    steps = abs(end - start) * 2
    angles = [math.radians(start + (end - start) * i / steps) for i in range(steps + 1)]
    return [(cx + r * math.cos(a), cy + r * math.sin(a)) for a in angles]


# The @ on a 230-unit grid, as (points, colour): the outer loop from the tail round to the stem,
# then the ring. `Mark` in app.slint draws the same ribbons for use inside the app.
RIBBONS = [
    ([(112, 230), (52, 230)], "#ff8d80"),
    (arc(52, 178, 52, 90, 180), "#e8303a"),
    ([(0, 178), (0, 52)], "#ff6b66"),
    (arc(52, 52, 52, 180, 270), "#c9142c"),
    ([(52, 0), (185, 0)], "#e8303a"),
    (arc(185, 45, 45, 270, 360), "#ff8d80"),
    ([(230, 45), (230, 125)], "#ff5a52"),
    (arc(185, 125, 45, 0, 180), "#c9142c"),
    ([(140, 125), (140, 72)], "#ffa79c"),
    (arc(95, 125, 45, 180, 360), "#ff9c8f"),
    (arc(95, 125, 45, 0, 180), "#ee4150"),
]


@dataclass
class Style:
    mark: float  # the @'s width, as a fraction of the tile
    stroke: float  # ribbon thickness on the 230-unit grid
    solid: bool  # one flat red instead of overlapping ribbons


def tile_mask(size):
    big = Image.new("L", (size * 2, size * 2), 0)
    ImageDraw.Draw(big).rounded_rectangle((0, 0, size * 2 - 1, size * 2 - 1), radius=size * 0.44, fill=255)
    return big.resize((size, size), Image.LANCZOS)


def ribbon_mask(points, width):
    mask = Image.new("L", (N * 2, N * 2), 0)
    draw = ImageDraw.Draw(mask)
    if len(points) == 2:
        draw.line(points, fill=255, width=round(width))
    # Round caps, and for curves the whole stroke: thick polylines leave gaps at their joints.
    for x, y in points if len(points) > 2 else (points[0], points[-1]):
        draw.ellipse((x - width / 2, y - width / 2, x + width / 2, y + width / 2), fill=255)
    return np.asarray(mask.resize((N, N), Image.LANCZOS), float) / 255


def draw_tile(style):
    scale = style.mark * N * 2 / (230 + style.stroke)
    offset = N  # the middle of the doubled canvas, which is the grid's point (115, 115)

    def place(points):
        return [(offset + (x - 115) * scale, offset + (y - 115) * scale) for x, y in points]

    image = np.ones((N, N, 3))
    for points, colour in RIBBONS:
        tint = np.array([int(colour[i : i + 2], 16) / 255 for i in (1, 3, 5)])
        if style.solid:
            tint = np.array([int(SOLID_RED[i : i + 2], 16) / 255 for i in (1, 3, 5)])
        mask = ribbon_mask(place(points), style.stroke * scale)[..., None]
        if style.solid:
            image = image * (1 - mask) + tint * mask  # a flat colour must not darken where it overlaps
        else:
            image = image * (1 - mask + mask * tint)

    colour = Image.fromarray((image * 255 + 0.5).astype(np.uint8))
    return Image.merge("RGBA", (*colour.split(), tile_mask(N)))


def with_shadow(tile):
    """Adds room for a soft shadow, so that white still has an edge on a light background."""
    room = round(N * 0.045)
    canvas = Image.new("RGBA", (N + 2 * room,) * 2, (0, 0, 0, 0))
    shadow = Image.new("RGBA", canvas.size, (0, 0, 0, 0))
    alpha = Image.new("L", canvas.size, 0)
    alpha.paste(tile.getchannel("A").point(lambda a: a * 0.32), (room, room + 8))
    shadow.putalpha(alpha.filter(ImageFilter.GaussianBlur(13)))
    canvas.alpha_composite(shadow)
    canvas.alpha_composite(tile, (room, room))
    return canvas


def fit(master, size, margin):
    """Scales a drawing to an icon size, optionally inside a transparent margin.

    The small drawings fill their frame, and Windows sets the title bar icon hard against the
    screen edge when maximized, so the margin is all that keeps it off that edge.
    """
    inset = round(size / 12) if margin else 0
    icon = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    icon.paste(master.resize((size - 2 * inset,) * 2, Image.LANCZOS), (inset, inset))
    if not margin:
        return icon

    # White needs an edge to show on a light background, and at this size a shadow would be a smudge.
    edge = Image.new("RGBA", icon.size, (110, 110, 120, 0))
    edge.putalpha(icon.getchannel("A").filter(ImageFilter.MaxFilter(3)).point(lambda a: a * 0.4))
    return Image.alpha_composite(edge, icon)


STYLES = {
    "full": Style(mark=0.64, stroke=30, solid=False),
    "bold": Style(mark=0.72, stroke=34, solid=False),
    "tiny": Style(mark=0.82, stroke=38, solid=True),
}
SIZES = {256: "full", 128: "full", 64: "full", 48: "bold", 40: "bold", 32: "bold", 24: "tiny", 20: "tiny", 16: "tiny"}


def with_dot(icon):
    """Cuts a ring out of the corner and sets the dot in it, so that red can sit on the red @."""
    zoom = 8
    size = icon.width * zoom
    right = size - 1
    left = right - size * DOT

    def disc(grow):
        mask = Image.new("L", (size, size), 0)
        ImageDraw.Draw(mask).ellipse((left - grow, left - grow, right + grow, right + grow), fill=255)
        return mask.resize(icon.size, Image.LANCZOS)

    alpha = np.asarray(icon.getchannel("A"), float) * (1 - np.asarray(disc(size * DOT_GAP), float) / 255)
    cut = icon.copy()
    cut.putalpha(Image.fromarray((alpha + 0.5).astype(np.uint8)))
    dot = Image.new("RGBA", icon.size, DOT_RED)
    dot.putalpha(disc(0))
    return Image.alpha_composite(cut, dot)


def main():
    masters = {name: draw_tile(style) for name, style in STYLES.items()}
    masters["full"] = with_shadow(masters["full"])
    images = [fit(masters[name], n, name != "full") for n, name in SIZES.items()]
    images[0].save(OUT, format="ICO", sizes=[i.size for i in images], append_images=images[1:])
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")
    # Windows wants a PNG for the app's name on a notification.
    next(i for i in images if i.width == 64).save(OUT.with_suffix(".png"))
    unread = [with_dot(i) for i in images if i.width <= 48]
    unread[0].save(UNREAD, format="ICO", sizes=[i.size for i in unread], append_images=unread[1:])

    if len(sys.argv) > 1:
        sheet = Image.new("RGBA", (1400, 640), "#6b2330")
        sheet.paste("#f3f3f3", (0, 320, 1400, 640))
        for top in (0, 320):
            x = 16
            for i in images[:1] + images[2:]:
                zoom = 1 if i.width >= 64 else 4 if i.width >= 32 else 7
                big = i.resize((i.width * zoom, i.height * zoom), Image.NEAREST)
                sheet.alpha_composite(big, (x, top + 20))
                if zoom > 1:
                    sheet.alpha_composite(i, (x, top + 30 + big.height))
                x += big.width + 14
        sheet.save(sys.argv[1])


if __name__ == "__main__":
    main()
