#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
exec bash "$ROOT/native/macos/build-helper.sh" MediaPreview "${1:-}"
