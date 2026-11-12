# Builds the portable Kestrel 2.0: a zip to unpack anywhere (a USB stick, a folder without admin
# rights). The "portable" file beside Kestrel.exe keeps settings and history in .\data, nothing in
# %LOCALAPPDATA%. Browser integration is optional: Options > Browser > Connect browsers.
#
#   pwsh scripts/Build-Portable.ps1 [-SkipBuild] [-TargetDir rust\target]
param([switch]$SkipBuild, [string]$TargetDir = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$rust = Join-Path $root 'rust'
if (!$TargetDir) { $TargetDir = Join-Path $rust 'target' }
$version = ((Get-Content -LiteralPath (Join-Path $rust 'Cargo.toml') -Raw) | Select-String -Pattern '(?m)^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value

if (!$SkipBuild) {
    & (Join-Path $PSScriptRoot 'Build-Installer.ps1') -TargetDir $TargetDir
    if ($LASTEXITCODE) { throw 'The build failed.' }
}
# The installer build stages exactly the files the app needs.
$stage = Join-Path $root 'artifacts\kestrel-stage'
if (!(Test-Path -LiteralPath (Join-Path $stage 'Kestrel.exe'))) { & (Join-Path $PSScriptRoot 'Build-Installer.ps1') -SkipBuild -TargetDir $TargetDir }

$portable = Join-Path $root "artifacts\Kestrel-$version-portable"
if (Test-Path -LiteralPath $portable) { Remove-Item -LiteralPath $portable -Recurse -Force }
Copy-Item -LiteralPath $stage -Destination $portable -Recurse
Remove-Item -LiteralPath (Join-Path $portable 'payload-sha256.json') -ErrorAction SilentlyContinue
Set-Content -LiteralPath (Join-Path $portable 'portable') -Value 'Kestrel keeps its settings and history in the data folder beside this file.' -Encoding utf8
@"
Kestrel $version, portable

Run Kestrel.exe. Settings, history and saved logins stay in the "data" folder here
(protected with your Windows account: copy it to another PC and they will not open there).

To let Chrome, Edge and Brave send downloads to this copy:
Options > Browser > Connect browsers (or run: kestrel-nmhost.exe --register).
"@ | Set-Content -LiteralPath (Join-Path $portable 'README.txt') -Encoding utf8

New-Item -ItemType Directory -Path (Join-Path $root 'installers') -Force | Out-Null
$zip = Join-Path $root "installers\Kestrel-$version-win-x64-portable.zip"
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
Compress-Archive -Path (Join-Path $portable '*') -DestinationPath $zip -CompressionLevel Optimal
Remove-Item -LiteralPath $portable -Recurse -Force
$sha = (Get-FileHash -LiteralPath $zip).Hash.ToLowerInvariant()
"$sha  $(Split-Path -Leaf $zip)" | Set-Content -LiteralPath "$zip.sha256" -Encoding ascii
Write-Output ('Portable: {0} ({1:N1} MB)' -f $zip, ((Get-Item $zip).Length / 1MB))
Write-Output "SHA-256: $sha"
