# Upgrade test from Flux 1.0.0 to the Kestrel installer just built, in Windows Sandbox (a clean,
# throw-away Windows; nothing is installed on this computer). Needs the Windows Sandbox feature.
#   pwsh scripts/Test-Upgrade.ps1 [-Installer installers\Kestrel-Setup-<version>-win-x64.exe]
param([string]$Installer)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (!(Test-Path "$env:windir\System32\WindowsSandbox.exe")) { throw 'Windows Sandbox is not enabled.' }
if (!$Installer) { $Installer = Get-ChildItem (Join-Path $root 'installers') -Filter 'Kestrel-Setup-*-win-x64.exe' | Sort-Object LastWriteTime | Select-Object -Last 1 -ExpandProperty FullName }
if (Get-Process WindowsSandboxRemoteSession, WindowsSandboxClient -ErrorAction SilentlyContinue) { throw 'Close the open Windows Sandbox first (only one can run).' }
# A sandbox closed a moment ago may still be shutting down.
for ($i = 0; $i -lt 30 -and (Get-Process vmmemWindowsSandbox -ErrorAction SilentlyContinue); $i++) { Start-Sleep 2 }

# A new folder per run (a folder left open in an editor or a shell cannot be emptied).
$work = Join-Path $root ('artifacts\upgrade-test\' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
$in = Join-Path $work 'in'; $out = Join-Path $work 'out'
foreach ($d in $in, $out) { New-Item -ItemType Directory $d -Force | Out-Null }

# Flux 1.0.0, checked against its published checksum.
$release = 'https://github.com/AchrafTsouli/kestrel-download-manager/releases/download/v1.0.0'
$flux = Join-Path $in 'Flux-Setup-1.0.0-win-x64.exe'
Invoke-WebRequest "$release/Flux-Setup-1.0.0-win-x64.exe" -OutFile $flux -UseBasicParsing
$sums = (Invoke-WebRequest "$release/Flux-Setup-1.0.0-win-x64.exe.sha256" -UseBasicParsing).Content
if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
$expected = ($sums.Trim() -split '\s+')[0]
if ((Get-FileHash $flux).Hash -ne $expected.ToUpperInvariant()) { throw 'Flux 1.0.0 does not match its checksum.' }

# Microsoft's WebView2 installer: Windows Sandbox's image lacks the runtime that Kestrel needs.
# The offline (standalone x64) installer: the small bootstrapper fails in the sandbox with 0x80040902.
$webview = Join-Path $root 'artifacts\MicrosoftEdgeWebview2Setup.exe'
if (!(Test-Path $webview)) { Invoke-WebRequest 'https://go.microsoft.com/fwlink/?linkid=2124701' -OutFile $webview -UseBasicParsing }
Copy-Item $webview $in
if ((Get-AuthenticodeSignature (Join-Path $in 'MicrosoftEdgeWebview2Setup.exe')).SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') { throw 'The WebView2 installer is not signed by Microsoft.' }

Push-Location (Join-Path $root 'rust'); cargo build --release -q -p labserver; if ($LASTEXITCODE) { throw 'labserver did not build' }; Pop-Location
Copy-Item (Join-Path $root 'rust\target\release\labserver.exe'), $Installer, (Join-Path $PSScriptRoot 'upgrade-test\inside.ps1') $in

$config = Join-Path $work 'upgrade.wsb'
@"
<Configuration>
  <MappedFolders>
    <MappedFolder><HostFolder>$in</HostFolder><SandboxFolder>C:\test</SandboxFolder><ReadOnly>true</ReadOnly></MappedFolder>
    <MappedFolder><HostFolder>$out</HostFolder><SandboxFolder>C:\results</SandboxFolder><ReadOnly>false</ReadOnly></MappedFolder>
  </MappedFolders>
  <LogonCommand><Command>powershell.exe -ExecutionPolicy Bypass -File C:\test\inside.ps1</Command></LogonCommand>
</Configuration>
"@ | Set-Content -LiteralPath $config -Encoding utf8

Write-Output "Starting Windows Sandbox with $(Split-Path -Leaf $Installer)..."
Start-Process "$env:windir\System32\WindowsSandbox.exe" -ArgumentList "`"$config`""
$done = Join-Path $out 'done.txt'; $until = (Get-Date).AddMinutes(20)
while (!(Test-Path $done) -and (Get-Date) -lt $until) { Start-Sleep 5 }
Get-Content (Join-Path $out 'log.txt') -ErrorAction SilentlyContinue
$result = if (Test-Path $done) { (Get-Content $done -Raw).Trim() } else { 'FAIL: no result within 20 minutes' }
Get-Process WindowsSandboxRemoteSession, WindowsSandboxClient, WindowsSandbox -ErrorAction SilentlyContinue | Stop-Process -Force
Write-Output "Result: $result"
if ($result -ne 'PASS') { exit 1 }
