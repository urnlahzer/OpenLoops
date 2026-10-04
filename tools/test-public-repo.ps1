[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$checker = Join-Path $PSScriptRoot 'check-public-repo.ps1'
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = [IO.Path]::GetFullPath((Join-Path $tempBase ('openloops-public-' + [guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe synthetic temp path.' }
[void](New-Item -ItemType Directory -Path $tempRoot)
$passed = 0

function Invoke-Case([string]$Text, [bool]$ShouldPass) {
    $caseRoot = Join-Path $tempRoot ([guid]::NewGuid().ToString('N'))
    [void](New-Item -ItemType Directory -Path $caseRoot)
    & git -C $caseRoot init --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Could not initialize synthetic repository.' }
    Set-Content -LiteralPath (Join-Path $caseRoot 'candidate.txt') -Value $Text -Encoding utf8NoBOM
    & git -C $caseRoot add -- candidate.txt
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage synthetic candidate.' }
    Push-Location $caseRoot
    try {
        $output = & pwsh -NoProfile -File $checker -Mode Staged 2>&1 | Out-String
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    if (($ShouldPass -and $code -ne 0) -or (-not $ShouldPass -and $code -eq 0)) {
        throw "Public-repo synthetic case produced the wrong decision: $output"
    }
    $script:passed++
}

try {
    Invoke-Case 'gmail/v1/users/me/messages' $true
    Invoke-Case ('/' + 'Users/' + 'somebody/Documents/x') $false
    Invoke-Case ('/' + 'home/' + 'somebody/.ssh/') $false
    Write-Host "OpenLoops public-repo negative tests passed ($passed synthetic cases)."
} finally {
    # Git marks its object files read-only, which [IO.Directory]::Delete refuses.
    if (Test-Path -LiteralPath $tempRoot) {
        Get-ChildItem -LiteralPath $tempRoot -Recurse -Force -File | ForEach-Object { $_.IsReadOnly = $false }
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
