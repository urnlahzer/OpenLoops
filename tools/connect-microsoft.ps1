[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if ($null -eq $cargo) {
    $cargoPath = Join-Path $env:USERPROFILE '.cargo/bin/cargo.exe'
    if (-not (Test-Path -LiteralPath $cargoPath)) {
        throw 'Install the pinned Rust toolchain before running the connection check.'
    }
} else {
    $cargoPath = $cargo.Source
}

Push-Location -LiteralPath $repoRoot
$oldClient = [Environment]::GetEnvironmentVariable('OPENLOOPS_CLIENT_ID', 'Process')
$oldShared = [Environment]::GetEnvironmentVariable('OPENLOOPS_SHARED_MAILBOX', 'Process')
$oldGroups = [Environment]::GetEnvironmentVariable('OPENLOOPS_GROUP_INBOXES', 'Process')
try {
    & $cargoPath build --locked -p openloops-desktop --features live-connection --quiet
    if ($LASTEXITCODE -ne 0) { throw 'The connection executable did not build.' }

    Write-Host 'In Entra, open the app Overview and copy Application (client) ID.'
    $clientValue = Read-Host 'Paste Application (client) ID (input hidden)' -MaskInput
    $groupValue = Read-Host 'Outlook Groups: primary email addresses separated by commas; Enter skips'
    $sharedValue = Read-Host 'Shared mailboxes (Shared with me, not Groups): addresses separated by commas; Enter skips'
    [Environment]::SetEnvironmentVariable('OPENLOOPS_CLIENT_ID', $clientValue.Trim(), 'Process')
    [Environment]::SetEnvironmentVariable('OPENLOOPS_SHARED_MAILBOX', $sharedValue.Trim(), 'Process')
    [Environment]::SetEnvironmentVariable('OPENLOOPS_GROUP_INBOXES', $groupValue.Trim(), 'Process')
    $clientValue = $null
    $sharedValue = $null
    $groupValue = $null

    & (Join-Path $repoRoot 'target/debug/openloops-desktop.exe') --check-connection
    $connectionExitCode = $LASTEXITCODE
    if ($connectionExitCode -ne 0) {
        Write-Host 'Some connection checks failed. See the results above.'
    }
} finally {
    [Environment]::SetEnvironmentVariable('OPENLOOPS_CLIENT_ID', $oldClient, 'Process')
    [Environment]::SetEnvironmentVariable('OPENLOOPS_SHARED_MAILBOX', $oldShared, 'Process')
    [Environment]::SetEnvironmentVariable('OPENLOOPS_GROUP_INBOXES', $oldGroups, 'Process')
    Pop-Location
}
exit $connectionExitCode
