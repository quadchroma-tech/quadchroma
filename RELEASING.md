# Releasing QuadChroma

How a release is built, signed, notarized and published; what has to be set up
once (and by whom); and what users still see after everything is signed. The
license texts are `LICENSE.txt` and `THIRD_PARTY_NOTICES.txt`; the user
manual is `MANUAL.txt`; the build scripts referenced here are the `Makefile`
(Mac host), `scripts/sign-windows.ps1` (Windows signing) and the two workflows
under `.github/workflows/`.

All prices, quotes and tool versions below were checked on 26 September 2026
against the primary sources listed in section 8. Prices are list prices and
change; check before buying.

## 1. Overview

| | Windows | macOS |
|---|---|---|
| Binary | `quadchroma.exe` (client and host role, x86_64, static CRT) plus two FFmpeg DLLs next to it (`avcodec-63.dll`, `avutil-61.dll`) | `QuadChroma.app` (host, Apple silicon, macOS 14 or later) |
| Signature | Authenticode (SHA-256, RFC 3161 timestamp) with a publicly trusted code-signing certificate | Developer ID Application certificate, Hardened Runtime, secure timestamp, then notarized by Apple and the ticket stapled |
| Tooling | `scripts/sign-windows.ps1` (signtool) locally, or `azure/artifact-signing-action` in CI | `make sign`, `make notarize`, `make staple`, `make dmg` ... locally; most of the same targets in CI (notarization there runs through `notarytool --apple-id` instead of `make notarize`) |
| What users see afterwards | SmartScreen still warns until the certificate has built reputation, but shows the verified publisher name (section 6) | One "downloaded from the internet" confirmation on first launch, no warning; then the usual Screen Recording and Accessibility prompts |
| Needs the author | a code-signing certificate on the author's own name (or, only for organizations in the EU, an Azure Artifact Signing account) | Apple Developer Program membership (individual), Developer ID certificate, notarytool credentials |

The Mac *client* (same Rust source as the Windows program, decoding with VideoToolbox,
no FFmpeg) is not yet released as a binary of its own; it is meant to become part of
`QuadChroma.app` with the one app on the Mac. The CI builds and tests it, nothing
more.

Two ways to produce a release:

- **CI** (`.github/workflows/release.yml`): push a tag `vX.Y.Z`; the workflow
  builds both platforms, signs and notarizes if the secrets exist, and creates a
  **draft** release with all files and `SHA256SUMS.txt`. Nothing is published
  automatically. Without secrets the workflow still runs end to end and produces
  files named `-unsigned` (macOS also `-unnotarized`); that is the dry run.
- **Locally**: `make release IDENT="..."` on the Mac, `scripts/sign-windows.ps1`
  on the Windows build machine, then `gh release create --draft` (section 4.3).

Until the certificates exist, every release is an unsigned pre-release and must
be labelled as such in the release notes.

## 2. One-time setup on the Apple side (needs the author)

### 2.0 Free option: the project's own certificate (no Apple account)

Without the Apple Developer Program the host can still be signed with a
self-signed code-signing certificate that belongs to the project: "QuadChroma
Release" (RSA 3072, valid until 26 September 2046, SHA-256 fingerprint
`3B:6D:5C:A6:EF:08:21:94:88:28:33:74:BC:08:5F:A0:28:FC:9B:9F:E7:66:CA:24:A2:B2:48:67:59:46:20:BA`).
It was created on 26 September 2026 and lies outside the repository in
`~/Documents/quadchroma-signierung/` (`quadchroma-release.p12`, its password,
the Base64 form for GitHub, the public certificate). Keep a backup of that
folder: a release signed with a different certificate makes every user grant
Screen Recording and Accessibility again.

What it gives: Gatekeeper still refuses the first start (the app is not
notarized; users click "Open Anyway" once, see README.md), but macOS ties the
two permissions to the designated requirement `identifier "tech.quadchroma.host"
and certificate root = H"eddb251662679f9761f59df958e435273230c095"`, which stays
the same for every release signed with this certificate - permissions survive
updates. With an ad-hoc signature they would be lost on every update.

To use it in CI, add the two repository secrets `MAC_SELFSIGNED_P12_BASE64`
(content of `quadchroma-release.p12.base64.txt`) and
`MAC_SELFSIGNED_P12_PASSWORD` (content of `p12-passwort.txt`). `release.yml`
then signs with `IDENT="QuadChroma Release"` (no secure timestamp - Apple's
timestamp service is only meant for Apple-issued certificates) and names the
macOS files `-selfsigned`. As soon as the Developer ID secrets of 2.1 to 2.3
exist, they take precedence. Local signing with this certificate would need it
imported into the login keychain, which asks for the keychain password; the CI
route needs no local step.

Nothing in this section can be done by a script or by anyone but the account
holder: it involves identity checks, payment and private keys.

### 2.1 Apple Developer Program, as an individual

- Enrol at <https://developer.apple.com/programs/enroll/> with an Apple Account
  that has two-factor authentication turned on, using the legal name (Apple:
  "Using an alias, nickname, or company name as your first or last name will
  cause a delay in the approval of your enrollment"). Individual enrolment shows
  the legal name as seller and in the certificate, which is what QuadChroma
  wants: no company name appears anywhere.
- Fee: "99 USD ... in local currency where available", yearly, auto-renewing
  when enrolled through the Apple Developer app
  (<https://developer.apple.com/support/enrollment/>).
- Identity verification: in the Apple Developer app on iPhone or iPad with a
  photo ID, or on the web (own credit card, otherwise Apple asks for a copy of a
  government-issued photo ID). Apple's stated bound is a confirmation "within 24
  hours of your purchase"; forum reports for individuals are typically 24-48 hours.
- Afterwards the ten-character **Team ID** is shown under Membership details at
  <https://developer.apple.com/account>. It is needed for notarization and appears
  in the certificate name.

### 2.2 Developer ID Application certificate (no Xcode needed)

Only the Account Holder can create it; up to five exist at a time
(<https://developer.apple.com/help/account/create-certificates/create-developer-id-certificates>).
A "Developer ID Installer" certificate is *not* needed (no `.pkg`); the same
Application certificate also signs the DMG.

1. Create a certificate signing request with Keychain Access (Certificate
   Assistant > Request a Certificate from a Certificate Authority; e-mail
   `hello@quadchroma.tech`, common name e.g. `QuadChroma Developer ID`, CA e-mail
   empty, "Saved to disk"). The private key stays in the login keychain.
2. In Certificates, Identifiers & Profiles click **+**, choose **Developer ID**
   (Application), upload the request, download the `.cer`, double-click it.
3. Check: `security find-identity -p codesigning -v` lists
   `Developer ID Application: Robert Brandt (TEAMID)` as valid.
4. If it is not "valid" or `codesign` reports "unable to build chain": this Mac
   (Command Line Tools only, no Xcode) lacks Apple's intermediate certificate
   "Developer ID - G2". Download it from <https://www.apple.com/certificateauthority/>
   (`DeveloperIDG2CA.cer`, valid until 17 September 2031, SHA-256
   `f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a`) and
   import it into the login keychain (double-click, or
   `security add-certificates -k ~/Library/Keychains/login.keychain-db DeveloperIDG2CA.cer`).
5. Back up: export certificate plus private key from Keychain Access as a
   password-protected `.p12` and keep it offline. Apple does not have the private
   key; losing it means revoking and reissuing. The same `.p12` becomes the CI
   secret `APPLE_CERTIFICATE_P12_BASE64` (section 3.3).

The signing identity string used everywhere (`IDENT` in the Makefile,
`APPLE_SIGNING_IDENTITY` in CI) is exactly the name shown by `find-identity`,
for example `Developer ID Application: Robert Brandt (ABCDE12345)`. If two
identities share that name (for example after a renewal), use the 40-character
SHA-1 hash printed in the same listing instead - Apple's own advice for that
case; the Makefile accepts the hash wherever it accepts the name.

### 2.3 notarytool credentials

Either an **app-specific password** (account.apple.com > Sign-In and Security >
App-Specific Passwords; <https://support.apple.com/en-us/102654>) or an App Store
Connect **API key** (<https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api>).
Important: the API key must be a **Team key**; Apple: "Individual keys aren't
able to use ... notaryTool". The `.p8` file can be downloaded exactly once.

Store the credentials once in the keychain so no secret is ever typed on the
command line again (profile name `quadchroma-notary` is the Makefile default):

```sh
# app-specific password (asked for interactively)
xcrun notarytool store-credentials "quadchroma-notary" --apple-id "<Apple Account e-mail>" --team-id "<TEAMID>"

# or: Team API key
xcrun notarytool store-credentials "quadchroma-notary" --key ~/Secure/AuthKey_<KEYID>.p8 --key-id <KEYID> --issuer <ISSUER-UUID>

# test (Apple: "The best way to test your credentials is to get a history of recent activity.")
make notary-history
```

For CI the workflow uses the Apple Account e-mail, Team ID and app-specific
password directly (`--apple-id`), because there is no keychain profile on a
fresh runner.

## 3. One-time setup on the Windows side (needs the author)

There is no free path and no path that removes the SmartScreen warning
immediately (section 6). A signature is still mandatory: without one every new
version starts from zero reputation; with one, the publisher identity carries
reputation from version to version. Microsoft, "SmartScreen reputation for
Windows app developers" (<https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation>):
"Use a consistent signing identity - changing your signing certificate affects
the publisher trust signal."

Two facts shape the choice:

- Since 1 June 2023 the private key of every publicly trusted code-signing
  certificate must live in a hardware module or a CA-operated cloud HSM (CA/Browser
  Forum Code Signing Baseline Requirements v3.11.0, section 6.2.7.4.2:
  "the Subscriber's Private Key is generated, stored, and used in a suitable
  Hardware Crypto Module"). A `.pfx` with an exportable key no longer exists, so a
  certificate cannot simply be put into a GitHub secret. Certificates issued from
  1 March 2026 are valid for at most 460 days (section 6.3.2).
- EV certificates no longer buy instant reputation. Microsoft Trusted Root
  Program requirements 3.D.3: "Beginning in August 2024, all EV Code Signing OIDs
  will be removed ... and all Code Signing certificates will be treated equally";
  Microsoft's SmartScreen page: "EV certificates no longer bypass SmartScreen".

### 3.1 Option A (recommended today): OV certificate on the author's name, signed locally

| Provider / product | Price (26 Sep 2026) | Key storage | Signing in CI? |
|---|---|---|---|
| **Certum, Standard Code Signing "in the Cloud" (SimplySign)** | from 209 EUR per year (VAT to be checked on the invoice) | Certum cloud HSM; SimplySign Desktop app acts as a virtual smart card plus an OTP app | no (needs the desktop app and OTP) |
| Certum, Standard Code Signing with card set | card set from 169 EUR, renewal as "electronic code" from 139 EUR | cryptoCertum smart card and reader | no |
| SSL.com, OV/IV Code Signing ("IV" = individual validation) | 129 USD per year, plus eSigner cloud from 20 USD per month (20 signatures) or a YubiKey FIPS for 379 USD | eSigner cloud or YubiKey | yes with eSigner (`sslcom/esigner-codesign`, tagged only `@develop`) |
| DigiCert, Code Signing OV | 44 USD per month per certificate | KeyLocker cloud, token or own HSM | with KeyLocker |
| Sectigo, Code Signing OV | from 536.25 USD per year (five-year option) | token by post or own HSM | no |

Not suitable:

- **Certum "Open Source Code Signing"** (from 25 EUR): the certificate carries
  "Open Source Developer" as common name and organization, and Certum states "If
  Certum determines that the certificate is being used to sign software
  distributed commercially, the certificate will be revoked"
  (<https://support.certum.eu/en/code-signing-required-documents/>). QuadChroma is
  PolyForm Strict with planned commercial licensing; this does not fit.
- **SignPath Foundation** (free): "The project must use an OSI-approved Open
  Source license without commercial dual-licensing" (<https://signpath.org/terms>).
  PolyForm Strict is not OSI-approved.
- **EV certificates**: roughly 1.5 to 2 times the price (DigiCert 62 vs 44 USD
  per month, Certum cloud 379 vs 209 EUR), no SmartScreen advantage any more.

What the author does for Certum Standard (cloud): order on the personal name at
<https://www.certum.eu/en/code-signing-certificates/>, pass the identity check
(photo of a government ID, a utility bill on the same name, automatic video or
photo verification; issuance 1-5 days), install SimplySign Desktop and the
SimplySign OTP app on the Windows build machine, then read the certificate's SHA-1
thumbprint from `certmgr.msc` (Details > Thumbprint) and keep it in the environment
variable `QC_SIGN_THUMBPRINT`. Signing is then one command per release
(section 4.3). Keep the same name and CA at every renewal so the reputation
carries over.

### 3.2 Option B: Azure Artifact Signing (formerly "Trusted Signing"), signed in CI

Microsoft's cloud service issues short-lived (three-day) certificates from an
HSM; the GitHub Action `azure/artifact-signing-action` and `signtool` with the
`Azure.CodeSigning.Dlib.dll` use it. `release.yml` and `scripts/sign-windows.ps1`
support it already. But the decisive restriction in the Quickstart
(<https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart>, checked
26 Sep 2026): "Public Trust certificates are available to organizations in the
United States, Canada, the European Union, ... **Individual developers must be
located in the United States or Canada.**" A private person in Germany cannot
book it; an organization in the EU can, but then the certificate's common name is
the validated legal name of the organization ("CN values must always be the legal
entity's validated name", FAQ), which contradicts "no company name anywhere".
Keep this option in mind only if that ever changes.

If it becomes possible:

1. Paid Azure subscription (Pay-as-you-go; Free, Trial and sponsored subscriptions
   are excluded: "you must have a paid Azure subscription"), Basic SKU
   9.99 USD per month (5,000 signatures, one certificate profile), Premium
   99.99 USD (100,000 signatures, ten profiles); no pro-rata.
2. Register the resource provider `Microsoft.CodeSigning`; create an Artifact
   Signing account in an EU region, e.g. West Europe
   (`https://weu.codesigning.azure.net`); complete the identity validation in the
   portal (role "Artifact Signing Identity Verifier"); create a certificate
   profile of type **Public Trust**.
3. An Entra app registration for the signer with the role "Artifact Signing
   Certificate Profile Signer" on the profile. Either a client secret (secret
   `AZURE_CLIENT_SECRET`) or, better, a federated credential for GitHub OIDC
   (subject `repo:quadchroma-tech/quadchroma:ref:refs/tags/v0.1.0` per tag, or use an
   environment) and the secret `AZURE_SUBSCRIPTION_ID` instead of a client
   secret.
4. Fill in the `AZURE_*` secrets of section 3.3. The next tag push signs the exe.

Local signing with the same account: install the "Artifact Signing Client Tools"
(winget `Microsoft.Azure.ArtifactSigningClientTools`: signtool, .NET 8, VC++
runtime, Dlib), write a `metadata.json` with `Endpoint`, `CodeSigningAccountName`
and `CertificateProfileName`, sign in with `az login`, then
`powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe quadchroma.exe -TrustedSigning -Dlib <...>\bin\x64\Azure.CodeSigning.Dlib.dll -Metadata metadata.json`
(without `-Metadata`, pass `-Endpoint`, `-Account` and `-CertificateProfile`, or
set `QC_SIGN_ENDPOINT`, `QC_SIGN_ACCOUNT` and `QC_SIGN_PROFILE`).

### 3.2a GitHub repository page ("About", topics)

Repository: `github.com/quadchroma-tech/quadchroma` (the name "quadchroma" is taken
by an unrelated account). In the repository's "About" box (gear icon):

- **Description** (GitHub allows 350 characters; this is 281):
  `Remote desktop from a Mac to a Windows PC in HEVC 4:4:4 at 10 bit - hardware-encoded
  on Apple silicon, decoded on any Windows PC (in hardware on NVIDIA GPUs, otherwise in
  software). Sharp text, local mouse pointer, no account, no cloud, one program per
  side. Free for personal use.`
- **Website:** leave empty until `https://quadchroma.tech` exists (the domain is registered, the site is not online yet)
- **Topics:** `remote-desktop`, `screen-sharing`, `macos`, `windows`, `hevc`, `h265`,
  `yuv444`, `chroma-subsampling`, `10-bit`, `videotoolbox`, `nvdec`, `low-latency`,
  `apple-silicon`, `rust`, `objective-c`
- Tick "Releases", untick "Packages" and "Deployments" (not used).

GitHub shows the `README.md` in the repository root on the repository page. Do
not add a `README.md` under `.github/`: GitHub would show that one instead.

### 3.3 GitHub: secrets, settings, protection

**Secrets** (Settings > Secrets and variables > Actions > Secrets > New
repository secret). The release workflow reads them only in the job
`vorpruefung`, which passes on nothing but `true`/`false` switches. Every value
is one secret (never a JSON blob); a secret may be up to 48 KB, enough for a
Base64-encoded `.p12` ("You can use Base64 encoding to store small binary blobs as
secrets", GitHub). Secrets are not passed to workflows triggered by pull
requests from forks, and `ci.yml` uses none.

| Secret | Content | Where it comes from | Used by |
|---|---|---|---|
| `MAC_SELFSIGNED_P12_BASE64` | the free project certificate "QuadChroma Release" with private key, Base64 (section 2.0) | `~/Documents/quadchroma-signierung/quadchroma-release.p12.base64.txt` | `macos` job: temporary keychain, only while no Developer ID secrets exist |
| `MAC_SELFSIGNED_P12_PASSWORD` | password of that `.p12` | `~/Documents/quadchroma-signierung/p12-passwort.txt` | same |
| `APPLE_CERTIFICATE_P12_BASE64` | the Developer ID Application certificate with private key, Base64 | Keychain Access export of the identity as `.p12`, then `base64 -i DeveloperID.p12 \| pbcopy` | `macos` job: temporary keychain |
| `APPLE_CERTIFICATE_PASSWORD` | password chosen at the `.p12` export | you | `macos` job |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Robert Brandt (TEAMID)` (or the identity's 40-character SHA-1 hash) | `security find-identity -p codesigning -v` | `macos` job: `make sign IDENT=...` |
| `APPLE_ID` | Apple Account e-mail of the developer account | Apple | `macos` job: `notarytool --apple-id` |
| `APPLE_TEAM_ID` | ten-character Team ID | developer.apple.com/account, Membership details | `macos` job: `notarytool --team-id` |
| `APPLE_APP_SPECIFIC_PASSWORD` | app-specific password | account.apple.com > App-Specific Passwords | `macos` job: `notarytool --password` |
| `AZURE_ENDPOINT` | `https://weu.codesigning.azure.net` (region of the account) | Azure portal, Artifact Signing account | `windows-signieren` |
| `AZURE_CODE_SIGNING_ACCOUNT` | name of the Artifact Signing account | Azure portal | `windows-signieren` |
| `AZURE_CERT_PROFILE` | name of the Public Trust certificate profile | Azure portal | `windows-signieren` |
| `AZURE_TENANT_ID` | Directory (tenant) ID | Entra ID | `windows-signieren` |
| `AZURE_CLIENT_ID` | Application (client) ID of the app registration | Entra ID | `windows-signieren` |
| `AZURE_CLIENT_SECRET` | client secret of that app registration (variant 1) | Entra ID > Certificates & secrets | `windows-signieren`; omit when using OIDC |
| `AZURE_SUBSCRIPTION_ID` | subscription ID (variant 2, OIDC through `azure/login`, no long-lived secret) | Azure portal | `windows-signieren`; needs a federated credential on the app registration |

Rules of thumb: all three `APPLE_CERTIFICATE_*`/`APPLE_SIGNING_IDENTITY`
secrets switch the Developer ID signature on; all three `APPLE_ID`,
`APPLE_TEAM_ID`, `APPLE_APP_SPECIFIC_PASSWORD` additionally switch notarization
on; the five `AZURE_ENDPOINT`, `AZURE_CODE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE`,
`AZURE_TENANT_ID`, `AZURE_CLIENT_ID` plus either `AZURE_CLIENT_SECRET` or
`AZURE_SUBSCRIPTION_ID` switch the Windows signature on. An incomplete set
produces a warning annotation on the `vorpruefung` job and an unsigned artefact,
never a failure; the job summary only lists the resulting on/off switches.
No secret is needed for creating the draft release (`GITHUB_TOKEN` with
`contents: write` in the `release` job only).

Optional hardening: create an environment named `release` (Settings >
Environments) with a deployment rule "protected tags" for `v*`, move the secrets
there and add `environment: release` to the jobs `vorpruefung`,
`windows-signieren` and `macos`. Then the secrets are exposed only to runs on
release tags.

**GitHub CLI** (once; needed for the local path in section 4.3 and for
replacing assets in a draft, section 4.2): neither the Mac nor the Windows build
machine has `gh` today. Install it with `brew install gh` on the Mac or
`winget install --id GitHub.cli --source winget` on Windows
(<https://github.com/cli/cli/blob/trunk/docs/install_macos.md>,
<https://github.com/cli/cli/blob/trunk/docs/install_windows.md>), then run
`gh auth login` once. Everything `gh` does here can also be done by hand in the
web UI of the draft release.

**Repository settings** worth doing once:

- Actions permissions: leave the default (GITHUB_TOKEN read-only; the workflows
  request `contents: write` only where needed). Under "Fork pull request
  workflows" keep "Require approval for first-time contributors".
- **Private vulnerability reporting**: Settings > Security and quality >
  Advanced Security > **Enable** next to "Private vulnerability reporting". GitHub:
  it "gives security researchers a secure, structured way to disclose
  vulnerabilities directly in your repository"; they then see a "Report a
  vulnerability" button on the Advisories page. `SECURITY.md` points there.
- **Branch ruleset for `main`** (Settings > Code and automation > Rulesets > New
  branch ruleset): "Restrict deletions", "Block force pushes", "Require status
  checks to pass before merging" with the two CI checks `Windows
  (x86_64-pc-windows-msvc)` and `macOS (Host, Pruefstaende, Verpackung, Client)`.
  "Require a pull request before merging" is sensible once there is a second
  contributor; for a solo maintainer it only adds ceremony.
- **Tag ruleset for `v*`** (New tag ruleset): "Restrict creations" and "Restrict
  deletions", with the repository owner in the bypass list. GitHub: "only users
  with bypass permissions can create branches or tags whose name matches the
  pattern you specify". A release tag starts the release workflow, so nobody else
  should be able to push one.
- **Dependabot** for GitHub Actions is configured in `.github/dependabot.yml`
  (weekly pull requests that move the pinned commit SHAs and their tag
  comments). Cargo dependencies are deliberately not covered: no new crates, no
  unrequested updates.

## 4. Release procedure

### 4.1 Version checklist (before tagging)

The tag, `client/Cargo.toml`, `client/Cargo.lock` and `host/Info.plist` must
agree; the `vorpruefung` job refuses the release otherwise.

1. `client/Cargo.toml`: `version = "X.Y.Z"`.
2. `client/Cargo.lock`: run `cargo build` once in `client/` (without `--locked`)
   so the lock file's own `quadchroma` entry follows; commit it. CI builds with
   `--locked` and fails on a stale lock file.
3. `host/Info.plist`: `CFBundleShortVersionString` = `X.Y.Z`; increase
   `CFBundleVersion` by one (Apple: "increment the build version before you
   distribute a build"). The Makefile reads the version from this file for the
   package names.
4. Windows version resource, icon and manifest come from `Cargo.toml` through
   `client/build.rs` - nothing to edit. Both workflows build with
   `QC_RESSOURCEN_PFLICHT=1` (a runner without `rc.exe`/`llvm-rc` fails at
   once), and `ci.yml` checks that the embedded `ProductVersion` equals the
   crate version.
5. `THIRD_PARTY_NOTICES.txt` (section 2) must describe the FFmpeg build that
   `scripts/build-ffmpeg-windows.sh` produces: FFmpeg version, SHA-256 of the two
   source archives and the configure line. If you change the script, update
   section 2 in the same commit. The release workflow builds FFmpeg fresh with
   the script, ships `avcodec-63.dll`, `avutil-61.dll` and
   `FFMPEG-BUILDINFO.txt`, and attaches the source archives and the script to
   the release (LGPL v2.1 section 6).
6. The four companion files `LICENSE.txt`, `THIRD_PARTY_NOTICES.txt`,
   `README.md` and `MANUAL.txt` exist on `main`
   (the first two come from the licensing work and must land before the first
   release tag). Both packaging paths refuse to package without them: the
   `windows-paket` job stops with the name of the missing file, and `make dmg`
   does the same with a Developer ID identity or `DMG_EXTRA_REQUIRED=1` (as in
   CI).
7. Status lines in `README.md` and `MANUAL.txt`.
8. Commit on `main`, then `git tag -a vX.Y.Z -m "QuadChroma X.Y.Z"` and
   `git push origin vX.Y.Z`.

### 4.2 Through CI (`release.yml`)

1. Push the tag. The workflow runs `vorpruefung` (versions, switches),
   `windows` (build, 300 tests: 299 run, 1 ignored; exe and DLLs), `windows-signieren` (only with
   Azure secrets), `windows-paket` (ZIP), `macos` (build, `make sign`, notarize,
   `make staple`, `make dmg`, notarize the DMG, `make staple-dmg`,
   `make gatekeeper`) and `release` (`SHA256SUMS.txt`, draft).
2. Open the draft under Releases. Check the job summary (which signatures were
   on), the notary logs artefact (`notary-logs`, always read it: Apple says the
   log "might contain warnings that you can fix prior to your next submission"),
   the file names (no unexpected `-unsigned`), and `SHA256SUMS.txt`.
3. Download and verify at least one file per platform (section 4.4). On the Mac
   verify a real download (Safari sets the quarantine attribute; `curl` and
   `scp` do not).
4. Edit the notes, then **Publish release**. Drafts are invisible to users; a
   published release is immutable enough that a mistake means a new version.

If the Windows exe came out unsigned because the author signs locally (option A
in section 3.1): download `quadchroma-X.Y.Z-windows-x64-unsigned.zip`, sign the
exe on the Windows machine with `scripts/sign-windows.ps1`, rebuild the ZIP
with the same layout (section 7) under the name without `-unsigned`, then
replace the asset and the checksum file in the draft:

```sh
gh release upload vX.Y.Z quadchroma-X.Y.Z-windows-x64.zip --clobber
gh release delete-asset vX.Y.Z quadchroma-X.Y.Z-windows-x64-unsigned.zip --yes
sha256sum quadchroma-X.Y.Z-windows-x64.zip QuadChroma-X.Y.Z-macos-arm64.zip QuadChroma-X.Y.Z-macos-arm64.dmg > SHA256SUMS.txt
gh release upload vX.Y.Z SHA256SUMS.txt --clobber
```

(`--clobber`: "Delete and re-upload existing assets of the same name"; `--yes`:
"Skip the confirmation prompt". Needs the GitHub CLI, section 3.3; the web UI
of the draft release does the same by hand.)

To rerun after a failure, fix the cause, delete the draft and the tag
(`gh release delete vX.Y.Z`, `git push --delete origin vX.Y.Z`,
`git tag -d vX.Y.Z`) and tag again. A first dry run without any secrets is
recommended: tag `v0.1.0`, inspect the draft, delete it again.

### 4.3 Locally, without CI

macOS host (on the Mac, in a normal terminal - `hdiutil`, `spctl` and the
notary service do not work inside sandboxes; needs internet for the timestamp,
the notary upload and the ticket):

```sh
make                                                              # build; signs with the local "QuadChroma Dev" identity
make release IDENT="Developer ID Application: Robert Brandt (TEAMID)"
# = make sign, verify, notarize, staple, notarize-dmg (builds the DMG), staple-dmg, gatekeeper
ls -l build/QuadChroma-*-macos.zip build/QuadChroma-*.dmg          # the two files to upload
```

Individual steps, if something in the chain needs a second look: `make sign
IDENT=...` (re-sign; the Makefile adds `--timestamp` automatically for a
Developer ID identity), `make verify` (`codesign --verify --deep --strict`,
entitlements must not contain `get-task-allow`, `syspolicy_check distribution`,
Gatekeeper preview), `make notarize` (ZIP, `notarytool submit --wait`, log to
`build/notary-log.json`), `make staple` (ticket onto the app, ZIP rebuilt),
`make dmg` (app plus `/Applications` link plus the four companion files, UDZO,
signed with identifier `tech.quadchroma.host.dmg`; a missing companion file is
an error with a Developer ID or `DMG_EXTRA_REQUIRED=1`, a warning otherwise),
`make notarize-dmg` (rebuilds the DMG first), `make staple-dmg`, `make
gatekeeper` (`spctl` must say `accepted`, `source=Notarized Developer ID`;
prints the SHA-256 of both files). `make notarize` deliberately fails with "The
binary is not signed with a valid Developer ID certificate." as long as `IDENT`
is not a Developer ID.

Three details of the Makefile worth knowing. `IDENT` may also be the
40-character SHA-1 hash from `security find-identity`; the Makefile treats such
a hash as a Developer ID (adds `--timestamp`, `make release` accepts it) - if a
hash ever belongs to a non-Apple certificate, pass `TIMESTAMP=` (empty). Before
uploading anything, `make release` checks the identity, the timestamp option and
the four companion files (`LICENSE.txt`, `THIRD_PARTY_NOTICES.txt`, `README.txt` made from `README.md`,
`MANUAL.txt`) and stops with `ERROR:` if one is missing. And an explicitly
given `IDENT` is enforced: `make IDENT=...` (likewise `make verify`, `make zip`,
`make dmg` with `IDENT=...`) re-signs the existing app without rebuilding when
the identity differs from the last signature (recorded in `build/.ident`),
whereas `make` without `IDENT` leaves the existing signature - and a stapled
ticket - untouched.

Windows exe (on the build machine, PowerShell, from the repository root; the
FFmpeg LGPL build lives in `FFMPEG_DIR`, `LIBCLANG_PATH` points to LLVM's `bin`,
`RUSTFLAGS` must not be set):

```powershell
cd client
$env:QC_RESSOURCEN_PFLICHT = "1"   # build.rs: fail instead of warn when no resource compiler is found
cargo build --release              # build.rs embeds version, icon and manifest
cargo test --release               # 300 tests (299 run, 1 ignored); FFmpeg bin folder on PATH, APPDATA pointed at a scratch folder
cd ..
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe client\target\release\quadchroma.exe -Thumbprint <SHA-1 thumbprint>   # or QC_SIGN_THUMBPRINT
```

`client/build.rs` has three switches, all environment variables.
`QC_RESSOURCEN_PFLICHT=1` turns a missing resource compiler (`rc.exe` from the
Windows SDK, or `llvm-rc.exe`) from a warning into a build error - set it for
every release build; both workflows set it too. `QC_RC=<path>` names the
resource compiler explicitly. `QC_OHNE_RESSOURCEN=1` builds without version
resource, icon and manifest (experiments only, never a release). Cargo caches
the result of the compiler search: after installing the SDK or LLVM, run
`cargo clean -p quadchroma` once (or touch `build.rs`), otherwise the old exe
without resources stays in `target/`; a switch into or out of a Developer
Command Prompt (`WindowsSdkVerBinPath`) re-runs the search by itself. Check the
result with `(Get-Item client\target\release\quadchroma.exe).VersionInfo`
(section 4.4).

PowerShell's default execution policy on Windows (`Restricted`) refuses to load
any script, so call the script through `powershell -NoProfile -ExecutionPolicy
Bypass -File scripts\sign-windows.ps1 ...` (or `pwsh -File`, as `release.yml`
does), not as `.\scripts\sign-windows.ps1`; alternatively run
`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` once and `Unblock-File` on
a copy that came out of a downloaded ZIP.

The script runs `signtool sign /v /fd SHA256 /tr http://timestamp.digicert.com
/td SHA256 /d QuadChroma /du https://github.com/quadchroma-tech/quadchroma /sha1 <thumbprint>`
followed by `signtool verify /pa /v` and exits non-zero if the signature does not
verify. SimplySign asks for the OTP when signtool touches the key. Then build
the ZIP with the layout of section 7 (top-level folder `quadchroma-X.Y.Z/`, the
exe, `avcodec-63.dll`, `avutil-61.dll` and `BUILDINFO.txt` renamed to
`FFMPEG-BUILDINFO.txt` from the output of `scripts/build-ffmpeg-windows.sh`,
`LICENSE.txt`, `THIRD_PARTY_NOTICES.txt`, `README.txt` and `MANUAL.txt`, as written by `scripts/package-texts.sh`), and
publish the files in its `quellen/` folder (`ffmpeg-9.0.2.tar.xz`, the
nv-codec-headers archive and `build-ffmpeg-windows.sh`) on the same release
page.

Publish:

```sh
sha256sum quadchroma-X.Y.Z-windows-x64.zip QuadChroma-X.Y.Z-macos-arm64.zip QuadChroma-X.Y.Z-macos-arm64.dmg > SHA256SUMS.txt
gh release create vX.Y.Z --draft --verify-tag --title "QuadChroma X.Y.Z" --notes-file notes.md \
   quadchroma-X.Y.Z-windows-x64.zip QuadChroma-X.Y.Z-macos-arm64.zip QuadChroma-X.Y.Z-macos-arm64.dmg SHA256SUMS.txt
```

(`--draft`: "Save the release as a draft instead of publishing it";
`--verify-tag`: "Abort in case the git tag doesn't already exist in the remote
repository".) Rename the local Makefile outputs (`QuadChroma-X.Y.Z-macos.zip`,
`QuadChroma-X.Y.Z.dmg`) to the release names first, or pass `ZIP=` and `DMG=`
to `make`.

### 4.4 Verifying a release

macOS, on a downloaded copy (quarantined; unpack the ZIP with the Finder or
`ditto -x -k` - the command-line `unzip` works too, because the archive carries
no AppleDouble `._*` entries):

```sh
xattr -p com.apple.quarantine QuadChroma.app          # attribute present = real download
codesign --verify --deep --strict -vvv QuadChroma.app
codesign -dvv QuadChroma.app 2>&1 | grep -E 'Authority|TeamIdentifier|Timestamp'
spctl --assess --type execute -vv QuadChroma.app      # expected: accepted, source=Notarized Developer ID
xcrun stapler validate -v QuadChroma.app              # "The validate action worked!"
spctl --assess --type open --context context:primary-signature -vv QuadChroma-X.Y.Z-macos-arm64.dmg
shasum -a 256 -c SHA256SUMS.txt
```

Expected outputs on a correctly notarized app: `Authority=Developer ID
Application: Robert Brandt (TEAMID)`, `Authority=Developer ID Certification
Authority`, `Authority=Apple Root CA`, a `Timestamp=` line (a `Signed Time=`
line instead means no secure timestamp), `TeamIdentifier=TEAMID`, and
`accepted`. An ad-hoc or "QuadChroma Dev" build shows `rejected`.

Windows:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe quadchroma.exe -VerifyOnly   # signtool verify /pa /v + Get-AuthenticodeSignature
Get-AuthenticodeSignature quadchroma.exe | Format-List Status, SignerCertificate, TimeStamperCertificate
(Get-Item quadchroma.exe).VersionInfo | Format-List ProductVersion, CompanyName, LegalCopyright
Get-FileHash -Algorithm SHA256 quadchroma-X.Y.Z-windows-x64.zip
```

Expected: `Status: Valid`, signer subject with the author's name, a timestamp
certificate, `ProductVersion X.Y.Z`, `CompanyName Robert Brandt`. Right-click >
Properties > Digital Signatures shows the same to users.

## 5. What the CI does with the credentials

- The `.p12` is decoded into `$RUNNER_TEMP`, imported into a temporary keychain
  created for the job (`security create-keychain`, `set-keychain-settings -lut
  21600`, `import ... -A -t cert -f pkcs12`, `set-key-partition-list -S
  apple-tool:,apple:`, `list-keychain -d user -s`, exactly the commands from
  GitHub's "Installing an Apple certificate on macOS runners"), the file is
  deleted at once, and the keychain is deleted in a final step that also runs on
  failure. Apple's "Developer ID - G2" intermediate certificate is fetched from
  apple.com with a pinned SHA-256 and imported too, in case the `.p12` does not
  carry the chain.
- notarytool receives Apple ID, Team ID and app-specific password as command
  arguments inside the runner only; the helper script is deleted afterwards.
  Notary logs are uploaded as the artefact `notary-logs`.
- Azure credentials go straight into `azure/login` (OIDC) or the signing action
  (client secret). The `windows-signieren` job is the only one with `id-token:
  write`.
- Nothing echoes a secret; GitHub masks secret values in logs anyway, and the
  `vorpruefung` job outputs only `true`/`false`.
- All actions are pinned to full commit SHAs (tag in a comment), `persist-credentials:
  false` on every checkout, `contents: read` everywhere except the `release` job.

## 6. What still warns after signing

**Windows, SmartScreen.** Even a correctly signed exe is "flagged as
unrecognized until reputation accumulates" (Microsoft's table for "Valid
Certificate (OV/EV)"); the difference is that the dialog then shows the verified
publisher name instead of "Unknown publisher". Reputation grows with downloads:
"it can take several weeks and hundreds of clean installs from a wide audience";
there is no application process for end users. The browser download warning in
Edge ("isn't commonly downloaded") follows the same rule. Windows 11 with Smart
App Control blocks *unsigned* programs without a "Run anyway" option. Release
notes should therefore say: download only from the GitHub release page, compare
the SHA-256 with `SHA256SUMS.txt`, then "More info" > "Run anyway", and check
that the dialog names the publisher.

**macOS, Gatekeeper.** A notarized Developer ID app still gets one dialog on
first launch ("asks if you're sure that you want to open it"; Apple's text adds
that Apple checked it for malicious software), but no warning and no detour
through System Settings. Recommend moving the app to `/Applications` before the
first start (Gatekeeper otherwise runs it from a randomised path on first
launch - App Translocation; harmless for the host, but confusing).

**macOS, TCC re-grants after the identity change.** macOS stores the Screen
Recording and Accessibility permissions against the app's designated requirement,
which contains the bundle identifier *and* the signing certificate (development
build) or Team ID (Developer ID). The first release changes both at once - new
bundle identifier `tech.quadchroma.host` and the Developer ID certificate - so
users of an earlier development build see **both prompts again, exactly once**.
This is expected, not a bug, and it is why bundle identifier and certificate
were changed in one step rather than two. Afterwards a renewed or replaced
Developer ID certificate of the same Team ID keeps the grants. Leftover entries
for the previous bundle identifier can be removed with `tccutil reset
ScreenCapture <old-bundle-id>` and `tccutil reset Accessibility <old-bundle-id>`
(never without the bundle id: that resets every app). The host no longer quits
without Screen Recording: it waits, shows the missing permission in its menu bar
menu and checks again every 3 s. Whether macOS lets the running host use a permission
granted meanwhile without a restart is not verified on a device yet; if the menu
still shows it as missing, quit the host from its menu and start it again.

**Unsigned releases (until the certificates exist).** macOS refuses an ad-hoc or
locally signed app; since macOS 15 the Control-click trick no longer works
(Apple: "users will no longer be able to Control-click to override Gatekeeper").
The only user path is System Settings > Privacy & Security > "Open Anyway" after
the first failed attempt, or removing the quarantine attribute (`xattr -d -r
com.apple.quarantine QuadChroma.app`). On Windows it is "More info" > "Run
anyway", and no path at all under Smart App Control. Label such builds clearly
as unsigned pre-releases. The local "QuadChroma Dev" identity exists only to
keep the TCC grants stable on the development Mac; it is never used for
distribution. Homebrew casks are out of reach for now regardless of signing
(notability thresholds: 30 forks, 30 watchers or 75 stars - 90 forks, 90
watchers or 225 stars for a self-submission by the repository owner; repository
at least 30 days old).

## 7. Contents of the release files

| File | Contents |
|---|---|
| `quadchroma-X.Y.Z-windows-x64.zip` (`-unsigned` if not signed) | folder `quadchroma-X.Y.Z/` with `quadchroma.exe`, `avcodec-63.dll`, `avutil-61.dll` (FFmpeg 9.0.2, minimal LGPL build by `scripts/build-ffmpeg-windows.sh`), `FFMPEG-BUILDINFO.txt` (sizes, SHA-256 and configuration of the two DLLs), `LICENSE.txt`, `THIRD_PARTY_NOTICES.txt`, `README.txt`, `MANUAL.txt` |
| `ffmpeg-9.0.2.tar.xz`, `nv-codec-headers-<commit>.tar.gz`, `build-ffmpeg-windows.sh` | the complete corresponding source of the FFmpeg DLLs (LGPL v2.1 section 6); the release notes carry the sentence from the FFmpeg license checklist with a link to the source archive |
| `QuadChroma-X.Y.Z-macos-arm64.zip` (`-unsigned` / `-unnotarized`) | `QuadChroma.app` as the top-level entry (`ditto --keepParent --norsrc`: no AppleDouble `._*` entries, so the Finder, `ditto -x -k` and the command-line `unzip` all restore a valid bundle), signed, notarized and stapled - the form the notary service accepts |
| `QuadChroma-X.Y.Z-macos-arm64.dmg` (`-unsigned` / `-unnotarized`) | `QuadChroma.app` (stapled), a link to `/Applications`, `LICENSE.txt`, `THIRD_PARTY_NOTICES.txt`, `README.txt`, `MANUAL.txt`; UDZO image, signed with identifier `tech.quadchroma.host.dmg`, notarized and stapled |
| `SHA256SUMS.txt` | `sha256sum` of every file above (`sha256sum -c SHA256SUMS.txt`) |

Not released: the Mac client, the test harnesses, `.pdb` files, the CI's
`quadchroma-*-ci` artefacts (unsigned, seven days, for trying out a pull
request only).

## 8. Sources (checked 26 September 2026)

Apple: <https://developer.apple.com/programs/enroll/>,
<https://developer.apple.com/support/enrollment/>,
<https://developer.apple.com/help/account/create-certificates/create-developer-id-certificates>,
<https://developer.apple.com/help/account/create-certificates/create-a-certificate-signing-request>,
<https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution>,
<https://developer.apple.com/documentation/security/customizing-the-notarization-workflow>,
<https://developer.apple.com/documentation/security/resolving-common-notarization-issues>,
<https://developer.apple.com/documentation/xcode/creating-distribution-signed-code-for-the-mac>,
<https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution>,
<https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements>,
<https://developer.apple.com/documentation/technotes/tn3147-migrating-to-the-latest-notarization-tool>,
<https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api>,
<https://support.apple.com/en-us/102654>, <https://support.apple.com/en-us/102445>,
<https://developer.apple.com/news/?id=saqachfa>,
<https://www.apple.com/certificateauthority/>.

Microsoft: <https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation>,
<https://learn.microsoft.com/en-us/security/trusted-root/program-requirements>,
<https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart>,
<https://learn.microsoft.com/en-us/azure/artifact-signing/faq>,
<https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-signing-integrations>,
<https://learn.microsoft.com/en-us/azure/artifact-signing/how-to-change-sku>,
<https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool>,
<https://support.microsoft.com/en-us/windows/what-is-smart-app-control-285ea03d-fa88-4d56-882e-6698afdb7003>.

Certificate authorities: <https://cabforum.org/working-groups/code-signing/requirements/>
(Code Signing Baseline Requirements v3.11.0, 16 June 2026),
<https://www.certum.eu/en/code-signing-certificates/>,
<https://shop.certum.eu/data-safety/code-signing-certificates.html>,
<https://support.certum.eu/en/code-signing-required-documents/>,
<https://www.ssl.com/products/software-integrity/code-signing/ov/>,
<https://www.ssl.com/guide/esigner-pricing-for-code-signing/>,
<https://www.digicert.com/signing/code-signing-certificates>,
<https://www.sectigo.com/ssl-certificates-tls/code-signing>,
<https://signpath.org/terms>.

GitHub: <https://docs.github.com/en/actions/how-tos/deploy/deploy-to-third-party-platforms/sign-xcode-applications>,
<https://docs.github.com/en/actions/how-tos/write-workflows/choose-what-workflows-do/use-secrets>,
<https://docs.github.com/en/actions/reference/workflows-and-actions/contexts>,
<https://docs.github.com/en/actions/reference/security/oidc>,
<https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions>,
<https://docs.github.com/en/code-security/security-advisories/working-with-repository-security-advisories/configuring-private-vulnerability-reporting-for-a-repository>,
<https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/available-rules-for-rulesets>,
<https://docs.github.com/en/code-security/dependabot/working-with-dependabot/keeping-your-actions-up-to-date-with-dependabot>,
<https://cli.github.com/manual/gh_release_create>, <https://cli.github.com/manual/gh_release_upload>,
<https://cli.github.com/manual/gh_release_delete-asset>,
<https://github.com/cli/cli/blob/trunk/docs/install_macos.md>,
<https://github.com/cli/cli/blob/trunk/docs/install_windows.md>,
<https://github.com/actions/runner-images> (images `windows-2025`, `macos-26`),
<https://github.com/Azure/artifact-signing-action>, <https://github.com/Azure/login>.

FFmpeg: <https://ffmpeg.org/legal.html> (license compliance checklist),
<https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>,
<https://github.com/FFmpeg/nv-codec-headers>.

Homebrew: <https://docs.brew.sh/Package-Acceptance-Policy> (cask notability
thresholds).
