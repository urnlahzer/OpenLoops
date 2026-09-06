[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
$cargoPath = if ($null -eq $cargo) { Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe' } else { $cargo.Source }
Push-Location -LiteralPath $repoRoot
$previousKey = [Environment]::GetEnvironmentVariable('OLLAMA_API_KEY', 'Process')
try {
    & $cargoPath build --locked -p openloops-desktop --features live-connection,ollama-cloud --quiet
    if ($LASTEXITCODE -ne 0) { throw 'The provider executable did not build.' }
    Write-Host 'Create an API key at https://ollama.com/settings/keys.'
    Write-Host 'This checks Ollama Cloud with no mailbox content. Analysis will send selected message text to https://ollama.com.'
    $keyValue = Read-Host 'Ollama API key (input hidden)' -MaskInput
    [Environment]::SetEnvironmentVariable('OLLAMA_API_KEY', $keyValue.Trim(), 'Process')
    $keyValue = $null
    & (Join-Path $repoRoot 'target/debug/openloops-desktop.exe') --check-ollama
    $providerExitCode = $LASTEXITCODE
} finally {
    [Environment]::SetEnvironmentVariable('OLLAMA_API_KEY', $previousKey, 'Process')
    Pop-Location
}
exit $providerExitCode
