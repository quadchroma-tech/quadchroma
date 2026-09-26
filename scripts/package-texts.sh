#!/usr/bin/env bash
# Legt die Begleittexte eines verteilten Pakets (Windows-ZIP, macOS-DMG) in
# ZIEL ab - alle als .txt, damit sie jeder ohne Markdown-Kenntnis oeffnet:
#
#   LICENSE.txt              Lizenz (liegt im Repository schon als .txt)
#   THIRD_PARTY_NOTICES.txt  Fremdlizenzen (ebenso)
#   README.txt               aus .github/README.md, ohne die Bildzeilen
#   BENUTZUNG.txt            deutsche Anleitung
#
# Aufruf: scripts/package-texts.sh ZIEL [pflicht]
#   pflicht = 1 (Voreinstellung): fehlt eine Quelle, Abbruch mit Fehler -
#             ein verteiltes Paket braucht alle Lizenz- und Hinweistexte.
#   pflicht = 0: fehlende Quellen nur melden (Alltagsbau).

set -euo pipefail

ZIEL="${1:?Aufruf: scripts/package-texts.sh ZIEL [pflicht]}"
PFLICHT="${2:-1}"
cd "$(dirname "$0")/.."
mkdir -p "$ZIEL"

fehlt() {
    if [ "$PFLICHT" = "1" ]; then
        echo "FEHLER: Begleittext $1 fehlt - ein verteiltes Paket braucht alle Lizenz- und Hinweistexte" >&2
        exit 1
    fi
    echo "WARNUNG: Begleittext $1 fehlt - nur im Alltagsbau erlaubt" >&2
}

for f in LICENSE.txt THIRD_PARTY_NOTICES.txt BENUTZUNG.txt; do
    if [ -f "$f" ]; then cp "$f" "$ZIEL/"; else fehlt "$f"; fi
done
# Bildzeilen (![...](datei.png)) haben im Paket kein Bild - weg damit.
if [ -f .github/README.md ]; then
    sed '/^!\[/d' .github/README.md > "$ZIEL/README.txt"
else
    fehlt .github/README.md
fi
