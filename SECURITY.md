# Security Policy

QuadChroma is a remote-desktop program: the host shares its screen, and the client
sends keyboard and mouse input, clipboard contents and files to it. Security problems in
it matter, and reports are welcome. Please read this page before you report.

## Supported versions

QuadChroma is developed by one person and has no maintenance branches. Fixes go into
`main` and into the next release.

| Version | Supported |
|---|---|
| The latest release (newest tag `vX.Y.Z` on the Releases page) | Yes |
| `main` since that release | Yes - fixes land here first |
| Older releases | No - please update to the latest release |

At the time of writing the version line is 0.1.x; the Releases page of
https://github.com/quadchroma/quadchroma is authoritative.

## How to report a vulnerability

**Please do not open a public issue for a security problem.** Use one of these two
channels:

1. **GitHub private vulnerability reporting** (preferred): on the repository's
   *Security* tab, choose *Report a vulnerability*
   (https://github.com/quadchroma/quadchroma/security/advisories/new). The report is
   visible only to you and the maintainer, the fix can be discussed there, and the
   result is published as a GitHub Security Advisory with credit to you.
2. **E-mail:** hello@quadchroma.tech, subject starting with `[security]`. There is no
   PGP key yet. If you need to send sensitive details encrypted, send a short
   unencrypted note first, and we will agree on a channel.

If you report by e-mail and hear nothing within 7 days, please send a reminder through
the other channel; mail does get lost.

## What happens next - honest expectations

- QuadChroma is maintained by one person in his spare time. You will get an
  **acknowledgement within 7 days** of your report.
- After that, the maintainer will tell you whether he can reproduce the problem, how
  serious he considers it, and roughly when a fix is likely, and will keep you informed
  at least every two weeks until the fix is released.
- Fixes are released as a new version (tag, Releases page, signed binaries where
  available) together with a GitHub Security Advisory that describes the problem, the
  affected versions and the fix, and credits the reporter if they wish.

## Coordinated disclosure

Please give the maintainer **90 days** from the acknowledgement before you disclose the
problem publicly. If a fix is released earlier, you are free to publish as soon as the
fixed version is available. If 90 days turn out not to be enough (a hard fix, a change to
the protocol on both sides), the maintainer will say so early and ask for an extension;
if the maintainer does not respond at all, you are free to publish after the 90 days.
Please do not disclose to third parties in the meantime, and do not exploit the problem
beyond what is needed to demonstrate it.

## What a good report contains

- Which side is affected: Mac host, Windows client, Windows host role, Mac client, the
  protocol between them, or the build and release scripts.
- The version (release tag) or the commit hash if you built from source, and the
  operating system and hardware (macOS version and Mac model; Windows version and GPU).
- Your setup: which host and which client, same local network or not, paired or first
  contact, any command-line options (`--host`, `--display`, `--fest` ...).
- Steps to reproduce, ideally a minimal proof of concept (a script, a captured message
  sequence, a crafted file). A description of the impact: what an attacker can do, from
  where, with what prerequisites.
- Relevant lines from the log: Windows client `%APPDATA%\QuadChroma\protokoll.txt`,
  Windows host role `%APPDATA%\QuadChroma\host-protokoll.txt`, Mac host
  `/tmp/quadchroma-m1.log` (older part in `/tmp/quadchroma-m1.alt.log`), Mac client
  `~/Library/Application Support/QuadChroma/protokoll.txt`.
- **Remove secrets before sending:** pairing codes, the contents of `host.key`,
  `client.key` and `known_hosts.txt`, and any addresses or host names you do not want
  the maintainer to see.

## Scope

In scope:

- the Mac host (`host/`), the Windows client and the Windows host role (`client/`), the
  Mac client, and the release binaries built from them;
- the protocol: discovery and announcement (port 9003), pairing and the Noise
  handshake, key storage (`host.key`, `client.key`, `known_hosts.txt`), the video and
  audio channel (port 9001), the input channel with clipboard and file transfer
  (port 9002);
- input injection, clipboard and file handling on all sides (path handling, size
  limits, what is read without a peer);
- the build, signing and release scripts, the CI workflows and the integrity of the
  released archives.

Examples of what the maintainer wants to hear about: access without pairing or bypass of
the pairing; remote code execution or memory-safety bugs reachable from the network or
through a crafted stream, message or file; path traversal or overwriting files through
the file transfer; leaking keys or clipboard contents; making the host unusable from the
network; weaknesses in the cryptography or its use.

Out of scope, or lower priority:

- vulnerabilities in the FFmpeg DLLs shipped with the Windows client: report them
  upstream (https://ffmpeg.org/security.html), but tell the maintainer too so that the
  shipped build can be updated;
- anything that requires an already compromised machine, physical access, or an
  attacker who already holds a paired key;
- reports from automated scanners without a reproducible impact, best-practice
  suggestions without an attack, and social engineering;
- the ad-hoc or development-signed builds that are not distributed.

## Rules for testing

Test only against devices you own or are explicitly allowed to test. Do not access,
alter or delete other people's data, and do not disturb other users. If you follow these
rules and this policy in good faith, the maintainer will not take legal action against
you for your research and will work with you to understand and fix the problem.

## No bounties

This project pays no bug bounties and no rewards. What you get is a fast, serious
response, a fix, and credit in the advisory and the release notes if you want it.
