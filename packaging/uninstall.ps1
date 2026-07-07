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

# Remove autostart: the scheduled task and any legacy Run-key entry.
schtasks /Delete /TN "Formant" /F 2>$null | Out-Null
Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'Formant' -ErrorAction SilentlyContinue

Write-Host "Removed Formant install + shortcuts."

if ($Purge) {
    Remove-Item -Recurse -Force (Join-Path $env:APPDATA 'Formant')
    Write-Host "Purged config + presets (%APPDATA%\Formant)."
}
