# Writes the winget manifests (version, installer, locale) for a published Kestrel release into
# winget\manifests\a\AchrafTsouli\KestrelDownloadManager\<version>\, and checks them with
# "winget validate" when winget is installed. Submitting them is a pull request to
# github.com/microsoft/winget-pkgs (copy the folder there, or use wingetcreate).
#
#   pwsh scripts/New-WingetManifest.ps1 [-Version 2.1.0] [-Installer installers\Kestrel-Setup-2.1.0-win-x64.exe]
param([string]$Version, [string]$Installer)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (!$Version) { $Version = ((Get-Content -LiteralPath (Join-Path $root 'rust/Cargo.toml') -Raw) | Select-String -Pattern '(?m)^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value }
if (!$Installer) { $Installer = Join-Path $root "installers/Kestrel-Setup-$Version-win-x64.exe" }
if (!(Test-Path -LiteralPath $Installer)) { throw "Build the installer first: $Installer" }

$id = 'AchrafTsouli.KestrelDownloadManager'
$repo = 'AchrafTsouli/kestrel-download-manager'
$name = Split-Path -Leaf $Installer
$url = "https://github.com/$repo/releases/download/v$Version/$name"
$sha = (Get-FileHash -LiteralPath $Installer -Algorithm SHA256).Hash.ToUpperInvariant()
$dir = Join-Path $root "winget/manifests/a/AchrafTsouli/KestrelDownloadManager/$Version"
New-Item -ItemType Directory -Path $dir -Force | Out-Null
$schema = '1.6.0'
$utf8 = New-Object Text.UTF8Encoding($false)

[IO.File]::WriteAllText((Join-Path $dir "$id.yaml"), @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.version.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: $schema
"@, $utf8)

[IO.File]::WriteAllText((Join-Path $dir "$id.installer.yaml"), @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.installer.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
Platform:
- Windows.Desktop
MinimumOSVersion: 10.0.19045.0
InstallerType: inno
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
ProductCode: '{F14B83CB-285D-459F-9E24-57D4E44F6D56}_is1'
Dependencies:
  PackageDependencies:
  - PackageIdentifier: Microsoft.EdgeWebView2Runtime
Protocols:
- magnet
FileExtensions:
- torrent
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $sha
ManifestType: installer
ManifestVersion: $schema
"@, $utf8)

[IO.File]::WriteAllText((Join-Path $dir "$id.locale.en-US.yaml"), @"
# yaml-language-server: `$schema=https://aka.ms/winget-manifest.defaultLocale.$schema.schema.json
PackageIdentifier: $id
PackageVersion: $Version
PackageLocale: en-US
Publisher: Achraf Tsouli
PublisherUrl: https://github.com/AchrafTsouli
PublisherSupportUrl: https://github.com/$repo/issues
PrivacyUrl: https://github.com/$repo/blob/main/PRIVACY.md
Author: Achraf Tsouli
PackageName: Kestrel Download Manager
PackageUrl: https://github.com/$repo
License: MIT
LicenseUrl: https://github.com/$repo/blob/main/LICENSE
ShortDescription: A fast, free download manager for Windows, with browser capture, videos, torrents and FTP.
Description: |-
  Kestrel Download Manager (KDM) splits each download across several connections, resumes after a crash or a
  lost connection, and catches downloads from Chrome, Edge, Brave and Firefox. It downloads videos (yt-dlp),
  torrents and magnet links, and FTP/SFTP files, with queues, a scheduler, speed limits and signed updates.
Moniker: kdm
Tags:
- download-manager
- downloader
- torrent
- youtube-dl
- yt-dlp
- ftp
- accelerator
ReleaseNotesUrl: https://github.com/$repo/releases/tag/v$Version
ManifestType: defaultLocale
ManifestVersion: $schema
"@, $utf8)

Write-Output "Manifests: $dir"
if (Get-Command winget -ErrorAction SilentlyContinue) { winget validate --manifest $dir } else { Write-Output 'winget is not installed here; the manifests were not validated.' }
