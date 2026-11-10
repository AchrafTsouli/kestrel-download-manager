# Quick test build of Kestrel: closes the running test app, builds the interface and the app with
# the "fast" profile, and puts it at rust\target\release\flux.exe (where it is run from).
# Installers keep using the full release build (Build-Flux2Installer.ps1).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root 'rust\target\release'
Get-Process flux -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$release*" } | Stop-Process -Force
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
$watch = [Diagnostics.Stopwatch]::StartNew()
Push-Location (Join-Path $root 'rust\app')
try { npm run build --silent | Out-Null; if ($LASTEXITCODE) { throw 'The interface did not build.' } } finally { Pop-Location }
Push-Location (Join-Path $root 'rust')
try {
    cargo build --profile fast -p flux-app --features tauri/custom-protocol 2>&1 | Select-String '^error|^warning: unused' | ForEach-Object { $_.Line }
    if ($LASTEXITCODE) { throw 'The app did not build.' }
} finally { Pop-Location }
New-Item -ItemType Directory $release -Force | Out-Null
Copy-Item (Join-Path $root 'rust\target\fast\flux.exe') (Join-Path $release 'flux.exe') -Force
"Built in {0:N0} s: {1}" -f $watch.Elapsed.TotalSeconds, (Join-Path $release 'flux.exe')
