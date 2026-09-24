# QuadChroma - Mac-Host
#
#   make            baut und signiert QuadChroma.app
#   make list       zeigt die Bildschirme
#   make capture    nimmt 10 s auf und legt sie in /tmp/qc.hevc
#   make check      zeigt, was ffprobe im Ergebnis sieht
#
# Signiert wird mit dem lokalen Entwicklerzertifikat, damit die einmal erteilte
# Bildschirmaufnahme-Freigabe jeden Neubau ueberlebt.

IDENT   := QuadChroma Dev
BUNDLE  := tech.quadchroma.host
APP     := build/QuadChroma.app
BIN     := $(APP)/Contents/MacOS/quadchroma-host
SRC     := host/main.m host/audio.m host/clipboard.m host/dateien.m host/zeiger.m host/testbild.m host/last.m host/qc_noise.c host/qc_secure.c host/qc_annahme.c host/vendor/monocypher/monocypher.c
FLAGS   := -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations -mmacosx-version-min=14.0
FRAMEWORKS := -framework Foundation -framework AppKit -framework ScreenCaptureKit \
              -framework VideoToolbox -framework CoreMedia -framework CoreVideo \
              -framework CoreGraphics -framework CoreFoundation -framework IOKit

SECONDS ?= 10
OUT     ?= /tmp/qc.hevc
ARGS    ?=

.PHONY: all clean list capture check permissions

all: $(BIN)

$(BIN): $(SRC) host/Info.plist Makefile
	@mkdir -p $(APP)/Contents/MacOS
	cp host/Info.plist $(APP)/Contents/Info.plist
	clang $(FLAGS) $(FRAMEWORKS) $(SRC) -o $(BIN)
	codesign --force --sign "$(IDENT)" --identifier $(BUNDLE) --options runtime $(APP)
	@codesign -d -r- $(APP) 2>&1 | tail -1

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
