#Requires -Version 5.1
<#
sign-windows.ps1 - quadchroma.exe mit Authenticode signieren (SHA-256,
RFC-3161-Zeitstempel) und die Signatur pruefen. Parameternamen englisch wie
bei signtool und in der CI (die deutschen Altnamen gehen als Aliase weiter),
Erlaeuterungen deutsch.

Zwei Wege:
  Zertifikat im Windows-Zertifikatspeicher (Standard): ein Codesigning-
      Zertifikat einer CA, Schluessel auf Token/Karte oder im Cloud-CSP der
      CA (z. B. Certum SimplySign); Auswahl ueber den SHA-1-Fingerabdruck
      (-Thumbprint, sonst Umgebungsvariable QC_SIGN_THUMBPRINT). Der CSP
      fragt beim Signieren PIN bzw. Einmalcode ab.
  -TrustedSigning: Azure Artifact Signing (so heisst "Trusted Signing" seit
      Januar 2026): signtool mit Azure.CodeSigning.Dlib.dll (-Dlib) und einer
      metadata.json, die das Skript aus -Endpoint, -Account und
      -CertificateProfile erzeugt (oder fertig ueber -Metadata bekommt).
      Vorher anmelden per "az login" oder ueber AZURE_TENANT_ID,
      AZURE_CLIENT_ID, AZURE_CLIENT_SECRET (in CI ueber OIDC). Braucht die
      .NET-8-Laufzeit neben signtool.

Braucht Robert: ein Zertifikat bzw. ein Artifact-Signing-Konto (siehe
RELEASING.md). Ohne beides zeigt -VerifyOnly nur den Signaturstatus.
Eine Signatur beseitigt die SmartScreen-Warnung nicht sofort; sie sorgt
dafuer, dass die Reputation des Herausgebers von Version zu Version
weitergetragen wird - deshalb jede Version mit derselben Identitaet
signieren.

Aufruf: Windows startet mit der Ausfuehrungsrichtlinie "Restricted", die
gar keine Skripte laedt (".\scripts\sign-windows.ps1" endet dann mit "die
Ausfuehrung von Skripts auf diesem System deaktiviert"). Deshalb das Skript
ueber powershell -NoProfile -ExecutionPolicy Bypass -File aufrufen (oder
pwsh -File wie in release.yml; PowerShell 7 hat RemoteSigned). Wer lieber
direkt aufruft: einmal "Set-ExecutionPolicy -Scope CurrentUser RemoteSigned"
und bei einer aus dem Internet geladenen Kopie "Unblock-File" auf die Datei.

Beispiele (im Wurzelverzeichnis des Repositorys):
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe client\target\release\quadchroma.exe -Thumbprint 0123ABCD...
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe quadchroma.exe -TrustedSigning -Endpoint https://weu.codesigning.azure.net -Account <konto> -CertificateProfile <profil> -Dlib C:\tools\bin\x64\Azure.CodeSigning.Dlib.dll
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\sign-windows.ps1 -Exe quadchroma.exe -VerifyOnly
  Altnamen: -Datei, -Fingerabdruck, -NurPruefen, -Zeitstempel, -Art ArtifactSigning -Metadata metadata.json

Exit-Code 0, wenn die Signatur gueltig und die Kette vertrauenswuerdig ist
(signtool verify /pa), sonst der Exit-Code von signtool.
#>
[CmdletBinding()]
param(
    # Zu signierende Datei(en): die exe, auf Wunsch auch DLLs.
    [Parameter(Mandatory, Position = 0)] [Alias('Datei')] [string[]] $Exe,

    # Weg 1: SHA-1-Fingerabdruck des Zertifikats im Speicher (certmgr.msc >
    # Details > Fingerabdruck; Leerzeichen und Doppelpunkte sind egal).
    [Alias('Fingerabdruck')] [string] $Thumbprint = $env:QC_SIGN_THUMBPRINT,
    # Zertifikat liegt im Computerspeicher (LocalMachine) statt im
    # Benutzerspeicher (signtool /sm).
    [Alias('Maschinenspeicher')] [switch] $MachineStore,

    # Weg 2: Azure Artifact Signing (frueher Trusted Signing).
    [Alias('ArtifactSigning')] [switch] $TrustedSigning,
    # Altform: -Art Zertifikat | ArtifactSigning (ArtifactSigning = -TrustedSigning).
    [ValidateSet('Zertifikat', 'ArtifactSigning')] [string] $Art,
    # Endpunkt der Region von Konto und Profil, z. B.
    # https://weu.codesigning.azure.net (West Europe).
    [string] $Endpoint = $env:QC_SIGN_ENDPOINT,
    # Name des Artifact-Signing-Kontos.
    [string] $Account = $env:QC_SIGN_ACCOUNT,
    # Name des Zertifikatsprofils (Typ Public Trust).
    [string] $CertificateProfile = $env:QC_SIGN_PROFILE,
    # Pfad zu Azure.CodeSigning.Dlib.dll (NuGet Microsoft.ArtifactSigning.Client,
    # Ordner bin\x64 fuer das x64-signtool).
    [string] $Dlib = $env:QC_SIGN_DLIB,
    # Fertige metadata.json (Endpoint, CodeSigningAccountName,
    # CertificateProfileName); dann werden -Endpoint/-Account/-CertificateProfile
    # nicht gebraucht.
    [string] $Metadata = $env:QC_SIGN_METADATA,

    # RFC-3161-Zeitstempeldienst. Leer = DigiCert; bei -TrustedSigning der
    # Microsoft-Dienst (das Zertifikat gilt dort nur drei Tage, der
    # Zeitstempel ist Pflicht).
    [Alias('Zeitstempel')] [string] $TimestampUrl = $env:QC_TIMESTAMP_URL,
    # signtool.exe; leer = QC_SIGNTOOL, PATH, sonst das neueste Windows SDK
    # unter "Windows Kits".
    [string] $Signtool = $env:QC_SIGNTOOL,
    # Beschreibung und URL in der Signatur (/d, /du); Windows zeigt die
    # Beschreibung in seinen Dialogen.
    [Alias('Beschreibung')] [string] $Description = 'QuadChroma',
    [Alias('BeschreibungUrl')] [string] $DescriptionUrl = 'https://github.com/quadchroma-tech/quadchroma',
    # Nichts signieren, nur pruefen (signtool verify /pa /v und
    # Get-AuthenticodeSignature).
    [Alias('NurPruefen')] [switch] $VerifyOnly
)
$ErrorActionPreference = 'Stop'
# PowerShell 7: ein Exit-Code ungleich 0 von signtool soll nicht als
# Ausnahme enden, sondern unten als Exit-Code des Skripts herauskommen.
$PSNativeCommandUseErrorActionPreference = $false
if ($Art -eq 'ArtifactSigning') { $TrustedSigning = $true }

# signtool.exe finden: Angabe, PATH, sonst das neueste Windows SDK.
function Find-Signtool {
    if ($Signtool) {
        if (Test-Path -LiteralPath $Signtool -PathType Leaf) { return (Resolve-Path -LiteralPath $Signtool).Path }
        throw "signtool nicht gefunden: $Signtool"
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
    throw 'signtool.exe nicht gefunden: Windows SDK installieren oder -Signtool bzw. QC_SIGNTOOL setzen'
}

$tool = Find-Signtool
Write-Host "signtool: $tool"
$dateien = foreach ($d in $Exe) {
    if (-not (Test-Path -LiteralPath $d -PathType Leaf)) { throw "Datei fehlt: $d" }
    (Resolve-Path -LiteralPath $d).Path
}

if (-not $VerifyOnly) {
    if (-not $TimestampUrl) {
        $TimestampUrl = if ($TrustedSigning) { 'http://timestamp.acs.microsoft.com' } else { 'http://timestamp.digicert.com' }
    }
    # /fd und /td sind seit SDK 20236 Pflicht; SHA-256 fuer Datei und Zeitstempel.
    $argumente = @('sign', '/v', '/fd', 'SHA256', '/tr', $TimestampUrl, '/td', 'SHA256', '/d', $Description, '/du', $DescriptionUrl)
    $metadataTemp = $null
    if ($TrustedSigning) {
        if (-not $Dlib) { throw '-TrustedSigning braucht -Dlib <Azure.CodeSigning.Dlib.dll> (oder die Umgebungsvariable QC_SIGN_DLIB)' }
        if (-not (Test-Path -LiteralPath $Dlib -PathType Leaf)) { throw "Dlib fehlt: $Dlib" }
        if ($Metadata) {
            if (-not (Test-Path -LiteralPath $Metadata -PathType Leaf)) { throw "metadata.json fehlt: $Metadata" }
            $metadataPfad = (Resolve-Path -LiteralPath $Metadata).Path
        } else {
            # Parameter -> Umgebungsvariable, die als Standardwert dient (siehe param()).
            $umgebung = [ordered]@{ Endpoint = 'QC_SIGN_ENDPOINT'; Account = 'QC_SIGN_ACCOUNT'; CertificateProfile = 'QC_SIGN_PROFILE' }
            foreach ($name in $umgebung.Keys) {
                if (-not (Get-Variable -Name $name -ValueOnly)) { throw "-TrustedSigning braucht -$name (oder die Umgebungsvariable $($umgebung[$name])) - oder eine fertige -Metadata metadata.json" }
            }
            # metadata.json fuer die Dlib. Der Endpunkt muss zur Region von
            # Konto UND Profil passen, sonst antwortet der Dienst mit 403.
            $metadataTemp = Join-Path ([System.IO.Path]::GetTempPath()) ("quadchroma-signing-{0}.json" -f [guid]::NewGuid())
            $json = [ordered]@{
                Endpoint               = $Endpoint
                CodeSigningAccountName = $Account
                CertificateProfileName = $CertificateProfile
                CorrelationId          = 'quadchroma-release'
            } | ConvertTo-Json
            # UTF-8 ohne BOM (Set-Content -Encoding UTF8 schriebe in PowerShell 5.1 eine BOM).
            [System.IO.File]::WriteAllText($metadataTemp, $json, (New-Object System.Text.UTF8Encoding $false))
            $metadataPfad = $metadataTemp
        }
        $argumente += @('/dlib', (Resolve-Path -LiteralPath $Dlib).Path, '/dmdf', $metadataPfad)
    } else {
        if (-not $Thumbprint) { throw 'Fingerabdruck fehlt: -Thumbprint <sha1> oder QC_SIGN_THUMBPRINT setzen (oder -TrustedSigning bzw. -VerifyOnly)' }
        $Thumbprint = $Thumbprint -replace '[\s:]', ''
        if ($Thumbprint -notmatch '^[0-9A-Fa-f]{40}$') { throw "Fingerabdruck ist kein SHA-1 (40 Hex-Zeichen): $Thumbprint" }
        $argumente += @('/sha1', $Thumbprint)
        if ($MachineStore) { $argumente += '/sm' }
    }
    $argumente += $dateien
    Write-Host "signtool $($argumente -join ' ')"
    try {
        & $tool @argumente
        if ($LASTEXITCODE -ne 0) { throw "signtool sign: Exit-Code $LASTEXITCODE" }
    } finally {
        if ($metadataTemp) { Remove-Item -LiteralPath $metadataTemp -ErrorAction SilentlyContinue }
    }
}

# Pruefen: /pa = normale Authenticode-Richtlinie (ohne /pa prueft signtool
# nach den Regeln fuer Treiber und meldet gueltige Signaturen als Fehler).
$fehler = 0
foreach ($d in $dateien) {
    $s = Get-AuthenticodeSignature -LiteralPath $d
    $unterzeichner = if ($s.SignerCertificate) { $s.SignerCertificate.Subject } else { '-' }
    $tsa = if ($s.TimeStamperCertificate) { $s.TimeStamperCertificate.Subject } else { '-' }
    Write-Host ("{0}: {1}; Unterzeichner: {2}; Zeitstempel: {3}" -f $d, $s.Status, $unterzeichner, $tsa)
    & $tool verify /pa /v $d
    if ($LASTEXITCODE -ne 0) { $fehler = $LASTEXITCODE }
}
exit $fehler
