#!/usr/bin/env bash
# Builds the FFmpeg libraries for the Windows client (quadchroma.exe) as a
# minimal LGPL build: only what QuadChroma actually calls, without any
# third-party library. Result: avcodec-63.dll, avformat-63.dll, avutil-61.dll
# plus headers and import libraries - the directory is FFMPEG_DIR for
# the Rust build.
#
# Why a custom build: the prebuilt "lgpl" builds statically link dozens of
# third-party libraries, GPL code among them (FFTW via chromaprint
# in avformat). This build contains only FFmpeg itself (LGPL 2.1 or later)
# and the NVIDIA codec headers (MIT, headers only).
#
# Runs on macOS (Homebrew: mingw-w64, nasm) and Linux (apt: mingw-w64,
# nasm, make, pkg-config). Usage:
#
#   scripts/build-ffmpeg-windows.sh [TARGET_DIR]
#
# Default target: ./ffmpeg-windows. The working directory (sources,
# build) is QC_FFMPEG_WORK or a folder in the temp directory.

set -euo pipefail

# Absolute path of this script: it later copies itself next to the sources.
SKRIPT="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

FFMPEG_VERSION=9.0.2
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

# nv-codec-headers, branch sdk/13.0 (NVENC API 13.0) - headers only, MIT.
NVHDR_COMMIT=ced4f8eba3ba5dd431932cba17928f0dffdaeb2b
NVHDR_URL="https://github.com/FFmpeg/nv-codec-headers/archive/${NVHDR_COMMIT}.tar.gz"
NVHDR_SHA256=625ddd8f2a603699fdba101bebcde0e7a97bcc7b8a82ec7f45a7d06d61336597

CROSS=x86_64-w64-mingw32-
ZIEL="$(mkdir -p "${1:-ffmpeg-windows}" && cd "${1:-ffmpeg-windows}" && pwd)"
WORK="${QC_FFMPEG_WORK:-${TMPDIR:-/tmp}/qc-ffmpeg}"
mkdir -p "$WORK"
cd "$WORK"

for w in "${CROSS}gcc" "${CROSS}dlltool" nasm make pkg-config; do
    command -v "$w" >/dev/null || { echo "Missing: $w (macOS: brew install mingw-w64 nasm pkg-config; Linux: apt install mingw-w64 nasm make pkg-config)" >&2; exit 1; }
done

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

holen() {  # holen <url> <file> <sha256>
    [ -f "$2" ] || curl -fsSL -o "$2" "$1"
    local ist; ist="$(sha256 "$2")"
    [ "$ist" = "$3" ] || { echo "Wrong checksum for $2: $ist (expected $3)" >&2; exit 1; }
}

holen "$FFMPEG_URL" "ffmpeg-${FFMPEG_VERSION}.tar.xz" "$FFMPEG_SHA256"
holen "$NVHDR_URL" "nv-codec-headers-${NVHDR_COMMIT}.tar.gz" "$NVHDR_SHA256"

rm -rf "ffmpeg-${FFMPEG_VERSION}" "nv-codec-headers-${NVHDR_COMMIT}" prefix-nv
tar xf "ffmpeg-${FFMPEG_VERSION}.tar.xz"
tar xzf "nv-codec-headers-${NVHDR_COMMIT}.tar.gz"

# The headers go into a prefix of their own; configure finds them via pkg-config.
make -C "nv-codec-headers-${NVHDR_COMMIT}" PREFIX="$WORK/prefix-nv" install >/dev/null
export PKG_CONFIG_PATH="$WORK/prefix-nv/lib/pkgconfig"
export PKG_CONFIG_LIBDIR="$WORK/prefix-nv/lib/pkgconfig"

cd "ffmpeg-${FFMPEG_VERSION}"
# -static links libgcc and winpthreads (the MinGW-w64 runtime) into the DLLs,
# so that no libgcc_s/libwinpthread DLL has to be shipped.
# Nothing is detected automatically (--disable-autodetect): every library and
# every hardware interface is listed here explicitly. No --enable-gpl,
# no --enable-version3, no --enable-nonfree: the build stays LGPL 2.1+.
#   Decoders: hevc, h264 (software and D3D11VA/DXVA2/NVDEC), hevc_cuvid, h264_cuvid
#   Encoders: hevc_nvenc, h264_nvenc, av1_nvenc, hevc_mf, h264_mf (Windows host role)
#   avformat: linked by the Rust bindings, stays empty without muxers/demuxers
./configure \
    --prefix=/ffmpeg \
    --cross-prefix="$CROSS" --arch=x86_64 --target-os=mingw32 \
    --pkg-config=pkg-config \
    --enable-shared --disable-static \
    --disable-programs --disable-doc --disable-debug \
    --disable-autodetect --disable-everything \
    --disable-avdevice --disable-avfilter --disable-swscale --disable-swresample \
    --disable-network \
    --enable-w32threads \
    --enable-d3d11va --enable-dxva2 --enable-mediafoundation \
    --enable-ffnvcodec --enable-cuda --enable-cuvid --enable-nvdec --enable-nvenc \
    --enable-decoder=hevc,h264,hevc_cuvid,h264_cuvid \
    --enable-encoder=hevc_nvenc,h264_nvenc,av1_nvenc,hevc_mf,h264_mf \
    --enable-parser=hevc,h264 \
    --enable-hwaccel=hevc_d3d11va,hevc_d3d11va2,h264_d3d11va,h264_d3d11va2,hevc_dxva2,h264_dxva2,hevc_nvdec,h264_nvdec \
    --extra-ldflags="-static" \
    --extra-version=quadchroma

make -j"$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"
# Install via DESTDIR so that the configuration string in the DLLs contains a
# neutral path (/ffmpeg) instead of the build folder.
rm -rf "$WORK/inst"
make install DESTDIR="$WORK/inst" >/dev/null
rm -rf "$ZIEL"/bin "$ZIEL"/include "$ZIEL"/lib
cp -R "$WORK/inst/ffmpeg/." "$ZIEL/"

# The Rust build (MSVC) expects avcodec.lib etc. in lib/; configure puts the
# .lib files generated with dlltool next to the DLLs in bin/.
for b in avcodec avformat avutil; do
    [ -f "$ZIEL/bin/$b.lib" ] && cp "$ZIEL/bin/$b.lib" "$ZIEL/lib/$b.lib"
done

# Build information for THIRD_PARTY_NOTICES.txt and for verification.
{
    echo "FFmpeg ${FFMPEG_VERSION} (${FFMPEG_URL}, sha256 ${FFMPEG_SHA256})"
    echo "nv-codec-headers ${NVHDR_COMMIT} (sha256 ${NVHDR_SHA256})"
    echo "Compiler: $(${CROSS}gcc --version | head -1)"
    echo "MinGW-w64: $(echo '#include <_mingw.h>' | ${CROSS}gcc -E -dM - | awk '$2 ~ /^__MINGW64_VERSION_(MAJOR|MINOR|BUGFIX)$/ { v[$2] = $3 } END { print v["__MINGW64_VERSION_MAJOR"] "." v["__MINGW64_VERSION_MINOR"] "." v["__MINGW64_VERSION_BUGFIX"] }')"
    echo "NASM: $(nasm -v | head -1)"
    echo "Configuration: $(sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"/\1/p' config.h)"
    echo "License according to configure: $(sed -n 's/^#define FFMPEG_LICENSE "\(.*\)"/\1/p' config.h)"
    for d in "$ZIEL"/bin/*.dll; do echo "$(basename "$d") $(wc -c < "$d" | tr -d ' ') $(sha256 "$d")"; done
} > "$ZIEL/BUILDINFO.txt"
cp COPYING.LGPLv2.1 LICENSE.md "$ZIEL/"
# The complete sources for distribution alongside the DLLs (LGPL 2.1
# section 6): both archives unmodified, plus this script.
mkdir -p "$ZIEL/quellen"
cp "$WORK/ffmpeg-${FFMPEG_VERSION}.tar.xz" "$WORK/nv-codec-headers-${NVHDR_COMMIT}.tar.gz" "$ZIEL/quellen/"
cp "$SKRIPT" "$ZIEL/quellen/build-ffmpeg-windows.sh"
echo "Done: $ZIEL"
cat "$ZIEL/BUILDINFO.txt"
