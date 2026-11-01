#!/usr/bin/env python3
"""Headless native color emoji contract checks using synthetic UTF-8 fixtures."""
import io
import os
import json
import subprocess
from pathlib import Path
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
HELPER = Path(os.environ.get("EMOJI_PREVIEW_HELPER", ROOT / "native/macos/build/EmojiPreview"))
results = []
rendered = {}


def check(name, data, success=False):
    result = subprocess.run([str(HELPER)], input=data, capture_output=True, timeout=2)
    if success:
        assert result.returncode == 0 and result.stderr == b"", (name, result.stderr)
        assert len(result.stdout) <= 128 * 1024, name
        output = Image.open(io.BytesIO(result.stdout))
        assert output.format == "PNG" and output.size == (64, 64), name
        assert output.mode == "RGBA", (name, output.mode)
        pixels = [output.getpixel((x, y)) for y in range(64) for x in range(64)]
        if name != "laptop-base":
            assert sum(a > 0 and max(r, g, b) - min(r, g, b) > 20
                       for r, g, b, a in pixels) > 20, name
        assert sum(a == 0 for _, _, _, a in pixels) > 100, name
        assert all(output.getpixel((x, y))[3] == 0
                   for x in range(64) for y in (0, 63)), name
        assert all(output.getpixel((x, y))[3] == 0
                   for y in range(64) for x in (0, 63)), name
        rendered[name] = output.tobytes()
    else:
        assert result.returncode == 1 and not result.stdout, name
        assert result.stderr == b"Emoji preview failed\n", (name, result.stderr)
    results.append({"case": name, "exit_status": result.returncode, "result": "PASS"})


for name, emoji in (("skin-tone", "👍🏽"), ("zwj-technologist", "👩‍💻"),
                    ("flag", "🇻🇳"), ("keycap", "1️⃣"), ("heart-hands", "🫶"),
                    ("thumb-base", "👍"), ("woman-base", "👩"), ("laptop-base", "💻")):
    check(name, emoji.encode("utf-8"), True)
assert rendered["skin-tone"] != rendered["thumb-base"], "skin tone was discarded"
assert rendered["zwj-technologist"] not in (rendered["woman-base"], rendered["laptop-base"]), "ZWJ was truncated"
for name, data in (("empty", b""), ("invalid-utf8", b"\xff"),
                   ("truncated-utf8", b"\xf0\x9f"), ("oversize", b"x" * 257),
                   ("multiline", "👍\n".encode()), ("crlf", "👍\r\n".encode()),
                   ("nul-control", b"\0"), ("tab-control", b"\t"),
                   ("line-separator", "\u2028".encode()),
                   ("multiple-graphemes", "👍👍".encode()),
                   ("plain-text", b"A"), ("whitespace", b" "),
                   ("command-text", b"$(touch /tmp/emoji-preview-input)") ):
    check(name, data)
print(json.dumps({"helper": "EmojiPreview", "tests": results,
                  "total_passed": len(results), "cluster_distinction_checks": 2}, indent=2))
