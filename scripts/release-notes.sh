#!/usr/bin/env bash
# Writes the GitHub release notes to stdout. The first thing a visitor sees is
# which single file to download for each computer - one app per platform, and
# every computer running it can both control the others and be controlled;
# everything else is listed as "not needed" so nobody has to guess.
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

QuadChroma is **one app per platform**, and every computer that runs it can both
control the others and be controlled. Install it on **every computer** you want to
use - download the file for that computer's platform:

| Computer | Download | Then |
|---|---|---|
| **Each Windows PC** | [\`$win\`]($base/$win) | Unzip it anywhere and start \`quadchroma.exe\` inside the folder. Keep the two \`.dll\` files next to it. |
| **Each Mac** | [\`$mac\`]($base/$mac) | Open it, drag **QuadChroma** to **Applications** and start it. So that other computers can control this Mac, allow **Screen Recording** and **Accessibility** in System Settings > Privacy & Security. |

That is all. After a few seconds every computer with QuadChroma shows the others in
its start screen, with name and device ID - from whichever computer you sit at, click
any other one to connect. The first time, the app asks for the other computer's
access password: click the QuadChroma icon there (menu bar on a Mac, notification
area on Windows; four small squares) to see it. Or someone at the other computer
clicks **Allow** in the window that appears there. After that this computer connects
without asking. During a session, the menu's **Computers** tab (hold Esc for two
seconds, or press F10) switches to another computer.

There are no separate host and client downloads. Sharing is on by default; on a
computer that should only control others, switch off **Share this PC** (**Share this
Mac**) in the icon's menu or on the start screen.

Requirements: Windows 10 or 11 (64-bit); a Mac with Apple silicon (M1 or newer)
and macOS 14 or later; all computers in the same network (or connected through a
VPN).

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

QuadChroma lives in the menu bar (Mac) or the notification area (Windows); the very
first start opens its window, later starts stay in the background. The icon's menu
opens the window, shows the device ID and the access password, lists the allowed
devices, and offers **Change password**, **Change device name**, **Share this Mac**
(**Share this PC**), **Start at login** (**Start with Windows**), **Prevent sleep while
QuadChroma is running** and **Quit**.

Updating from an earlier test version: each computer now uses one key in both
directions, so another computer that knew it only by its former client key asks once
more for the password or **Allow**.

The complete manual is \`MANUAL.txt\` in the Windows ZIP and in the DMG.

---

The Windows download uses code of [FFmpeg](https://ffmpeg.org) licensed under the
[LGPLv2.1](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html) and its source
can be downloaded [here]($base/ffmpeg-9.0.2.tar.xz); the Mac app contains no FFmpeg. The complete corresponding
source of the FFmpeg DLLs is attached to this release (see
\`THIRD_PARTY_NOTICES.txt\`, section 2).

Verify downloads (optional): \`sha256sum -c SHA256SUMS.txt\` (Windows:
\`Get-FileHash -Algorithm SHA256 <file>\`).
EOF
