# Makes a password-protected backup of the release signing key, which opens on any computer. Put the
# backup file in a cloud drive and on a USB stick; keep the password somewhere else (a password
# manager or on paper). Without the password the file is useless; without the file, so is the password.
#
#   pwsh scripts/Backup-ReleaseKey.ps1 [-Output <file>]
#
# Restore on another computer with scripts/Restore-ReleaseKey.ps1.
param(
  [string]$Output = (Join-Path ([Environment]::GetFolderPath('Desktop')) 'kestrel-release-key-backup.json'),
  [string]$KeyPath = (Join-Path $env:LOCALAPPDATA 'KestrelRelease\update-signing-2.key')
)
$ErrorActionPreference = 'Stop'
if (!(Test-Path -LiteralPath $KeyPath)) { throw "No release key at $KeyPath." }
$root = Split-Path -Parent $PSScriptRoot
Push-Location (Join-Path $root 'rust')
try {
  $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
  cargo build -q --release -p release-signer
  if ($LASTEXITCODE) { throw 'release-signer did not build.' }
  & (Join-Path $root 'rust\target\release\release-signer.exe') export-backup $KeyPath $Output
}
finally { Pop-Location }
exit $LASTEXITCODE
