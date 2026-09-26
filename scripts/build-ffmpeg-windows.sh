#!/usr/bin/env bash
# Baut die FFmpeg-Bibliotheken fuer den Windows-Client (quadchroma.exe) als
# schlanken LGPL-Build: nur das, was QuadChroma wirklich aufruft, ohne jede
# Fremdbibliothek. Ergebnis: avcodec-63.dll, avformat-63.dll, avutil-61.dll
# samt Headern und Importbibliotheken - das Verzeichnis ist FFMPEG_DIR fuer
# den Rust-Bau.
#
# Warum ein eigener Bau: die fertigen "lgpl"-Builds binden Dutzende
# Fremdbibliotheken statisch ein, darunter GPL-Code (FFTW ueber chromaprint
# in avformat). Dieser Bau enthaelt nur FFmpeg selbst (LGPL 2.1 oder spaeter)
# und die NVIDIA-Codec-Header (MIT, nur Header).
#
# Laeuft auf macOS (Homebrew: mingw-w64, nasm) und Linux (apt: mingw-w64,
# nasm, make, pkg-config). Aufruf:
#
#   scripts/build-ffmpeg-windows.sh [ZIELVERZEICHNIS]
#
# Voreinstellung fuer das Ziel: ./ffmpeg-windows. Das Arbeitsverzeichnis
# (Quellen, Bau) ist QC_FFMPEG_WORK oder ein Ordner im Temp-Verzeichnis.

set -euo pipefail

# Absoluter Pfad dieses Skripts: es legt sich spaeter zu den Quellen.
SKRIPT="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

FFMPEG_VERSION=9.0.2
FFMPEG_URL="https://ffmpeg.org/releases/ffmpeg-${FFMPEG_VERSION}.tar.xz"
FFMPEG_SHA256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e

# nv-codec-headers, Zweig sdk/13.0 (NVENC-API 13.0) - nur Header, MIT.
NVHDR_COMMIT=ced4f8eba3ba5dd431932cba17928f0dffdaeb2b
NVHDR_URL="https://github.com/FFmpeg/nv-codec-headers/archive/${NVHDR_COMMIT}.tar.gz"
NVHDR_SHA256=625ddd8f2a603699fdba101bebcde0e7a97bcc7b8a82ec7f45a7d06d61336597

CROSS=x86_64-w64-mingw32-
ZIEL="$(mkdir -p "${1:-ffmpeg-windows}" && cd "${1:-ffmpeg-windows}" && pwd)"
WORK="${QC_FFMPEG_WORK:-${TMPDIR:-/tmp}/qc-ffmpeg}"
mkdir -p "$WORK"
cd "$WORK"

for w in "${CROSS}gcc" "${CROSS}dlltool" nasm make pkg-config; do
    command -v "$w" >/dev/null || { echo "Fehlt: $w (macOS: brew install mingw-w64 nasm pkg-config; Linux: apt install mingw-w64 nasm make pkg-config)" >&2; exit 1; }
done

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

holen() {  # holen <url> <datei> <sha256>
    [ -f "$2" ] || curl -fsSL -o "$2" "$1"
    local ist; ist="$(sha256 "$2")"
    [ "$ist" = "$3" ] || { echo "Pruefsumme falsch fuer $2: $ist (erwartet $3)" >&2; exit 1; }
}

holen "$FFMPEG_URL" "ffmpeg-${FFMPEG_VERSION}.tar.xz" "$FFMPEG_SHA256"
holen "$NVHDR_URL" "nv-codec-headers-${NVHDR_COMMIT}.tar.gz" "$NVHDR_SHA256"

rm -rf "ffmpeg-${FFMPEG_VERSION}" "nv-codec-headers-${NVHDR_COMMIT}" prefix-nv
tar xf "ffmpeg-${FFMPEG_VERSION}.tar.xz"
tar xzf "nv-codec-headers-${NVHDR_COMMIT}.tar.gz"

# Die Header in ein eigenes Praefix; configure findet sie ueber pkg-config.
make -C "nv-codec-headers-${NVHDR_COMMIT}" PREFIX="$WORK/prefix-nv" install >/dev/null
export PKG_CONFIG_PATH="$WORK/prefix-nv/lib/pkgconfig"
export PKG_CONFIG_LIBDIR="$WORK/prefix-nv/lib/pkgconfig"

cd "ffmpeg-${FFMPEG_VERSION}"
# -static bindet libgcc und winpthreads (MinGW-w64-Laufzeit) in die DLLs
# ein, damit keine libgcc_s-/libwinpthread-DLL mitgeliefert werden muss.
# Nichts wird automatisch erkannt (--disable-autodetect): jede Bibliothek,
# jede Hardwareschnittstelle steht hier ausdruecklich. Kein --enable-gpl,
# kein --enable-version3, kein --enable-nonfree: der Bau bleibt LGPL 2.1+.
#   Decoder:  hevc, h264 (Software und D3D11VA/DXVA2/NVDEC), hevc_cuvid, h264_cuvid
#   Encoder:  hevc_nvenc, h264_nvenc, av1_nvenc, hevc_mf, h264_mf (Windows-Host-Rolle)
#   avformat: wird von der Rust-Bindung gelinkt, bleibt ohne Muxer/Demuxer leer
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
# Installiert wird ueber DESTDIR, damit im Konfigurationstext der DLLs ein
# neutraler Pfad (/ffmpeg) steht statt des Bauordners.
rm -rf "$WORK/inst"
make install DESTDIR="$WORK/inst" >/dev/null
rm -rf "$ZIEL"/bin "$ZIEL"/include "$ZIEL"/lib
cp -R "$WORK/inst/ffmpeg/." "$ZIEL/"

# Der Rust-Bau (MSVC) erwartet avcodec.lib usw. in lib/; configure legt die
# mit dlltool erzeugten .lib neben die DLLs in bin/.
for b in avcodec avformat avutil; do
    [ -f "$ZIEL/bin/$b.lib" ] && cp "$ZIEL/bin/$b.lib" "$ZIEL/lib/$b.lib"
done

# Bauangaben fuer THIRD_PARTY_NOTICES.md und zur Nachpruefung.
{
    echo "FFmpeg ${FFMPEG_VERSION} (${FFMPEG_URL}, sha256 ${FFMPEG_SHA256})"
    echo "nv-codec-headers ${NVHDR_COMMIT} (sha256 ${NVHDR_SHA256})"
    echo "Compiler: $(${CROSS}gcc --version | head -1)"
    echo "MinGW-w64: $(echo '#include <_mingw.h>' | ${CROSS}gcc -E -dM - | awk '$2 ~ /^__MINGW64_VERSION_(MAJOR|MINOR|BUGFIX)$/ { v[$2] = $3 } END { print v["__MINGW64_VERSION_MAJOR"] "." v["__MINGW64_VERSION_MINOR"] "." v["__MINGW64_VERSION_BUGFIX"] }')"
    echo "NASM: $(nasm -v | head -1)"
    echo "Konfiguration: $(sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"/\1/p' config.h)"
    echo "Lizenz laut configure: $(sed -n 's/^#define FFMPEG_LICENSE "\(.*\)"/\1/p' config.h)"
    for d in "$ZIEL"/bin/*.dll; do echo "$(basename "$d") $(wc -c < "$d" | tr -d ' ') $(sha256 "$d")"; done
} > "$ZIEL/BUILDINFO.txt"
cp COPYING.LGPLv2.1 LICENSE.md "$ZIEL/"
# Die vollstaendigen Quellen fuer die Weitergabe neben den DLLs (LGPL 2.1
# Abschnitt 6): beide Archive unveraendert und dieses Skript.
mkdir -p "$ZIEL/quellen"
cp "$WORK/ffmpeg-${FFMPEG_VERSION}.tar.xz" "$WORK/nv-codec-headers-${NVHDR_COMMIT}.tar.gz" "$ZIEL/quellen/"
cp "$SKRIPT" "$ZIEL/quellen/build-ffmpeg-windows.sh"
echo "Fertig: $ZIEL"
cat "$ZIEL/BUILDINFO.txt"
