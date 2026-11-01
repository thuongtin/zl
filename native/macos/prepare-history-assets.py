#!/usr/bin/env python3
"""Prepare pinned Zalo Web codec assets for explicit local use only."""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import urllib.request

WORKER = "https://chat.zalo.me/sync-v2-worker.1b2d86e9567f6be024ce.js"
WASM = "https://chat.zalo.me/libs/libzproto_wasm_bg.ecc0555ba9d0f8867a4699ce943ec745.wasm"
WORKER_HASH = "b650e02c390f27b6e11e879c0475bfcdbeaa3211368772a5028fd20772fb8dec"
WASM_HASH = "3a868037a32739a4fcb3c22425150745613006f9dbc561f30fd705effaed55fa"
MODULE_HASHES = {
    "Vox7": "a4465258026f8e66916bc0c2500aca800c1853faaf665077a217cb3b7e03ea65",
    "hRO2": "519ba21bd73c52b0618be7d3904b5a97bfb675a97ddbf98b7294678defa9ddce",
    "1y5R": "11af8ae3e6e737423e0775fa2c6150e121b2a7b9ba0c04ce9404336c6aae6ff1",
    "Ho1Y": "0dc9efd3ef60fb525bf49521f6ee45fe8a91be756f8dde4628f6ac1dfb92e552",
}
MAX_DOWNLOAD = 16 * 1024 * 1024
MAX_MODULE = 256 * 1024


def verify(data, expected, label):
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise ValueError(f"Hash mismatch for {label}: expected {expected}, got {actual}")
    return data


def download(url):
    request = urllib.request.Request(url, headers={"User-Agent": "HistoryCodecAssetBuilder/1", "Accept-Encoding": "identity"})
    with urllib.request.urlopen(request, timeout=60) as response:
        if not response.geturl().startswith("https://chat.zalo.me/"):
            raise ValueError("Unexpected asset redirect")
        data = response.read(MAX_DOWNLOAD + 1)
    if len(data) > MAX_DOWNLOAD:
        raise ValueError("Asset exceeds download limit")
    if data.startswith(b"\x1f\x8b"):
        # Bound decompression even if a server returns an unexpected gzip body.
        import io
        with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
            data = stream.read(MAX_DOWNLOAD + 1)
        if len(data) > MAX_DOWNLOAD:
            raise ValueError("Asset exceeds decoded limit")
    return data


def extract_module(source, module_id):
    prefix = ('"1y5R"' if module_id == "1y5R" else module_id) + ":function"
    if source.count(prefix) != 1:
        raise ValueError(f"Ambiguous/missing module {module_id}")
    start = source.index(prefix)
    opening = source.index("{", start)
    depth = 0
    # This bounded scanner is intentionally pinned to these exact sources.
    # Exact output hashes reject strings/regexes that upset brace balancing.
    for position in range(opening, min(len(source), start + MAX_MODULE)):
        char = source[position]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                if source[position + 1:position + 2] not in (",", "}", "]"):
                    raise ValueError(f"Invalid module boundary {module_id}")
                return verify(source[start:position + 1].encode(), MODULE_HASHES[module_id], module_id)
    raise ValueError(f"Unterminated/oversized module {module_id}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parent / "build/HistoryCodecAssets")
    parser.add_argument("--local-use", action="store_true", help="Explicitly download external assets for local use; redistribution rights are not established")
    parser.add_argument("--verify-only", action="store_true", help="Verify existing assets without network access")
    args = parser.parse_args()
    if args.verify_only:
        for key, expected in MODULE_HASHES.items():
            verify((args.output / f"web-module-{key}.js").read_bytes(), expected, key)
        verify((args.output / "libzproto.wasm").read_bytes(), WASM_HASH, "WASM")
        print("PASS: pinned local codec assets")
        return
    if not args.local_use:
        parser.error("Pass --local-use for an explicit local download; public releases must omit these assets")
    worker = verify(download(WORKER), WORKER_HASH, "worker")
    wasm = verify(download(WASM), WASM_HASH, "WASM")
    source = worker.decode("utf-8")
    assets = {f"web-module-{key}.js": extract_module(source, key) for key in MODULE_HASHES}
    assets["libzproto.wasm"] = wasm
    manifest = {"sources": {"worker": WORKER, "workerDecodedSha256": WORKER_HASH, "wasm": WASM}, "files": {name: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in assets.items()}}
    # Validate every downloaded/extracted byte before changing output files.
    args.output.mkdir(parents=True, exist_ok=True)
    for name, data in assets.items():
        target = args.output / name
        target.write_bytes(data)
        verify(target.read_bytes(), manifest["files"][name]["sha256"], name)
    target = args.output / "MANIFEST.json"
    target.write_text(json.dumps(manifest, indent=2) + "\n")
    if json.loads(target.read_text()) != manifest:
        raise ValueError("Manifest readback mismatch")
    print(f"Verified {len(assets)} pinned assets: {args.output.resolve()}")


if __name__ == "__main__":
    main()
