# Regenerate the Formant app icon (crates/app/icon.ico and icon.rgba).
# The drawing lives in make-icon.py; this runs it with Pillow through uv, so
# nothing has to be installed. Pass a path to also write a preview sheet.
param([string]$Preview = "")

$script = Join-Path $PSScriptRoot "make-icon.py"
if ($Preview) {
    uv run --no-project --with pillow python -I $script $Preview
} else {
    uv run --no-project --with pillow python -I $script
}
