#!/usr/bin/env python3
# nix-deps: python3 fonttools picosvg skia-pathops
# Regenerate drawbar-icons.ttf, the vendored Lucide art as outlines: each glyph listed in
# crates/drawbar/src/icon.rs, at U+E000 plus its position in the list.

import re
from pathlib import Path

import pathops
from fontTools.fontBuilder import FontBuilder
from fontTools.misc.timeTools import timestampSinceEpoch
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.transformPen import TransformPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.svgLib.path import parse_path
from picosvg.svg import SVG

DRAWBAR = Path(__file__).resolve().parent.parent / "crates/drawbar"
OUT = DRAWBAR / "assets/fonts/drawbar-icons.ttf"
FIRST = 0xE000
# Lucide draws in a 24-unit view box, which becomes the em: ascent 2400, descent 0.
UNIT = 100
UNITS_PER_EM = 24 * UNIT
# The largest distance, in font units, between a cubic curve and its quadratic stand-in.
CURVE_ERROR = 1.0


def outline(svg):
    union = pathops.Path()
    for shape in SVG.parse(svg).topicosvg().shapes():
        path = pathops.Path()
        parse_path(shape.d, path.getPen())
        union = pathops.op(union, path, pathops.PathOp.UNION, fix_winding=True)
    pen = TTGlyphPen(None)
    # SVG's y grows downward and a font's upward.
    flip = (UNIT, 0, 0, -UNIT, 0, UNITS_PER_EM)
    union.draw(Cu2QuPen(TransformPen(pen, flip), CURVE_ERROR, reverse_direction=False))
    return pen.glyph()


def main():
    files = re.findall(r'=> "([a-z0-9-]+\.svg)"', (DRAWBAR / "src/icon.rs").read_text())
    names = {file: f"uni{FIRST + at:04X}" for at, file in enumerate(files)}
    builder = FontBuilder(UNITS_PER_EM, isTTF=True)
    builder.setupGlyphOrder([".notdef", *names.values()])
    builder.setupCharacterMap({FIRST + at: names[file] for at, file in enumerate(files)})
    builder.setupGlyf(
        {".notdef": TTGlyphPen(None).glyph()}
        | {name: outline(str(DRAWBAR / "assets/icons" / file)) for file, name in names.items()}
    )
    glyf = builder.font["glyf"]
    builder.setupHorizontalMetrics(
        {
            name: (UNITS_PER_EM, getattr(glyf[name], "xMin", 0))
            for name in [".notdef", *names.values()]
        }
    )
    builder.setupHorizontalHeader(ascent=UNITS_PER_EM, descent=0)
    builder.setupNameTable(
        {
            "copyright": (DRAWBAR / "assets/icons/LICENSE").read_text().splitlines()[2],
            "familyName": "drawbar icons",
            "styleName": "Regular",
            "uniqueFontIdentifier": "drawbar icons Regular",
            "fullName": "drawbar icons Regular",
            "psName": "drawbarIcons-Regular",
            "version": "Version 0.469",
            "licenseDescription": (DRAWBAR / "assets/icons/LICENSE").read_text(),
        }
    )
    builder.setupOS2(
        sTypoAscender=UNITS_PER_EM,
        sTypoDescender=0,
        sTypoLineGap=0,
        usWinAscent=UNITS_PER_EM,
        usWinDescent=0,
    )
    builder.setupPost()
    # The Unix epoch as both dates, so rerunning the script reproduces the file exactly.
    builder.updateHead(created=timestampSinceEpoch(0), modified=timestampSinceEpoch(0))
    builder.font.recalcTimestamp = False
    builder.save(OUT)


if __name__ == "__main__":
    main()
