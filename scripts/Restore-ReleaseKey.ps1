# Restores the release signing key from a backup made by scripts/Backup-ReleaseKey.ps1 (asks for its
# password), protected for this Windows account where scripts/Sign-Release.ps1 looks for it.
#
#   pwsh scripts/Restore-ReleaseKey.ps1 -Backup <kestrel-release-key-backup.json>
param(
  [Parameter(Mandatory)][string]$Backup,
  [string]$KeyPath = (Join-Path $env:LOCALAPPDATA 'KestrelRelease\update-signing-2.key')
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location (Join-Path $root 'rust')
try {
  $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
  cargo build -q --release -p release-signer
  if ($LASTEXITCODE) { throw 'release-signer did not build.' }
  & (Join-Path $root 'rust\target\release\release-signer.exe') import-backup (Resolve-Path -LiteralPath $Backup) $KeyPath
}
finally { Pop-Location }
exit $LASTEXITCODE
