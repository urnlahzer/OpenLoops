[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) {
    throw 'Run this script from inside the OpenLoops Git repository.'
}

git -C $repoRoot config --local core.hooksPath .githooks
if ($LASTEXITCODE -ne 0) {
    throw 'Could not configure the repository hook path.'
}

$configured = (git -C $repoRoot config --local --get core.hooksPath).Trim()
if ($configured -ne '.githooks') {
    throw 'The repository hook path did not verify correctly.'
}

Write-Host 'OpenLoops public-repo Git hooks are active.'
