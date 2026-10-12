"""Generate the Formant app icon: a machined knob on a brushed-metal plate,
with a glowing LED arc that runs from cyan into ember.

Every size is drawn at 4x and scaled down, so edges are clean. Small sizes
(16 and 32 px) are drawn with bolder strokes and no fine detail, because at
that size thin lines turn into grey mush.

Writes crates/app/icon.ico (16, 24, 32, 48, 64, 128, 256) and
crates/app/icon.rgba (256x256 raw RGBA, used for the window and tray icon).

    uv run --no-project --with pillow python packaging/make-icon.py
"""

import math
import random
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

CYAN = (45, 212, 238)
EMBER = (255, 138, 40)
SS = 4  # supersampling factor


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def vertical_gradient(w, h, top, bottom):
    col = Image.new("RGBA", (1, h))
    for y in range(h):
        col.putpixel((0, y), lerp(top, bottom, y / max(h - 1, 1)) + (255,))
    return col.resize((w, h))


def disc(size, c, r, top, bottom):
    """A disc lit from above, as an RGBA layer."""
    layer = vertical_gradient(size, size, top, bottom)
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).ellipse((c[0] - r, c[1] - r, c[0] + r, c[1] + r), fill=255)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(layer, (0, 0), mask)
    return out


def arc_points(c, r, a0, a1, steps=200):
    return [
        (c[0] + r * math.cos(a0 + (a1 - a0) * i / steps), c[1] + r * math.sin(a0 + (a1 - a0) * i / steps))
        for i in range(steps + 1)
    ]


def render(px):
    """Draw the icon for a target size of `px` pixels."""
    small = px <= 32
    S = px * SS
    u = S / 256.0  # design units: the layout below is in a 256 grid
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))

    # Plate: rounded square with a drop shadow, brushed gradient and bevel.
    m, rad = 10 * u, 46 * u
    box = (m, m, S - m, S - m)
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((m, m + 5 * u, S - m, S - m + 5 * u), rad, fill=(0, 0, 0, 170))
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(6 * u)))

    plate_mask = Image.new("L", (S, S), 0)
    ImageDraw.Draw(plate_mask).rounded_rectangle(box, rad, fill=255)
    plate = vertical_gradient(S, S, (62, 68, 77), (22, 25, 29))
    if not small:
        streaks = ImageDraw.Draw(plate)
        rng = random.Random(7)
        y = 0
        while y < S:
            v = rng.randint(0, 255)
            shade = (255, 255, 255, v % 9) if v & 1 else (0, 0, 0, v % 14)
            streaks.line((0, y, S, y), fill=shade, width=max(1, int(u * 0.6)))
            y += max(1, int(u * (0.8 + rng.random() * 1.6)))
    layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    layer.paste(plate, (0, 0), plate_mask)
    img.alpha_composite(layer)

    edge = ImageDraw.Draw(img)
    edge.rounded_rectangle(box, rad, outline=(0, 0, 0, 230), width=max(1, int(2 * u)))
    # Bevel: highlight along the top edge, shadow along the bottom.
    edge.arc((m + 2 * u, m + 2 * u, m + 2 * rad, m + 2 * rad), 180, 270, fill=(255, 255, 255, 70), width=int(2 * u))
    edge.line((m + rad, m + 2 * u, S - m - rad, m + 2 * u), fill=(255, 255, 255, 70), width=int(2 * u))
    edge.arc((S - m - 2 * rad, m + 2 * u, S - m - 2 * u, m + 2 * rad), 270, 360, fill=(255, 255, 255, 70), width=int(2 * u))

    if not small:
        for cx, cy in [(m + 22 * u, m + 22 * u), (S - m - 22 * u, m + 22 * u), (m + 22 * u, S - m - 22 * u), (S - m - 22 * u, S - m - 22 * u)]:
            r = 6 * u
            ImageDraw.Draw(img).ellipse((cx - r - u, cy - r, cx + r + u, cy + r + 2 * u), fill=(0, 0, 0, 140))
            img.alpha_composite(disc(S, (cx, cy), r, (150, 156, 163), (54, 58, 64)))
            a = math.radians(rng_angle(cx, cy))
            dx, dy = math.cos(a) * r * 0.75, math.sin(a) * r * 0.75
            ImageDraw.Draw(img).line((cx - dx, cy - dy, cx + dx, cy + dy), fill=(26, 28, 32, 255), width=max(1, int(1.6 * u)))

    # LED arc around the knob: dark track, then a glowing lit sweep from cyan
    # into ember, like a knob turned most of the way up.
    c = (S / 2, S / 2 + 6 * u)
    r_arc = (84 if small else 90) * u
    width = int((30 if small else 16) * u)
    start, sweep, lit = math.radians(135), math.radians(270), 0.78
    track = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    track.paste((4, 6, 8, 255), (0, 0), annulus_mask(S, c, r_arc, width + int(6 * u), start, start + sweep))
    img.alpha_composite(track)

    # The lit sweep is one smooth band used as a mask over an angular color
    # field, so the color runs continuously from cyan to ember. The field is
    # computed per pixel on a coarse grid and scaled up (it is smooth, so
    # nothing is lost).
    g = 128
    field = Image.new("RGBA", (g, g))
    span = sweep * lit
    for y in range(g):
        for x in range(g):
            ang = math.atan2((y + 0.5) * S / g - c[1], (x + 0.5) * S / g - c[0])
            t = ((ang - start) % math.tau) / span
            t = min(max(t, 0.0), 1.0) if t <= 1.2 else 0.0
            field.putpixel((x, y), lerp(CYAN, EMBER, max(0.0, (t - 0.45) / 0.55)) + (255,))
    color_field = field.resize((S, S), Image.BILINEAR)

    def stroke_mask(w):
        return annulus_mask(S, c, r_arc, w, start, start + span)

    glow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    glow.paste(color_field, (0, 0), stroke_mask(width + int(16 * u)))
    glow = glow.filter(ImageFilter.GaussianBlur(9 * u))
    glow.putalpha(glow.getchannel("A").point(lambda v: int(v * 0.55)))
    img.alpha_composite(glow)
    bright = Image.blend(color_field, Image.new("RGBA", (S, S), (255, 255, 255, 255)), 0.2)
    lit_layer = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    lit_layer.paste(bright, (0, 0), stroke_mask(width))
    img.alpha_composite(lit_layer)
    # Knob: soft shadow, knurled skirt, domed cap, pointer.
    r_body = (60 if small else 64) * u
    kshadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(kshadow).ellipse((c[0] - r_body, c[1] - r_body + 8 * u, c[0] + r_body, c[1] + r_body + 8 * u), fill=(0, 0, 0, 200))
    img.alpha_composite(kshadow.filter(ImageFilter.GaussianBlur(7 * u)))
    img.alpha_composite(disc(S, c, r_body, (84, 90, 99), (20, 22, 26)))
    if not small:
        kd = ImageDraw.Draw(img)
        for i in range(56):
            a = i / 56 * math.tau
            light = int((-math.sin(a) * 0.5 + 0.5) * 70)
            col = (255, 255, 255, 18 + light // 2) if i % 2 == 0 else (0, 0, 0, 90)
            kd.line(
                (c[0] + math.cos(a) * (r_body - 7 * u), c[1] + math.sin(a) * (r_body - 7 * u),
                 c[0] + math.cos(a) * (r_body - u), c[1] + math.sin(a) * (r_body - u)),
                fill=col, width=max(1, int(1.6 * u)),
            )
    r_cap = r_body - (6 if small else 9) * u
    img.alpha_composite(disc(S, c, r_cap, (128, 136, 146), (40, 44, 50)))
    rim = ImageDraw.Draw(img)
    rim.arc((c[0] - r_cap, c[1] - r_cap, c[0] + r_cap, c[1] + r_cap), 200, 340, fill=(255, 255, 255, 110), width=max(1, int(2 * u)))

    a = start + sweep * lit
    p0 = (c[0] + math.cos(a) * r_cap * 0.25, c[1] + math.sin(a) * r_cap * 0.25)
    p1 = (c[0] + math.cos(a) * r_cap * 0.88, c[1] + math.sin(a) * r_cap * 0.88)
    pw = int((12 if small else 8) * u)
    rim.line((p0, p1), fill=(0, 0, 0, 120), width=pw + int(3 * u))
    rim.line((p0, p1), fill=(238, 244, 247, 255), width=pw)

    return img.resize((px, px), Image.LANCZOS)


def annulus_mask(S, c, r, w, a0, a1):
    """A smooth band of width `w` centered on radius `r`, from angle a0 to a1
    (radians), with round caps. Built as a pie slice minus an inner disc,
    which has none of the notches a wide polyline leaves at its joints."""
    mask = Image.new("L", (S, S), 0)
    md = ImageDraw.Draw(mask)
    ro, ri = r + w / 2, r - w / 2
    md.pieslice((c[0] - ro, c[1] - ro, c[0] + ro, c[1] + ro), math.degrees(a0), math.degrees(a1), fill=255)
    md.ellipse((c[0] - ri, c[1] - ri, c[0] + ri, c[1] + ri), fill=0)
    for a in (a0, a1):
        p = (c[0] + r * math.cos(a), c[1] + r * math.sin(a))
        md.ellipse((p[0] - w / 2, p[1] - w / 2, p[0] + w / 2, p[1] + w / 2), fill=255)
    return mask


def rng_angle(x, y):
    return (x * 13 + y * 7) % 180


def main():
    root = Path(__file__).resolve().parent.parent / "crates" / "app"
    sizes = [16, 24, 32, 48, 64, 128, 256]
    images = {s: render(s) for s in sizes}
    big = images[256]
    big.save(root / "icon.ico", format="ICO", sizes=[(s, s) for s in sizes], append_images=[images[s] for s in sizes[:-1]])
    (root / "icon.rgba").write_bytes(big.tobytes())
    if len(sys.argv) > 1:
        # Optional preview sheet: every size side by side, for checking by eye.
        sheet = Image.new("RGBA", (sum(sizes) + 20 * len(sizes), 276), (30, 30, 30, 255))
        x = 10
        for s in sizes:
            sheet.alpha_composite(images[s], (x, 266 - s))
            x += s + 20
        sheet.save(sys.argv[1])
    print(f"wrote {root / 'icon.ico'} and icon.rgba")


if __name__ == "__main__":
    main()
