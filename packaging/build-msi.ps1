# Build the Formant MSI into dist/ using the WiX v5 toolset (`wix build`).
param(
    [string]$Version = '0.3.0',
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'

$repo = (Resolve-Path "$PSScriptRoot/..").Path
$dist = Join-Path $repo 'dist'
$wxs = Join-Path $repo 'packaging/wix/formant.wxs'
$msi = Join-Path $dist "Formant-$Version-x64.msi"

if (-not (Get-Command wix -ErrorAction SilentlyContinue)) {
    throw "The WiX toolset is not installed. Install it with: dotnet tool install --global wix"
}

if (-not $SkipBuild) {
    Push-Location $repo
    cargo build --release -p formant
    Pop-Location
}

New-Item -ItemType Directory -Force -Path $dist | Out-Null
Remove-Item -Force $msi -ErrorAction SilentlyContinue
& wix build $wxs -arch x64 -d "RepoDir=$repo" -d "Version=$Version" -o $msi
if ($LASTEXITCODE -ne 0) { throw "wix build failed ($LASTEXITCODE)" }

Write-Host "Wrote $msi ($([math]::Round((Get-Item $msi).Length / 1MB, 1)) MB)" -ForegroundColor Green
