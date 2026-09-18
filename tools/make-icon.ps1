# Regenerate the desktop executable icon by running `pwsh ./tools/make-icon.ps1`
# from the repository root. The PNG dimensions are read from each IHDR chunk.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$assetDir = Join-Path $repoRoot 'crates/openloops-desktop/ui/assets'
$inputPaths = @(
    (Join-Path $assetDir 'openloops-32.png'),
    (Join-Path $assetDir 'openloops-64.png'),
    (Join-Path $assetDir 'openloops-256.png')
)
$outputPath = Join-Path $assetDir 'openloops.ico'

function Read-PngImage {
    param([Parameter(Mandatory)][string] $Path)

    [byte[]] $bytes = [System.IO.File]::ReadAllBytes($Path)
    [byte[]] $signature = 137, 80, 78, 71, 13, 10, 26, 10
    if ($bytes.Length -lt 24) {
        throw "PNG is too short to contain an IHDR chunk: $Path"
    }
    for ($index = 0; $index -lt $signature.Length; $index++) {
        if ($bytes[$index] -ne $signature[$index]) {
            throw "Invalid PNG signature: $Path"
        }
    }
    if ([System.Text.Encoding]::ASCII.GetString($bytes, 12, 4) -ne 'IHDR') {
        throw "PNG does not start with an IHDR chunk: $Path"
    }

    $width = ([uint32]$bytes[16] -shl 24) -bor ([uint32]$bytes[17] -shl 16) -bor
        ([uint32]$bytes[18] -shl 8) -bor [uint32]$bytes[19]
    $height = ([uint32]$bytes[20] -shl 24) -bor ([uint32]$bytes[21] -shl 16) -bor
        ([uint32]$bytes[22] -shl 8) -bor [uint32]$bytes[23]
    if ($width -ne $height -or $width -lt 1 -or $width -gt 256) {
        throw "Icon PNG must be square and between 1 and 256 pixels: $Path ($width x $height)"
    }

    [pscustomobject]@{ Bytes = $bytes; Width = $width; Height = $height }
}

$images = @($inputPaths | ForEach-Object { Read-PngImage -Path $_ })
$stream = [System.IO.MemoryStream]::new()
$writer = [System.IO.BinaryWriter]::new($stream)
try {
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$images.Count)

    $imageOffset = 6 + (16 * $images.Count)
    foreach ($image in $images) {
        $writer.Write([byte]($image.Width % 256))
        $writer.Write([byte]($image.Height % 256))
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$image.Bytes.Length)
        $writer.Write([uint32]$imageOffset)
        $imageOffset += $image.Bytes.Length
    }
    foreach ($image in $images) {
        $writer.Write($image.Bytes)
    }
    $writer.Flush()
    [System.IO.File]::WriteAllBytes($outputPath, $stream.ToArray())
}
finally {
    $writer.Dispose()
    $stream.Dispose()
}

Write-Output "Wrote $outputPath with $($images.Count) PNG-compressed images."
