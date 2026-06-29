# Remove the per-user Formant install and its shortcuts.
# Leaves %APPDATA%\Formant (config + presets) intact unless -Purge is given.
param([switch]$Purge)
$ErrorActionPreference = 'SilentlyContinue'

$installDir = Join-Path $env:LOCALAPPDATA 'Programs/Formant'
$startLnk = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs/Formant.lnk'
$desktopLnk = Join-Path ([Environment]::GetFolderPath('Desktop')) 'Formant.lnk'

Remove-Item -Recurse -Force $installDir
Remove-Item -Force $startLnk
Remove-Item -Force $desktopLnk
Write-Host "Removed Formant install + shortcuts."

if ($Purge) {
    Remove-Item -Recurse -Force (Join-Path $env:APPDATA 'Formant')
    Write-Host "Purged config + presets (%APPDATA%\Formant)."
}
