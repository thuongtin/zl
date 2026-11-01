#!/usr/bin/env python3
"""Verify bundled fallback font hashes and outline coverage with fontTools."""
from hashlib import sha256
from pathlib import Path
from fontTools.pens.boundsPen import BoundsPen
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent
CASES = {
    "NotoSansMath-Regular.ttf": (
        "3f495fe933c06786e4d5f6d86b8ee70b6753a68ee3b9d87528726de0f6e2c47d",
        [0x1D5D4, 0x1D5D5, 0x1D5D8, 0x1D5DA, 0x1D5DB, 0x1D5DC,
         0x1D5DF, 0x1D5E6, 0x1D5EE, 0x1D5F2, 0x1D5F9, 0x1D5FB,
         0x1D5FC, 0x1D601, 0x1D604, 0x1D7EC, 0x1D7ED],
    ),
    "NotoEmoji-Variable.ttf": (
        "de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551",
        [0x1F383, 0x1F4A5],
    ),
}
for filename, (expected_hash, codepoints) in CASES.items():
    path = ROOT / filename
    assert sha256(path.read_bytes()).hexdigest() == expected_hash, filename
    font = TTFont(path)
    assert "glyf" in font and "CBDT" not in font, filename
    cmap = font.getBestCmap()
    glyphs = font.getGlyphSet()
    for codepoint in codepoints:
        name = cmap[codepoint]
        assert font.getGlyphID(name) > 0, (filename, codepoint)
        pen = BoundsPen(glyphs)
        glyphs[name].draw(pen)
        assert pen.bounds is not None, (filename, codepoint)
    print(f"{filename}: SHA-256 and {len(codepoints)} nonzero glyph IDs/drawable outlines PASS")
