# Packs Kestrel's browser extension for the stores (nothing is uploaded):
#   installers\kestrel-extension-<version>-chromium.zip  Chrome Web Store, Microsoft Edge Add-ons
#   installers\kestrel-extension-<version>-firefox.zip   Firefox Add-ons (AMO)
# Store uploads may not carry the fixed "key"; each store gives the extension its own ID, which
# the browser bridge must then accept: add it to installer/Kestrel.iss (ExtensionId) and run
# kestrel-nmhost --register <id>. See docs/STORE-LISTING.md.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$source = Join-Path $root 'BrowserExtension'
$output = Join-Path $root 'installers'
$work = Join-Path $root 'artifacts\extension'

Push-Location $source
try { node --test (Get-ChildItem -Filter '*.test.js' | ForEach-Object Name); if ($LASTEXITCODE) { throw 'Extension tests failed; nothing was packed.' } } finally { Pop-Location }

$manifest = Get-Content -LiteralPath (Join-Path $source 'manifest.json') -Raw | ConvertFrom-Json
$version = $manifest.version
New-Item -ItemType Directory -Path $output -Force | Out-Null

function Pack([string]$flavor, [scriptblock]$adapt) {
    $dir = Join-Path $work $flavor
    if (Test-Path -LiteralPath $dir) { Remove-Item -LiteralPath $dir -Recurse -Force }
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    Get-ChildItem -LiteralPath $source -File | Where-Object { $_.Name -ne 'package.json' -and $_.Name -notlike '*.test.js' } | Copy-Item -Destination $dir
    $m = Get-Content -LiteralPath (Join-Path $dir 'manifest.json') -Raw | ConvertFrom-Json
    $m.PSObject.Properties.Remove('key')
    & $adapt $m
    [IO.File]::WriteAllText((Join-Path $dir 'manifest.json'), ($m | ConvertTo-Json -Depth 10), (New-Object Text.UTF8Encoding($false)))
    $zip = Join-Path $output "kestrel-extension-$version-$flavor.zip"
    if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
    Compress-Archive -Path (Join-Path $dir '*') -DestinationPath $zip
    Write-Output ('{0}: {1} ({2:N0} KB)' -f $flavor, $zip, ((Get-Item $zip).Length / 1KB))
}

Pack 'chromium' { param($m) }
Pack 'firefox' {
    param($m)
    # Firefox runs MV3 background pages as scripts, not service workers, and knows no "split" mode.
    $m.background = [pscustomobject]@{ scripts = @('background.js'); type = 'module' }
    $m.incognito = 'spanning'
    $m | Add-Member -NotePropertyName browser_specific_settings -NotePropertyValue ([pscustomobject]@{ gecko = [pscustomobject]@{ id = 'kestrel@achraftsouli.github.io'; strict_min_version = '128.0' } })
    foreach ($script in $m.content_scripts) { $script.PSObject.Properties.Remove('match_origin_as_fallback') }
}
Remove-Item -LiteralPath $work -Recurse -Force
