#!/bin/sh
# hdrprobe bauen und starten (HDR-Plan G0, Beschreibung in scripts/hdrprobe.m).
# Der Bericht geht auf den Bildschirm und zusaetzlich nach
# build/hdrprobe-<Datum>-<Zeit>.txt.
#
#   scripts/hdrprobe.sh                        Bildschirme und EDR-Werte, keine Aufnahme
#   scripts/hdrprobe.sh messen [N]             alle Messungen (Bildschirm N der Liste)
#   scripts/hdrprobe.sh spitze [N] [sekunden]  hoechster Wert, dabei ein HDR-Video abspielen
#   scripts/hdrprobe.sh beobachten [sekunden]  Ereignisse beim HDR an/aus
#
# Die Aufnahme braucht die Freigabe "Bildschirmaufnahme" fuer das Terminal.
set -eu
cd "$(dirname "$0")/.."
mkdir -p build
clang -fobjc-arc -O2 -Wall -mmacosx-version-min=15.0 \
    -framework Foundation -framework AppKit -framework ScreenCaptureKit -framework VideoToolbox \
    -framework CoreMedia -framework CoreVideo -framework CoreGraphics \
    scripts/hdrprobe.m -o build/hdrprobe
bericht="build/hdrprobe-$(date +%Y%m%d-%H%M%S).txt"
build/hdrprobe "$@" 2>&1 | tee "$bericht"
echo "Bericht: $bericht"
