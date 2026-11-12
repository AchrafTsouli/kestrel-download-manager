# Builds the Kestrel 2.0 (Rust) installer: the Tauri app, the browser bridge, the extension and the
# video tools, packed by Inno Setup with installer/Kestrel.iss (same AppId as Flux 1.x, so it installs
# over 1.x in place). Prints the installer's SHA-256 for Sign-Release.ps1.
#
#   pwsh scripts/Build-Installer.ps1 [-SkipBuild] [-TargetDir rust\target]
param([switch]$SkipBuild, [string]$TargetDir = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$rust = Join-Path $root 'rust'
if (!$TargetDir) { $TargetDir = Join-Path $rust 'target' }
$release = Join-Path $TargetDir 'release'
$version = ((Get-Content -LiteralPath (Join-Path $rust 'Cargo.toml') -Raw) | Select-String -Pattern '(?m)^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$stage = Join-Path $root 'artifacts\kestrel-stage'
$output = Join-Path $root 'installers'
$compiler = @((Join-Path $root '.tools/InnoSetup/ISCC.exe'), "${env:ProgramFiles(x86)}/Inno Setup 6/ISCC.exe", "$env:ProgramFiles/Inno Setup 6/ISCC.exe") | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (!$compiler) { throw 'Inno Setup 6 was not found. Run scripts/Install-InnoCompiler.ps1 first.' }

if (!$SkipBuild) {
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    if ($TargetDir) { $env:CARGO_TARGET_DIR = $TargetDir }
    Push-Location (Join-Path $rust 'app')
    try {
        npx tauri build --no-bundle
        if ($LASTEXITCODE -ne 0) { throw 'The Kestrel app did not build.' }
    } finally { Pop-Location }
    Push-Location $rust
    try {
        cargo build --release -p kestrel-nmhost -p kestrel-cli
        if ($LASTEXITCODE -ne 0) { throw 'The browser bridge did not build.' }
    } finally { Pop-Location }
}

# The video tools are kept out of git; restore them (checked against video-tools/sources.json).
$tools = Join-Path $root 'video-tools'
if (@('yt-dlp.exe','ffmpeg.exe','ffprobe.exe','qjs.exe','aria2c.exe') | Where-Object { !(Test-Path -LiteralPath (Join-Path $tools $_)) }) {
    & (Join-Path $PSScriptRoot 'Get-VideoTools.ps1')
}

if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage, (Join-Path $stage 'BrowserExtension'), (Join-Path $stage 'video-tools'), (Join-Path $stage 'licenses') -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $release 'kestrel.exe') -Destination (Join-Path $stage 'Kestrel.exe')
Copy-Item -LiteralPath (Join-Path $release 'kestrel-nmhost.exe'), (Join-Path $release 'kestrel-cli.exe') -Destination $stage
Get-ChildItem -LiteralPath (Join-Path $root 'BrowserExtension') -File | Where-Object { $_.Name -ne 'package.json' -and $_.Name -notlike '*.test.js' } |
    Copy-Item -Destination (Join-Path $stage 'BrowserExtension')
Get-ChildItem -LiteralPath $tools -File | Where-Object { $_.Extension -in @('.exe','.dll','.txt','.json') -and $_.Name -notin @('node.exe','NODE-LICENSE.txt') } |
    Copy-Item -Destination (Join-Path $stage 'video-tools')
Copy-Item -LiteralPath (Join-Path $rust 'KESTREL_Icon_Pack/app/windows/kestrel.ico') -Destination (Join-Path $stage 'app.ico')
Copy-Item -LiteralPath (Join-Path $rust 'KESTREL_Icon_Pack/app/png/kestrel-256.png') -Destination (Join-Path $stage 'app-icon.png')
Copy-Item -LiteralPath (Join-Path $root 'installer/Browser-Setup.html'), (Join-Path $root 'installer/INSTALLATION.txt') -Destination $stage
Copy-Item -LiteralPath (Join-Path $root 'LICENSE') -Destination (Join-Path $stage 'licenses/Kestrel-LICENSE.txt')

# Code signing (optional): set KESTREL_SIGN_THUMBPRINT to the SHA-1 thumbprint of a code-signing
# certificate in your Windows certificate store (a Certum token, an OV/EV certificate…). Without it the
# build is unsigned and Windows SmartScreen warns about an unknown publisher.
$signThumbprint = $env:KESTREL_SIGN_THUMBPRINT
$timestamp = if ($env:KESTREL_SIGN_TIMESTAMP) { $env:KESTREL_SIGN_TIMESTAMP } else { 'http://timestamp.digicert.com' }
function Sign-Files([string[]]$Paths) {
    if (!$signThumbprint) { return }
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | Select-Object -First 1
    if (!$signtool) { throw 'signtool.exe not found: install the Windows SDK (Signing Tools).' }
    & $signtool.FullName sign /sha1 $signThumbprint /fd sha256 /tr $timestamp /td sha256 /d 'Kestrel Download Manager' @Paths
    if ($LASTEXITCODE -ne 0) { throw 'Code signing failed.' }
}
Sign-Files @((Join-Path $stage 'Kestrel.exe'), (Join-Path $stage 'kestrel-nmhost.exe'), (Join-Path $stage 'kestrel-cli.exe'))
if (!$signThumbprint) { Write-Output 'Not signed (set KESTREL_SIGN_THUMBPRINT to sign).' }

foreach ($required in @('Kestrel.exe','kestrel-nmhost.exe','kestrel-cli.exe','BrowserExtension/manifest.json','video-tools/yt-dlp.exe','video-tools/ffmpeg.exe','video-tools/ffprobe.exe','video-tools/qjs.exe','video-tools/aria2c.exe','video-tools/sources.json','app.ico')) {
    if (!(Test-Path -LiteralPath (Join-Path $stage $required))) { throw "Incomplete payload: $required" }
}

# The extension's fixed key gives its ID (Chrome: SHA-256 of the key, first 16 bytes, letters a-p).
$manifest = Get-Content -LiteralPath (Join-Path $stage 'BrowserExtension/manifest.json') -Raw | ConvertFrom-Json
$hash = [Security.Cryptography.SHA256]::HashData([Convert]::FromBase64String($manifest.key))
$extensionId = -join ([BitConverter]::ToString($hash).Replace('-','').Substring(0,32).ToLowerInvariant().ToCharArray() | ForEach-Object { [char](97 + [Convert]::ToInt32([string]$_,16)) })

$prefix = $stage.TrimEnd('\') + '\'
Get-ChildItem -LiteralPath $stage -File -Recurse | ForEach-Object {
    [pscustomobject]@{ path = $_.FullName.Substring($prefix.Length); bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName).Hash.ToLowerInvariant() }
} | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $stage 'payload-sha256.json') -Encoding utf8
$bytes = (Get-ChildItem -LiteralPath $stage -File -Recurse | Measure-Object Length -Sum).Sum
Write-Output ('Payload: {0:N1} MB (app {1:N1} MB)' -f ($bytes / 1MB), ((Get-Item (Join-Path $stage 'Kestrel.exe')).Length / 1MB))

New-Item -ItemType Directory -Path $output -Force | Out-Null
& $compiler /Q "/DPayloadDir=$stage" "/DOutputDir=$output" "/DAppVersion=$version" "/DExtensionId=$extensionId" (Join-Path $root 'installer/Kestrel.iss')
if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
$installer = Join-Path $output "Kestrel-Setup-$version-win-x64.exe"
Sign-Files @($installer)
$sha = (Get-FileHash -LiteralPath $installer).Hash.ToLowerInvariant()
"$sha  $(Split-Path -Leaf $installer)" | Set-Content -LiteralPath "$installer.sha256" -Encoding ascii
Write-Output ('Installer: {0} ({1:N1} MB)' -f $installer, ((Get-Item $installer).Length / 1MB))
Write-Output "SHA-256: $sha"
