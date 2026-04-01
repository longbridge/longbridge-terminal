#!/usr/bin/env pwsh
# Longbridge Terminal CLI installer for Windows
# Usage: iwr https://github.com/longbridge/longbridge-terminal/raw/main/install.ps1 | iex

$ErrorActionPreference = 'Stop'

$repo        = 'longbridge/longbridge-terminal'
$binName     = 'longbridge'
$packageName = 'longbridge-terminal'
$installDir  = Join-Path $env:LOCALAPPDATA 'Programs\longbridge'

# ── Resolve latest release version ───────────────────────────────────────────

Write-Host "Fetching latest release..."

try {
    $req = [System.Net.HttpWebRequest]::Create("https://github.com/$repo/releases/latest")
    $req.AllowAutoRedirect = $false
    $req.Timeout = 15000
    $resp = $req.GetResponse()
    $location = $resp.Headers['Location']
    $resp.Close()
} catch [System.Net.WebException] {
    $location = $_.Exception.Response.Headers['Location']
    if (-not $location) { throw "Failed to fetch the latest release: $_" }
}

$version = $location -replace '^.*/tag/', ''
if (-not $version -or $version -eq $location) {
    throw "Failed to parse version from redirect URL: $location"
}

Write-Host "Latest release: $version"

# ── Download ──────────────────────────────────────────────────────────────────

$downloadUrl = "https://github.com/$repo/releases/download/$version/$packageName-windows-amd64.zip"

Write-Host "Downloading $packageName@$version ..."
Write-Host $downloadUrl

$tmpDir  = Join-Path $env:TEMP ([System.IO.Path]::GetRandomFileName())
$zipPath = Join-Path $tmpDir "$binName.zip"

New-Item -ItemType Directory -Path $tmpDir | Out-Null

try {
    $wc = New-Object System.Net.WebClient
    $wc.DownloadFile($downloadUrl, $zipPath)
    $wc.Dispose()

    # ── Extract ───────────────────────────────────────────────────────────────

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $tmpDir)

    # ── Install ───────────────────────────────────────────────────────────────

    if (-not (Test-Path $installDir)) {
        New-Item -ItemType Directory -Path $installDir | Out-Null
    }

    $srcExe  = Join-Path $tmpDir "$binName.exe"
    $destExe = Join-Path $installDir "$binName.exe"
    Move-Item -Path $srcExe -Destination $destExe -Force

} finally {
    Remove-Item -Recurse -Force $tmpDir -ErrorAction SilentlyContinue
}

# ── Add to user PATH if needed ────────────────────────────────────────────────

$userPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
if ($userPath -notlike "*$installDir*") {
    $newPath = ($userPath.TrimEnd(';') + ";$installDir").TrimStart(';')
    [Environment]::SetEnvironmentVariable('PATH', $newPath, 'User')
    Write-Host ""
    Write-Host "Added $installDir to your PATH."
    Write-Host "Restart your terminal for the PATH change to take effect."
}

Write-Host ""
Write-Host "Longbridge CLI $version has been installed successfully."
Write-Host ""
Write-Host "Run 'longbridge login' to authenticate, then 'longbridge -h' for help."
Write-Host ""
