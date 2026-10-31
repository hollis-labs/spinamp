# Spinamp installer for Windows (PowerShell):   irm https://spinamp.fm/install.ps1 | iex
#
# Finds the newest release of hollis-labs/spinamp on GitHub, downloads the Windows installer,
# checks it against the release's SHA256SUMS and runs it silently for the current user.
$ErrorActionPreference = 'Stop'
$repo = 'hollis-labs/spinamp'

Write-Host 'Looking for the latest Spinamp release...'
$release = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases?per_page=1")[0]
$asset = $release.assets | Where-Object { $_.name -like '*x64-setup.exe' } | Select-Object -First 1
if (-not $asset) { throw 'No Windows installer found in the latest release.' }

$exe = Join-Path $env:TEMP $asset.name
Write-Host "Downloading $($asset.name)..."
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $exe

$sums = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS' } | Select-Object -First 1
if ($sums) {
  $line = (Invoke-RestMethod $sums.browser_download_url) -split "`n" | Where-Object { $_ -match [regex]::Escape($asset.name) } | Select-Object -First 1
  $expect = ($line -split '\s+')[0].ToLower()
  $actual = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
  if ($expect -ne $actual) { Remove-Item $exe -Force; throw 'Checksum mismatch, not installing.' }
  Write-Host 'Checksum OK.'
} else {
  Write-Host 'Note: this release has no SHA256SUMS; skipping the checksum check.'
}

Write-Host 'Installing...'
Start-Process -FilePath $exe -ArgumentList '/S' -Wait
Remove-Item $exe -Force
Write-Host 'Installed. Find Spinamp in the Start menu.'
Write-Host 'Windows may have shown a SmartScreen notice: early releases are not code-signed yet.'
Write-Host 'Spinamp controls the Spotify desktop app, so you need Spotify Premium and the Spotify app installed.'
