#!/usr/bin/env python3
"""Synthetic ImageIO helper checks; no remote content or credentials."""
import io
import os
import json
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
HELPER = Path(os.environ.get("MEDIA_PREVIEW_HELPER", ROOT / "native/macos/build/MediaPreview"))
results = []

def check(name, data, expected_size=None):
    result = subprocess.run([str(HELPER)], input=data, capture_output=True, timeout=12)
    if expected_size is None:
        assert result.returncode != 0 and not result.stdout, name
    else:
        assert result.returncode == 0, (name, result.stderr)
        output = Image.open(io.BytesIO(result.stdout))
        assert output.format == "PNG" and output.size == expected_size, (name, output.size)
    results.append({"case": name, "exit_status": result.returncode, "result": "PASS"})

for fmt in ("PNG", "JPEG", "WEBP"):
    output = io.BytesIO()
    Image.new("RGB", (2048, 1024), (20, 80, 140)).save(output, format=fmt)
    check(fmt.lower() + "-bounded", output.getvalue(), (1024, 512))
output = io.BytesIO()
exif = Image.Exif()
exif[274] = 6
Image.new("RGB", (80, 40), (140, 20, 40)).save(output, format="JPEG", exif=exif)
check("jpeg-exif-rotation", output.getvalue(), (40, 80))
with tempfile.TemporaryDirectory(prefix="media-preview-test-") as directory:
    source = Path(directory) / "source.png"
    heic = Path(directory) / "source.heic"
    Image.new("RGB", (128, 64), (30, 90, 150)).save(source)
    subprocess.run(["sips", "-s", "format", "heic", str(source), "--out", str(heic)],
                   capture_output=True, check=True, timeout=12)
    check("heic-bounded", heic.read_bytes(), (128, 64))
check("nonimage", b"<html>not image</html>")
check("truncated-png", b"\x89PNG\r\n\x1a\n")
check("oversize-input", b"\0" * (16 * 1024 * 1024 + 1))
# Valid PNG signature/IHDR with dimensions exceeding the pixel budget.
def chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
bomb = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 16384, 16384, 8, 2, 0, 0, 0))
bomb += chunk(b"IDAT", zlib.compress(b"\0")) + chunk(b"IEND", b"")
check("pixel-bound", bomb)
print(json.dumps({"helper": "MediaPreview", "tests": results, "total_passed": len(results)}, indent=2))
