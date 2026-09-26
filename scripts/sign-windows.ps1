#Requires -Version 5.1
<#
sign-windows.ps1 - sign quadchroma.exe with Authenticode (SHA-256,
RFC 3161 timestamp) and verify the signature. The parameter names follow
signtool and the CI; the old German parameter names remain available as
aliases.

Two ways:
  Certificate in the Windows certificate store (default): a code signing
      certificate from a CA, with the key on a token/smart card or in the
      CA's cloud CSP (e.g. Certum SimplySign); selected by its SHA-1
      thumbprint (-Thumbprint, otherwise the environment variable
      QC_SIGN_THUMBPRINT). The CSP asks for the PIN or one-time code
      while signing.
  -TrustedSigning: Azure Artifact Signing (the name of "Trusted Signing"
      since January 2026): signtool with Azure.CodeSigning.Dlib.dll (-Dlib)
      and a metadata.json that the script generates from -Endpoint, -Account
      and -CertificateProfile (or takes ready-made from -Metadata).
      Log in beforehand with "az login" or via AZURE_TENANT_ID,
      AZURE_CLIENT_ID, AZURE_CLIENT_SECRET (in CI via OIDC). Needs the
      .NET 8 runtime next to signtool.

Prerequisites (set up by the maintainer): a certificate or an Artifact
Signing account (see RELEASING.md). Without either, -VerifyOnly only shows
the signature status. A signature does not remove the SmartScreen warning
right away; it makes sure that the publisher's reputation carries over from
version to version - so sign every version with the same identity.

Invocation: Windows ships with the execution policy "Restricted", which
does not load any scripts (".\scripts\sign-windows.ps1" then fails with
"running scripts is disabled on this system"). So call the script via
powershell -NoProfile -ExecutionPolicy Bypass -File (or pwsh -File as in
release.yml; PowerShell 7 defaults to RemoteSigned). To call it directly
instead: run "Set-ExecutionPolicy -Scope CurrentUser RemoteSigned" once,
and "Unblock-File" on a copy downloaded from the internet.

Examples (in the root directory of the repository):
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe client\target\release\quadchroma.exe -Thumbprint 0123ABCD...
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe quadchroma.exe -TrustedSigning -Endpoint https://weu.codesigning.azure.net -Account <account> -CertificateProfile <profile> -Dlib C:\tools\bin\x64\Azure.CodeSigning.Dlib.dll
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe quadchroma.exe -VerifyOnly

Exit code 0 if the signature is valid and the chain is trusted
(signtool verify /pa), otherwise the exit code of signtool.
#>
[CmdletBinding()]
param(
    # File(s) to sign: the exe, optionally DLLs as well.
    [Parameter(Mandatory, Position = 0)] [string[]] $Exe,

    # Way 1: SHA-1 thumbprint of the certificate in the store (certmgr.msc >
    # Details > Thumbprint; spaces and colons are ignored).
    [string] $Thumbprint = $env:QC_SIGN_THUMBPRINT,
    # The certificate is in the machine store (LocalMachine) instead of the
    # user store (signtool /sm).
    [switch] $MachineStore,

    # Way 2: Azure Artifact Signing (formerly Trusted Signing).
    [Alias('ArtifactSigning')] [switch] $TrustedSigning,
    # Endpoint of the region of the account and profile, e.g.
    # https://weu.codesigning.azure.net (West Europe).
    [string] $Endpoint = $env:QC_SIGN_ENDPOINT,
    # Name of the Artifact Signing account.
    [string] $Account = $env:QC_SIGN_ACCOUNT,
    # Name of the certificate profile (type Public Trust).
    [string] $CertificateProfile = $env:QC_SIGN_PROFILE,
    # Path to Azure.CodeSigning.Dlib.dll (NuGet Microsoft.ArtifactSigning.Client,
    # folder bin\x64 for the x64 signtool).
    [string] $Dlib = $env:QC_SIGN_DLIB,
    # Ready-made metadata.json (Endpoint, CodeSigningAccountName,
    # CertificateProfileName); -Endpoint/-Account/-CertificateProfile are
    # not needed then.
    [string] $Metadata = $env:QC_SIGN_METADATA,

    # RFC 3161 timestamp service. Empty = DigiCert; with -TrustedSigning the
    # Microsoft service (the certificates there are valid for only three
    # days, so the timestamp is mandatory).
    [string] $TimestampUrl = $env:QC_TIMESTAMP_URL,
    # signtool.exe; empty = QC_SIGNTOOL, PATH, otherwise the newest Windows SDK
    # under "Windows Kits".
    [string] $Signtool = $env:QC_SIGNTOOL,
    # Description and URL in the signature (/d, /du); Windows shows the
    # description in its dialogs.
    [string] $Description = 'QuadChroma',
    [string] $DescriptionUrl = 'https://github.com/quadchroma-tech/quadchroma',
    # Sign nothing, only verify (signtool verify /pa /v and
    # Get-AuthenticodeSignature).
    [switch] $VerifyOnly
)
$ErrorActionPreference = 'Stop'
# PowerShell 7: a non-zero exit code from signtool must not end as an
# exception; it is passed on below as the exit code of the script.
$PSNativeCommandUseErrorActionPreference = $false

# Find signtool.exe: explicit path, PATH, otherwise the newest Windows SDK.
function Find-Signtool {
    if ($Signtool) {
        if (Test-Path -LiteralPath $Signtool -PathType Leaf) { return (Resolve-Path -LiteralPath $Signtool).Path }
        throw "signtool not found: $Signtool"
    }
    $c = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($c) { return $c.Source }
    $arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
    foreach ($pf in @(${env:ProgramFiles(x86)}, $env:ProgramFiles)) {
        if (-not $pf) { continue }
        $bin = Join-Path $pf 'Windows Kits\10\bin'
        if (-not (Test-Path -LiteralPath $bin)) { continue }
        $k = Get-ChildItem -LiteralPath $bin -Directory -Filter '10.*' |
            Sort-Object { [version]$_.Name } -Descending |
            ForEach-Object { Join-Path $_.FullName "$arch\signtool.exe" } |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
            Select-Object -First 1
        if ($k) { return $k }
    }
    throw 'signtool.exe not found: install the Windows SDK or set -Signtool or QC_SIGNTOOL'
}

$tool = Find-Signtool
Write-Host "signtool: $tool"
$dateien = foreach ($d in $Exe) {
    if (-not (Test-Path -LiteralPath $d -PathType Leaf)) { throw "File not found: $d" }
    (Resolve-Path -LiteralPath $d).Path
}

if (-not $VerifyOnly) {
    if (-not $TimestampUrl) {
        $TimestampUrl = if ($TrustedSigning) { 'http://timestamp.acs.microsoft.com' } else { 'http://timestamp.digicert.com' }
    }
    # /fd and /td are mandatory since SDK 20236; SHA-256 for the file and the timestamp.
    $argumente = @('sign', '/v', '/fd', 'SHA256', '/tr', $TimestampUrl, '/td', 'SHA256', '/d', $Description, '/du', $DescriptionUrl)
    $metadataTemp = $null
    if ($TrustedSigning) {
        if (-not $Dlib) { throw '-TrustedSigning needs -Dlib <Azure.CodeSigning.Dlib.dll> (or the environment variable QC_SIGN_DLIB)' }
        if (-not (Test-Path -LiteralPath $Dlib -PathType Leaf)) { throw "Dlib not found: $Dlib" }
        if ($Metadata) {
            if (-not (Test-Path -LiteralPath $Metadata -PathType Leaf)) { throw "metadata.json not found: $Metadata" }
            $metadataPfad = (Resolve-Path -LiteralPath $Metadata).Path
        } else {
            # Parameter -> environment variable that serves as its default (see param()).
            $umgebung = [ordered]@{ Endpoint = 'QC_SIGN_ENDPOINT'; Account = 'QC_SIGN_ACCOUNT'; CertificateProfile = 'QC_SIGN_PROFILE' }
            foreach ($name in $umgebung.Keys) {
                if (-not (Get-Variable -Name $name -ValueOnly)) { throw "-TrustedSigning needs -$name (or the environment variable $($umgebung[$name])) - or a ready-made -Metadata metadata.json" }
            }
            # metadata.json for the Dlib. The endpoint must match the region of
            # the account AND the profile, otherwise the service responds with 403.
            $metadataTemp = Join-Path ([System.IO.Path]::GetTempPath()) ("quadchroma-signing-{0}.json" -f [guid]::NewGuid())
            $json = [ordered]@{
                Endpoint               = $Endpoint
                CodeSigningAccountName = $Account
                CertificateProfileName = $CertificateProfile
                CorrelationId          = 'quadchroma-release'
            } | ConvertTo-Json
            # UTF-8 without BOM (Set-Content -Encoding UTF8 would write a BOM in PowerShell 5.1).
            [System.IO.File]::WriteAllText($metadataTemp, $json, (New-Object System.Text.UTF8Encoding $false))
            $metadataPfad = $metadataTemp
        }
        $argumente += @('/dlib', (Resolve-Path -LiteralPath $Dlib).Path, '/dmdf', $metadataPfad)
    } else {
        if (-not $Thumbprint) { throw 'Thumbprint missing: set -Thumbprint <sha1> or QC_SIGN_THUMBPRINT (or use -TrustedSigning or -VerifyOnly)' }
        $Thumbprint = $Thumbprint -replace '[\s:]', ''
        if ($Thumbprint -notmatch '^[0-9A-Fa-f]{40}$') { throw "Thumbprint is not a SHA-1 hash (40 hex characters): $Thumbprint" }
        $argumente += @('/sha1', $Thumbprint)
        if ($MachineStore) { $argumente += '/sm' }
    }
    $argumente += $dateien
    Write-Host "signtool $($argumente -join ' ')"
    try {
        & $tool @argumente
        if ($LASTEXITCODE -ne 0) { throw "signtool sign: exit code $LASTEXITCODE" }
    } finally {
        if ($metadataTemp) { Remove-Item -LiteralPath $metadataTemp -ErrorAction SilentlyContinue }
    }
}

# Verify: /pa = default Authenticode policy (without /pa signtool checks
# against the rules for drivers and reports valid signatures as errors).
$fehler = 0
foreach ($d in $dateien) {
    $s = Get-AuthenticodeSignature -LiteralPath $d
    $unterzeichner = if ($s.SignerCertificate) { $s.SignerCertificate.Subject } else { '-' }
    $tsa = if ($s.TimeStamperCertificate) { $s.TimeStamperCertificate.Subject } else { '-' }
    Write-Host ("{0}: {1}; signer: {2}; timestamp: {3}" -f $d, $s.Status, $unterzeichner, $tsa)
    & $tool verify /pa /v $d
    if ($LASTEXITCODE -ne 0) { $fehler = $LASTEXITCODE }
}
exit $fehler
