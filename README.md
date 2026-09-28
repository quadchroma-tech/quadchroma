# QuadChroma

**Remote desktop between Macs and Windows PCs, pixel-sharp: HEVC 4:4:4 with 10 bits,
encoded in hardware by Apple silicon or an NVIDIA GPU and shown on any Mac or Windows
10/11 PC - decoded in hardware where the GPU can, otherwise in software. Free for
personal use.**

As far as we know, QuadChroma is the only free remote desktop that streams a Mac to a
Windows PC in 4:4:4 at 10 bits with the Mac's hardware encoder, the Media Engine of
Apple silicon (comparison below). Every Windows 10 or 11 PC can show that stream. An
NVIDIA GPU decodes it in hardware (NVDEC); on AMD or Intel graphics, or without a
suitable GPU, the PC decodes 4:4:4 in software, in about 8.6 ms per 1080p frame and
slice-parallel. AMD and Intel GPUs decode HEVC 4:2:0 and H.264 in hardware (D3D11VA),
and the codec can be switched while the session runs.

Windows PCs send the same picture quality: a PC with an NVIDIA GPU encodes 4:4:4 at
10 bits with NVENC - tested from PC to PC with an RTX 3080 Ti laptop GPU as the
host. Without an NVIDIA GPU a Windows PC can still be controlled, but it only sends
H.264 in software for now.

- **Sharp text, clean color edges.** 4:4:4 keeps a color value for every pixel;
  the usual 4:2:0 shares one between four, which makes red text and thin coloured
  lines fray. 10 bits per sample instead of 8 remove banding in gradients.
- **The pointer is yours.** It is never part of the video: Windows draws its own
  pointer in the shape the Mac reports, so the mouse feels local even when the
  picture is a few milliseconds behind.
- **Latency you can see.** 15 to 20 ms from capture on a Mac mini M1 to hand-over to
  the display at 1080p and 120 frames/s in the LAN (about 24 ms when the PC decodes
  4:4:4 in software); the statistics panel shows every link of the chain live
  (encoder, network, decoder), and a built-in benchmark recommends codec, bit rate
  and frame rate for your network.
- **One app on every computer, nothing else.** `QuadChroma.app` on the Mac,
  `quadchroma.exe` on Windows - each one is client and host at once. Install it on
  every computer you use; from whichever one you sit at, pick any other from the
  list and connect. No separate host program, no account, no cloud, no relay server,
  no extra driver or helper service - connected directly in your network (or through
  your VPN). Always encrypted (Noise protocol). A new device gets in with the other
  computer's access password or a click on "Allow" there - no command-line switches.
- **HDR10 when both sides can.** New in 0.2.0: an HDR screen on the host and an HDR
  screen on the viewer give HDR10 (BT.2020, PQ, 10 bits) in HEVC 4:4:4 or 4:2:0, in
  both directions between Mac and Windows; the HDR switch in the menu then glows
  golden. When only one side can, the picture stays SDR - and an HDR source on an SDR
  screen is mapped so that its SDR content looks exactly as before (see "HDR").
- **Everyday comfort.** Copy files between Mac and PC through the clipboard, like
  with Windows Remote Desktop; choose which of the Mac's screens to show (it follows
  the main screen automatically); audio; a desktop shortcut per host; 29 languages.

![The start screen of the app on a Mac: two computers found on the network - one already known (check mark) with its device ID, one older host without an ID - the address field, the buttons Connect, Share this Mac (with its check box) and Close window, below them the line "This computer: Office PC · 305 114 872" with Rename and the check boxes "Prevent sleep while QuadChroma is running" and "Start at login". On Windows the same screen says Share this PC and Start with Windows and has a Desktop shortcut button in every row. The interface is available in 29 languages.](start-screen.png)

## How it compares

Streaming *from a Mac* in 4:4:4, checked on 26 September 2026 from the vendors'
documentation and source code:

| | 4:4:4 from a Mac host | 10 bit | Encoding on the Mac | Cost |
|---|---|---|---|---|
| **QuadChroma** | yes | yes | hardware (VideoToolbox, HEVC) | free for personal use |
| Jump Desktop (Fluid) | yes | only "Ultra" mode | not stated | paid app; 10-bit 4:4:4 only in Jump Desktop for Teams Enterprise |
| Parsec | no - "Prefer 4:4:4 color" needs a host with NVIDIA or Intel H.265 4:4:4 encoding | - | - | 4:4:4 only in Teams and Warp |
| Sunshine + Moonlight | only with the software encoder (the VideoToolbox encoder has no 4:4:4) | not documented for this path | software | free |
| Splashtop | no - 4:4:4 on Windows streamers only | - | - | paid |
| RustDesk | yes, VP9/AV1 4:4:4 | not documented | software | free |

Sources: [Jump Desktop Fluid 2.0](https://changelog.jumpdesktop.com/fluid-2.0-beta-2-studio-quality-remote-desktop-with-4-4-4-and-10-bit-color-1IKhva),
[Jump Desktop 10](https://docs.jumpdesktop.com/whats-new/jump-desktop-10/),
[Parsec: stream quality and color accuracy](https://support.parsec.app/hc/en-us/articles/32381785123860-Improve-Stream-Quality-and-Color-Accuracy),
[Sunshine `src/video.cpp`](https://github.com/LizardByte/Sunshine/blob/master/src/video.cpp) (encoder flags),
[Splashtop performance options](https://support-splashtopbusiness.splashtop.com/hc/en-us/articles/6193537936027-Performance-Options),
[RustDesk discussion #8128](https://github.com/rustdesk/rustdesk/discussions/8128).
Products change; corrections are welcome as an issue.

## How it works

Remote desktop with full color resolution. The Mac captures its screen, encodes it
in hardware as HEVC 4:4:4 with 10 bits per sample and sends it to a Windows PC. The
mouse pointer is deliberately *not* rendered into the video: what you see is the
Windows pointer, and the Mac follows it invisibly, so the mouse feels local. To make
it still look like the Mac's pointer, the host sends only the pointer *shape* (arrow,
hand, resize arrows, text cursor, spinning wait cursor) whenever it changes.

Status as of 28 September 2026: it runs, but it is still a scaffold, not a finished
application. Releases: https://github.com/quadchroma-tech/quadchroma/releases. The Mac
app ships as a DMG and as a ZIP (a folder with the app and the license texts); unpack
the ZIP with the Finder or `ditto -x -k` (it contains no AppleDouble `._*` entries, so
the command-line `unzip` works as well).

This README is the overview. `MANUAL.txt` is the complete manual: every switch, log
line and protocol message. Code comments and the program's log lines are in German;
where this README quotes a log line, it keeps the German original and explains it.

## Why 4:4:4

Common remote-desktop tools transmit color at half the width and half the height of
the picture. At 1080p the chroma ends up at 960×540, and red edges visibly fray.
QuadChroma transmits a separate color value for every pixel. Apple's Media Engine can
do this in hardware, but FFmpeg does not expose these profiles, so the host drives the
encoder directly through VideoToolbox.

## Decoding on Windows

Every Windows 10 or 11 PC can show every codec the host offers, including HEVC 4:4:4
with 10 bits. Which decoder does the work depends on the graphics hardware:

| Graphics in the PC | HEVC 4:4:4 | HEVC 4:2:0 and H.264 |
|---|---|---|
| NVIDIA | hardware (NVDEC) | hardware (NVDEC) |
| AMD or Intel | software | hardware (D3D11VA) |
| no suitable GPU | software | software |

The default choice, Automatic, takes NVDEC when an NVIDIA card is present, otherwise
D3D11VA on the card that draws the picture, otherwise software. D3D11VA in FFmpeg 9
handles only HEVC 4:2:0 and H.264; with a 4:4:4 codec the client says so and decodes
in software, and the codec can be changed in the Picture tab of the menu. A hardware
decoder that accepts data but delivers no picture, or only unreadable ones, for 1.5 s
is replaced by software automatically; the reason appears in the statistics and in
the log. The Display tab lets you pick the decoder yourself (Automatic, Graphics card,
Integrated, Processor), effective immediately; on the command line
`--decoder auto|gpu|gpu2|integriert|software`.

The software decoder needs about 8.6 ms per 1080p frame for HEVC 4:4:4 at 10 bits and
works slice-parallel, so that no frame is held back. Measured over the network at
1080p with a target of 120 frames/s:

| Codec, software decoder | Latency | Decoder |
|---|---|---|
| HEVC 4:4:4 10 bit | about 24 ms | 8.6 ms |
| HEVC 4:2:0 8 bit | about 19 ms | 6.8 ms |
| H.264 High | about 33 ms | 15.0 ms |

Drawing is separate from decoding: with a graphics card the client draws with
Direct3D 11, whichever decoder runs. The raw decoder planes go to the GPU, and color
conversion and scaling run there in shaders, bit-identical to the CPU path; without a
graphics card the CPU draws.

## Design in brief

- One program per side: `QuadChroma.app` on the Mac, `quadchroma.exe` on Windows. Each
  is one app for both directions - client and host in one process with one icon; on the
  Mac the Rust client has the Objective-C host engine built in.
- Pointer on the client side: the client shows its own local pointer in the shape the
  host reports; the video never contains one.
- Always encrypted: Noise XX with X25519, ChaCha20-Poly1305 and SHA-256, no switch to
  turn it off.
- Access by password or click: every device has a permanent key and a nine-digit
  device ID derived from it. A host lets a new device in when it proves the host's
  access password - the password itself never crosses the network - or when someone
  at the host clicks "Allow"; after that it knows the device by its key. The client
  remembers every host that let it in by its key as well, whatever address it has.
- The host does nothing without a viewer: no capture, no encoder, 0.6 % CPU load when
  idle; capture and encoder appear when someone connects and go when the viewer leaves.

## Platforms and status

There is one app per platform, and every copy is client and host at once: `QuadChroma.app`
on the Mac, `quadchroma.exe` on Windows. The table lists the two roles of each.

| Role | Platform | State |
|---|---|---|
| Host | Mac (developed and measured on a Mac mini M1), macOS 14 or later; part of `QuadChroma.app` ("Share this Mac", on by default); host engine in Objective-C and C | Main role. HEVC 4:4:4 and 4:2:0 in 8 and 10 bit and H.264, all in hardware and switchable while running; HDR10 in the 10-bit HEVC modes from macOS 15 on, when the streamed screen shows HDR; audio; clipboard including files; choice of the streamed screen; the app's menu-bar icon with device ID, access password, allowed devices, the sharing switch, "Start at login", the device name and "Prevent sleep". |
| Client | Windows 10 or 11, 64-bit; part of `quadchroma.exe`; Rust | Main role. NVDEC, D3D11VA or software decoding; Direct3D 11 display, HDR10 output on a screen with "Use HDR"; notification-area icon, single instance, desktop shortcut per host. |
| Host | Windows, part of the same `quadchroma.exe` ("Share this PC", on by default; `--nur-host` runs it alone for tests) | Secondary. Capture via Desktop Duplication, encoder via NVENC on NVIDIA; without NVIDIA only H.264 in software via Media Foundation, which holds back 16 frames and is enough to test the chain but not for real use. A desktop with "Use HDR" is streamed as HDR10 through NVENC when the viewer can show it, otherwise converted to SDR on the GPU. The app's icon in the notification area carries device ID, access password, allowed devices, the sharing switch, "Start with Windows", the device name and "Prevent sleep". Missing: AMF/QSV, scaling and rotation on the GPU for SDR (both run on the CPU today). HEVC 4:4:4 at 10 bits through NVENC verified on real hardware (RTX 3080 Ti laptop GPU as the host, 1080p, a Windows PC as the viewer, 28 September 2026). |
| Client | Mac (arm64), part of the same `QuadChroma.app`; the same Rust source as on Windows | Secondary. Decoding with VideoToolbox (no FFmpeg on the Mac), audio via AudioToolbox, clipboard including files via NSPasteboard, display through Metal (the CPU as fallback), HDR10 through EDR on a screen with headroom, the app's one menu-bar icon, no desktop shortcut. Ships inside `QuadChroma.app`. |

Signing: the project does not pay for certificates yet. Mac releases are signed with
the project's own free certificate "QuadChroma Release" (permissions survive updates,
but macOS asks once before the first start), the Windows executable is unsigned; see
"First start of a downloaded release". `RELEASING.md` describes the paid route (Apple
Developer ID with notarisation, a Windows code-signing certificate) for later.

## Requirements

- Mac: macOS 14 or later on Apple silicon; `QuadChroma.app`, nothing else (no
  FFmpeg). For sharing the Mac, two permissions in System Settings > Privacy &
  Security: Screen Recording (for the picture) and Accessibility (for mouse and
  keyboard).
- Windows: Windows 10 or 11, 64-bit, with any graphics hardware (see "Decoding on
  Windows"). `quadchroma.exe` with two FFmpeg DLLs next to it: `avcodec-63.dll` and
  `avutil-61.dll`.
  No Visual C++ runtime is needed: the C runtime is linked into the exe, and the DLLs
  use only the Universal C Runtime that is part of Windows 10 and 11. Direct3D, DXGI
  and `d3dcompiler_47.dll` are parts of Windows; none of them is shipped.
- Windows: an administrator account. The app requires administrator rights, so that
  remote mouse and keyboard also reach windows with higher rights (Task Manager,
  installers), and Windows asks via UAC at every manual start - also on a PC that only
  controls others. "Start with Windows" starts it elevated at logon without asking (see
  "Sharing a Windows PC"). On a standard account every start needs an administrator's
  password, and "Start with Windows" is not available.
- HDR (optional, see "HDR"): an HDR screen on both sides; a Mac as the host needs
  macOS 15 or later, a Windows PC as the host an NVIDIA card.
- Network: three ports on every computer that shares. 9001 carries video and audio
  (everything from host to client), 9002 input and clipboard including files
  (everything from client to host), 9003 the announcement. A sharing computer
  announces itself every two seconds on the local network. On the first start the
  Windows Firewall asks whether the program may use the network; allow it, otherwise
  the list stays empty and other computers cannot connect.

## First start of a downloaded release

QuadChroma is not signed with a paid certificate (Apple Developer Program, Windows
code-signing certificate), so Windows and macOS warn before the first start.
Download only from the project's Releases page and compare the SHA-256 of the file
with `SHA256SUMS.txt` (Windows: `Get-FileHash -Algorithm SHA256 <file>`; macOS:
`shasum -a 256 <file>`). From version 0.1.1 on, every download also carries a build
provenance attestation: a signed statement that GitHub Actions built exactly this
file from the tagged source, so nobody replaced it afterwards. With the GitHub CLI:
`gh attestation verify <file> --repo quadchroma-tech/quadchroma`. It does not
replace a code signature, but it can be checked by anyone.

**Windows.** Before unpacking, right-click the ZIP > Properties > tick "Unblock" > OK;
that avoids the SmartScreen warning. Without it, SmartScreen shows "Windows protected
your PC": choose "More info" > "Run anyway" once. Either way Windows asks for
administrator rights (UAC) at every manual start, with "Unknown publisher" as long as
the exe is not code-signed - confirm with "Yes" (see Requirements for the reason).
Keep `avcodec-63.dll` and `avutil-61.dll` next to `quadchroma.exe`. Switching on "Start
with Windows" installs the app into `C:\Program Files\QuadChroma` and starts it from
there at every logon, elevated and without a prompt; the unpacked folder is then no
longer needed for it. On Windows 11 with Smart App Control switched on, unsigned
programs can be blocked without a "Run anyway" option; then only building from source
helps.

**macOS.** Open the DMG and drag `QuadChroma.app` to Applications. On the
first start macOS refuses the app because Apple has not notarised it: open System
Settings > Privacy & Security, scroll down to the message about QuadChroma and click
"Open Anyway", then confirm. The same in Terminal:
`xattr -dr com.apple.quarantine /Applications/QuadChroma.app`. Then grant Screen
Recording and Accessibility (see Requirements). Release files named `-selfsigned` are
signed with the project's own free certificate "QuadChroma Release": both permissions
stay granted across updates. Files named `-unsigned` carry only an ad-hoc signature;
after each update the two permissions must be granted again.

The very first start opens the window (the start screen with the hosts found on the
network); after that the app sits as an icon in the menu bar - four squares, the lower
right one only outlined - and in the Dock only while its window is open. A start at
login always goes silently to the menu bar, even before the window was ever shown. Its
menu opens the window, connects to a host found on the network, shows the Mac's device
ID and access password and the allowed devices, switches "Share this Mac" (on by
default), "Start at login" (offered once the app lies in Applications) and "Prevent
sleep while QuadChroma is running", and is the one place to quit the app. While a permission is missing, sharing keeps
running; the menu says which one and opens its page in System Settings. macOS 15 and
later also ask once for access to the local network.

## Getting started

### One app on every computer

Install QuadChroma on every computer you want to use - the DMG on each Mac, the ZIP on
each Windows PC. There are no separate host and client programs and no extra process
for sharing: each copy sits in the menu bar or the notification area, lets the other
computers in ("Share this Mac" or "Share this PC", on by default, switchable in its menu
and on the start screen) and lists every other QuadChroma computer in the network. From
whichever computer you sit at, open the window and click any other one in the list; in a
session the menu's "Computers" tab (hold ESC for two seconds or press F10) switches to
another computer directly.

Each computer has one device ID and one name, the same in both directions. The name is
the computer's name until you change it with "Change device name ..." in the menu or
"Rename" next to "This computer: <name> · <ID>" on the start screen (1 to 40 bytes,
effective at once). "Prevent sleep while QuadChroma is running" (menu and start screen,
off by default) keeps a computer reachable that would otherwise go to sleep; during a
session a host keeps its display awake anyway. The Mac app contains no FFmpeg; only the
Windows download carries the two FFmpeg DLLs.

### Mac

    make
    open -n build/QuadChroma.app --args --serve 9001 --fps 120 --mbit 50 --fest

Without arguments - a double-click in the Finder, or as a login item - the app shares
the Mac on port 9001 with the default values (unless "Share this Mac" is off). Only
one app runs per user: a second start or a double-click on the running app opens its
window. Full screen (F11, the default) hides the menu bar and the Dock completely, also
at the top edge; they come back when full screen is left, the window is hidden or
another app comes to the front (Cmd+Tab). On the first start grant the two permissions
listed above. `make` signs the bundle
with a local development certificate so that the Screen Recording permission survives
a rebuild. The host captures its main screen; the client can choose another one in
its menu, and `--display n` pins entry `n` of the `--list` output for this run (see
"The host's screen"). `--fest` keeps the rate of `--fps` even when the screen is still;
without it frames go out only on changes (the client can switch this in its menu and
stores it per host). `--mbit` is a cap the encoder does use: 150 Mbit/s means
150 Mbit/s with motion, so outside your own network 25 to 50 is the better choice.
Logs: `host-protokoll.txt` (sharing) and `protokoll.txt` (connecting to others) in
`~/Library/Application Support/QuadChroma/`; above 8 MB the host log moves to
`host-protokoll.alt.txt` and starts anew (a few audio and clipboard lines still go to
`/tmp/quadchroma-m1.log`). Keys and lists: `~/Library/Application Support/QuadChroma/`
(`host.key`, `host-devices.txt` with the allowed devices, `host-password.txt` with
the access password, `bildschirm.txt`).

### Windows

    quadchroma.exe
    quadchroma.exe 192.168.178.194:9001

Every start asks for administrator rights (UAC, see Requirements); from a terminal,
start it in one opened with "Run as administrator". Without an address the very first
start opens the start screen; the Mac appears in the list after a few seconds with its
name and device ID, and a click connects. The address field also takes a device ID.
Later starts go silently to the notification area, a start through "Start with Windows"
always - a click on the icon, or starting `quadchroma.exe` again, opens the window.
Closing the window does not quit (see "Closing, single instance, desktop shortcut").

The client's files live in `%APPDATA%\QuadChroma\`: `host.key` (the device key, which
the client uses as well), `hosts.txt` (the hosts that let this PC in),
`einstellungen.txt` (settings), `protokoll.txt` (the log, restarted at every start),
`benchmark.txt`. The log records what the client decides
(decoder, display, pointer shape) and what FFmpeg reports about it.

### Sharing a Windows PC

The same `quadchroma.exe` is also the host - there is no second program and no second
process for it: the host role runs inside the app, and
"Share this PC" - on by default, a toggle on the start screen and a check item in the
icon's menu - decides whether it listens. The one icon in the notification area opens
the window with a left click; its menu (right click) has the same host items as the Mac
host - device ID, access password, allowed devices - plus "Change device name ...",
"Share this PC", "Start with Windows", "Prevent sleep while QuadChroma is running"
and "Quit". On Windows the app asks for administrator rights once via UAC at every
start; this is required so that the host role can control windows with higher rights
(Task Manager, the registry editor, installer windows) - without it Windows drops the
injected mouse and keyboard events whenever such a window is in front. The one thing
that stays out of reach is the UAC consent prompt itself, on Windows' secure desktop
(that would need a system service). A viewer used only as a client still gets the UAC
prompt at launch.

"Start with Windows" (menu and start screen) is therefore a scheduled task that runs
at logon with highest privileges, so the app starts elevated without a prompt - not a
Startup-folder shortcut, which Windows would not launch silently for an elevated exe.
The task also starts and keeps running on battery, has no time limit and runs at normal
priority, like a start by hand (a task from 0.1.0 with Windows' defaults is corrected at
the next start).
Every account has its own task, "QuadChroma (<the account's SID>)": two administrator
accounts on one PC switch their autostart independently, and check box and menu item
show only the task of the account the app runs under. Because that task runs a
program as administrator without asking, it never points at the folder you unpacked
(Downloads, Desktop, a network share), where any program with normal rights could swap
the exe or a DLL: switching it on first installs the app - exe, the two DLLs and the
text files - into `C:\Program Files\QuadChroma`, where only administrators can write,
checks folder and files, and points the task at that copy; the start screen (or,
without a window, a balloon at the icon) says "Installed to C:\Program
Files\QuadChroma – QuadChroma starts from there with Windows." What gets copied cannot
be swapped: from the moment `quadchroma.exe` starts until it quits, the exe and the two
DLLs it loaded are locked - no program can rename, replace or delete them - and each is
compared with the code actually loaded before it is copied; if that fails, nothing is
installed. Switching it off deletes the task and leaves the installed copy; to remove
it, delete the folder. When "Start with Windows" is on, every start of a
`quadchroma.exe` whose files differ from the installed copy replaces it, unless the
installed copy is a newer version - an older `quadchroma.exe` never replaces a newer
one; a task of an earlier version that still points elsewhere is moved to it, and the
single task "QuadChroma" that earlier versions created is taken over if it belongs to
your account. If that fails, a task that would still start the app from a folder
others can change is switched off, and the start screen says that "Start with
Windows" could not be changed. To update, quit QuadChroma in the icon's menu and start
the new `quadchroma.exe` once. On a standard account (the app then runs with an
administrator's credentials, not as the signed-in user) the check box and the menu item
are disabled with "Only with an administrator account". `quadchroma.exe --autostart
on|off` (also `an|aus`) does the same from a terminal started as administrator; its
output can be redirected to a file.

`quadchroma.exe --host` starts the app in the background with sharing on
(shortcuts of earlier versions keep working). The host role's files are `host.key`,
`host-devices.txt`, `host-password.txt` and `host-protokoll.txt` in
`%APPDATA%\QuadChroma\`. What it can and cannot do yet is under "Platforms and status";
details in `MANUAL.txt`, "Windows: one app for both directions" and "Windows as host".

### Pairing

A host lets a device in once; after that the device comes straight in. There are no
command-line switches for this. The first time a client connects, the host answers
that it does not know the device yet, and the client shows a dialog with two ways in:

- **Password.** Type the host's access password. The host shows it in its menu (menu
  bar on the Mac, notification area on Windows): nine characters such as
  `k7m-4wq-9tz`, made up at random on the first start; "Change password …" sets your
  own (at least 8 characters). Spaces, hyphens and the case of A-Z do not matter. The
  password never crosses the network: the client sends a proof derived from it that
  is valid for this one connection, and the host proves in return that it knows the
  password as well - otherwise the client stops and remembers nothing.
- **Allow.** Someone at the host clicks "Allow" in the window that appears there. It
  shows the client's name, its device ID and a six-digit comparison code such as
  "628 306"; the client's dialog shows the same code. If both match, nobody sits in
  between.

It works the other way round, too: a client that does not know the host's key yet (the
first connection, or after `hosts.txt` was deleted) asks the host to prove itself, so
even a host that already knows the device goes through the password or "Allow" step.
A host that just answers "known" is refused ("… did not prove its identity …") and not
remembered.

Afterwards the host lists the device under "Allowed devices" in its menu, where
"Remove" takes it out again; the client remembers the host by its key in `hosts.txt`,
whatever address it has, and marks it with a check mark in the host list. Every
computer has one key, `host.key`, which it uses in both directions, and a nine-digit
device ID derived from it, shown in the menu, on the start screen ("This computer") and
in the other computers' host lists; the address field, a desktop shortcut and the command line accept
it instead of an address. Wrong passwords are throttled (from the third on 5 s,
doubling up to 300 s), five in one connection end it, and an attempt that is refused,
fails or gets no answer ends with a message on the start screen instead of an
automatic retry. A key or list file that exists but is unreadable or damaged never
lets anyone in and is never replaced silently. All of it in detail - messages, files,
log lines, the wire format - is in `MANUAL.txt`, "Encryption and access".

Earlier pairings stay valid: at its first start a host takes over `authorized.txt`
into `host-devices.txt`, a client `known_hosts.txt` into `hosts.txt`, and the old
files are renamed to `*.migriert`. Versions before the one app called with a second key,
`client.key`; it stays on disk unused and is never changed. A computer that knew this
one only by that former key therefore asks once more for its password or "Allow" -
the access dialog says why - and afterwards knows the device key. A client of an earlier version that a new host does
not know yet cannot answer its access request, and a new client reports a host of an
earlier version that does not know it ("… uses an older QuadChroma version"): update
both sides.

![Statistics panel (F9) during a session with HEVC 4:4:4 at 10 bits, 1920x1080, decoded by NVDEC; the last line shows the six-digit comparison code.](session-stats.png)

## Using the client

### Start screen

The list shows every QuadChroma computer that announces itself in the network, except
this one: its name on the left, its device ID on the right ("ID -" for a host of an
earlier version), and a check mark for a computer that has let this one in before;
hovering shows the address. Known computers come first, then the others by name; a
computer heard under several addresses appears once, and the rows keep their order
while others come and go. Four rows are visible, the mouse wheel or trackpad scrolls
the rest. A click connects, and on Windows the "Desktop shortcut" button puts a
shortcut to that host on the Desktop. The address field takes an IP address, a name or
a device ID (nine digits, spaces or hyphens allowed) and pastes with Ctrl+V (Cmd+V on
the Mac). "Share this PC" ("Share this Mac") switches sharing of this computer on and
off, and below the buttons stand "This computer: <name> · <ID>" with "Rename", the
check box "Prevent sleep while QuadChroma is running" and directly below it "Start with
Windows" (on the Mac "Start at login"; disabled with its reason while the app is not in
Applications, on Windows on a standard account) - the same state as the item in the
icon's menu. The button on the right, "Close window", hides the window like its close
button; quitting is in the icon's menu (without an icon the button reads "Quit" and
quits). While the access dialog is open (see "Pairing"), Enter connects, Esc cancels,
and no key reaches the host.

### Keys

| Key | Effect |
|---|---|
| F9 | statistics on and off |
| F10, or hold ESC for two seconds | menu: Picture, Display, Encryption, Shortcuts, Benchmark, Computers |
| F11 | full screen on and off (on the Mac without menu bar and Dock) |
| F12 | pixel-exact rendering instead of scaled |
| Ctrl+Esc | back to the start screen |

Every key function is also a switch in the menu. All other keys go to the Mac. What
travels is the key's position, not the character, so umlauts, accents and AltGr work
without any mapping; the Windows key is the Mac's Command key. While the menu is
open, mouse and keyboard belong to the menu, not to the Mac.

### Menu

"Disconnect" sits in the menu's header at the right, on every tab, with the host's
address below it.

- **Picture:** bit rate, frame rate, gaming mode, fixed frame rate, sound, the host's
  screen (see "The host's screen") and the codec.
- **Display:** full screen, pixel-exact, statistics, nerd mode, which statistics
  lines, and one row each for the display and for the decoder with the roles
  Automatic, Graphics card, Integrated and Processor - useful on laptops with Intel
  graphics and an NVIDIA card. The client detects the cards at start (shared memory
  means integrated), shows only the roles for which it found a card (Automatic and
  Processor always) and names the detected card in the tooltip. The decoder switches
  at once, the display from the next start.
- **Encryption:** method, comparison code, fingerprint, desktop shortcut.
- **Shortcuts:** every key the client intercepts.
- **Benchmark:** see below.
- **Computers:** the same list as on the start screen - every QuadChroma computer in
  the network with name, device ID and check mark, this one left out - with the
  current host marked "Connected". A click on another computer ends this session and
  connects there, with the access dialog if that computer does not know this one yet;
  a click on the current one or ESC keeps the session.

Hovering over a switch shows a one-sentence explanation. The host applies a change
immediately and reports back what is actually in effect; the client stores the values
per host (by fingerprint) and restores them on the next connection. The choice of
screen is stored by the host itself.

![The menu (F10, or hold ESC) with its six tabs - Picture, Display, Encryption, Shortcuts, Benchmark, Computers - and Disconnect at the right of the header, above the host's address. The Picture tab is open: latency and frame rate, maximum bit rate and frame rate, gaming mode, fixed frame rate, sound, the HDR switch lit gold with a sparkle (HDR active on both sides), the host's screens (Automatic and two screens) and the codecs the host offers.](hud.png)

### Codecs

The Picture tab lists what the host's encoder really supports (the host queries its
encoder at start); on the M1 that is HEVC in 4:4:4 and 4:2:0, each in 8 and 10 bit,
and H.264. Switching happens while running, the picture stands still for a blink; two
modes are marked "converted" because the capture has no 8-bit 4:4:4, so the Mac
converts first, which costs one extra pass. AV1 is not offered yet.

### Benchmark

The Benchmark tab runs codecs, frame rates and bit rates in sequence, 3 to 15 seconds
per step, without stopping the picture: arrived frame rate, the whole chain in
milliseconds, dropped frames, the host's encoder time against its budget, CPU load on
both sides. A step passes when at least 95 % of the target frame rate arrives, under
1 % is dropped and the chain is at most 30 ms. On request the host sends a fixed
moving test pattern so that every step sees the same content. At the end a
recommendation can be applied with one click; the table is written to
`%APPDATA%\QuadChroma\benchmark.txt`. Headless:
`quadchroma.exe <address> --headless --benchmark 5`.

### Test mode

`quadchroma.exe <address> --headless` runs without a window and prints a status line
every three seconds; with `--passwort <password>` it answers a host's access request
once, with `--hdr-schirm hdr|sdr` it reports an imagined HDR or SDR screen to the host
and names the HDR state the window would show; `--decodertest` tries every decoder
choice without a connection, `--anzeigetest
<dir>` checks the GPU display path against the CPU path (Direct3D 11 on Windows; Metal
on the Mac, where it also measures the frame time at 1440p and 120 Hz), `--shot` writes
a BMP of the interface. `MANUAL.txt` lists all switches.

On Windows run these - and every other command-line switch - from a terminal started
as administrator ("Run as administrator"): the exe requires administrator rights, so
from a normal terminal Windows refuses the start ("The requested operation requires
elevation") or opens it elevated in a new process whose output does not appear in
that terminal.

## The host's screen

The host streams exactly one screen. The client chooses which one in the menu:
Picture tab, row "Screen", above the codec buttons (the Display tab is about the
client's own display). The row holds an "Automatic" button and one button per screen
of the host with name, size and refresh rate, such as "X27 X1 · 1920×1080 · 120 Hz"
(without the Hz part when the host does not know the rate). The streamed screen is
shown in cyan, together with "Automatic" when that is the choice, or with the chosen
entry if it is connected. Clicking the entry that is already the choice does nothing;
under Automatic the streamed entry is not the choice, so clicking it pins it. Until
the host has answered, "Switching screen …" appears in amber on the right; with the
menu closed the same hint appears over the picture as during a codec switch (at most
5 s; if both run, the box shows the codec switch). The row exists only with a host
that supports the choice (bit 1 of its capabilities); an older host never receives a
request, and an older client ignores the list.

- **Automatic** (default): the host streams its main screen - on the Mac the one with
  the menu bar, on Windows the monitor Windows lists as primary - and follows it when
  it changes, without a restart and without a new viewer. The reason for this default:
  on 26 September a monitor had been connected to the Mac mini and had become the main
  screen; the host kept streaming the virtual screen chosen at start, on which no
  window lay - the picture was empty, and mouse and keyboard seemed dead.
- **Fixed:** the host streams the chosen screen, recognized by a stable identifier,
  never by list position: on the Mac `v<vendor>-m<model>-s<serial>` (with `-u<unit>`
  appended when two are identical), on Windows the second part of the monitor's
  device ID such as `ACR0501` (with `-<list position>` when two are identical). If it
  disappears, the host streams the main screen as a fallback and the menu says so in
  amber ("<identifier> not connected – fallback: <name>"); when it returns, the host
  switches back by itself. The choice applies host-wide (one stream, the last request
  wins) and stays until someone chooses something else: it is stored in
  `bildschirm.txt` in the host's data folder, one line, `auto` or the identifier. If
  the file is missing or broken, Automatic applies; a broken file is never replaced
  silently.
- `--display N` (Mac) or `--output n` (Windows host role) pins list position N for
  this run (`--list` shows it with identifier and name) without changing the file; a
  choice from the menu overrides the pin and is stored.

macOS reports a monitor change to the Mac host at once; the host re-evaluates 300 ms
after the last report (debounced: three reports in quick succession are one change).
The Windows host role enumerates its outputs every 2 s, and immediately after losing
the Desktop Duplication. The switch itself works like a codec switch: the old stream
stops, the new one starts on the target screen, and the viewer receives message 7
(SWITCH with the current codec, so that the client rebuilds its decoder), then 1
(INFO with the dimensions), then the new list (message 12), then the first frame as a
keyframe; the mouse follows the picture, which stands still briefly. If the new
screen has a different size, a new encoder is created; with the same size the encoder
stays on the Mac, while on Windows only an encoder on the CPU path stays (one on the
texture path is bound to the device of the old Duplication). The client also rebuilds
its decoder on an INFO with new dimensions and waits for the keyframe. While a codec
switch is in progress, the screen switch waits until it is done (the Mac host switches
after 5 s regardless).

Unplugging the streamed screen is not a switch but a loss: the stream ends, the host
reports "no screen" (message 9) and rebuilds the stream - the Mac after 2 s, the
Windows host role every 2 s - on the screen then selected, which with a fixed choice
is the fallback. SWITCH and INFO reach the viewer from the Mac only when the stream
size changes, from the Windows host role whenever it is another screen; the Mac then
logs no `Bildschirmwechsel` line but `Aufnahme wiederhergestellt` ("capture
restored").

On a switch both hosts log `Bildschirmwechsel: <old> -> <new> (<reason>)` ("screen
switch") with one of the reasons `Wunsch des Zuschauers` (the viewer's request),
`Wunsch des Zuschauers: Automatik` (the viewer chose Automatic),
`Hauptbildschirm gewechselt` (the main screen changed), `Ausweichplatz` (fallback) or
`zurueck zum gewuenschten Bildschirm` (back to the requested screen). All log lines,
messages 12 and 70 in detail and the test mode (`--bildschirm <identifier|auto>`, the
field `Strom` in the status line) are in `MANUAL.txt`.

## HDR

New in 0.2.0: HDR10 in both directions - Mac to Windows, Windows to Mac, and between
two computers of the same kind. The stream is then HEVC with BT.2020 primaries, the PQ
curve (SMPTE ST 2084) and 10 bits, in the full range like every QuadChroma stream, as
HEVC 4:4:4 or 4:2:0 at 10 bits; the host sends the mastering values along. Everything
else stays as it is: when either side cannot do HDR, the picture is the same SDR as
before.

HDR runs when all of this holds, and the host decides alone:

- The host's streamed screen shows HDR: on the Mac a screen with HDR headroom (an XDR
  display, an HDR monitor with HDR switched on, or a virtual display with HDR), from
  macOS 15 on; on Windows a screen with "Use HDR" switched on.
- The host's encoder can do HDR10: the Media Engine of Apple silicon on the Mac, NVENC
  on an NVIDIA card on Windows (the host checks this at start).
- The codec is HEVC 10-bit (4:4:4 or 4:2:0). 8-bit HEVC and H.264 stay SDR.
- The viewer's window is on an HDR screen ("Use HDR" on Windows, a screen with EDR
  headroom on the Mac), drawn through the graphics card, and its HDR switch is on.

The **HDR switch** sits in the menu, Picture tab: on (the default) uses HDR whenever
both sides can, off always sends SDR. The client stores it per host, like the other
values of the tab. Next to the switch stands what is happening: "active on both
sides", "HDR → SDR" (the host sends HDR, this screen shows it as SDR), or "SDR" with
the reason (this screen is SDR, needs HEVC 10-bit, the host's screen is SDR, the host
cannot send HDR, ...). When HDR is active on both sides, the switch glows golden with
moving sparkles, a golden sparkling "HDR" badge appears over the picture for about
three seconds, and the statistics (F9) show "HDR10 · PQ" in gold. One side alone is
never golden.

Brightness is relative: the host's SDR white becomes the viewer's SDR white, so a
desktop looks as bright as locally; everything brighter is mapped onto the headroom of
the viewer's screen with a smooth knee above SDR white, and passed through unchanged
where it fits (on Windows bit-exact when both sides use the same SDR white). On an SDR
screen, everything above SDR white is clipped without changing its hue, so SDR content
of an HDR source comes out exactly as before. The client reports its screen to the host
(message 71) when the session starts and whenever the window moves to another screen
or the switch changes; the host then switches between HDR and SDR like a codec switch
(the picture stands still for a blink, at most once every 2 s), and in between the
client maps whatever arrives. A Windows desktop with "Use HDR" that is streamed to an
SDR viewer is converted to SDR on the host's GPU - before 0.2.0 such a desktop could
not be streamed at all.

Limits: no HLG, no dynamic metadata (HDR10+, Dolby Vision); the mouse pointer stays
SDR; a computer with QuadChroma 0.1.0 on either side gets SDR. HDR has been checked in
test harnesses, offscreen golden-image tests and across Mac and Windows with recorded
HDR10 clips, not yet on real HDR screens (see "What is missing"). `MANUAL.txt` has the
details: every log line, messages 1, 7, 11 and 71, the test mode `--hdr-schirm
hdr|sdr` and the Mac tool `scripts/hdrprobe.sh`.

## Files via the clipboard

Copy files or folders in Explorer or Finder and paste them on the other side - in both
directions, between the client (Windows or Mac) and either host (Mac or Windows host
role), over the existing encrypted connection. Unlike RDP, the transfer starts when
you copy, not when you paste: the other side writes the files into its own folder in
the temp directory (`QuadChroma-Ablage`, on the hosts `QuadChroma-Host-Ablage`) and
puts them into its clipboard as a file list once everything has arrived. The three
newest transfers are kept there; at start, whatever is older than 24 h or was left
half-received by an earlier run is removed. In the client a thin line at the bottom
of the picture shows progress.

- Limits: 4 GB and 10,000 entries per copy, the list of entries up to 1 MiB.
- Picture, audio and input come first: at most 256 KiB are in flight unacknowledged
  (64 KiB in gaming mode and in the Windows host role), on the input channel at most
  one file packet waits behind the keystrokes, and the transfer threads run at lower
  priority.
- New clipboard content, the end of the session and 30 s without progress abort a
  transfer; half-received data is deleted.
- The receiver checks paths, lengths and order as if they were hostile and sanitises
  names its system does not allow. Links (symbolic links, on Windows also junctions)
  are neither sent, followed nor created.
- Older counterparts receive nothing: each side announces when connecting whether it
  handles files (capabilities, messages 11 and 69).

Text travels too (up to 4 MB). Entries a program marks as concealed, as password
managers do, stay on the computer where they were copied, text and files alike.
Received content is kept out of clipboard history and cloud clipboards, and the
clipboard is read only while a counterpart is connected.

On the Mac, macOS may ask for access the first time you copy from Desktop, Documents
or Downloads, and from macOS 15.4 also when QuadChroma reads the clipboard. Throughput
figures are under "Measurements"; the progress line, the log lines and messages 50 to
53 in detail are in `MANUAL.txt`.

## Closing, single instance, desktop shortcut

Closing the window - its close button, "Close window" and ESC on the start screen, on
the Mac also Cmd+W and Cmd+Q there - disconnects a running session and puts the client
away: on Windows as an icon in the notification area (sharing keeps running), on the
Mac in the menu bar. The icon brings the window back, connects to a found host through
its menu, or quits the program - only its menu quits; `tray=aus` (off) in
`einstellungen.txt` restores close-to-quit. Only
one client with a window runs per user session: a second start hands its address to
the running one and exits.

The desktop shortcut (Windows only) builds on this. The "Desktop shortcut" button in
each host row of the start screen, or in the Encryption tab of the menu, puts
`QuadChroma - <name>.lnk` with the program icon on the Desktop. A double-click
connects at once, even when the client already sits in the notification area. Without
a window:

    quadchroma.exe --verknuepfung <address> [--name <name>] [--ordner <folder>]
                   [--id <id>]

The shortcut carries the full path of the exe, the host's address and - if the host
announces one - its device ID (`--verbinden "<address>" --id <id>`). With the ID the
client finds the host under a new address as long as it announces itself in the same
network, and it checks that the key behind it gives that ID. After moving the exe,
create the shortcut again.

The client draws its program icon, four coloured squares, itself: on Windows for the
title bar, taskbar and notification area, on the Mac in monochrome for the menu bar.
Each shortcut points to a copy the client writes to
`%APPDATA%\QuadChroma\quadchroma.ico`; the same icon is embedded in the exe.

## Interface

Dark background, fine grid, neon lines in cyan and magenta, corners drawn as brackets.
Everything is drawn by QuadChroma itself, without a third-party widget toolkit. On the
CPU path, picture and interface share one buffer; on the GPU the interface lies over
the picture as a separate layer with transparency - only the changed region is
uploaded, and only when something is visible. The statistics panel can be composed
line by line. Nerd mode widens it and shows the latency chain as a bar on a fixed
millisecond scale - capture and queue, encoding, network, decoding, display - with
marks for one frame and one monitor refresh, plus the load on host and client; a link
is marked as the bottleneck when the chain is longer than one frame and that link
makes up more than half of it.

![Nerd mode: the statistics panel widened, with the comparison code, host and client load and the latency chain as a bar on a millisecond scale.](nerd.png)

## Measurements

Measured on a Mac mini M1 as host:

| | |
|---|---|
| Picture | 1920×1080, 4:4:4, 10 bit, 120 frames/s target; the host caps at that rate |
| Video bit rate | 3 to 11 Mbit/s with a still picture, up to the configured cap with motion |
| Encoder | hardware, about 8 ms per frame, a few percent of one core |
| Codecs | HEVC 4:4:4 and 4:2:0 in 8 and 10 bit, H.264, all in hardware, switchable while running |
| Decoding on Windows | NVDEC in hardware; D3D11VA on AMD and Intel (4:2:0 and H.264); software 8.6 ms per frame, slice-parallel so that no frame is held back (see "Decoding on Windows") |
| Decoding on the Mac | VideoToolbox in hardware, HEVC 4:4:4 at 10 bits included: about 5.6 ms per 1080p frame on the M1, measured in the client (see "Mac client") |
| Display on Windows | Direct3D 11: raw decoder planes go to the GPU, conversion and scaling in shaders, bit-identical to the CPU path |
| Latency | 15 to 20 ms from capture to hand-over to the display, measured with per-frame timestamps |
| Audio | uncompressed, stereo, 48 kHz, about 3 Mbit/s; can be switched off |

File transfer, measured in the integration test of the final state (commit e134fc6,
24 September 2026):

- On the VM without a GPU (software encoder; Windows host role and client on the same
  machine; 1 GiB each way, bit-identical): about 30 MB/s from client to host, with
  about 2 frames/s fewer and around 60 ms more encoder delay - the cause is not yet
  explained, CPU load it is not; 69 to 93 MB/s from host to client without measurable
  effect on picture and delay. In both directions the host reported no congestion and
  no dropped frame.
- In the LAN from the Windows client to the Mac host: 209.7 MB in 7.4 s, that is
  28 MB/s, at an unchanged ~114 frames/s and 30 to 37 ms delay.

The Windows host role has been verified on the VM and in the test harnesses only.

## Security

Every connection is encrypted and mutually authenticated; there is no switch to turn
that off. QuadChroma uses the Noise pattern XX with X25519, ChaCha20-Poly1305 and
SHA-256: both sides identify themselves with a permanent key, every connection gets
fresh session keys, and the handshake yields a six-digit comparison code - the client
shows it (F9, menu, access dialog), the host in its "Allow" window and its log. If the
codes match, nobody sits in between.

The video channel is set up first. The input channel includes the video channel's
handshake hash in its own handshake, and without that hash nobody gets in. So nobody
can take over the Mac's keyboard without first having set up the video channel
legitimately. The hash identifies only the session, not the host; the client
therefore also compares the key of the input channel's peer with that of the video
channel, during the handshake and before identifying itself, and keystrokes,
clipboard and files go only to that same host. The host binds the input channel to
exactly one viewer: when a new viewer replaces it or its picture breaks off, the host
cuts the channel and releases that viewer's pressed keys and mouse buttons.

A host lets a device in only if its full key is in the host's list of allowed devices,
or if it proves the host's access password, or if someone at the host clicks "Allow";
see "Pairing" under "Getting started". The password stays on both machines: the client
derives a key from it with PBKDF2-HMAC-SHA256 (100,000 rounds, salted with the host's
public key) and sends an HMAC proof bound to the handshake hash of this very
connection; the host answers with a proof of its own, which the client checks before
it remembers the host. Proofs are compared in constant time, and a replayed proof is
worthless on another connection. Wrong passwords are throttled per address and per key
(from the third on 5 s, doubling up to 300 s; after more than 30 failures in 60 s
every new attempt waits at least 60 s), and a proof that comes too early is not even
checked. An unknown device waits in an access phase of at most 120 s that takes no
lock: at most 4 of them at a time, 2 per address and 1 per key, and until it is let in
it touches no running viewer and gets no input channel. The "Allow" window does not
take the keyboard, and its "Allow" button stays disabled for the first second, so that
a stray click or Return lets nobody in.

Every handshake has an overall deadline (host 5 s, client 3 s) and runs in its own
thread on the host, at most 32 at a time per port and only a few per sender; whoever
stays silent or trickles holds only their own slot. What anyone on the network can
trigger without being let in - failed handshakes, access requests and wrong passwords,
input channels without a picture - goes into the host log rate-limited: at most one
line per 10 s per kind and address, with the number of suppressed lines (counted per
address for up to 4 (Mac host) or 64 (Windows host) addresses per kind, beyond that
together). Both hosts cap their log at 8 MB and move the older part to an `.alt` file.

One viewer at a time: when another allowed device connects, it takes over the
session. The previous one first receives message 10, shows "Another device has taken
over the session." and does not reconnect by itself. The new viewer inherits nothing
from the old one: a test pattern ends, and it gets a first frame at once, even on a
still screen.

The host says goodbye before it closes a session on purpose (message 13): the client
goes back to the start screen and shows "QuadChroma was closed on <name>.", "<name>
stopped sharing." or "<name> removed this device." - again without reconnecting by
itself.
If a connection just drops, the client retries for about 30 s and then shows
"Connection lost" on the start screen.

On the Mac the Noise pattern is implemented in-house (about 400 lines of C on
Monocypher); the Windows side uses the established Rust implementation `snow`. The two
were checked against each other: same comparison code, same fingerprints, encrypted
exchange in both directions. The proofs of the access phase use CommonCrypto on the
Mac and a small HMAC and PBKDF2 on top of the `sha2` crate in Rust; both sides check
the same test vectors.

Known limitations: the network announcement - name, device ID, whether "Allow" is
possible - is not authenticated, and names are only for display. Connecting by device
ID (a row of the host list, a desktop shortcut) is the safer way: before identifying
itself the client checks that the host's key gives that ID and, for a host it knows,
that it is exactly the remembered key. Connecting by a bare address (typed IP or
name), the client accepts whatever key answers there as soon as that side lets it in:
only the password answer is a proof - "Allow" can come from any device that answers in
the host's place. The plain statement that the device is already known counts only
from a remembered key: to any other key the client says in the handshake that it does
not know it (bit 0 of message 3), the host then has to go through the password or
"Allow" step, and a host that lets the client in without it is refused and not
remembered. A different key at a remembered address shows up in the access dialog and
in the client's log, and the comparison code tells the real host from an impostor,
but nothing stops a connection that is let in by "Allow". The device ID has nine digits and is
not a secret; it serves display and search. The access password lies in plain text in
`host-password.txt`, because the host has to show it, and whoever watches the host's
screen - a connected viewer included - can read it in the menu. The clipboard is read
during a session even when the client window has no focus (see "What is missing").
On Windows the whole app runs with administrator rights (see Requirements), so a flaw
in it weighs more there; "Start with Windows" therefore starts only the copy in
`C:\Program Files\QuadChroma` that only administrators can change, copied only from
files that are locked while the app runs and match the code it loaded, and never
replaced by an older version (see "Sharing a Windows PC"). Report vulnerabilities as
described in `SECURITY.md`.

## What is missing

**Verification on real hardware.** Covered on the VM and in the test harnesses, still
open on real devices:

- The Windows host role with an NVIDIA card (NVENC via bgra, yuv444 and d3d11, test
  pattern and codec switch on d3d11, an AV1-capable card), at 125 and 150 % scaling,
  on rotated outputs and handhelds with a portrait panel, and with a real audio
  device.
- HDR (new in 0.2.0) on real HDR screens: the Mac host's HDR capture (the tool
  `scripts/hdrprobe.sh` measures what ScreenCaptureKit delivers - HDR values, the
  level of SDR white), NVENC in HDR10 and the Windows host role on an HDR desktop, the
  HDR10 swapchain of the Windows client and the EDR layer of the Mac client, moving the
  window between HDR and SDR screens, cost and banding. Covered so far by unit tests,
  `hosttest` (HDR10 with the real VideoToolbox encoder and decoder), offscreen golden
  images on both clients, the converter on WARP, and recorded HDR10 clips streamed by
  the Windows host role to a Mac and a Windows client in test mode.
- The congestion rule of both hosts on a real link that is too slow (also below the
  audio rate) and in gaming mode at a high bit rate.
- On the running Mac host - where picture, input channel, audio and the conversion
  after a codec switch with a format change are verified, all at a fixed frame rate:
  the codec switch on a truly still screen, the viewer takeover with message 10,
  re-sending the last frame on a still screen, an audio format of 44.1 kHz, clipboard
  managers, a damaged device list.
- The access with password or "Allow": covered by the same test vectors on both sides,
  `zugangtest`, `menuetest`, the access sections of `hosttest` and loopback tests in
  Rust (the client against a test host, the Windows host role against a test client).
  Still open on real devices: the Mac app's menu bar, its windows, the sharing
  switch and "Start at login" with the project's own certificate; whether a Screen Recording permission
  granted while the host runs takes effect without a restart; the Windows host role's
  icon, windows and "Start with Windows" on an interactive desktop (the VM is driven
  over ssh, without Explorer; the install into Program Files, the locked source files,
  the version check and the scheduled tasks were checked there through `--autostart`
  and a start with `--hintergrund`), including the lock on a real standard account
  (only its decision logic is covered by unit tests) and a second administrator account
  signed in at the same PC (tasks of other accounts were only simulated, with SYSTEM as
  their principal); and a Windows client
  against the Mac host through the access phase.
- Screen selection on the device: for the Windows host role the switch itself (new
  Duplication, SWITCH, INFO, keyframe, list afterwards; with the same size the encoder
  on the CPU path stays) - it needs two real outputs and the VM has one; the pure parts
  are covered by unit tests - and there also identifier, name and refresh rate from
  real monitors; for the Mac host the live change of the main screen including names
  from AppKit, which `hosttest` covers with mocks for list and stream. Verified on the
  device on 26 September 2026: a request for the Mac host's second screen with the
  switch, the mouse on the requested screen via `--eingabeprobe`, and back with
  `auto`.
- The Windows client with two NVIDIA cards, on audio loss, with an audio device that
  appears only after connecting, and on a fresh Windows without the Visual C++
  runtime; the Mac client with Handoff and clipboard managers.
- Files, desktop shortcut and notification area are covered by two integration
  tests - on the VM with Windows host role and client, and client to Mac host in the
  LAN, last at commit e134fc6 - plus `dateitest`, `ablagetest` and the file sections
  of `hosttest`. Covered only in test harnesses and unit tests: the single instance
  while the first app is just quitting, and quitting when macOS does not show the
  menu-bar icon. Still open: copying with Ctrl+C and pasting in Explorer by hand on
  the laptop (on the VM a script copied via .NET and a shell command pasted), pasting
  in Finder on the Mac, files from the Mac host to the client during operation, the
  Mac client with files, notification area and menu bar in real use (clicks, menu,
  balloon or bubble, the window really in the foreground after a double-click on the
  shortcut), the shortcut via the button on a real Desktop, including one redirected
  to OneDrive, and throughput and latency after the latest fixes (a 64 KiB window for
  the Windows host role, threads with lower priority; the Mac client sends under the
  QoS class "utility", which stretches deadlines and sleeps considerably - in the
  code's model 20 MB in 4.1 instead of 1.1 s).

**Publication.** License, notices file, contribution rules and release workflows are
ready (see "License and publication"). Missing are the certificates: an Apple
Developer ID for signing and notarising the Mac app, which is signed with the
project's own certificate "QuadChroma Release" and not notarised, and a code signature
for the exe. The one-time steps are in `RELEASING.md`.

**Known limitations.**

- No access before sign-in: after a restart or a logout a computer is reachable only
  once a user has signed in on it - QuadChroma starts at login ("Start with Windows",
  "Start at login"), not as a system service. The login screen and the Windows UAC
  consent prompt on the secure desktop cannot be controlled; while such a prompt is in
  front, the viewer shows a short hint and has to wait for someone at the PC.
- Windows: the app runs with administrator rights and asks via UAC at every manual
  start; on a standard account every start needs an administrator's password and
  "Start with Windows" is not available (see Requirements).
- Not code-signed and not notarised: SmartScreen warns once on Windows, and Smart App
  Control can block the exe entirely; macOS refuses the first start until "Open
  Anyway" (see "First start of a downloaded release").
- Connecting by a bare address trusts the answering side more than connecting by
  device ID, and the access password lies in plain text on the host (see
  "Security").
- The clipboard crosses over during a session even when the client window has no
  focus - copied files immediately and up to 4 GB, even if they are never pasted on
  the other side. On all sides it is read only while a counterpart is connected;
  without a viewer the Mac host only counts that something has changed.

**Planned or conceivable.**

- Transfer files only on paste (like RDP) instead of immediately on copy; send the
  list of entries in parts instead of in one piece (up to 1 MiB, during which
  keystrokes or frames wait briefly).
- A desktop shortcut on the Mac client.
- Windows host role: AMF/QSV encoders, scaling and rotation on the GPU for SDR (both
  run on the CPU today; in HDR the GPU does them already).
- HDR: HLG, dynamic metadata (HDR10+, Dolby Vision), an HDR pointer.
- AV1: no host offers it until protocol and client support it.
- Audio compression as an option; uncompressed audio can take more bandwidth than the
  picture.
- A virtual microphone on the host, so that programs there can hear the client.
- Several viewers at once.
- Stream size as the client wishes (it reports its window size, the host adjusts the
  stream); today the size changes only with the host's screen, and the client follows.
- Display: a choice between immediate and vsync presentation in the menu, a fresh
  window after device loss; from 4K on, the zero-copy path (the NVDEC frame stays on
  the GPU).
- D3D11VA without a copy: the decoded frame stays on the GPU that draws (today it is
  copied via the CPU).
- Conceivable later as add-ons: streaming both of the host's screens on request; a
  relay to which further viewers attach (picture and audio, no input).

## Building from source

### Windows client and host role

Rust with the MSVC toolchain, LLVM (for bindgen) and the FFmpeg libraries built by
`scripts/build-ffmpeg-windows.sh`: a minimal LGPL build of FFmpeg 9.0.2 without any
external library, cross-compiled with MinGW-w64 on macOS (Homebrew: `mingw-w64`,
`nasm`, `pkgconf`) or Linux (`mingw-w64`, `nasm`, `make`, `pkg-config`). The script
downloads the FFmpeg release and the NVIDIA codec headers, checks their SHA-256 and
writes the result (DLLs, headers, import libraries) to the folder given as argument:

    scripts/build-ffmpeg-windows.sh ffmpeg-windows

Copy that folder to the Windows machine and point `FFMPEG_DIR` to it:

    set FFMPEG_DIR=C:\path\to\ffmpeg-windows
    set LIBCLANG_PATH=C:\Program Files\LLVM\bin
    cd client
    cargo build --release

Build from `client\`: only there does `client\.cargo\config.toml` apply, which links
the C runtime statically into the exe (otherwise it would need `VCRUNTIME140.dll`
from the Visual C++ Redistributable); a `RUSTFLAGS` environment variable replaces
that setting silently, so the build aborts with a hint if the static runtime is
missing. `client/build.rs` embeds version information, icon and manifest from
`client/res/` using `rc.exe` from the Windows SDK or `llvm-rc`; without either, the
exe is built without them and cargo prints a warning. Put the two DLLs named under
Requirements (from `ffmpeg-windows\bin`) next to the exe. Prebuilt FFmpeg builds from
the internet are not suitable for passing on: most of them include external
libraries, some of those under the GPL. Tests: see "Test harnesses".

### Mac app

The Xcode Command Line Tools and Rust are enough; a full Xcode installation is not
required. `make` runs `cargo build --release --locked` in `client/` and builds
`build/QuadChroma.app` from the result (the binary becomes `Contents/MacOS/quadchroma`),
signed with the local development identity named in the Makefile; bundle identifier and
signature are those of the former host app, so granted permissions stay. The host
engine uses only its own code, Apple frameworks (Foundation, AppKit, ScreenCaptureKit,
VideoToolbox, CoreMedia, CoreVideo, CoreGraphics, CoreFoundation, IOKit,
ServiceManagement for "Start at login", SystemConfiguration for the computer name;
CommonCrypto for the access proofs) and the vendored Monocypher; no FFmpeg. The app
icon for Finder, Dock and Cmd+Tab is `client/res/AppIcon.icns` (made from the logo in
`client/src/logo.rs` in Apple's icon grid, all sizes from 16 to 1024 pixels);
`make` copies it into `Contents/Resources` before signing, `host/Info.plist` names it
(`CFBundleIconFile`), `make check-packages` checks it in the app, the ZIP and the DMG,
and the test `icns_res_gleich_logo` fails if it no longer matches the code. `make
host-allein` still builds the former stand-alone host (`host/start.m`) as
`build/quadchroma-host`, for the test harnesses only. The harnesses run without screen
capture, see "Test harnesses".

### Mac client

The client of the app is the same Rust client as on Windows (arm64), with audio via
AudioToolbox and the clipboard via NSPasteboard (text and files); the display runs
through Metal, with the CPU (softbuffer) as fallback. Closing puts it into the menu
bar, into the app's one icon; there is no desktop shortcut on the Mac. It needs no
FFmpeg; the binary alone also runs from the terminal (the permissions then belong to
the terminal, not to the app):

    cd client
    cargo build --release
    ./target/release/quadchroma 192.168.178.194:9001

On the Mac the client decodes with VideoToolbox directly (`client/src/vt_decoder.rs`):
the Annex B stream of the host becomes a format description from VPS, SPS and PPS
plus length-prefixed samples for a `VTDecompressionSession` in the media engine,
which outputs IOSurface-backed pixel buffers in the stream's own layout (`xf44` for
HEVC 4:4:4 at 10 bits, `444f`, `xf20`, `420f`) that the client's YUV-to-RGB path reads
as they are. HEVC 4:4:4 and 4:2:0 in 8 and 10 bits and H.264 run in hardware; on the
M1, HEVC 4:4:4 at 10 bits takes about 5.6 ms per 1080p frame (the decode time of the
client's own statistics). Automatic uses the media engine, Processor uses VideoToolbox
without hardware, and a media engine that cannot decode the stream falls back to the
processor as on Windows. If the media engine fails in the middle of a stream (after
sleep, or a transient decoder error), the client drops the session and starts a new
one with the next key frame; decode errors go to the log at most once per five
seconds. `--decodertest` encodes a short HEVC 4:4:4 sample with the hardware encoder
and decodes it both ways.

The display on the Mac is Metal (`client/src/anzeige_mac.rs`), called through
`objc_msgSend` and the C functions of Metal, QuartzCore and CoreVideo like the rest of
the Mac code, without a new crate. The decoder's pixel buffers go to the GPU without a
copy (`CVMetalTextureCache` on their IOSurface); a shader compiled at startup converts
them with the same 16.16 integer arithmetic as the CPU path (4:4:4 and 4:2:0, 8 and 10
bits, full and limited range), a second one fits the picture into the window with the
same source points and integer weights as the CPU scaler, and the user interface is a
second texture on top. A `CAMetalLayer` below winit's view presents in pixels (HiDPI),
synchronized to the display refresh, with at most two frames queued and never waiting
on the window thread. If Metal cannot be set up, or with `--anzeige cpu`, the client
draws with softbuffer; it also switches to softbuffer while running when no Metal frame
ever reaches the screen of a visible window (`presentedTime`) or the device is lost. A
covered window does not present, and the last frame is presented again once it is
visible. `--anzeige-selbsttest` checks this in a real window: frames on screen, the
layer's geometry, hiding and showing, the fallback (exit code 3: the window was never
visible, for example on a locked screen). `--anzeigetest <dir>` compares every VideoToolbox format with the
CPU path (tolerance 0 for the conversion, the scaled and the 1:1 picture; 2 for the
interface on top, as on Windows) and measures 240 frames at 2560x1440 in a 120 Hz
cadence (M1: about 1.8 ms GPU time per frame); it opens no window and starts neither
host nor screen capture.

On the Mac, `client/build.rs` also compiles the Mac host engine into the client: the
`SRC` list of the Makefile without `host/start.m` (that file holds the stand-alone
host's `main`), with the Makefile's `FLAGS`, archived as `libqchost.a` and linked with
`-force_load`, the Makefile's frameworks and clang's `libclang_rt.osx.a` (for
`@available`). It needs the Xcode Command Line Tools (`clang`, `libtool`), and it makes
macOS 14 the minimum for the Mac client too (`MACOSX_DEPLOYMENT_TARGET` in
`client/.cargo/config.toml`). The C interface is `host/dienst.h`
(`client/src/host_mac.rs`): in `resumed` the app sets up the engine
(`qc_app_einrichten`: menu bar with the client's entries, key, access), starts the
service while sharing is on (`qc_dienst_starten`; off and on again:
`qc_dienst_anhalten`, `qc_dienst_fortsetzen`) and then shows the icon
(`qc_oberflaeche_fertig`).

Its files live in `~/Library/Application Support/QuadChroma` (`hosts.txt`,
`einstellungen.txt`, `protokoll.txt`, `benchmark.txt`, and `einzel.sock` and
`einzel.lock` for the single instance) - next to the host's `host-devices.txt` and
`host-password.txt`. `host.key` is the device key of the whole app: the host engine
accepts with it and the client calls with it; a `client.key` of an earlier version stays
there unused. A host lets it in like every client: with its access password or a click
on "Allow".

License note: the Mac app contains no FFmpeg, only Apple frameworks, the vendored
Monocypher and the Rust crates listed in `THIRD_PARTY_NOTICES.txt`.

## Test harnesses

For the Mac host there are small test programs that run without screen capture and
leave a running host, with its ports, device list, password and clipboard, alone.
Build them from the repository root; each file's header holds its build line, and the
exit code is the number of failures.

- `host/annahmetest.c`: handshake deadline, slots and eviction; reading with a
  deadline after the handshake (as the access phase waits for the client: deadline,
  end of connection, trickling, buffered bytes); the client's name in handshake
  message 3; `host.key`. Loopback from port 19000, its own `HOME`, about 25 s.
- `host/zugangtest.c`: the access core `host/zugang.c` without network - device ID,
  names, name and flags of handshake message 3, announcement, messages 20 to 23, norm, HMAC (RFC 4231) and PBKDF2 with the
  test vectors of the Rust side, the proofs, constant-time comparison, throttle,
  limits of the access phases, requests to the menu bar, `host-devices.txt` (format,
  damaged files, removing, resetting), the takeover from `authorized.txt`,
  `host-password.txt` and the single instance. Its own `HOME`, about 2 s.
- `host/hosttest.m`: includes `main.m` and checks the rate limiting (per kind and
  address) and the size cap of the log; the viewer takeover (message 10) including a
  leftover test pattern; teardown between start-up and registration; re-sending the
  last frame on a still screen; the codec switch without a viewer and with a format
  change (real VideoToolbox encoders); `--fest`; the congestion rule including audio
  during congestion; the announcement of the audio format and AV1 in the capability
  list.
  - Files via the clipboard over the real channels: capabilities (messages 11 and 69);
    client to host with acknowledgements on the video channel and the lines
    "empfange …" and "empfangen …" (receiving, received) in the host log; host to
    client with the window; names with umlauts in the log; a viewer takeover in the
    middle of a transfer including the session binding of the send path; a viewer
    without an input channel; an older client without capabilities; a capability
    valid only for the input channel that announced it; size limits on the input
    channel. Received files go to a recorder instead of the clipboard, and the
    clipboard base lies in the test's own `HOME`.
  - Screen selection (`host/bildschirm.m`): test vectors of messages 12 and 70;
    truncation at character boundaries; the pure selection logic; stream size;
    `bildschirm.txt` (also broken); `--display` as a pin for the run; the greeting
    with capabilities 7 and the list; Automatic follows the main screen (debounced);
    a request via the real input channel; a missing requested screen with fallback
    and return; a different size with a new encoder and keyframe; waiting for a
    running codec switch (every request gets its answer, the waiting ends as soon as
    no switch is pending, after 5 s the switch happens anyway); an empty list with a
    running stream; screen loss and recovery. List and stream come from mocks, the
    encoders are real.
  - HDR negotiation: capabilities with bit 2, message 71 over the real input channel
    (a new stream info in SDR with the host's reason, nothing on an unchanged state,
    unreadable data skipped), then up to HDR10 and back - HDR on the host's screen on
    and off, the codec switch with color, the 2-second lock, reason 6 when the capture
    refuses HDR, a takeover in the middle of HDR10, a double failure without a loop;
    and HDR10 with the real VideoToolbox encoder and decoder (SEI 137/144 byte-exact
    after the PPS of every keyframe, VUI and attachments BT.2020/PQ, the HDR test
    pattern, Display P3 to BT.2020).
  - Access, with real handshakes and a client that behaves like the Rust client: an
    unknown client gets "QCA1" and message 20; the right password gives result 0 with
    a correct host proof and then "QCH1"; a known client with bit 0 in message 3 goes
    through the same access phase (password or "Allow") and keeps its entry; wrong
    passwords with the throttle and the end after five; "Allow" and "Deny" through a
    test hook instead of the menu bar;
    cancel and end of connection withdraw the request; only one request shown at a
    time; the limits per address and per key; the deadline; a waiting client does not
    disturb the running viewer; a damaged device list; removing a device ends its
    session; host status 1 instead of a refusal without the Screen Recording
    permission or without a screen; a name that fakes a device ID; switches without
    a value. Also the service clock without a run loop and the farewell on quitting.

  Loopback from ports 19100 and 19400/19450, its own `HOME` under `$TMPDIR` (removed
  after a passing run), about 100 s. It briefly uses real HEVC encoders (640×360,
  1920×1080, 1280×720) - do not start it during a stream.
- `host/dateitest.m`: the file protocol from `host/dateien.m` without network and
  without clipboard:
  - test vectors; path rules including sanitising for macOS and (for the sender's
    duplicate check) for Windows; receiver and sender against each other in memory;
    stragglers from another session, including a late offer in the middle of a
    running transfer;
  - when the receiver acknowledges (from 16 KiB outstanding and as soon as nothing is
    waiting); its queue by data bytes (window plus one chunk, at most 8 ends, a new
    offer empties it); the line "Dateien: empfange …" (files: receiving) on
    acceptance, but not when acknowledgement 0 can no longer go out;
  - names in NFC on disk (the raw bytes, even when the offer carries NFD); a `/` or
    `\` before a combining character; names that become duplicates only after the
    Windows sanitising;
  - window; throttling via the send buffer including its timing (every 2 to 3 ms,
    not 10 ms); stall (a repeated acknowledgement without progress does not keep the
    sender alive); aborts;
  - a source file swapped for a FIFO or a symbolic link after listing, likewise a
    folder above it (swapped for a link, and for another real folder, where comparing
    device and inode holds back the foreign file); a folder swapped for a link
    continuously while it is listed (1.5 s endurance run); sending from a source
    whose absolute path exceeds `PATH_MAX`;
  - cleanup - also with a base that is a symbolic link, after the clock jumps back,
    of transfers whose absolute paths exceed `PATH_MAX`, with the `.laeuft` marker of
    running transfers, and of orphans (unfinished at start);
  - the QoS class of the two queues (`dateien-senden` with `QOS_CLASS_DEFAULT` and
    relative priority -15, `dateien-empfang` with `QOS_CLASS_UTILITY`).

  All files in a fresh folder under `$TMPDIR`, about 8 s.
- `host/ablagetest.m`: the marking of received text, the rule "read only with a
  viewer", and reading and writing file references (`public.file-url`), on a
  separate named pasteboard instead of the general one; its files are under
  `$TMPDIR`.
- `host/menuetest.m`: the menu bar without showing anything - the text tables in 29
  languages (every text in every table, placeholders, no duplicates), the choice of
  language, the menu for various states, the password check of the password window
  and the queue of "Allow" requests, against a stand-in for the access core.
- `host/einbettungstest.m`: the menu bar layer embedded as in the Rust client, where
  winit owns the application delegate. A stand-in for winit's delegate (only
  `applicationDidFinishLaunching:` and `applicationWillTerminate:`) starts it from
  its launch callback; checked: the delegate stays winit's, its class then answers
  `applicationShouldHandleReopen:hasVisibleWindows:` and `applicationShouldTerminate:`,
  a reopen event (as from a double-click on the running app) calls the client's
  callback, and quitting runs the farewell exactly once, off the main thread, before
  `applicationWillTerminate:`. Needs a login session with a window server, no
  permission; the process has no Dock icon and shows nothing.

```
clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/annahmetest.c host/qc_annahme.c \
      host/qc_secure.c host/qc_noise.c host/vendor/monocypher/monocypher.c -o /tmp/annahmetest

clang -fobjc-arc -O2 -Wall -Ihost -Ihost/vendor/monocypher -Wno-deprecated-declarations \
      -mmacosx-version-min=14.0 -framework Foundation -framework AppKit \
      -framework ScreenCaptureKit -framework VideoToolbox -framework CoreMedia \
      -framework CoreVideo -framework CoreGraphics -framework CoreFoundation -framework IOKit \
      -framework SystemConfiguration \
      host/hosttest.m host/audio.m host/clipboard.m host/zeiger.m host/testbild.m host/last.m \
      host/dateien.m host/bildschirm.m host/hdr.c host/qc_noise.c host/qc_secure.c host/qc_annahme.c \
      host/zugang.c host/vendor/monocypher/monocypher.c -o /tmp/hosttest

clang -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter -Ihost -mmacosx-version-min=14.0 \
      -framework Foundation host/dateitest.m host/dateien.m -o /tmp/dateitest

clang -fobjc-arc -O2 -Wall -Ihost -mmacosx-version-min=14.0 -framework Foundation \
      -framework AppKit host/ablagetest.m -o /tmp/ablagetest

clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/noisetest.c host/qc_noise.c host/zugang.c \
      host/qc_secure.c host/vendor/monocypher/monocypher.c -o /tmp/noisetest

clang -O2 -Wall -Ihost -Ihost/vendor/monocypher host/zugangtest.c host/zugang.c \
      host/qc_secure.c host/qc_noise.c host/vendor/monocypher/monocypher.c -o /tmp/zugangtest

clang -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter -Ihost -mmacosx-version-min=14.0 \
      -framework Foundation -framework AppKit -framework ServiceManagement \
      host/menuetest.m host/menue.m host/texte.m -o /tmp/menuetest

clang -fobjc-arc -O2 -Wall -Wextra -Wno-unused-parameter -Ihost -mmacosx-version-min=14.0 \
      -framework Foundation -framework AppKit -framework ServiceManagement -framework CoreServices \
      host/einbettungstest.m host/menue.m host/texte.m -o /tmp/einbettungstest
```

`noisetest` without an argument is the self-test of the crypto layer ("OK", otherwise
the place of the failure), including a failed handshake after which no DH result may
remain on the stack; with a port it waits for the counterpart test of the Rust side
(`quadchroma --noisetest address:port`). CI (`.github/workflows/ci.yml`) compiles all
eight and runs `noisetest`, `annahmetest`, `zugangtest`, `dateitest`, `ablagetest`,
`menuetest` (the menu bar texts in 29 languages and the menu model, nothing shown) and
`einbettungstest`; `hosttest` needs the Screen Recording permission at run time and is
only compiled there.

The Rust side tests itself with `cargo test --release` in `client\` or `client/`;
several tests need loopback (TCP over 127.0.0.1). On Windows, FFmpeg's `bin` folder
belongs in `PATH`. The clipboard tests there write the session's real clipboard, one
after another behind the named mutex `Global\QuadChromaAblageTest`, and in a session
with Explorer a real notification-area icon appears briefly (without a balloon). On
the Mac run `cargo test --release -- --skip clipboard`, otherwise
`setzen_zaehlen_lesen` reads and writes the real clipboard; the other clipboard tests
of the Mac client work on their own pasteboards and run individually by name
(`-- --exact clipboard_mac::tests::<name>`). The VideoToolbox tests of the Mac client
(`vt_decoder`) encode a small HEVC 4:4:4 10-bit sample with the hardware encoder and
check the decoded planes and the RGB conversion against the known pattern, with and
without hardware; the splitting of Annex B, the parameter sets and the choice of the
output format are checked on Windows too. Where VideoToolbox cannot be reached (a
virtual machine without a hardware encoder, a sandbox that blocks its services), the
tests that need it say "uebersprungen" and pass. The Metal tests (`anzeige_mac`)
compare the GPU conversion and scaling with the CPU path for `xf44`, `420f` and `x420`
at tolerance 0 and present into a `CAMetalLayer` without a window; without a Metal
device (a sandbox that blocks the GPU, a virtual machine) they also say
"uebersprungen". The tests never touch the data folder:
keys, settings, `protokoll.txt` and `quadchroma.ico` live per run in
`qc-test-<pid>-<ms>/QuadChroma` in the temp folder, so `APPDATA` need not be
redirected. Received files go to `qc-test-<pid>-ablage` or
`qc-test-<pid>-host-ablage`, also in the temp folder, never to `QuadChroma-Ablage`.

For the screen selection the Rust tests check the test vectors of messages 12 and 70
(`bildschirm.rs`); in the client the session against a fake host (bit 1, list,
request including hint and re-sending, a stream info with a new size rebuilds the
decoder, reset at the end of the session) and the texts in all 29 languages; in the
Windows host role the pure selection (Automatic follows the main screen, request,
fallback, return, reasons), identifier and name from the device ID, `bildschirm.txt`
including leftovers, message 12 only after setup, the Win32 paths of the list, the
request over the real input channel and the greeting with capabilities 7 and list 12.

For the access the Rust tests check the test vectors (device ID, norm, K, both proofs,
HMAC after RFC 4231, PBKDF2), messages 20 to 23 including truncated ones and a wrong
version, the throttle, `hosts.txt` and `host-devices.txt` with the takeover from the
old lists, the announcement with ID, and the texts in all 29 languages, which must
match `host/texte.m` of the Mac host for every shared text (the test skips that part
when `host/` is missing). End to end over loopback: the client against test hosts
(known, password wrong then right, "Allow" with and without having been offered, a
wrong host proof that pins nothing, refused, deadline, no free place, older host,
silent host, foreign reply, cancel, new identity at a known address, another ID,
removed device) and the Windows host role against a test client (known, password
right and wrong with the throttle, "Allow" and "Deny" through a test hook, cancel,
deadline, replayed proofs, limits, damaged list, unreadable password, trickling
bytes, removal while a device comes in), plus the host role's Win32 windows driven by
messages.

A self-test without network checks the icon in the notification area or menu bar:
`quadchroma.exe --tray-selbsttest` (needs a session with Explorer) or
`quadchroma --menueleiste-selbsttest` on the Mac - the app's real icon and menu with
the engine set up in a throw-away home folder, without the service and without
screen capture; exit code 0 or 1. `quadchroma --ruhe-selbsttest` sets "Prevent sleep",
finds it in `pmset -g assertions` and releases it (Mac only; on Windows `powercfg
/requests` in a terminal started as administrator shows the request). On Windows the
self-tests, like every switch, need a terminal started as administrator.

## Repository layout

    host/          Mac host engine (Objective-C and C), built into the Mac app: the
                   service (main.m, C interface in dienst.h) and the stand-alone
                   host's main (start.m, harnesses only), capture, encoder,
                   network, input, audio, clipboard, files (dateien.m), screen
                   selection (bildschirm.m), pointer shape, access with password or
                   Allow (zugang.c), menu bar and its windows (menue.m) with the
                   texts in 29 languages (texte.m), the test harnesses;
                   host/vendor/monocypher is the vendored Monocypher
    client/        Rust: receive, decode, display (anzeige.rs: Direct3D 11), input,
                   audio, clipboard; files (dateien.rs) and screen list
                   (bildschirm.rs), both shared with the host role; device ID,
                   access proofs, throttle and the lists (zugang.rs, shared with
                   the host role) and the client's access dialog
                   (zugangsphase.rs); notification area (tray.rs, tray_win.rs)
                   and the model of the icon's menu (symbolmenue.rs), the Mac app
                   around the host engine (host_mac.rs), the sharing switch
                   (freigabe.rs), "Prevent sleep" (ruhezustand.rs), announcement
                   and search (discovery.rs), settings (einstellungen.rs), the
                   texts in 29 languages (strings*.rs), single instance
                   (einzel.rs), desktop shortcut and "Start with Windows" as a
                   scheduled task per account (verknuepfung.rs) with the
                   install into Program Files from locked files, its checks
                   and the version comparison (installation.rs), program icon
                   (logo.rs); version information, icon and manifest of the exe
                   (build.rs, res/); the Windows host role in client/src/host/
                   (capture with screen selection, encoder, network, access phase
                   (einlass.rs), menu and windows (oberflaeche.rs, fenster.rs),
                   input, audio, pointer shape)
    Makefile       builds and signs the Mac app bundle; sign, notarize, staple, zip
                   and dmg for distribution, check-packages to check them
    scripts/       build-ffmpeg-windows.sh (FFmpeg for Windows), sign-windows.ps1
                   (code signature of the exe), package-texts.sh (the text files of
                   the release packages), release-notes.sh (the release notes)
    .github/       CI and release (workflows/), templates for issues and pull
                   requests, Dependabot
    README.md      this overview
    MANUAL.txt     the complete manual: every switch, log line and protocol message
    RELEASING.md   signing and publishing
    *.png          screenshots used by this README
    LICENSE.txt, THIRD_PARTY_NOTICES.txt, CLA.md, CONTRIBUTING.md, SECURITY.md,
    CODE_OF_CONDUCT.md   license and community files, see below

The host is written in C and Objective-C, the client in Rust. Code comments and the
program's log lines are in German.

## License and publication

QuadChroma is source-available, not open source. The project is licensed under the
**PolyForm Strict License 1.0.0**, reproduced unchanged in `LICENSE.txt`, together
with **Additional Terms of the licensor** in the same file. In short:

- You may use QuadChroma for personal, noncommercial purposes.
- You may not distribute it, modify it or create works based on it.
- You may not embed or build it, its code or its protocol into other products,
  devices, services or libraries, and you may not offer it as a service.
- Any use in or for companies, public authorities and other organizations is
  commercial, including purely internal use, and requires a separate license from the
  licensor.

This summary is for orientation only and has no legal effect; `LICENSE.txt` is the
binding text. Commercial licenses and other permissions are available on request:
hello@quadchroma.tech.

- `THIRD_PARTY_NOTICES.txt` collects all third-party parts of the distributed programs
  with their license texts: FFmpeg (LGPL 2.1, own build, source references and a
  written offer), the NVIDIA headers, the compiler runtime, the Rust crates per target
  and Monocypher.
- `CLA.md`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md` and `.github/`
  (templates for issues and pull requests): contributions are accepted only under the
  Contributor License Agreement in `CLA.md`, which grants for each contribution the
  rights that a later dual licensing needs, so the licensor can offer the project
  under other terms as well; see `CONTRIBUTING.md`. Security reports: `SECURITY.md`.
  Conduct: `CODE_OF_CONDUCT.md`.
- `RELEASING.md` covers signing and publishing: what is needed once (Apple Developer
  Program and a Developer ID certificate; for Windows Azure Artifact Signing or a
  code-signing certificate; the GitHub secrets), and how a release runs locally
  (`make sign notarize staple dmg`, `scripts/sign-windows.ps1`) or through
  `.github/workflows/release.yml` (a tag `vX.Y.Z` produces a draft with all packages,
  `SHA256SUMS.txt` and the FFmpeg source).

Every release package contains `LICENSE.txt`, `THIRD_PARTY_NOTICES.txt`, `README.txt`
(this README without its images) and `MANUAL.txt` next to the program; the Windows ZIP
also contains `FFMPEG-BUILDINFO.txt` (sizes, SHA-256 and configuration of the two
DLLs). The Mac app carries the same four texts inside as well
(`QuadChroma.app/Contents/Resources`), so they travel with every copy of it, and "Start
with Windows" copies them, with `FFMPEG-BUILDINFO.txt`, into `C:\Program
Files\QuadChroma` together with the exe and the DLLs.

FFmpeg: the Windows client uses libraries from the FFmpeg project under the LGPLv2.1,
loaded as separate DLLs next to the exe (`avcodec-63.dll`, `avutil-61.dll`). They are
built by `scripts/build-ffmpeg-windows.sh` from the unmodified FFmpeg 9.0.2 without
any external library; the exe contains no FFmpeg code, and the DLLs can be replaced by
another build of the same FFmpeg version. (Before, the project used a prebuilt build
that contained GPL code through chromaprint and therefore could not be passed on.)
The FFmpeg source is attached to every release that contains the Windows package and
available at https://ffmpeg.org. The start screen of the Windows program names FFmpeg
in its footer, as the LGPL requires once a program shows copyright notices; the Mac
app contains no FFmpeg and does not. The exe carries version
information, icon and manifest (`client/build.rs`, `client/res/`), the Mac app the
bundle identifier `tech.quadchroma.host`. The Mac app's host engine contains Monocypher
(BSD-2-Clause OR CC0-1.0). The licenses of these components and of the Rust crates per
target, together with the FFmpeg build details, the source references and a written
offer, are collected in `THIRD_PARTY_NOTICES.txt`.

Copyright 2026 Robert Brandt. Contact: hello@quadchroma.tech, https://github.com/quadchroma-tech/quadchroma.

## Dedication

For Michelle, who always has my back. The little heart with "Mi" in the corner of the
app is hers.
