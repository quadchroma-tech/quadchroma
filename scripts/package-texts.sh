#!/usr/bin/env bash
# Puts the companion texts of a distributed package (Windows ZIP, macOS DMG)
# into TARGET - all as .txt, so that anyone can open them without knowing Markdown:
#
#   LICENSE.txt              license (already a .txt in the repository)
#   THIRD_PARTY_NOTICES.txt  third-party licenses (likewise)
#   README.txt               made from README.md, without the image lines
#   MANUAL.txt               manual (likewise)
#
# Usage: scripts/package-texts.sh TARGET [required]
#   required = 1 (default): if a source is missing, stop with an error -
#              a distributed package needs all license and notice texts.
#   required = 0: only report missing sources (non-release build).

set -euo pipefail

ZIEL="${1:?usage: scripts/package-texts.sh TARGET [required]}"
PFLICHT="${2:-1}"
cd "$(dirname "$0")/.."
mkdir -p "$ZIEL"

fehlt() {
    if [ "$PFLICHT" = "1" ]; then
        echo "ERROR: companion text $1 is missing - a distributed package needs all license and notice texts" >&2
        exit 1
    fi
    echo "WARNING: companion text $1 is missing - allowed only in a non-release build" >&2
}

for f in LICENSE.txt THIRD_PARTY_NOTICES.txt MANUAL.txt; do
    if [ -f "$f" ]; then cp "$f" "$ZIEL/"; else fehlt "$f"; fi
done
# Image lines (![...](file.png)) have no image in the package - drop them.
if [ -f README.md ]; then
    sed '/^!\[/d' README.md > "$ZIEL/README.txt"
else
    fehlt README.md
fi
