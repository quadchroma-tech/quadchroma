<!-- Vorlage fuer Pull Requests. Sichtbare Texte Englisch (GitHub-Datei); dieser Kommentar deutsch (Projektregel). -->
<!-- Thank you. Keep this pull request to one topic and fill in every section. A pull request without the ticked CLA sentence and without Signed-off-by trailers is not merged (CONTRIBUTING.md, Section 2). -->

## Related issue

Fixes #

<!-- Every change in behaviour, protocol or user interface needs an issue first (CONTRIBUTING.md, Section 1). For a bug fix, link the bug report. -->

## What this changes and why

<!-- What was wrong or missing, what the change does, what you decided against. Describe protocol and user-interface changes precisely. -->

## Side(s) affected

- [ ] Mac host (`host/`)
- [ ] Windows client (`client/`)
- [ ] Windows host role (`client/src/host/`)
- [ ] Mac client (`client/`, macOS build)
- [ ] Protocol (all sides changed together in this pull request)
- [ ] Build, release, CI or documentation only

## How it was tested

<!-- Which test sets you ran, on which machine, and what you tried by hand. CI cannot run hosttest and has no NVIDIA card, so a green CI is necessary but not sufficient. -->

| Side | Command or harness | Machine | Result |
|---|---|---|---|
| Windows client / host role | `cargo test --release` in `client\` (FFmpeg `bin` in `PATH`) | | |
| Mac client | `cargo test --release -- --skip clipboard` in `client/` | | |
| Mac host | `make`; `noisetest`, `annahmetest`, `dateitest`, `ablagetest`; `hosttest` only with the Screen Recording permission | | |
| By hand | host and client on real hardware - which host, which client, which GPU | | |

## Checklist

### Agreement

- [ ] **I have read and agree to the QuadChroma CLA v1.0** (`CLA.md`; ticking this box is my agreement for this contribution).
- [ ] Every commit carries `Signed-off-by: Name <email>` with the same name and e-mail as the commit author (`git commit -s`).
- [ ] I am a natural person of legal age contributing in my own name; no employer or client holds rights in this change (otherwise a separate agreement was arranged by e-mail before this pull request).

### Project rules (CONTRIBUTING.md, Section 3)

- [ ] One program per side: no helper process, service, driver or second program.
- [ ] The mouse pointer stays on the client side.
- [ ] No new Rust crate: the dependencies in `client/Cargo.toml` and `client/Cargo.lock` are unchanged (or a crate was agreed in the linked issue and its notices are in `THIRD_PARTY_NOTICES.md`).
- [ ] New or changed user-interface texts go through a `Key` in `client/src/strings.rs` with an entry in every language table (`strings.rs`, `strings_west.rs`, `strings_north.rs`, `strings_east.rs`, `strings_balt.rs`, `strings_asia.rs`), in enum order; `cargo test` passes `alle_tabellen_in_enum_reihenfolge`.
- [ ] Code comments and log lines are in German with `ae`, `oe`, `ue`, `ss` instead of umlauts and ß.
- [ ] No reformatting of code I did not otherwise touch; no `cargo fmt` on whole files.
- [ ] `host/vendor/monocypher` is untouched.
- [ ] Third-party code, if any, is marked with origin and licence, is under a permissive licence, and its notice is added to `THIRD_PARTY_NOTICES.md`; nothing under GPL, LGPL, AGPL, MPL, EPL or SSPL, nothing from other remote-desktop projects.
- [ ] Documentation updated where behaviour changed (`README.en.md`; `README.md` and `BENUTZUNG.txt` in German, or the change is described above for the author to document).
- [ ] No secrets, keys, `known_hosts*`, recordings or build output in the diff.
