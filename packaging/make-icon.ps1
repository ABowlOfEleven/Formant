# Generate a themed Formant app icon (cyan + ember waveform on a dark tile).
param([string]$Out = "$PSScriptRoot/../crates/app/icon.ico")

Add-Type -AssemblyName System.Drawing

$size = 256
$bmp = New-Object System.Drawing.Bitmap $size, $size
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::Transparent)

# Rounded dark tile.
$bg = [System.Drawing.Color]::FromArgb(255, 12, 17, 22)
$path = New-Object System.Drawing.Drawing2D.GraphicsPath
$r = 48; $m = 8; $w = $size - 2 * $m
$path.AddArc($m, $m, $r, $r, 180, 90)
$path.AddArc($m + $w - $r, $m, $r, $r, 270, 90)
$path.AddArc($m + $w - $r, $m + $w - $r, $r, $r, 0, 90)
$path.AddArc($m, $m + $w - $r, $r, $r, 90, 90)
$path.CloseFigure()
$brush = New-Object System.Drawing.SolidBrush($bg)
$g.FillPath($brush, $path)
$border = New-Object System.Drawing.Pen([System.Drawing.Color]::FromArgb(120, 34, 211, 238), 3)
$g.DrawPath($border, $path)

# Two waveforms: ember (back) and cyan (front).
function Wave($color, $width, $amp, $phase, $yc) {
    $pen = New-Object System.Drawing.Pen($color, $width)
    $pen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
    $pts = New-Object 'System.Collections.Generic.List[System.Drawing.PointF]'
    for ($x = 40; $x -le 216; $x += 4) {
        $t = ($x - 40) / 176.0
        $env = [Math]::Sin([Math]::PI * $t) # taper at the ends
        $y = $yc + $amp * $env * [Math]::Sin($t * 6.283 * 2 + $phase)
        $pts.Add((New-Object System.Drawing.PointF($x, $y)))
    }
    $g.DrawLines($pen, $pts.ToArray())
}
Wave ([System.Drawing.Color]::FromArgb(220, 255, 122, 24)) 10 34 0.9 138
Wave ([System.Drawing.Color]::FromArgb(255, 34, 211, 238)) 14 48 0.0 122

$g.Dispose()

# Export raw RGBA (for the egui window/title-bar icon - no image-crate decode).
$rgbaOut = Join-Path (Split-Path $Out) 'icon.rgba'
$rect = New-Object System.Drawing.Rectangle(0, 0, $size, $size)
$data = $bmp.LockBits($rect, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$len = $data.Stride * $size
$buf = New-Object byte[] $len
[System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $buf, 0, $len)
$bmp.UnlockBits($data)
$rgba = New-Object byte[] ($size * $size * 4)
for ($i = 0; $i -lt ($size * $size); $i++) {
    $rgba[$i * 4 + 0] = $buf[$i * 4 + 2] # R <- B
    $rgba[$i * 4 + 1] = $buf[$i * 4 + 1] # G
    $rgba[$i * 4 + 2] = $buf[$i * 4 + 0] # B <- R
    $rgba[$i * 4 + 3] = $buf[$i * 4 + 3] # A
}
[System.IO.File]::WriteAllBytes($rgbaOut, $rgba)
Write-Host "wrote $rgbaOut ($($rgba.Length) bytes, ${size}x${size})"

# Save as .ico via an HICON.
$hicon = $bmp.GetHicon()
$icon = [System.Drawing.Icon]::FromHandle($hicon)
New-Item -ItemType Directory -Force -Path (Split-Path $Out) | Out-Null
$fs = [System.IO.File]::Create((Resolve-Path -LiteralPath (Split-Path $Out)).Path + "/" + (Split-Path $Out -Leaf))
$icon.Save($fs)
$fs.Close()
$bmp.Dispose()
Write-Host "wrote $Out ($((Get-Item $Out).Length) bytes)"
