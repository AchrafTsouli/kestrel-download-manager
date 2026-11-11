#requires -Version 7.4
# One-step Kestrel release: build the installer, sign version.json, and publish both to a GitHub
# release. Installed copies (Flux 1.x and Kestrel) pick it up through
# .../releases/latest/download/version.json. A version with a suffix (2.0.0-beta.1) is published as
# a pre-release, which "latest" skips: installed copies are only offered final versions.
# Requires: GitHub CLI signed in (gh auth login), the release signing key, and a committed, pushed tree.
param([string]$Notes = '')
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
  $version = ((Get-Content -LiteralPath 'rust/Cargo.toml' -Raw) | Select-String -Pattern '(?m)^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
  $tag = "v$version"
  if (git status --porcelain) { throw 'Commit or stash your changes before publishing a release.' }
  $repo = gh repo view --json nameWithOwner -q .nameWithOwner
  if (!$repo) { throw 'Run gh auth login and make sure the origin remote points at GitHub.' }
  if (gh release view $tag 2>$null) { throw "Release $tag already exists. Bump the version in rust/Cargo.toml first." }

  & (Join-Path $PSScriptRoot 'Build-Flux2Installer.ps1')
  if ($LASTEXITCODE -ne 0) { throw 'The installer build failed.' }
  $name = "Kestrel-Setup-$version-win-x64.exe"
  $installer = Join-Path $root "installers/$name"
  $url = "https://github.com/$repo/releases/download/$tag/$name"
  $feed = Join-Path $root 'installers/version.json'
  & (Join-Path $PSScriptRoot 'Sign-Release.ps1') -Installer $installer -DownloadUrl $url -Version $version -Output $feed
  if ($LASTEXITCODE -ne 0) { throw 'Signing failed.' }

  git tag $tag; git push origin $tag
  $notesArgs = if ($Notes) { @('--notes', $Notes) } else { @('--generate-notes') }
  $preArgs = if ($version.Contains('-')) { @('--prerelease') } else { @() }
  gh release create $tag $installer "$installer.sha256" $feed --title "Kestrel $version" @notesArgs @preArgs
  Write-Output "Published ${tag}: https://github.com/$repo/releases/tag/$tag"
}
finally { Pop-Location }
