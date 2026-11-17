#!/usr/bin/env python3
# nix-deps: python3 fonttools
# Regenerate the Ubuntu and Hack faces in crates/drawbar/assets/fonts from their released
# files, keeping only what egui reads: outlines, metrics, the character map, the `kern`
# table and the names. Every character keeps its outline, advance and kerning.
#
# usage: scripts/fonts.py UBUNTU_R UBUNTU_B HACK
#   UBUNTU_R, UBUNTU_B: Ubuntu-R.ttf and Ubuntu-B.ttf of the Ubuntu font family 0.83
#   HACK: fonts/Hack-Regular.ttf of the epaint_default_fonts 0.32.3 crate

import hashlib
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont

FONTS = Path(__file__).resolve().parent.parent / "crates/drawbar/assets/fonts"

# Each input, by the SHA-256 of the released file, and where its output goes.
INPUTS = [
    (
        "52c1afa489ae7bfd893af6cdd9f1af258005703600449e70d338caabcff507e5",
        FONTS / "Ubuntu-derivative-drawbar-R.ttf",
    ),
    (
        "c90e629e932f768909bdce1481ccd259d8aff1490a4bc1cf94d5413492317336",
        FONTS / "Ubuntu-derivative-drawbar-B.ttf",
    ),
    (
        "15f55cc0c85a2988d2b4b3a8cdb5d77fdfbaf319e1bb5309d725db9818fb7125",
        FONTS / "egui/Hack-Regular.ttf",
    ),
]

# Ubuntu Font Licence 1.0, condition 2(c): a modified version keeps the original name and
# appends "derivative X".
UBUNTU = "Ubuntu"
DERIVED = "Ubuntu derivative drawbar"
DERIVED_POSTSCRIPT = "UbuntuDerivativeDrawbar"

# Hinting, OpenType layout and device metrics: egui neither hints nor shapes.
UNREAD = ["DSIG", "GDEF", "GPOS", "GSUB", "LTSH", "TTFA", "VDMX", "gasp", "hdmx"]


def strip(font):
    options = subset.Options()
    options.drop_tables += UNREAD
    options.glyph_names = False
    options.hinting = False
    options.layout_features = []
    options.legacy_kern = True
    options.name_IDs = ["*"]
    options.name_languages = ["*"]
    options.name_legacy = True
    options.notdef_outline = True
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=font.getBestCmap().keys())
    subsetter.subset(font)


def rename(font):
    names = font["name"]
    style = names.getDebugName(2)
    version = names.getDebugName(5).removeprefix("Version ")
    full = DERIVED if style == "Regular" else f"{DERIVED} {style}"
    postscript = DERIVED_POSTSCRIPT if style == "Regular" else f"{DERIVED_POSTSCRIPT}-{style}"
    for record in names.names:
        name = {
            1: DERIVED,
            3: f"{DERIVED} {style} {version}",
            4: full,
            6: postscript,
        }.get(record.nameID)
        if name is not None:
            record.string = name


def main():
    if len(sys.argv) != 1 + len(INPUTS):
        sys.exit(f"usage: {sys.argv[0]} UBUNTU_R UBUNTU_B HACK")
    for source, (digest, target) in zip(sys.argv[1:], INPUTS):
        found = hashlib.sha256(Path(source).read_bytes()).hexdigest()
        if found != digest:
            sys.exit(f"{source} has SHA-256 {found}, not the release's {digest}")
        font = TTFont(source)
        strip(font)
        if font["name"].getDebugName(1) == UBUNTU:
            rename(font)
        # The released dates stay, so rerunning the script reproduces each file exactly.
        font.recalcTimestamp = False
        font.save(target)


if __name__ == "__main__":
    main()
