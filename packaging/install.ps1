# Build and install Formant for the current user (no admin required).
#   pwsh packaging/install.ps1            # build + install + Start Menu shortcut
#   pwsh packaging/install.ps1 -Desktop   # also create a Desktop shortcut
#   pwsh packaging/install.ps1 -SkipBuild # use an existing release build
param(
    [switch]$Desktop,
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'

$repo = Resolve-Path "$PSScriptRoot/.."
$exeSrc = Join-Path $repo 'target/release/formant.exe'
$iconSrc = Join-Path $repo 'crates/app/icon.ico'
$installDir = Join-Path $env:LOCALAPPDATA 'Programs/Formant'
$startMenu = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs'

if (-not $SkipBuild) {
    Write-Host 'Building release...' -ForegroundColor Cyan
    Push-Location $repo
    cargo build --release -p formant
    Pop-Location
}
if (-not (Test-Path $exeSrc)) { throw "release exe not found: $exeSrc (build first)" }

Write-Host "Installing to $installDir" -ForegroundColor Cyan
New-Item -ItemType Directory -Force -Path $installDir | Out-Null
Copy-Item $exeSrc (Join-Path $installDir 'formant.exe') -Force
if (Test-Path $iconSrc) { Copy-Item $iconSrc (Join-Path $installDir 'icon.ico') -Force }
foreach ($doc in 'README.md', 'SPEC.md', 'LICENSE') {
    $p = Join-Path $repo $doc
    if (Test-Path $p) { Copy-Item $p $installDir -Force }
}

$exe = Join-Path $installDir 'formant.exe'
$icon = Join-Path $installDir 'icon.ico'

function New-Shortcut($linkPath) {
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($linkPath)
    $sc.TargetPath = $exe
    $sc.WorkingDirectory = $installDir
    if (Test-Path $icon) { $sc.IconLocation = $icon } else { $sc.IconLocation = "$exe,0" }
    $sc.Description = 'Formant - Rust-native vocal processor'
    $sc.Save()
    Write-Host "  shortcut: $linkPath"
}

New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
New-Shortcut (Join-Path $startMenu 'Formant.lnk')
if ($Desktop) { New-Shortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Formant.lnk') }

Write-Host "`nInstalled. Launch 'Formant' from the Start Menu." -ForegroundColor Green
Write-Host "Config + presets live in %APPDATA%\Formant. Uninstall: packaging/uninstall.ps1"
