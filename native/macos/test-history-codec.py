#!/usr/bin/env python3
"""Disposable helper checks. No account data or network access is used."""
import base64
import json
import os
import pathlib
import subprocess
import tempfile

root = pathlib.Path(__file__).resolve().parent
temporary = tempfile.TemporaryDirectory(prefix="history-codec-test-")
working = pathlib.Path(temporary.name)
helper = pathlib.Path(os.environ.get("HISTORY_CODEC_HELPER", root / "build/HistoryCodec"))
process = subprocess.Popen([str(helper)], stdin=subprocess.PIPE,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=working)


def request(value):
    process.stdin.write(json.dumps(value) + "\n")
    process.stdin.flush()
    return json.loads(process.stdout.readline())


try:
    initial = request({"op": "init"})
    assert initial["ok"]
    assert len(base64.b64decode(initial["result"]["ik"])) == 33
    assert len(base64.b64decode(initial["result"]["ek"])) == 33
    additional = request({"op": "newSession", "id": "fixture"})
    assert additional["ok"] and additional["result"]["ik"] == initial["result"]["ik"]
    assert additional["result"]["ek"] != initial["result"]["ek"]
    checked = request({"op": "selfTest"})
    assert checked["ok"] and len(checked["result"]["checks"]) == 8
    for blob in [b"", b"\x01", b"\x02\0\0\0\x01", b"\0\0\0\0", b"\x01\0\0\0\x01"]:
        failed = request({"op": "decode", "id": "fixture", "kind": "message", "batchType": 2,
                          "blob": base64.b64encode(blob).decode()})
        assert failed["ok"] is False and "result" not in failed
    diagnostics = working / "history_codec_diagnostics.json"
    assert not diagnostics.exists()
    marker = working / ".history-codec-diagnostics-enabled"
    marker.touch(mode=0o600)
    missing = request({"op": "decode", "id": "fixture", "kind": "conversation", "batchType": 1, "blob": "AQAAAAE="})
    assert missing["ok"] is False
    entries = json.loads(diagnostics.read_text())
    assert entries[-1]["kind"] == "conversation" and entries[-1]["reason"] == "missingMetadata"
    assert entries[-1]["hasMetadata"] is False and entries[-1]["recordBytes"] == 0
    assert entries[-1]["chunkBytes"] == 1 and entries[-1]["publicKeyBytes"] == 33
    assert diagnostics.stat().st_mode & 0o777 == 0o600
    allowed = {"time_ms", "ok", "kind", "reason", "batchType", "chunkIndex", "chunkBytes", "recordBytes", "publicKeyBytes", "hasMetadata"}
    for _ in range(45):
        assert request({"op": "decode", "id": "fixture", "kind": "message", "batchType": 2, "blob": "AQAAAAE="})["ok"] is False
    entries = json.loads(diagnostics.read_text())
    assert len(entries) == 40 and all(set(entry).issubset(allowed) for entry in entries)
    assert entries[-1]["reason"] == "decryptFailure"
    marker.unlink()
    snapshot = diagnostics.read_bytes()
    assert request({"op": "decode", "id": "fixture", "kind": "message", "batchType": 1, "blob": "AQAAAAE="})["ok"] is False
    assert diagnostics.read_bytes() == snapshot
    assert request({"op": "releaseSession", "id": "fixture"})["ok"]
    assert request({"op": "decode", "id": "fixture", "kind": "message", "batchType": 1, "blob": "AQAAAAE="})["ok"] is False
    assert request({"op": "newSession", "id": "after-errors"})["ok"]
finally:
    process.stdin.close()
    process.wait(timeout=5)
    errors = process.stderr.read()
    assert process.returncode == 0 and errors == ""
    temporary.cleanup()
print("PASS: init, stable identity, fresh ephemeral keys, 8 codec self-checks, malformed/encrypted rejection, session release, recovery, opt-in bounded private structural diagnostics")
