# Contributing to QuadChroma

Thank you for your interest. QuadChroma is a one-person project by Robert Brandt
(hello@quadchroma.tech). It is source-available under the PolyForm Strict License 1.0.0
with Additional Terms (`LICENSE.txt`), not open source, and contributions are accepted
only under the Contributor License Agreement in `CLA.md`. Please read this page before
you start. It is short on purpose, and the rules in it are binding for pull requests.

## 1. Before you write code: open an issue

- **Bugs:** use the bug report form (Issues > New issue). Include the log excerpt the
  form asks for, with secrets removed.
- **Features, and any change in behavior, protocol or user interface:** use the feature
  request form and wait for a reply before you start. QuadChroma has a tight design
  (Section 3); a pull request that conflicts with it is closed no matter how good the
  code is, and an issue first saves both of us that.
- **Small pull requests:** one topic each, reviewable in one sitting. Split larger
  changes into a series.
- **Security problems:** never in a public issue - see `SECURITY.md`.
- **Questions:** hello@quadchroma.tech.
- Contributions are accepted **only as pull requests** on GitHub. Patches by e-mail or
  code pasted into issues cannot be merged, because the agreement in Section 2 is tied
  to the pull request.

## 2. Contributor License Agreement (CLA)

Every contribution needs your agreement to `CLA.md` (version 1.0), given for that
contribution. Two things, both required:

1. The trailer `Signed-off-by: Your Name <you@example.com>` in **every commit** -
   `git commit -s` adds it. Name and e-mail must match the commit author and be an
   address you control and are willing to have public (a personal address rather than a
   work address).
2. The sentence **`I have read and agree to the QuadChroma CLA v1.0`** in the pull
   request. The pull request template contains it as a checkbox; tick it, or write the
   sentence in the description or a comment.

Without both, the pull request is not merged. The CLA is for natural persons of legal
age acting in their own name. If you contribute for an employer or as a company, or if
an employer or client has rights in what you write (the usual case for code written
during working hours, § 69b UrhG in Germany), write to hello@quadchroma.tech **before**
opening the pull request: a separate agreement is needed. It is not a big thing, but it
has to exist first. Contributions from minors are not accepted.

What the CLA does, in one paragraph: you keep your copyright; you give the author a
non-exclusive license to use your contribution in every known way, including under
other licenses (commercial and proprietary), so that QuadChroma can stay free for private
users and be licensed to companies for a fee; you confirm that the code is yours and
free of copyleft; you are credited in the Git history and in `CONTRIBUTORS.md`; there is
no payment. Read the full text - it is written to be readable, and it explains why the
agreement is given per contribution rather than once for all time.

## 3. Project rules that apply to every contribution

These are design decisions of the author, not open questions. Pull requests that break
them are closed.

1. **One program per side, no helper processes.** The Mac host is one app bundle
   (`QuadChroma.app`, built from `host/`). Windows is one executable (`quadchroma.exe`,
   built from `client/`) that contains the client and, started with `--host`, the host
   role. The Mac client is the same Rust source. No helper tools, daemons, services,
   launch agents, background helpers, separate installers, drivers or second processes -
   everything runs inside the one program.
2. **The mouse pointer is drawn on the client side.** The host sends the pointer shape
   and position separately from the picture, and the client draws it. Do not encode the
   pointer into the video stream and do not move that work to the host.
3. **No new Rust crates without prior agreement.** The dependency list in
   `client/Cargo.toml` and `client/Cargo.lock` stays as it is. Propose a new crate in an
   issue with a reason, and expect "no" more often than "yes": every crate ends up in the
   binary and in `THIRD_PARTY_NOTICES.txt`. New features of the `windows` crate are fine.
   Helper scripts (PowerShell, sh, Makefile targets, `build.rs`) are fine.
4. **User-interface text only through keys in `strings.rs`, in all languages.** Every
   text shown in the client's window, in the notification area or menu bar, or in a
   dialog goes through a `Key` in `client/src/strings.rs` and gets an entry in **every**
   language table: `EN` and `DE` in `strings.rs` and the tables in `strings_west.rs`,
   `strings_north.rs`, `strings_east.rs`, `strings_balt.rs` and `strings_asia.rs` (29
   languages, all listed in `strings_more.rs`), in the order of the `Key` enum. The test
   `alle_tabellen_in_enum_reihenfolge` fails otherwise. If you cannot translate a text
   into all of these languages, put the English text into the tables you cannot fill and
   say so in the pull request; the author translates. No string literals in UI code.
   The Mac host is the one exception to the file: the texts of its menu bar and windows
   live in `host/texte.m` (`QCText`), with the same 29 languages in the same order, and
   a text the client or the Windows host role shows as well must read exactly the same
   there (the Rust test `zugang_texte_wie_mac_host` compares them; `menuetest` checks
   the tables of `texte.m` themselves).
5. **Code comments and log lines in German, everything else in English.** Code
   comments and the program's log and console lines are written in German (a project
   rule for the source code); documentation, commit messages and everything else on
   GitHub are English. The German part applies to contributors too: comments (including
   `///` doc comments) in Rust, Objective-C, C, shell, PowerShell, Makefile and workflow
   files, and every line the program writes to the console, `protokoll.txt`,
   `host-protokoll.txt` or `/tmp/quadchroma-m1.log`, are German, with `ae`, `oe`, `ue`
   and `ss` instead of umlauts and ß, as in the existing code
   (`// Zu frueh wird gar nicht erst gerechnet - so kostet Raten nichts ausser Zeit.` -
   "a proof that comes too early is not even computed, so guessing costs nothing but
   time"). Identifiers follow the file you
   are editing (mostly German as well). The documentation - `README.md`, `MANUAL.txt`,
   this file, `CLA.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `RELEASING.md`, the issue
   and pull request templates - is in English. If your change alters documented
   behavior, update `README.md` and `MANUAL.txt` in the same pull request (`MANUAL.txt`
   is plain ASCII with lines of at most 78 characters). If you really cannot write
   German comments, say so in the issue before you start; the author may, at his
   discretion, accept English comments in the pull request and translate them himself
   before merging, which takes longer.
6. **No changes to the user interface without an issue first.** Layout, controls,
   colors, behavior of the window - agree first.
7. **`host/vendor/monocypher` is third-party code.** Never modify it; updates come from
   upstream as a whole.
8. **Protocol changes** (message numbers, capability bits, wire formats, the handshake,
   the access phase and its proofs) affect the Mac host (`host/`), the Windows host role
   (`client/src/host/`) and the client. They need an issue first, all sides in the same
   pull request, test vectors in the unit tests on both sides, and compatibility with
   older peers wherever the capability mechanism allows it.
9. **Nothing that talks to servers.** QuadChroma works in the local network without
   accounts, telemetry, crash reporting, update checks or any connection to a server.
   Keep it that way.

## 4. Building and testing

Short form; the complete recipes with paths and explanations are in `README.md`
("Building from source"). Continuous integration
(`.github/workflows/ci.yml`) runs the same steps on Windows and macOS for every pull
request with `cargo ... --locked`, so `Cargo.lock` must be committed exactly as your
build used it.

**Windows client and host role.** Rust stable with the MSVC toolchain, LLVM (for
bindgen: `LIBCLANG_PATH=C:\Program Files\LLVM\bin`) and the FFmpeg libraries built by
`scripts/build-ffmpeg-windows.sh` (minimal LGPL build, cross-compiled with MinGW-w64
on macOS or Linux; `FFMPEG_DIR` points to its output folder). Do not use other FFmpeg
builds for release work: most ready-made builds link external libraries, some of them
under the GPL. Build from `client\` - only there does `client\.cargo\config.toml` (static C
runtime) apply, and an exported `RUSTFLAGS` aborts the build on purpose:

    cd client
    cargo build --release
    set PATH=%FFMPEG_DIR%\bin;%PATH%
    cargo test --release

The tests need loopback TCP; the clipboard tests use the session's real clipboard;
nothing touches `%APPDATA%\QuadChroma`. Expected: all tests pass. A handful of compiler
warnings are known; do not add new ones.

**Mac host.** The Xcode Command Line Tools are enough (no Xcode.app). In the repository
root:

    make

builds and signs `build/QuadChroma.app` with the local development identity
`QuadChroma Dev` (`make IDENT=...` overrides it; `make sign`, `make verify`, `make zip`
and `make dmg` are described in `RELEASING.md`). Do not start the freshly built app
(`open build/QuadChroma.app`, `make list`, `make capture`) unless you are prepared for
macOS to ask for Screen Recording and Accessibility permissions on that machine and
unless no host is running there. The test harnesses `noisetest`, `annahmetest`,
`zugangtest`, `dateitest`, `ablagetest` and `menuetest` run without screen capture and
without touching a running host; their `clang` lines are in
`.github/workflows/ci.yml`, which runs them.
`hosttest` includes `main.m`, needs the Screen Recording permission granted to your
terminal, and uses real HEVC encoders for about 100 seconds: run it only on a machine
where you have granted that permission, never during a stream, never in CI.

**Mac client.** FFmpeg from Homebrew (a GPL build - fine for building and testing on
your own machine, not for distribution):

    cd client
    FFMPEG_DIR=/opt/homebrew/opt/ffmpeg cargo build --release
    FFMPEG_DIR=/opt/homebrew/opt/ffmpeg cargo test --release -- --skip clipboard

`--skip clipboard` keeps one test from reading and writing your real clipboard.

**Before you open the pull request:** run the test set of every side your change
touches, locally, and say in the pull request which ones you ran on which machine. CI
cannot run `hosttest` and has no NVIDIA card, so a green CI is necessary but not
sufficient.

## 5. Style

- **Rust:** there is no `rustfmt.toml`, and the code is not formatted with `rustfmt`.
  **Do not reformat code you do not otherwise touch**, and do not run `cargo fmt` on
  whole files: the diff must show your change and nothing else. Follow the style of the
  surrounding code (line length, `match` layout, comment density). `cargo clippy` is not
  enforced; existing warnings are known, new ones are not welcome.
- **Objective-C and C:** `-Wall` clean, ARC, `-mmacosx-version-min=14.0`; the build lines
  are in the file headers and in the Makefile.
- Every source file starts with a short German header comment saying what the file does
  and how it fits in - look at any existing file.
- **Commits:** one logical change per commit, message in English and in the
  imperative, `Signed-off-by` trailer (Section 2). Rebase on `main` before opening the pull
  request; the history is linear (squash or rebase merges, no merge commits from
  contributors).
- Line endings are handled by `.gitattributes` (LF in the repository, CRLF for `*.ps1`,
  `*.bat`, `*.cmd`); UTF-8 without BOM; no trailing whitespace.
- Do not commit anything from `build/`, `client/target/` or `paket/`, no keys (`vmkey`,
  `host.key`, `client.key`), no `known_hosts*`, no device lists or passwords
  (`host-devices.txt`, `host-password.txt`, `hosts.txt`), no recordings (`*.hevc`,
  `*.bmp`). `.gitignore` covers the build output, `vmkey`, `known_hosts`, `*.hevc` and
  `*.bmp`; the program's own files live outside the repository anyway. Check
  `git status` all the same.

## 6. Licensing of contributions and third-party code

- PolyForm Strict does not allow changes. The section "Contributions" at the end of
  `LICENSE.txt` gives you a narrow permission to change the source code solely to
  prepare a contribution, to build and test it on your own devices, and to submit it
  through the official repository (a fork and a pull request on GitHub). It does not
  allow using or distributing the changed program for anything else.
- Your contribution is published under `LICENSE.txt` (PolyForm Strict 1.0.0 with the
  Additional Terms) as part of QuadChroma and may also be licensed by the author under
  other terms, as set out in `CLA.md`. You keep your copyright and may use your own code
  elsewhere.
- **Third-party code** only under permissive licenses (MIT, BSD-2-Clause, BSD-3-Clause,
  ISC, Apache-2.0, Zlib, 0BSD, CC0-1.0), clearly marked in the code and in the pull
  request with origin, version and license, and with the required notice added to
  `THIRD_PARTY_NOTICES.txt` in the same pull request. Nothing under GPL, LGPL, AGPL, MPL,
  EPL, SSPL or any other copyleft or non-commercial license, no snippets of unclear
  origin, nothing copied from other remote-desktop projects (most of them are GPL or
  AGPL). This also applies to anything a code assistant produces for you: you are
  responsible for what you submit (`CLA.md`, Section 7).
- **New Rust crates:** see rule 3 in Section 3. If one is agreed, its license and notices
  go into `THIRD_PARTY_NOTICES.txt` in the same pull request.

## 7. Code of conduct

`CODE_OF_CONDUCT.md` (Contributor Covenant 2.1) applies in issues, pull requests and all
other project spaces. Reports go to hello@quadchroma.tech. Be aware that the project has
a single maintainer: reports are read and handled by him, including reports about his
own behavior; there is no separate committee. If that is not acceptable for a
particular report, say so in your message and we will look for a neutral person
together.

## 8. Contact

Robert Brandt - hello@quadchroma.tech - https://github.com/quadchroma-tech/quadchroma

Commercial licenses and any use beyond `LICENSE.txt`: same address.
