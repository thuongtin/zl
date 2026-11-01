#!/bin/bash
# Build the pinned BSD/GPL dual-licensed library from verified source for each slice.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
CACHE="${NATIVE_BUILD_DIR:-$ROOT/native/macos/build}/zstd-1.5.7"
ARCH="${1:?Expected arm64 or x86_64}"
case "$ARCH" in arm64|x86_64) ;; *) exit 2 ;; esac
SHA=37d7284556b20954e56e1ca85b80226768902e2edabd3b649e9e72c0c9012ee3
mkdir -p "$CACHE"
ARCHIVE="$CACHE/source.tar.gz"
if [[ ! -f "$ARCHIVE" ]]; then
    curl --fail --location --retry 3 https://github.com/facebook/zstd/archive/refs/tags/v1.5.7.tar.gz -o "$ARCHIVE.part"
    mv "$ARCHIVE.part" "$ARCHIVE"
fi
printf '%s  %s\n' "$SHA" "$ARCHIVE" | shasum -a 256 -c - >&2
SOURCE="$CACHE/$ARCH/source"
if [[ ! -d "$SOURCE" ]]; then
    mkdir -p "$SOURCE"
    tar -xzf "$ARCHIVE" -C "$SOURCE" --strip-components=1
fi
# Explicit target flags prevent cross-compilation from inheriting the host architecture.
make -C "$SOURCE/lib" -j "$(sysctl -n hw.ncpu)" libzstd.a \
    CC="$(xcrun -f clang) -arch $ARCH -mmacosx-version-min=11.0 -isysroot $(xcrun --sdk macosx --show-sdk-path)" \
    AR="$(xcrun -f ar)" CFLAGS='-O2 -fPIC' ZSTD_LEGACY_SUPPORT=0 ZSTD_MULTITHREAD=0 >&2
lipo -verify_arch "$ARCH" "$SOURCE/lib/libzstd.a"
printf '%s\n' "$SOURCE/lib/libzstd.a"
