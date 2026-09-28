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
ffmpeg=$(eine 'ffmpeg-*.tar.xz')
base="https://github.com/$repo/releases/download/$tag"

cat <<EOF
## Which file do I need?

QuadChroma is **one app per platform**, and every computer that runs it can both
control the others and be controlled. Install it on **every computer** you want to
use - download the file for that computer's platform:

| Computer | Download | Then |
|---|---|---|
| **Each Windows PC** | [\`$win\`]($base/$win) | Unzip it and start \`quadchroma.exe\` inside the folder; keep the two \`.dll\` files next to it. Windows asks for **administrator rights** (UAC) at every start - confirm with **Yes** (see "First start"). |
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

Requirements: Windows 10 or 11 (64-bit) with an administrator account; a Mac with
Apple silicon (M1 or newer) and macOS 14 or later; all computers in the same network
(or connected through a VPN).

**You do not need any of the other files:**

- \`$mac_zip\` - the same Mac app as a ZIP instead of a DMG
- \`$ffmpeg\`, \`$nvhdr\`, \`build-ffmpeg-windows.sh\` - source code of the FFmpeg libraries in the Windows download (required by their license)
- \`SHA256SUMS.txt\` - checksums, only if you want to verify your downloads
- **Source code** (zip / tar.gz) - QuadChroma's own source code, added automatically by GitHub

## First start

EOF

case "$win" in
    *-unsigned*) echo "**Windows:** the program is not code-signed yet, so SmartScreen warns once. Easiest: before unzipping, right-click the ZIP > Properties > tick **Unblock** > OK. Otherwise SmartScreen shows \"Windows protected your PC\": click **More info** > **Run anyway**. Then Windows asks for administrator rights at every start, with \"Unknown publisher\".";;
    *)           echo "**Windows:** \`quadchroma.exe\` is Authenticode-signed (SHA-256, RFC 3161 timestamp). Windows asks for administrator rights at every start.";;
esac
echo
echo "Why administrator rights: QuadChroma needs them so that a PC can also be controlled while Task Manager, the registry editor or an installer window is in front - without them Windows drops remote mouse and keyboard input there. Every start by hand shows the UAC prompt (confirm with **Yes**), also on a PC that only controls others. **Start with Windows** (icon menu or start screen) installs the app into \`C:\\Program Files\\QuadChroma\`, where only administrators can change it, and from then on starts it at every logon of your account elevated and without a prompt (each account has its own task; an older version never replaces a newer installed one). Command-line switches need a terminal started as administrator. On a Windows standard account every start needs an administrator's password, and **Start with Windows** is not available."
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

The complete manual is \`MANUAL.txt\` - in the Windows ZIP, next to the Mac app in the DMG
and the Mac ZIP, and inside the app; the license texts are next to it.

## HDR

Since 0.2.0 QuadChroma streams **HDR10** (BT.2020, PQ, 10 bits) in HEVC 4:4:4 or 4:2:0,
in both directions between Mac and Windows, whenever both sides can: the host's streamed
screen shows HDR (a Mac from macOS 15 on with an HDR or XDR screen, or a Windows PC with
**Use HDR** and an NVIDIA GPU), and the viewer's window is on an HDR screen. The
**HDR** switch in the menu's Picture tab (on by default, stored per computer) turns it
off. When HDR is active on both sides, the switch glows golden with sparkles, a golden
**HDR** badge shows for a few seconds and the statistics (F9) say **HDR10 · PQ** in gold.
If only one side can, the picture stays SDR - an HDR source on an SDR screen shows
**HDR → SDR** and its SDR content looks exactly as before. A Windows desktop with **Use
HDR** can now be streamed at all (as SDR to a viewer without HDR).

## Known limitations

- **No access before sign-in.** After a restart or a logout a computer is reachable
  only once someone has signed in on it: QuadChroma starts at login, not as a system
  service. The login screen cannot be controlled remotely.
- **The Windows UAC prompt itself cannot be controlled.** While Windows shows its
  consent prompt on the secure desktop, remote mouse and keyboard do not reach it; the
  viewer shows a hint, and someone at the PC has to answer it.
- **Administrator rights on Windows** at every manual start (see above); a standard
  account needs an administrator's password and gets no **Start with Windows**.
- **HDR is HDR10 only** - no HLG, no HDR10+ or Dolby Vision; 8-bit codecs and H.264
  stay SDR, the mouse pointer is always SDR, and a computer with QuadChroma 0.1.0 on
  either side gets SDR. HDR has been tested with test harnesses and recorded HDR10
  clips, not yet on every kind of real HDR screen.
EOF

case "$win" in
    *-unsigned*) echo "- **The Windows program is not code-signed.** SmartScreen warns once, and on Windows 11 Smart App Control can block it entirely (then only building from source helps).";;
esac
case "$mac" in
    *-unsigned*|*-selfsigned*|*-unnotarized*) echo "- **The Mac app is not notarized by Apple** - see **Mac** under First start for the extra step before the first start.";;
esac

cat <<EOF

---

The Windows download uses code of [FFmpeg](https://ffmpeg.org) licensed under the
[LGPLv2.1](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html) and its source
can be downloaded [here]($base/$ffmpeg); the Mac app contains no FFmpeg. The complete corresponding
source of the FFmpeg DLLs is attached to this release (see
\`THIRD_PARTY_NOTICES.txt\`, section 2).

Verify downloads (optional): \`sha256sum -c SHA256SUMS.txt\` (Windows:
\`Get-FileHash -Algorithm SHA256 <file>\`). Each download also carries a build
provenance attestation - proof that GitHub Actions built exactly this file from the
tagged source. Check it with the GitHub CLI:
\`gh attestation verify <file> --repo $repo\`.
EOF
