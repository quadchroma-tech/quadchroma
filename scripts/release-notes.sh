#!/usr/bin/env bash
# Writes the GitHub release notes to stdout. The first thing a visitor sees is
# which single file to download for each computer; everything else is listed
# as "not needed" so nobody has to guess.
#
# Usage: scripts/release-notes.sh DIST_DIR VERSION REPOSITORY TAG
#   DIST_DIR    folder with the release files (only their names are used)
#   VERSION     e.g. 0.1.0
#   REPOSITORY  owner/name, e.g. quadchroma-tech/quadchroma
#   TAG         e.g. v0.1.0

set -euo pipefail

dist="${1:?usage: scripts/release-notes.sh DIST_DIR VERSION REPOSITORY TAG}"
version="${2:?}"; repo="${3:?}"; tag="${4:?}"
cd "$dist"

eine() {  # the single file matching a pattern, or an error
    local treffer=( $1 )
    [ "${#treffer[@]}" -eq 1 ] && [ -e "${treffer[0]}" ] || { echo "release-notes: expected exactly one file for $1" >&2; exit 1; }
    printf '%s' "${treffer[0]}"
}
win=$(eine 'quadchroma-*-windows-x64*.zip')
mac=$(eine 'QuadChroma-*-macos-arm64*.dmg')
mac_zip=$(eine 'QuadChroma-*-macos-arm64*.zip')
nvhdr=$(eine 'nv-codec-headers-*.tar.gz')
base="https://github.com/$repo/releases/download/$tag"

cat <<EOF
## Which file do I need?

QuadChroma runs on two computers: the **Mac** you want to control and the
**Windows PC** you control it from. Download **one file for each**:

| Computer | Download | Then |
|---|---|---|
| **Windows PC** (you sit here) | [\`$win\`]($base/$win) | Unzip it anywhere and start \`quadchroma.exe\` inside the folder. Keep the two \`.dll\` files next to it. |
| **Mac** (the computer you control) | [\`$mac\`]($base/$mac) | Open it and drag **QuadChroma** to **Applications**. Start it and allow **Screen Recording** and **Accessibility** in System Settings > Privacy & Security. |

That is all. The Mac appears in the start screen of the Windows program after a
few seconds; click it to connect. The first time, the Windows program asks for the
Mac's access password: click the QuadChroma icon in the Mac's menu bar (four small
squares) to see it. Or someone at the Mac clicks **Allow** in the window that
appears there. After that this PC connects without asking.

The same Windows download can also share a Windows PC: click **Share this PC** on
the start screen.

Requirements: Windows 10 or 11 (64-bit); a Mac with Apple silicon (M1 or newer)
and macOS 14 or later; both in the same network (or connected through a VPN).

**You do not need any of the other files:**

- \`$mac_zip\` - the same Mac app as a ZIP instead of a DMG
- \`ffmpeg-9.0.2.tar.xz\`, \`$nvhdr\`, \`build-ffmpeg-windows.sh\` - source code of the FFmpeg libraries in the Windows download (required by their license)
- \`SHA256SUMS.txt\` - checksums, only if you want to verify your downloads
- **Source code** (zip / tar.gz) - QuadChroma's own source code, added automatically by GitHub

## First start

EOF

case "$win" in
    *-unsigned*) echo "**Windows:** the program is not code-signed yet, so Windows warns once. Easiest: before unzipping, right-click the ZIP > Properties > tick **Unblock** > OK. Otherwise SmartScreen shows \"Windows protected your PC\": click **More info** > **Run anyway**.";;
    *)           echo "**Windows:** \`quadchroma.exe\` is Authenticode-signed (SHA-256, RFC 3161 timestamp).";;
esac
echo
case "$mac" in
    *-unsigned*)    echo "**Mac:** the app is ad-hoc signed only and not notarized. macOS refuses the first start: System Settings > Privacy & Security > **Open Anyway**. After every update Screen Recording and Accessibility must be allowed again.";;
    *-selfsigned*)  echo "**Mac:** the app is signed with the project's own free certificate \"QuadChroma Release\" but not notarized by Apple, so macOS refuses the first start: open System Settings > Privacy & Security, scroll down and click **Open Anyway**. Screen Recording and Accessibility stay allowed across updates.";;
    *-unnotarized*) echo "**Mac:** the app is Developer ID signed but not notarized; macOS asks once before the first start.";;
    *)              echo "**Mac:** the app is Developer ID signed, notarized and stapled; macOS only asks once whether to open it.";;
esac

cat <<EOF

On the Mac, QuadChroma lives in the menu bar, not in the Dock. Its menu shows the
device ID and the access password, lists the allowed devices, and offers
**Change password**, **Start at login** and **Quit QuadChroma**.

The complete manual is \`MANUAL.txt\` in the Windows ZIP and in the DMG.

---

This software uses code of [FFmpeg](https://ffmpeg.org) licensed under the
[LGPLv2.1](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html) and its source
can be downloaded [here]($base/ffmpeg-9.0.2.tar.xz). The complete corresponding
source of the FFmpeg DLLs is attached to this release (see
\`THIRD_PARTY_NOTICES.txt\`, section 2).

Verify downloads (optional): \`sha256sum -c SHA256SUMS.txt\` (Windows:
\`Get-FileHash -Algorithm SHA256 <file>\`).
EOF
