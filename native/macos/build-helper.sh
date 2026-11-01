#!/bin/bash
# Shared slice compiler keeps helper architectures aligned with the containing app.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${NATIVE_BUILD_DIR:-$ROOT/native/macos/build}"
HELPER="${1:?Expected helper name}"
APP="${2:-}"
case "$HELPER" in
    EmojiPreview) FRAMEWORKS=(-framework CoreText -framework CoreGraphics -framework ImageIO -framework Foundation); DEST=Resources ;;
    MediaPreview) FRAMEWORKS=(-framework ImageIO -framework UniformTypeIdentifiers); DEST=Resources ;;
    DailyMediaViewer) FRAMEWORKS=(-framework AppKit -framework AVKit -framework AVFoundation -framework ImageIO -framework UniformTypeIdentifiers); DEST=Resources ;;
    DailyNotifier) FRAMEWORKS=(-framework AppKit -framework UserNotifications); DEST=Helpers/DailyNotifier.app/Contents/MacOS ;;
    HistoryCodec) FRAMEWORKS=(-framework JavaScriptCore -framework Security -lz); DEST=Helpers ;;
    *) printf 'Unknown helper: %s\n' "$HELPER" >&2; exit 2 ;;
esac
mkdir -p "$OUT"
IFS=',' read -r -a TARGETS <<< "${SWIFT_TARGETS:-$(uname -m)-apple-macosx11.0}"
BINARIES=()
for target in "${TARGETS[@]}"; do
    case "$target" in arm64-apple-macosx*|x86_64-apple-macosx*) ;; *) printf 'Unsupported Swift target: %s\n' "$target" >&2; exit 2 ;; esac
    EXTRA=()
    if [[ "$HELPER" == HistoryCodec ]]; then
        EXTRA+=("$(bash "$ROOT/native/macos/build-zstd.sh" "${target%%-*}")")
    fi
    binary="$OUT/$HELPER-$target"
    xcrun swiftc -O -target "$target" "$ROOT/native/macos/$HELPER.swift" \
        ${EXTRA[@]+"${EXTRA[@]}"} "${FRAMEWORKS[@]}" -o "$binary"
    BINARIES+=("$binary")
done
if [[ "${#BINARIES[@]}" -gt 1 ]]; then
    lipo -create "${BINARIES[@]}" -output "$OUT/$HELPER"
else
    cp "${BINARIES[0]}" "$OUT/$HELPER"
fi
if [[ "$HELPER" == DailyNotifier ]]; then
    BUNDLE="$OUT/DailyNotifier.app"
    mkdir -p "$BUNDLE/Contents/MacOS"
    cp "$OUT/$HELPER" "$BUNDLE/Contents/MacOS/$HELPER"
    cp "$ROOT/packaging/macos/DailyNotifier-Info.plist" "$BUNDLE/Contents/Info.plist"
    codesign --force --sign - "$BUNDLE"
fi
if [[ -n "$APP" ]]; then
    mkdir -p "$APP/Contents/$DEST"
    cp "$OUT/$HELPER" "$APP/Contents/$DEST/$HELPER"
    if [[ "$HELPER" == DailyNotifier ]]; then
        cp "$BUNDLE/Contents/Info.plist" "$APP/Contents/Helpers/DailyNotifier.app/Contents/Info.plist"
    fi
    if [[ "$HELPER" == HistoryCodec ]]; then
        mkdir -p "$APP/Contents/Resources/ThirdPartyLicenses"
        cp "$OUT/zstd-1.5.7/${TARGETS[0]%%-*}/source/LICENSE" "$APP/Contents/Resources/ThirdPartyLicenses/zstd-LICENSE"
        if [[ -n "${HISTORY_CODEC_ASSETS:-}" ]]; then
            python3 "$ROOT/native/macos/prepare-history-assets.py" --verify-only --output "$HISTORY_CODEC_ASSETS"
            mkdir -p "$APP/Contents/Resources/HistoryCodecAssets"
            for name in web-module-hRO2.js web-module-Ho1Y.js web-module-1y5R.js web-module-Vox7.js libzproto.wasm; do
                cp "$HISTORY_CODEC_ASSETS/$name" "$APP/Contents/Resources/HistoryCodecAssets/"
            done
        fi
    fi
fi
printf '%s\n' "$OUT/$HELPER"
