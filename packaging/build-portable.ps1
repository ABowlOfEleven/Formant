# Build a portable ZIP (no install) into dist/.
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'

$repo = Resolve-Path "$PSScriptRoot/.."
$staging = Join-Path $repo 'dist/Formant'
$zip = Join-Path $repo 'dist/Formant-portable.zip'

if (-not $SkipBuild) {
    Push-Location $repo
    cargo build --release -p formant
    Pop-Location
}

Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $staging | Out-Null
Copy-Item (Join-Path $repo 'target/release/formant.exe') $staging -Force
foreach ($f in 'crates/app/icon.ico', 'README.md', 'SPEC.md', 'LICENSE') {
    $p = Join-Path $repo $f
    if (Test-Path $p) { Copy-Item $p $staging -Force }
}
# Bundle the docs folder so the README's screenshots resolve in the zip too.
$docs = Join-Path $repo 'docs'
if (Test-Path $docs) { Copy-Item $docs $staging -Recurse -Force }
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path "$staging/*" -DestinationPath $zip
Write-Host "Wrote $zip ($([math]::Round((Get-Item $zip).Length/1MB,1)) MB)" -ForegroundColor Green
