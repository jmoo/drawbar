#!/usr/bin/env python3
# nix-deps: python3 fonttools
# Regenerate drawbar-glyphs.ttf, the characters drawbar's text uses that no other bundled
# font draws.

from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.misc.timeTools import timestampSinceEpoch
from fontTools.pens.ttGlyphPen import TTGlyphPen

OUT = (
    Path(__file__).resolve().parent.parent
    / "crates/drawbar/assets/fonts/drawbar-glyphs.ttf"
)

# Ubuntu Regular's vertical metrics. egui scales a face by its ascent minus descent, so
# sharing them gives these glyphs Ubuntu's size and baseline.
UNITS_PER_EM = 1000
HHEA = (932, -189, 28)  # ascent, descent, line gap
TYPO = (776, -185, 56)  # ascender, descender, line gap
WIN = (932, 189)  # ascent, descent
X_HEIGHT, CAP_HEIGHT = 520, 693
# Ubuntu's plus and minus: 80 units thick, centered at 300.
WEIGHT = 80
AXIS = 300

# Each glyph is an advance width and its shapes, in font units:
#   ("line", points) strokes an open polyline WEIGHT wide, with flat ends;
#   ("ring", points) strokes a closed polygon WEIGHT wide, leaving its middle open;
#   ("fill", points) fills a polygon.
# Corners are mitered.
GLYPHS = {
    "→": (  # RIGHTWARDS ARROW
        820,
        [
            ("line", [(70, AXIS), (690, AXIS)]),
            ("line", [(500, AXIS + 200), (700, AXIS), (500, AXIS - 200)]),
        ],
    ),
    "⇧": (  # UPWARDS WHITE ARROW, the Shift key
        760,
        [
            (
                "ring",
                [
                    (265, 40),
                    (265, 320),
                    (130, 320),
                    (380, 625),
                    (630, 320),
                    (495, 320),
                    (495, 40),
                ],
            )
        ],
    ),
    "⌃": (  # UP ARROWHEAD, the Control key
        600,
        [("line", [(90, 360), (300, 625), (510, 360)])],
    ),
    "⌥": (  # OPTION KEY
        800,
        [
            (
                "line",
                [(50, CAP_HEIGHT - 40), (290, CAP_HEIGHT - 40), (510, 40), (750, 40)],
            ),
            ("line", [(460, CAP_HEIGHT - 40), (750, CAP_HEIGHT - 40)]),
        ],
    ),
    "▸": (  # BLACK RIGHT-POINTING SMALL TRIANGLE
        480,
        [("fill", [(80, AXIS - 180), (80, AXIS + 180), (392, AXIS)])],
    ),
    # VARIATION SELECTOR-16 asks for the emoji form of the character before it, and is
    # not itself drawn. Without a glyph, egui draws it as an empty box.
    "\ufe0f": (0, []),
}


def unit(dx, dy):
    length = (dx * dx + dy * dy) ** 0.5
    return dx / length, dy / length


def left_normal(a, b):
    dx, dy = unit(b[0] - a[0], b[1] - a[1])
    return -dy, dx


def offset(points, distance, closed):
    """The polyline moved `distance` to its left, with mitered corners."""
    count = len(points)
    shifted = []
    for i, point in enumerate(points):
        before = left_normal(points[i - 1], point) if closed or i > 0 else None
        after = (
            left_normal(point, points[(i + 1) % count])
            if closed or i < count - 1
            else None
        )
        if before is None or after is None:
            nx, ny = before or after
            shifted.append((point[0] + nx * distance, point[1] + ny * distance))
            continue
        # The miter of two offset edges lies on the sum of their normals.
        scale = distance / (1 + before[0] * after[0] + before[1] * after[1])
        shifted.append(
            (
                point[0] + (before[0] + after[0]) * scale,
                point[1] + (before[1] + after[1]) * scale,
            )
        )
    return shifted


def area(contour):
    return sum(
        x0 * y1 - x1 * y0
        for (x0, y0), (x1, y1) in zip(contour, contour[1:] + contour[:1])
    )


def wound(contour, clockwise):
    """TrueType fills clockwise contours and cuts holes with counterclockwise ones."""
    return contour if (area(contour) < 0) == clockwise else contour[::-1]


def contours(kind, points):
    half = WEIGHT / 2
    if kind == "fill":
        return [wound(points, True)]
    if kind == "line":
        outline = offset(points, half, False) + offset(points, -half, False)[::-1]
        return [wound(outline, True)]
    if kind == "ring":
        return [
            wound(offset(points, half, True), True),
            wound(offset(points, -half, True), False),
        ]
    raise ValueError(kind)


def outline(shapes):
    pen = TTGlyphPen(None)
    for kind, points in shapes:
        for contour in contours(kind, points):
            pen.moveTo(tuple(round(c) for c in contour[0]))
            for point in contour[1:]:
                pen.lineTo(tuple(round(c) for c in point))
            pen.closePath()
    return pen.glyph()


def main():
    names = {char: f"uni{ord(char):04X}" for char in GLYPHS}
    builder = FontBuilder(UNITS_PER_EM, isTTF=True)
    builder.setupGlyphOrder([".notdef", *names.values()])
    builder.setupCharacterMap({ord(char): name for char, name in names.items()})
    builder.setupGlyf(
        {".notdef": outline([])}
        | {names[char]: outline(shapes) for char, (_, shapes) in GLYPHS.items()}
    )
    glyf = builder.font["glyf"]
    advances = {".notdef": 0} | {
        names[char]: advance for char, (advance, _) in GLYPHS.items()
    }
    builder.setupHorizontalMetrics(
        {
            name: (advance, getattr(glyf[name], "xMin", 0))
            for name, advance in advances.items()
        }
    )
    builder.setupHorizontalHeader(ascent=HHEA[0], descent=HHEA[1], lineGap=HHEA[2])
    builder.setupNameTable(
        {
            "familyName": "drawbar glyphs",
            "styleName": "Regular",
            "uniqueFontIdentifier": "drawbar glyphs Regular",
            "fullName": "drawbar glyphs Regular",
            "psName": "drawbarGlyphs-Regular",
            "version": "Version 1.000",
            "licenseDescription": "Part of drawbar, under its BSD 3-Clause License.",
        }
    )
    builder.setupOS2(
        sTypoAscender=TYPO[0],
        sTypoDescender=TYPO[1],
        sTypoLineGap=TYPO[2],
        usWinAscent=WIN[0],
        usWinDescent=WIN[1],
        sxHeight=X_HEIGHT,
        sCapHeight=CAP_HEIGHT,
    )
    builder.setupPost()
    # The Unix epoch as both dates, so rerunning the script reproduces the file exactly.
    builder.updateHead(created=timestampSinceEpoch(0), modified=timestampSinceEpoch(0))
    builder.font.recalcTimestamp = False
    builder.save(OUT)


if __name__ == "__main__":
    main()
