#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
# Assets are local opt-in input, never fetched by a build or public release.
export HISTORY_CODEC_ASSETS="${1:-${HISTORY_CODEC_ASSETS:-}}"
exec bash "$ROOT/native/macos/build-helper.sh" HistoryCodec "${2:-}"
