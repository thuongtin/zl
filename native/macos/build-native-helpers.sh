#!/bin/bash
# Public builds contain all native helpers, but never download proprietary assets.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
for helper in EmojiPreview MediaPreview DailyMediaViewer DailyNotifier HistoryCodec; do
    bash "$ROOT/native/macos/build-helper.sh" "$helper" "${1:-}"
done
printf 'Native helpers built for %s\n' "${SWIFT_TARGETS:-$(uname -m)}"
