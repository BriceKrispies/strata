"""Draw the publication's cover plates as SVG line art.

Run from the repository root:  python scripts/covers.py
The plates are deterministic: the same script always writes the same files.
"""

import math
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "web" / "public" / "media"
PLATE, INK, SOFT = "#e9e2d2", "#2b2924", "#8d8672"
W, H = 640, 480


def svg(title, body, accent):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" role="img" aria-label="{title}">\n'
        f'<rect width="{W}" height="{H}" fill="{PLATE}"/>\n'
        f'<g fill="none" stroke="{INK}" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">\n'
        f"{body}\n</g>\n</svg>\n"
    ).replace("ACCENT", accent)


def line(x1, y1, x2, y2, extra=""):
    return f'<path d="M{x1:.1f} {y1:.1f}L{x2:.1f} {y2:.1f}" {extra}/>'


def wobble(seed, amount=1.0):
    """A small repeatable offset, so ruled lines look drawn rather than plotted."""
    return math.sin(seed * 12.9898) * amount


def jig():
    parts = [
        # The bench top.
        line(60, 380, 580, 380),
        line(60, 396, 580, 396, f'stroke="{SOFT}" stroke-width="1.5"'),
        # The plywood fixture: a base and an angled fence.
        '<path d="M120 380V330H500V380"/>',
        '<path d="M170 330L250 150L300 150L225 330" fill="ACCENT" fill-opacity="0.28"/>',
        '<path d="M360 330V250H470V330"/>',
        # Plies along the base edge.
        *[line(120, 340 + i * 10, 500, 340 + i * 10, f'stroke="{SOFT}" stroke-width="1"') for i in range(4)],
        # The chair leg resting in the fixture at eleven degrees.
        '<path d="M286 96L330 104L270 372L226 364Z" fill="#f3eee2"/>',
        *[line(292 - i * 9, 120 + i * 40, 318 - i * 9, 125 + i * 40, f'stroke="{SOFT}" stroke-width="1"') for i in range(6)],
        # A clamp.
        '<path d="M392 250V200H440V250M416 200V168M400 168H432"/>',
        # Pencil marks and a date.
        line(132, 322, 160, 322, f'stroke="{SOFT}" stroke-width="1.5"'),
        line(140, 314, 140, 330, f'stroke="{SOFT}" stroke-width="1.5"'),
        f'<path d="M380 312q10 -8 20 0t20 0t20 0t20 0" stroke="{SOFT}" stroke-width="1.5"/>',
        f'<path d="M380 300q8 -6 16 0t16 0" stroke="{SOFT}" stroke-width="1.5"/>',
    ]
    return svg("A plywood fixture holding a chair leg at an angle", "\n".join(parts), "#4f6140")


def margin():
    parts = ['<path d="M70 110Q195 84 320 116Q445 84 570 110V380Q445 354 320 386Q195 354 70 380Z" fill="#f3eee2"/>', line(320, 116, 320, 386)]
    # Left page: generous margins. Right page: the gutter margin squeezed.
    for i in range(13):
        y = 150 + i * 16
        parts.append(line(118, y + wobble(i), 276 - (i % 4) * 14, y + wobble(i + 1), f'stroke="{SOFT}" stroke-width="2"'))
        parts.append(line(334, y - 6 + wobble(i + 2), 522 - (i % 3) * 18, y - 6 + wobble(i + 3), f'stroke="{SOFT}" stroke-width="2"'))
    parts.append('<path d="M322 136V366M334 136V366" stroke="ACCENT" stroke-width="2"/>')
    parts.append('<path d="M322 124H334M322 378H334" stroke="ACCENT" stroke-width="2"/>')
    parts.append('<path d="M76 392Q198 366 320 398Q442 366 564 392" stroke-width="1.5"/>')
    return svg("An open book with one narrow margin marked", "\n".join(parts), "#8c2f1f")


def kiln():
    parts = []
    # Courses of brick in an arch.
    for ring, radius in enumerate((210, 176, 142)):
        parts.append(f'<path d="M{320 - radius} 400V{300}A{radius} {radius} 0 0 1 {320 + radius} 300V400"/>')
        count = 9 + ring * -1
        for i in range(1, count):
            a = math.pi * i / count
            r2 = radius - 34
            parts.append(
                line(320 - math.cos(a) * radius, 300 - math.sin(a) * radius, 320 - math.cos(a) * r2, 300 - math.sin(a) * r2, f'stroke="{SOFT}" stroke-width="1.5"')
            )
    parts.append('<path d="M212 400V300A108 108 0 0 1 428 300V400Z" fill="#3a352c"/>')
    parts.append('<circle cx="320" cy="300" r="20" fill="ACCENT" stroke="none"/>')
    parts.append('<circle cx="320" cy="300" r="38" stroke="ACCENT" stroke-width="1.5" stroke-dasharray="3 7"/>')
    parts.append(line(60, 400, 580, 400))
    # Pots waiting in front.
    for x, w, h in ((96, 26, 50), (150, 34, 34), (470, 30, 58), (528, 24, 40)):
        parts.append(f'<path d="M{x - w} {400}Q{x - w - 8} {400 - h * 0.6} {x - w * 0.6} {400 - h}H{x + w * 0.6}Q{x + w + 8} {400 - h * 0.6} {x + w} 400" fill="#f3eee2"/>')
    return svg("The arched mouth of a brick kiln with pots in front", "\n".join(parts), "#b5602f")


def slow():
    parts = []
    for i in range(1, 19):
        r = i * 11.5
        # Each ring is a slightly irregular closed curve.
        points = []
        for step in range(48):
            a = 2 * math.pi * step / 48
            rr = r + wobble(i * 7 + step, 1.6) + math.sin(a * 3 + i) * (i * 0.22)
            points.append((320 + math.cos(a) * rr * 1.18, 240 + math.sin(a) * rr))
        d = "M" + "L".join(f"{x:.1f} {y:.1f}" for x, y in points) + "Z"
        style = 'stroke="ACCENT" stroke-width="4"' if i == 11 else f'stroke-width="{1.2 if i % 3 else 2}"'
        parts.append(f'<path d="{d}" {style}/>')
    parts.append(line(320, 240, 536, 164, f'stroke="{SOFT}" stroke-width="1.5"'))
    parts.append(line(320, 240, 330, 248, f'stroke="{SOFT}" stroke-width="1.5"'))
    return svg("Growth rings of a tree with one ring picked out", "\n".join(parts), "#4f6140")


def drawing():
    parts = []
    for n, x in enumerate((90, 250, 410)):
        tilt = (n - 1) * 3
        parts.append(f'<path d="M{x} 110L{x + 130} {112 + tilt}L{x + 128} 350L{x + 2} {348 - tilt}Z" fill="#f3eee2"/>')
        parts.append(f'<path d="M{x + 16} 128H{x + 114}V228H{x + 16}Z" stroke="{SOFT}" stroke-width="1.5"/>')
        parts.append(f'<path d="M{x + 16} 246H{x + 114}V334H{x + 16}Z" stroke="{SOFT}" stroke-width="1.5"/>')
        handle_x = x + 26 if n < 2 else x + 104
        colour = 'stroke="ACCENT" stroke-width="5"' if n == 2 else ""
        parts.append(line(handle_x, 222, handle_x, 262, colour))
        parts.append(line(x - 14, 352, x + 146, 352, f'stroke="{SOFT}" stroke-width="1.5"'))
    # The first two are struck through.
    parts.append(line(84, 120, 226, 344, f'stroke="{SOFT}" stroke-width="1.5"'))
    parts.append(line(244, 120, 386, 344, f'stroke="{SOFT}" stroke-width="1.5"'))
    # A pencil lying beneath.
    parts.append('<path d="M150 410L470 398L492 404L470 412Z" fill="#f3eee2"/>')
    parts.append('<path d="M470 398V412M492 404L500 404"/>')
    return svg("Three sketches of the same door, the third with its handle moved", "\n".join(parts), "#2f4a5c")


def hours():
    parts = [line(50, 372, 590, 372), line(50, 384, 590, 384, f'stroke="{SOFT}" stroke-width="1.5"')]
    for i in range(7):
        x = 96 + i * 76
        height = 96 + i * 16
        lean = (6 - i) * 2.2
        width = 24
        top = 372 - height
        fill = 'fill="ACCENT" fill-opacity="0.3"' if i == 6 else 'fill="#f3eee2"'
        parts.append(f'<path d="M{x - width} 372L{x - width + lean} {top}H{x + width + lean * 0.4}L{x + width} 372Z" {fill}/>')
        parts.append(f'<ellipse cx="{x + lean * 0.7:.1f}" cy="{top}" rx="{width - lean * 0.3:.1f}" ry="5"/>')
        for ring in range(1, 4):
            y = 372 - ring * height / 4
            parts.append(line(x - width + 4, y, x + width - 4, y + wobble(i + ring, 1.2), f'stroke="{SOFT}" stroke-width="1"'))
    return svg("A row of thrown cylinders, each a little taller and straighter", "\n".join(parts), "#765610")


def mark():
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
        '<rect width="64" height="64" rx="10" fill="#efeadf"/>'
        '<path d="M14 20h36M14 32h28M14 44h20" fill="none" stroke="#4f6140" stroke-width="6" stroke-linecap="round"/>'
        "</svg>\n"
    )


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    plates = {"jig": jig(), "margin": margin(), "kiln": kiln(), "slow": slow(), "drawing": drawing(), "hours": hours(), "mark": mark()}
    for name, text in plates.items():
        (OUT / f"{name}.svg").write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {name}.svg ({len(text)} bytes)")


if __name__ == "__main__":
    main()
