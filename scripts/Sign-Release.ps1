# Writes a signed version.json for an installer (with tools/release-signer, in Rust). Upload both
# files to the release; installed copies read .../releases/latest/download/version.json.
param(
  [Parameter(Mandatory)][string]$Installer,
  [Parameter(Mandatory)][string]$DownloadUrl,
  [Parameter(Mandatory)][string]$Version,
  [string]$Output = (Join-Path (Split-Path -Parent $Installer) 'version.json'),
  [string]$KeyPath = (Join-Path $env:LOCALAPPDATA 'ApexRelease\update-signing.key')
)
$root = Split-Path -Parent $PSScriptRoot
Push-Location (Join-Path $root 'rust')
try { cargo run -q --release -p release-signer -- sign $KeyPath (Resolve-Path $Installer) $DownloadUrl $Version $Output }
finally { Pop-Location }
exit $LASTEXITCODE
