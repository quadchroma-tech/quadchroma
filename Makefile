# QuadChroma - Mac host: build, sign, package, notarize
#
# Everyday use (local, with the self-signed certificate "QuadChroma Dev"):
#   make                 builds build/QuadChroma.app and signs it
#   make verify          checks the signature (codesign --strict, entitlements,
#                        Gatekeeper preview; the latter may fail locally)
#   make zip             build/QuadChroma-<version>-macos.zip (ditto --norsrc, app as the top-level entry)
#   make dmg             build/QuadChroma-<version>.dmg (UDZO, link to /Applications, signed)
#   make clean
#
# Publishing - needs the Developer ID certificate and a notarytool profile
# in the keychain (see RELEASING.md); the author sets all of this up himself:
#   make release IDENT="Developer ID Application: Robert Brandt (TEAMID)"
#   IDENT, TIMESTAMP and the four companion files are checked first, then these
#   targets run in order: sign verify notarize staple notarize-dmg (builds the DMG) staple-dmg gatekeeper.
#   Individual steps:
#   make sign            signs the built app again with IDENT (for a Developer ID the
#                        timestamp is added automatically). Caution: every new signature
#                        invalidates a notarization ticket that is already stapled.
#   make notarize        recreate the ZIP, submit it for notarization and wait,
#                        log to build/notary-log.json (always read it, even for Accepted)
#   make staple          staple the ticket to the app, recreate the ZIP with the stapled app
#   make dmg             DMG from the (stapled) app, signed with IDENT
#   make notarize-dmg    rebuild and submit the DMG, log to build/notary-log-dmg.json
#   make staple-dmg      staple the ticket to the DMG
#   make gatekeeper      strict final check: spctl must accept, stapler validate,
#                        SHA-256 of ZIP and DMG for the release notes
#   make notary-history  login test for the notarytool profile (lists earlier submissions)
#
# Developer targets that START the app (they trigger the permission dialogs):
#   make list            lists the displays
#   make capture         records 10 s and writes them to /tmp/qc.hevc
#   make check           shows what ffprobe sees in the result
#   make permissions     status of the Screen Recording permission
#
# Variables (all can be overridden on the command line, e.g. make sign IDENT="..."):
#   IDENT           signing identity. Default "QuadChroma Dev" (self-signed, only on
#                   this Mac). Release: "Developer ID Application: Robert Brandt (TEAMID)"
#                   or the 40-character SHA-1 hash of the identity from
#                   `security find-identity -p codesigning -v` (Apple, for identities with
#                   the same name: "sign your code using this hash rather than the identity
#                   name"). "-" is the ad-hoc signature (CI without a certificate).
#                   An explicitly given IDENT is enforced: if it differs from the last
#                   signature (stamp build/.ident), even `make IDENT=...` signs the
#                   existing app again (no rebuild). Without IDENT on the command line an
#                   existing signature is kept - that way the Developer ID signature and
#                   its ticket survive a later `make verify` without variables.
#   BUNDLE          identifier in the signature; must match CFBundleIdentifier in host/Info.plist
#                   (the signature depends on it, and TCC remembers it). If it differs,
#                   every signing path stops with an ERROR.
#   TIMESTAMP       timestamp option for codesign. Automatically "--timestamp" as soon as IDENT
#                   contains "Developer ID" or is a 40-character hash; empty otherwise, because
#                   Apple's timestamp service (timestamp.apple.com) only serves certificates
#                   issued by Apple. If a hash belongs to a different certificate, pass
#                   TIMESTAMP= (empty).
#   ENTITLEMENTS    path of an entitlements file; only used if it exists. The host needs
#                   none: Screen Recording and Accessibility are TCC permissions, not
#                   entitlements, and the hardened runtime does not get in its way.
#   NOTARY_PROFILE  profile name from `xcrun notarytool store-credentials <name> ...`.
#   VERSION         read from CFBundleShortVersionString in host/Info.plist; determines
#                   the package names. Increase CFBundleVersion there before every distributed build.
#   Companion files (not a variable)
#                   license and notice texts for the DMG, all as .txt (LICENSE.txt,
#                   THIRD_PARTY_NOTICES.txt, README.txt made from README.md, MANUAL.txt) -
#                   scripts/package-texts.sh puts them in place. If a source is missing, make dmg
#                   stops when signing with a Developer ID (or when DMG_EXTRA_REQUIRED=1 is set,
#                   as in release.yml); a non-release build only prints a WARNING.
#
# Why the app is always signed: TCC remembers permissions by the app's designated requirement
# (identifier + certificate). With the self-signed certificate, a Screen Recording permission
# granted once survives every rebuild; after an ad-hoc signature (IDENT=-) it would have to
# be granted again after every build. Changing the identifier or the certificate costs one
# new grant each - so change both in the same step.
# Following Apple's guide "Creating distribution-signed code for macOS": no --deep,
# no sudo, --options runtime for the hardened runtime, --timestamp for Developer ID.

IDENT          ?= QuadChroma Dev
BUNDLE         ?= tech.quadchroma.host
NOTARY_PROFILE ?= quadchroma-notary
ENTITLEMENTS   ?= host/entitlements.plist

# Developer ID detection: the name or the 40-character SHA-1 hash from `security find-identity`.
# A hash counts as a Developer ID here (Apple recommends it only for that); TIMESTAMP can
# still be overridden in case the hash belongs to a different certificate after all.
IDENT_IS_HASH  := $(shell printf '%s' "$(IDENT)" | grep -qxE '[0-9A-Fa-f]{40}' && echo 1)
RELEASE_IDENT  := $(if $(findstring Developer ID,$(IDENT))$(IDENT_IS_HASH),1,)
TIMESTAMP      ?= $(if $(RELEASE_IDENT),--timestamp,)
# Only an identity from the command line or the environment is enforced (see
# sign-if-changed); the default value from this file keeps an existing signature.
IDENT_EXPLICIT := $(if $(filter file default undefined,$(origin IDENT)),,1)

ifndef VERSION
VERSION := $(shell /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' host/Info.plist)
endif

APP      := build/QuadChroma.app
BIN      := $(APP)/Contents/MacOS/quadchroma-host
ZIP      ?= build/QuadChroma-$(VERSION)-macos.zip
DMG      ?= build/QuadChroma-$(VERSION).dmg
DMG_ROOT := build/dmg-root
# Companion files for the DMG (outside the app's seal, but covered by the DMG signature), put
# in place by scripts/package-texts.sh. Mandatory as soon as the signing identity is a
# Developer ID or DMG_EXTRA_REQUIRED is set.
DMG_EXTRA_REQUIRED ?= $(RELEASE_IDENT)
# Stamp: what the last signature was made with (identity, identifier, timestamp, entitlements).
IDENT_STAMP := build/.ident

SRC     := host/main.m host/audio.m host/clipboard.m host/dateien.m host/bildschirm.m host/zeiger.m host/testbild.m host/last.m host/menue.m host/texte.m host/zugang.c host/qc_noise.c host/qc_secure.c host/qc_annahme.c host/vendor/monocypher/monocypher.c
FLAGS   := -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations -mmacosx-version-min=14.0
# ServiceManagement: "Start at login" (SMAppService); SystemConfiguration: the computer
# name for the announcement (SCDynamicStoreCopyComputerName).
FRAMEWORKS := -framework Foundation -framework AppKit -framework ScreenCaptureKit \
              -framework VideoToolbox -framework CoreMedia -framework CoreVideo \
              -framework CoreGraphics -framework CoreFoundation -framework IOKit \
              -framework ServiceManagement -framework SystemConfiguration

SECONDS ?= 10
OUT     ?= /tmp/qc.hevc
ARGS    ?=

# Entitlements only if the file actually exists (currently: none).
ENT_OPT       = $(if $(wildcard $(ENTITLEMENTS)),--entitlements $(ENTITLEMENTS),)
SIGN_PARAMS   = $(IDENT)|$(BUNDLE)|$(TIMESTAMP)|$(ENT_OPT)
WRITE_STAMP   = printf '%s\n' '$(SIGN_PARAMS)' > $(IDENT_STAMP)
# The identifiers in the signature and in Info.plist must match (designated requirement,
# TCC permissions); otherwise ERROR before anything is signed.
BUNDLE_CHECK  = id=$$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' host/Info.plist); \
                if [ "$$id" != "$(BUNDLE)" ]; then echo "ERROR: BUNDLE=$(BUNDLE), but CFBundleIdentifier in host/Info.plist=$$id - the signature and Info.plist must carry the same identifier"; exit 1; fi
CODESIGN_APP  = codesign --force --sign "$(IDENT)" --identifier $(BUNDLE) --options runtime $(TIMESTAMP) $(ENT_OPT) $(APP)
# ZIP for notarization and distribution: --keepParent stores QuadChroma.app as the top-level entry.
# --norsrc leaves out extended attributes (e.g. com.apple.provenance) that ditto would otherwise
# pack as AppleDouble entries (._*); the command-line unzip turns those into real files inside
# the bundle and thereby breaks the seal. The bundle has neither resource forks nor Finder info;
# the signature and the notarization ticket live in files, not in attributes. To be safe, the
# finished ZIP is checked for ._ entries.
MAKE_ZIP      = rm -f $(ZIP) && ditto -c -k --keepParent --norsrc $(APP) $(ZIP) \
                && if unzip -Z1 $(ZIP) | grep -qE '(^|/)\._'; then echo "ERROR: $(ZIP) contains AppleDouble entries (._*)"; exit 1; fi \
                && ls -l $(ZIP)

# $(call notarize,<file>,<log.json>): submit, wait, fetch and show the log, exit with an
# error for any status other than "Accepted". Needs internet access and the profile.
define notarize
set -e; \
ausgabe=$$(xcrun notarytool submit "$(1)" --keychain-profile "$(NOTARY_PROFILE)" --wait --timeout 30m | tee /dev/stderr); \
id=$$(printf '%s\n' "$$ausgabe" | sed -n 's/^ *id: *//p' | head -n 1); \
if [ -z "$$id" ]; then echo "ERROR: no submission ID in the notarytool output"; exit 1; fi; \
xcrun notarytool log "$$id" --keychain-profile "$(NOTARY_PROFILE)" "$(2)"; \
cat "$(2)"; echo; \
if ! printf '%s\n' "$$ausgabe" | grep -q '^ *status: Accepted'; then echo "ERROR: notarization not accepted - log: $(2)"; exit 1; fi
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

# Bring the signature in line with an explicitly given IDENT (or BUNDLE, TIMESTAMP,
# ENTITLEMENTS): if the stamp differs, the app is only signed again, not rebuilt. Without
# variables on the command line nothing happens here. all, verify, zip and dmg depend on it.
sign-if-changed: $(BIN)
	@if [ -n "$(IDENT_EXPLICIT)" ] && [ "$$(cat $(IDENT_STAMP) 2>/dev/null)" != '$(SIGN_PARAMS)' ]; then \
	   echo "Signing parameters changed - signing again with IDENT=\"$(IDENT)\""; \
	   $(BUNDLE_CHECK); $(CODESIGN_APP) && $(WRITE_STAMP) && codesign -d -r- $(APP) 2>&1 | tail -1; \
	 fi

# Sign again, e.g. with the Developer ID. Info.plist and the binary are part of the seal,
# so do this after every change to them; a stapled ticket is invalid afterwards (make staple).
sign: $(BIN)
	@$(BUNDLE_CHECK)
	$(CODESIGN_APP)
	@$(WRITE_STAMP)
	@codesign -d -r- $(APP) 2>&1 | tail -1

# Check like the notary service (--strict), entitlements, distribution pre-check, Gatekeeper preview.
verify: sign-if-changed
	codesign --verify --deep --strict --verbose=2 $(APP)
	@codesign -dvv $(APP) 2>&1 | grep -E '^(Identifier|Format|CodeDirectory|Authority|TeamIdentifier|Timestamp|Signed Time|Runtime Version)'
	@echo "Entitlements (only the Executable line is expected):"
	@codesign -d --entitlements - $(APP)
	@if codesign -d --entitlements - $(APP) 2>/dev/null | grep -q 'get-task-allow'; then \
	   echo "ERROR: com.apple.security.get-task-allow is embedded - notarization would reject it"; exit 1; fi
	@syspolicy_check distribution $(APP) || true
	@if spctl --assess --type execute -vv $(APP) 2>&1; then echo "Gatekeeper: accepted"; \
	 else echo "Gatekeeper rejects the app. Expected as long as IDENT=\"$(IDENT)\" is not a Developer ID or the app has not been notarized yet (make gatekeeper checks strictly after the release)."; fi

zip: sign-if-changed
	$(MAKE_ZIP)

# DMG: a folder with the app, a link to /Applications and the companion files, then a UDZO
# image, then a signature with its own identifier (Apple: "Use a unique code-signing identifier
# that differs from the identifiers on your other products"). On macOS 27 hdiutil warns that
# this form is deprecated ("Please use 'diskutil image create from ...'"); it still works and
# also runs on older systems. hdiutil verify checks the checksum of the image.
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

# Notarization: first the ZIP (the notary service does not accept a bare .app), then staple
# the app. Without a Developer ID signature this deliberately ends with "The binary is not
# signed with a valid Developer ID certificate." in the log.
notarize: zip
	$(call notarize,$(ZIP),build/notary-log.json)

# A ZIP cannot be stapled: staple the ticket to the app and recreate the ZIP.
staple:
	xcrun stapler staple $(APP)
	xcrun stapler validate -v $(APP)
	$(MAKE_ZIP)

# Builds the DMG itself (prerequisite dmg) so that it contains the stapled app.
notarize-dmg: dmg
	$(call notarize,$(DMG),build/notary-log-dmg.json)

staple-dmg:
	xcrun stapler staple $(DMG)
	xcrun stapler validate -v $(DMG)

# Strict final check after the release: here Gatekeeper must accept
# ("accepted", "source=Notarized Developer ID"), otherwise make stops.
gatekeeper:
	spctl --assess --type execute -vv $(APP)
	xcrun stapler validate -v $(APP)
	spctl --assess --type open --context context:primary-signature -vv $(DMG)
	xcrun stapler validate -v $(DMG)
	shasum -a 256 $(ZIP) $(DMG)

# The whole chain. Deliberately a sequence of sub-makes so that the order is also right with
# -j; the sub-makes inherit IDENT and the other command-line variables through MAKEFLAGS.
# Checks before anything is uploaded: Developer ID (name or hash), timestamp, all companion
# files present. notarize-dmg builds the DMG; a separate dmg step before it would only build
# and sign it a second time (two timestamp requests).
release:
	@if [ -z "$(RELEASE_IDENT)" ]; then echo "ERROR: make release needs IDENT=\"Developer ID Application: <Name> (<TEAMID>)\" or the SHA-1 hash of that identity (currently: \"$(IDENT)\")"; exit 1; fi
	@if [ -z "$(TIMESTAMP)" ]; then echo "ERROR: TIMESTAMP is empty - without a secure timestamp notarization is rejected (for a Developer ID: TIMESTAMP=--timestamp)"; exit 1; fi
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
	@echo "Screen Recording:  System Settings > Privacy & Security > Screen Recording"
	@echo "Status reported by the system:"; open -n $(APP) --args --list; sleep 2; tail -5 /tmp/quadchroma-m1.log

clean:
	rm -rf build
