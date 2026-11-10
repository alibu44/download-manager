# Builds the store packages from extension\ :
#   store\download-manager-chrome.zip   Chrome Web Store (also Brave, Opera, Vivaldi) and Edge Add-ons
#   store\download-manager-firefox.zip  addons.mozilla.org
# Usage (from the project root):  powershell -ExecutionPolicy Bypass -File tools\paketle.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$out = Join-Path $root 'store'
$utf8 = New-Object Text.UTF8Encoding($false)
Add-Type -AssemblyName System.IO.Compression.FileSystem
if (Test-Path $out) { Remove-Item $out -Recurse -Force }

function New-Package($name, [scriptblock]$editManifest, [bool]$noYoutube) {
    $dir = Join-Path $out $name
    Copy-Item (Join-Path $root 'extension') $dir -Recurse
    $mf = Join-Path $dir 'manifest.json'
    $m = Get-Content $mf -Raw -Encoding UTF8 | ConvertFrom-Json
    $m.PSObject.Properties.Remove('key')   # stores assign their own ID and reject a fixed key
    & $editManifest $m
    [IO.File]::WriteAllText($mf, ($m | ConvertTo-Json -Depth 10), $utf8)
    if ($noYoutube) {
        foreach ($f in 'background.js', 'video.js') {
            $p = Join-Path $dir $f
            $t = [IO.File]::ReadAllText($p, $utf8)
            if ($t -notmatch 'const NO_YOUTUBE = false;') { throw "${f}: NO_YOUTUBE switch not found" }
            [IO.File]::WriteAllText($p, $t.Replace('const NO_YOUTUBE = false;', 'const NO_YOUTUBE = true;'), $utf8)
        }
    }
    $zip = Join-Path $out "download-manager-$name.zip"
    [IO.Compression.ZipFile]::CreateFromDirectory($dir, $zip)
    Remove-Item $dir -Recurse -Force
    "{0}  ({1:N0} KB)" -f $zip, ((Get-Item $zip).Length / 1KB)
}

New-Package 'chrome' { param($m) } $true

New-Package 'firefox' {
    param($m)
    # Firefox runs the background as an event page, not a service worker, and needs an add-on ID.
    $m.background = [pscustomobject]@{ scripts = @('background.js') }
    $m | Add-Member browser_specific_settings ([pscustomobject]@{
        gecko = [pscustomobject]@{ id = 'download-manager@alibu44.github.io'; strict_min_version = '121.0' }
    })
} $false
