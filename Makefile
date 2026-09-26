# QuadChroma - Mac-Host: bauen, signieren, verpacken, notarisieren
#
# Alltag (lokal, mit dem selbstsignierten Zertifikat "QuadChroma Dev"):
#   make                 baut build/QuadChroma.app und signiert sie
#   make verify          prueft die Signatur (codesign --strict, Entitlements,
#                        Gatekeeper-Vorschau; letztere darf lokal scheitern)
#   make zip             build/QuadChroma-<version>-macos.zip (ditto --norsrc, App als oberster Eintrag)
#   make dmg             build/QuadChroma-<version>.dmg (UDZO, Verweis auf /Applications, signiert)
#   make clean
#
# Veroeffentlichung - braucht das Developer-ID-Zertifikat und ein notarytool-Profil
# im Schluesselbund (siehe RELEASING.md); alles davon richtet der Urheber selbst ein:
#   make release IDENT="Developer ID Application: Robert Brandt (TEAMID)"
#   Vorab werden IDENT, TIMESTAMP und die vier Beilagen geprueft, dann laeuft
#   der Reihe nach: sign verify notarize staple notarize-dmg (baut die DMG) staple-dmg gatekeeper.
#   Einzelschritte:
#   make sign            signiert die gebaute App erneut mit IDENT (Zeitstempel kommt bei
#                        Developer ID automatisch dazu). Achtung: jede neue Signatur
#                        entwertet ein bereits angeheftetes Notar-Ticket.
#   make notarize        ZIP neu erzeugen, beim Notar einreichen und warten,
#                        Protokoll nach build/notary-log.json (immer lesen, auch bei Accepted)
#   make staple          Ticket an die App heften, ZIP mit der gestapelten App neu erzeugen
#   make dmg             DMG aus der (gestapelten) App, mit IDENT signiert
#   make notarize-dmg    DMG neu bauen und einreichen, Protokoll nach build/notary-log-dmg.json
#   make staple-dmg      Ticket an die DMG heften
#   make gatekeeper      strenge Endpruefung: spctl muss annehmen, stapler validate,
#                        SHA-256 von ZIP und DMG fuer die Release-Notizen
#   make notary-history  Anmeldetest fuer das notarytool-Profil (zeigt fruehere Einreichungen)
#
# Entwicklerziele, die die App STARTEN (loesen die Freigabedialoge aus):
#   make list            zeigt die Bildschirme
#   make capture         nimmt 10 s auf und legt sie in /tmp/qc.hevc
#   make check           zeigt, was ffprobe im Ergebnis sieht
#   make permissions     Status der Bildschirmaufnahme-Freigabe
#
# Variablen (alle auf der Kommandozeile ueberschreibbar, z. B. make sign IDENT="..."):
#   IDENT           Signieridentitaet. Standard "QuadChroma Dev" (selbstsigniert, nur auf
#                   diesem Mac). Release: "Developer ID Application: Robert Brandt (TEAMID)"
#                   oder der 40-stellige SHA-1-Hash der Identitaet aus
#                   `security find-identity -p codesigning -v` (Apple: bei gleichnamigen
#                   Identitaeten "sign your code using this hash rather than the identity
#                   name"). "-" ist die Ad-hoc-Signatur (CI ohne Zertifikat).
#                   Ein ausdruecklich angegebenes IDENT wird durchgesetzt: weicht es von der
#                   letzten Signatur ab (Stempel build/.ident), signiert schon `make IDENT=...`
#                   die vorhandene App neu (kein Neubau). Ohne IDENT auf der Kommandozeile
#                   bleibt eine vorhandene Signatur stehen - so ueberlebt die Developer-ID-
#                   Signatur samt Ticket ein spaeteres `make verify` ohne Variablen.
#   BUNDLE          Bezeichner in der Signatur; muss CFBundleIdentifier in host/Info.plist
#                   entsprechen (die Signatur haengt daran, und TCC merkt sich ihn). Weicht
#                   er ab, bricht jeder Signierpfad mit FEHLER ab.
#   TIMESTAMP       Zeitstempel-Option fuer codesign. Automatisch "--timestamp", sobald IDENT
#                   "Developer ID" enthaelt oder ein 40-stelliger Hash ist; sonst leer, weil
#                   Apples Zeitstempeldienst (timestamp.apple.com) nur von Apple ausgestellte
#                   Zertifikate bedient. Gehoert ein Hash zu einem anderen Zertifikat:
#                   TIMESTAMP= (leer) mitgeben.
#   ENTITLEMENTS    Pfad einer Entitlements-Datei; nur benutzt, wenn sie existiert. Der Host
#                   braucht keine: Bildschirmaufnahme und Bedienungshilfen sind TCC-Freigaben,
#                   keine Entitlements, und die Hardened Runtime stoert ihn nicht.
#   NOTARY_PROFILE  Profilname aus `xcrun notarytool store-credentials <name> ...`.
#   VERSION         wird aus CFBundleShortVersionString in host/Info.plist gelesen und
#                   bestimmt die Paketnamen; CFBundleVersion dort vor jedem verteilten Bau erhoehen.
#   Beilagen        Lizenz- und Hinweistexte fuer die DMG, alle als .txt (LICENSE.txt,
#                   THIRD_PARTY_NOTICES.txt, README.txt aus .github/README.md, BENUTZUNG.txt) -
#                   scripts/package-texts.sh legt sie ab. Fehlt eine Quelle, bricht make dmg
#                   mit einer Developer ID ab (oder wenn DMG_EXTRA_REQUIRED=1 gesetzt ist, so
#                   in release.yml); im Alltagsbau gibt es nur eine WARNUNG.
#
# Warum immer signiert wird: TCC merkt sich Freigaben ueber die Designated Requirement der
# App (Bezeichner + Zertifikat). Mit dem selbstsignierten Zertifikat ueberlebt die einmal
# erteilte Bildschirmaufnahme-Freigabe jeden Neubau; nach einer Ad-hoc-Signatur (IDENT=-)
# muesste sie nach jedem Bau neu erteilt werden. Wechsel von Bezeichner oder Zertifikat
# kosten je eine Neufreigabe - deshalb beides in einem Schritt umstellen.
# Nach Apples Anleitung "Creating distribution-signed code for macOS": kein --deep,
# kein sudo, --options runtime fuer die Hardened Runtime, --timestamp fuer Developer ID.

IDENT          ?= QuadChroma Dev
BUNDLE         ?= tech.quadchroma.host
NOTARY_PROFILE ?= quadchroma-notary
ENTITLEMENTS   ?= host/entitlements.plist

# Developer-ID-Erkennung: der Name oder der 40-stellige SHA-1-Hash aus `security find-identity`.
# Ein Hash gilt hier als Developer ID (nur dafuer empfiehlt Apple ihn); TIMESTAMP bleibt
# ueberschreibbar, falls er doch zu einem anderen Zertifikat gehoert.
IDENT_IS_HASH  := $(shell printf '%s' "$(IDENT)" | grep -qxE '[0-9A-Fa-f]{40}' && echo 1)
RELEASE_IDENT  := $(if $(findstring Developer ID,$(IDENT))$(IDENT_IS_HASH),1,)
TIMESTAMP      ?= $(if $(RELEASE_IDENT),--timestamp,)
# Nur eine Identitaet von der Kommandozeile oder aus der Umgebung wird durchgesetzt (siehe
# sign-if-changed); der Standardwert aus dieser Datei laesst eine vorhandene Signatur stehen.
IDENT_EXPLICIT := $(if $(filter file default undefined,$(origin IDENT)),,1)

ifndef VERSION
VERSION := $(shell /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' host/Info.plist)
endif

APP      := build/QuadChroma.app
BIN      := $(APP)/Contents/MacOS/quadchroma-host
ZIP      ?= build/QuadChroma-$(VERSION)-macos.zip
DMG      ?= build/QuadChroma-$(VERSION).dmg
DMG_ROOT := build/dmg-root
# Beilagen fuer die DMG (ungesiegelt, aber von der DMG-Signatur abgedeckt), abgelegt von
# scripts/package-texts.sh. Pflicht, sobald mit einer Developer ID signiert wird oder
# DMG_EXTRA_REQUIRED gesetzt ist.
DMG_EXTRA_REQUIRED ?= $(RELEASE_IDENT)
# Stempel: womit zuletzt signiert wurde (Identitaet, Bezeichner, Zeitstempel, Entitlements).
IDENT_STAMP := build/.ident

SRC     := host/main.m host/audio.m host/clipboard.m host/dateien.m host/bildschirm.m host/zeiger.m host/testbild.m host/last.m host/qc_noise.c host/qc_secure.c host/qc_annahme.c host/vendor/monocypher/monocypher.c
FLAGS   := -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations -mmacosx-version-min=14.0
FRAMEWORKS := -framework Foundation -framework AppKit -framework ScreenCaptureKit \
              -framework VideoToolbox -framework CoreMedia -framework CoreVideo \
              -framework CoreGraphics -framework CoreFoundation -framework IOKit

SECONDS ?= 10
OUT     ?= /tmp/qc.hevc
ARGS    ?=

# Entitlements nur, wenn die Datei wirklich existiert (heute: keine).
ENT_OPT       = $(if $(wildcard $(ENTITLEMENTS)),--entitlements $(ENTITLEMENTS),)
SIGN_PARAMS   = $(IDENT)|$(BUNDLE)|$(TIMESTAMP)|$(ENT_OPT)
WRITE_STAMP   = printf '%s\n' '$(SIGN_PARAMS)' > $(IDENT_STAMP)
# Bezeichner in Signatur und Info.plist muessen uebereinstimmen (Designated Requirement,
# TCC-Freigaben); sonst FEHLER, bevor irgendetwas signiert wird.
BUNDLE_CHECK  = id=$$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' host/Info.plist); \
                if [ "$$id" != "$(BUNDLE)" ]; then echo "FEHLER: BUNDLE=$(BUNDLE), aber CFBundleIdentifier in host/Info.plist=$$id - Signatur und Info.plist muessen denselben Bezeichner tragen"; exit 1; fi
CODESIGN_APP  = codesign --force --sign "$(IDENT)" --identifier $(BUNDLE) --options runtime $(TIMESTAMP) $(ENT_OPT) $(APP)
# ZIP fuer Notar und Verteilung: --keepParent legt QuadChroma.app als obersten Eintrag ab.
# --norsrc laesst erweiterte Attribute (z. B. com.apple.provenance) weg, die ditto sonst als
# AppleDouble-Eintraege (._*) mitpackt; das Kommandozeilen-unzip macht daraus echte Dateien
# im Bundle und bricht so das Siegel. Das Bundle hat weder Resource-Forks noch Finder-Infos,
# Signatur und Notar-Ticket liegen in Dateien, nicht in Attributen. Zur Sicherheit wird das
# fertige ZIP auf ._-Eintraege geprueft.
MAKE_ZIP      = rm -f $(ZIP) && ditto -c -k --keepParent --norsrc $(APP) $(ZIP) \
                && if unzip -Z1 $(ZIP) | grep -qE '(^|/)\._'; then echo "FEHLER: $(ZIP) enthaelt AppleDouble-Eintraege (._*)"; exit 1; fi \
                && ls -l $(ZIP)

# $(call notarize,<datei>,<protokoll.json>): einreichen, warten, Protokoll holen und anzeigen,
# bei jedem Status ausser "Accepted" mit Fehler enden. Braucht Internet und das Profil.
define notarize
set -e; \
ausgabe=$$(xcrun notarytool submit "$(1)" --keychain-profile "$(NOTARY_PROFILE)" --wait --timeout 30m | tee /dev/stderr); \
id=$$(printf '%s\n' "$$ausgabe" | sed -n 's/^ *id: *//p' | head -n 1); \
if [ -z "$$id" ]; then echo "FEHLER: keine Submission-ID in der Ausgabe von notarytool"; exit 1; fi; \
xcrun notarytool log "$$id" --keychain-profile "$(NOTARY_PROFILE)" "$(2)"; \
cat "$(2)"; echo; \
if ! printf '%s\n' "$$ausgabe" | grep -q '^ *status: Accepted'; then echo "FEHLER: Notarisierung nicht angenommen - Protokoll: $(2)"; exit 1; fi
endef

.PHONY: all sign sign-if-changed verify zip dmg notarize staple notarize-dmg staple-dmg gatekeeper \
        release notary-history list capture check permissions clean

all: sign-if-changed

$(BIN): $(SRC) host/Info.plist Makefile $(wildcard $(ENTITLEMENTS))
	@$(BUNDLE_CHECK)
	@mkdir -p $(APP)/Contents/MacOS
	cp host/Info.plist $(APP)/Contents/Info.plist
	clang $(FLAGS) $(FRAMEWORKS) $(SRC) -o $(BIN)
	$(CODESIGN_APP)
	@$(WRITE_STAMP)
	@codesign -d -r- $(APP) 2>&1 | tail -1

# Signatur an ein ausdruecklich angegebenes IDENT (bzw. BUNDLE, TIMESTAMP, ENTITLEMENTS)
# angleichen: weicht der Stempel ab, wird nur neu signiert, nicht neu gebaut. Ohne Variablen
# auf der Kommandozeile passiert hier nichts. Haengt an all, verify, zip und dmg.
sign-if-changed: $(BIN)
	@if [ -n "$(IDENT_EXPLICIT)" ] && [ "$$(cat $(IDENT_STAMP) 2>/dev/null)" != '$(SIGN_PARAMS)' ]; then \
	   echo "Signierparameter geaendert - signiere neu mit IDENT=\"$(IDENT)\""; \
	   $(BUNDLE_CHECK); $(CODESIGN_APP) && $(WRITE_STAMP) && codesign -d -r- $(APP) 2>&1 | tail -1; \
	 fi

# Erneut signieren, z. B. mit der Developer ID. Info.plist und Binary sind Teil des Siegels,
# also nach jeder Aenderung daran; ein angeheftetes Ticket ist danach ungueltig (make staple).
sign: $(BIN)
	@$(BUNDLE_CHECK)
	$(CODESIGN_APP)
	@$(WRITE_STAMP)
	@codesign -d -r- $(APP) 2>&1 | tail -1

# Pruefen wie der Notar (--strict), Entitlements, Vorabpruefung, Gatekeeper-Vorschau.
verify: sign-if-changed
	codesign --verify --deep --strict --verbose=2 $(APP)
	@codesign -dvv $(APP) 2>&1 | grep -E '^(Identifier|Format|CodeDirectory|Authority|TeamIdentifier|Timestamp|Signed Time|Runtime Version)'
	@echo "Entitlements (nur die Executable-Zeile erwartet):"
	@codesign -d --entitlements - $(APP)
	@if codesign -d --entitlements - $(APP) 2>/dev/null | grep -q 'get-task-allow'; then \
	   echo "FEHLER: com.apple.security.get-task-allow ist eingebettet - der Notar lehnt das ab"; exit 1; fi
	@syspolicy_check distribution $(APP) || true
	@if spctl --assess --type execute -vv $(APP) 2>&1; then echo "Gatekeeper: angenommen"; \
	 else echo "Gatekeeper lehnt ab. Erwartet, solange IDENT=\"$(IDENT)\" keine Developer ID ist oder die App noch nicht notarisiert wurde (make gatekeeper prueft nach dem Release streng)."; fi

zip: sign-if-changed
	$(MAKE_ZIP)

# DMG: Ordner mit App, Verweis auf /Applications und Beilagen, dann UDZO-Abbild, dann Signatur
# mit eigenem Bezeichner (Apple: "Use a unique code-signing identifier that differs from the
# identifiers on your other products"). Auf macOS 27 warnt hdiutil, dass diese Form veraltet
# sei ("Please use 'diskutil image create from ...'"); sie funktioniert weiterhin und laeuft
# auch auf aelteren Systemen. hdiutil verify prueft die Pruefsumme des Abbilds.
dmg: sign-if-changed
	rm -rf $(DMG_ROOT) $(DMG)
	mkdir -p $(DMG_ROOT)
	scripts/package-texts.sh $(DMG_ROOT) $(if $(DMG_EXTRA_REQUIRED),1,0)
	ditto $(APP) $(DMG_ROOT)/QuadChroma.app
	ln -s /Applications $(DMG_ROOT)/Applications
	hdiutil create -volname QuadChroma -srcfolder $(DMG_ROOT) -ov -format UDZO $(DMG)
	hdiutil verify $(DMG)
	codesign --force --sign "$(IDENT)" $(TIMESTAMP) --identifier $(BUNDLE).dmg $(DMG)
	@ls -l $(DMG)

# Notarisierung: erst das ZIP (der Notar nimmt keine nackte .app), dann die App stapeln.
# Ohne Developer-ID-Signatur endet das absichtlich mit "The binary is not signed with a valid
# Developer ID certificate." im Protokoll.
notarize: zip
	$(call notarize,$(ZIP),build/notary-log.json)

# Ein ZIP laesst sich nicht stapeln: Ticket an die App heften und das ZIP neu erzeugen.
staple:
	xcrun stapler staple $(APP)
	xcrun stapler validate -v $(APP)
	$(MAKE_ZIP)

# Baut die DMG selbst (Voraussetzung dmg), damit sie die gestapelte App enthaelt.
notarize-dmg: dmg
	$(call notarize,$(DMG),build/notary-log-dmg.json)

staple-dmg:
	xcrun stapler staple $(DMG)
	xcrun stapler validate -v $(DMG)

# Strenge Endpruefung nach dem Release: hier muss Gatekeeper annehmen
# ("accepted", "source=Notarized Developer ID"), sonst bricht make ab.
gatekeeper:
	spctl --assess --type execute -vv $(APP)
	xcrun stapler validate -v $(APP)
	spctl --assess --type open --context context:primary-signature -vv $(DMG)
	xcrun stapler validate -v $(DMG)
	shasum -a 256 $(ZIP) $(DMG)

# Die ganze Kette. Absichtlich als Folge von Unter-Aufrufen, damit die Reihenfolge auch mit
# -j stimmt; IDENT und die anderen Kommandozeilenvariablen erben die Unter-Aufrufe ueber MAKEFLAGS.
# Vorabpruefungen, bevor irgendetwas hochgeladen wird: Developer ID (Name oder Hash), Zeitstempel,
# alle Beilagen vorhanden. Die DMG baut notarize-dmg; ein eigener dmg-Schritt davor wuerde sie
# nur ein zweites Mal bauen und signieren (zwei Zeitstempelanfragen).
release:
	@if [ -z "$(RELEASE_IDENT)" ]; then echo "FEHLER: make release braucht IDENT=\"Developer ID Application: <Name> (<TEAMID>)\" oder den SHA-1-Hash dieser Identitaet (jetzt: \"$(IDENT)\")"; exit 1; fi
	@if [ -z "$(TIMESTAMP)" ]; then echo "FEHLER: TIMESTAMP ist leer - ohne sicheren Zeitstempel lehnt der Notar ab (bei Developer ID TIMESTAMP=--timestamp)"; exit 1; fi
	@scripts/package-texts.sh build/beilagen-probe 1 && rm -rf build/beilagen-probe
	$(MAKE) sign
	$(MAKE) verify
	$(MAKE) notarize
	$(MAKE) staple
	$(MAKE) notarize-dmg
	$(MAKE) staple-dmg
	$(MAKE) gatekeeper

notary-history:
	xcrun notarytool history --keychain-profile "$(NOTARY_PROFILE)"

list: all
	open -n $(APP) --args --list
	@sleep 2 && tail -20 /tmp/quadchroma-m1.log

capture: all
	open -n $(APP) --args --capture $(SECONDS) $(OUT) $(ARGS)
	@sleep $$(($(SECONDS) + 4)) && tail -25 /tmp/quadchroma-m1.log

check:
	@ls -l $(OUT)
	@ffprobe -v error -show_entries stream=codec_name,profile,pix_fmt,width,height,color_range,color_space,color_primaries -of default=nw=1 $(OUT)
	@ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames -of default=nw=1 $(OUT)

permissions:
	@echo "Bildschirmaufnahme:  Systemeinstellungen > Datenschutz & Sicherheit > Bildschirmaufnahme"
	@echo "Status laut System:"; open -n $(APP) --args --list; sleep 2; tail -5 /tmp/quadchroma-m1.log

clean:
	rm -rf build
