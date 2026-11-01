#!/usr/bin/env python3
"""Validate a staged, relocatable public bundle without launching desktop UI."""
import argparse
import json
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile

HELPERS = {
    "EmojiPreview": "Resources/EmojiPreview",
    "MediaPreview": "Resources/MediaPreview",
    "DailyMediaViewer": "Resources/DailyMediaViewer",
    "DailyNotifier": "Helpers/DailyNotifier.app/Contents/MacOS/DailyNotifier",
    "HistoryCodec": "Helpers/HistoryCodec",
}


def output(*args):
    return subprocess.check_output(args, text=True).strip()


def validate(app, arches):
    contents = app / "Contents"
    required = [contents / "MacOS/ctdozlite"] + [contents / path for path in HELPERS.values()]
    for binary in required:
        assert binary.is_file() and binary.stat().st_mode & 0o111, f"Missing executable: {binary}"
    assert not (contents / "Resources/HistoryCodecAssets").exists(), "Public bundle contains external codec assets"
    assert (contents / "Resources/ThirdPartyLicenses/zstd-LICENSE").is_file(), "Missing zstd license"
    for binary in contents.rglob("*"):
        if not binary.is_file() or "Mach-O" not in output("file", "-b", str(binary)):
            continue
        actual = set(output("lipo", "-archs", str(binary)).split())
        assert actual == arches, f"Architecture mismatch: {binary}: {actual} != {arches}"
        linked = output("otool", "-L", str(binary)).splitlines()[1:]
        for line in linked:
            if not line[:1].isspace():
                continue
            dependency = line.strip().split(" (", 1)[0]
            if not dependency or dependency.endswith(":"):
                continue
            assert dependency.startswith(("/System/Library/", "/usr/lib/", "@rpath/libswift")), f"Non-system dependency: {binary}: {dependency}"
        loads = output("otool", "-l", str(binary))
        assert "/opt/homebrew" not in loads and "/usr/local/" not in loads, f"Homebrew reference: {binary}"
    # Intel-only slices are build-checked on arm64, not represented as Intel runtime QA.
    if platform.machine() not in arches:
        print("PASS: architecture and dependency checks (runtime skipped for non-host slices)")
        return
    with tempfile.TemporaryDirectory(prefix="native-relocated-") as temporary:
        relocated = Path(temporary) / "Relocated.app"
        shutil.copytree(app, relocated)
        resource = relocated / "Contents/Resources"
        emoji = subprocess.run([str(resource / "EmojiPreview")], input="👍".encode(), capture_output=True, timeout=10, cwd=temporary)
        assert emoji.returncode == 0 and emoji.stdout.startswith(b"\x89PNG\r\n\x1a\n"), "Relocated emoji contract"
        media = subprocess.run([str(resource / "MediaPreview")], input=emoji.stdout, capture_output=True, timeout=10, cwd=temporary)
        assert media.returncode == 0 and media.stdout.startswith(b"\x89PNG\r\n\x1a\n"), "Relocated media contract"
        history = subprocess.run([str(relocated / "Contents/Helpers/HistoryCodec")], input='{"op":"init"}\n', capture_output=True, text=True, timeout=10, cwd=temporary)
        response = json.loads(history.stdout)
        assert history.returncode == 0 and response["ok"] is False and response["code"] == "history_codec_assets_missing", "Relocated history missing-assets contract"
        assert "--local-use" in response["error"] and "assetsDir" in response["error"]
    print("PASS: all executable architectures, system-only dependencies, relocated headless contracts")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("--arches", required=True, help="Comma-separated exact architecture set")
    args = parser.parse_args()
    validate(args.app.resolve(), set(args.arches.split(",")))
