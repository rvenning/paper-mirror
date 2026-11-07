"""Draws Paper Mirror's original app icon: a sheet of paper with a folded corner whose
lower half is a mirror (a cool reflective panel with a diagonal highlight).

Run with any Python that has Pillow:  python make_icon.py <assets-dir>
Writes paper-mirror.ico (16-256 px) and icon-256.png.
"""
import sys
from PIL import Image, ImageDraw, ImageFilter

S = 1024  # drawing size; downsampled for each icon size


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def draw():
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    # Page geometry
    left, top, right, bottom = 196, 92, 828, 932
    fold = 190
    radius = 56
    page = [
        (left + radius, top), (right - fold, top), (right, top + fold), (right, bottom - radius),
        (right - radius, bottom), (left + radius, bottom), (left, bottom - radius), (left, top + radius),
    ]
    # Soft shadow
    shadow = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).polygon([(x + 10, y + 22) for x, y in page], fill=(0, 0, 0, 90))
    shadow = shadow.filter(ImageFilter.GaussianBlur(26))
    img.alpha_composite(shadow)

    # Page body with rounded corners: polygon + corner circles
    body = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(body)
    d.polygon(page, fill=255)
    for cx, cy in [(left + radius, top + radius), (left + radius, bottom - radius), (right - radius, bottom - radius)]:
        d.ellipse((cx - radius, cy - radius, cx + radius, cy + radius), fill=255)
    d.rectangle((left + radius, top, right - fold, top + radius), fill=255)

    paper = Image.new("RGBA", (S, S), (250, 250, 252, 255))
    img.paste(paper, (0, 0), body)

    # Mirror panel: lower part of the page, cool gradient, clipped to the page.
    mirror_top = 520
    grad = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grad)
    for y in range(mirror_top, bottom + 1):
        t = (y - mirror_top) / (bottom - mirror_top)
        gd.line([(left, y), (right, y)], fill=lerp((86, 170, 255, 255), (28, 84, 196, 255), t))
    # Diagonal highlights
    hl = Image.new("L", (S, S), 0)
    hd = ImageDraw.Draw(hl)
    hd.polygon([(left + 60, bottom), (left + 250, mirror_top), (left + 340, mirror_top), (left + 150, bottom)], fill=95)
    hd.polygon([(left + 230, bottom), (left + 420, mirror_top), (left + 455, mirror_top), (left + 265, bottom)], fill=70)
    white = Image.new("RGBA", (S, S), (255, 255, 255, 255))
    grad.paste(white, (0, 0), hl.filter(ImageFilter.GaussianBlur(2)))
    mask = Image.new("L", (S, S), 0)
    mask.paste(body, (0, 0))
    ImageDraw.Draw(mask).rectangle((0, 0, S, mirror_top - 1), fill=0)
    img.paste(grad, (0, 0), mask)

    # Text lines on the paper half
    d2 = ImageDraw.Draw(img)
    for i, w in enumerate([420, 470, 360]):
        y = 250 + i * 80
        d2.rounded_rectangle((left + 90, y, left + 90 + w, y + 30), radius=15, fill=(150, 160, 178, 255))
    # Their reflection, faint, in the mirror
    for i, w in enumerate([360, 470]):
        y = 600 + i * 80
        d2.rounded_rectangle((left + 90, y, left + 90 + w, y + 30), radius=15, fill=(255, 255, 255, 70))

    # Folded corner
    d2.polygon([(right - fold, top), (right - fold, top + fold - 30), (right - fold + 30, top + fold), (right, top + fold)],
               fill=(214, 220, 232, 255))
    # Outline for contrast on light backgrounds
    outline = Image.new("L", (S, S), 0)
    ImageDraw.Draw(outline).polygon(page, outline=255, width=10)
    img.paste(Image.new("RGBA", (S, S), (120, 130, 150, 255)), (0, 0), Image.composite(outline, Image.new("L", (S, S), 0), body))
    return img


def main():
    out = sys.argv[1]
    big = draw()
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    big.resize((256, 256), Image.LANCZOS).save(f"{out}/icon-256.png")
    big.resize((256, 256), Image.LANCZOS).save(f"{out}/paper-mirror.ico", sizes=[(s, s) for s in sizes])


if __name__ == "__main__":
    main()
