$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$source = Join-Path $repoRoot 'contracts\support\support-matrix.json'
$checker = Join-Path $PSScriptRoot 'check-support-matrix.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('openloops-support-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tempRoot)
$passed = 0

function Invoke-Check([string]$Path, [switch]$SemanticTestMode) {
    $arguments = @('-NoProfile','-File',$checker,'-ManifestPath',$Path)
    if ($SemanticTestMode) { $arguments += '-SemanticTestMode' }
    $output = (& pwsh @arguments 2>&1 | Out-String)
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
}
function Get-ExpectedFailure([string]$Name) {
    if ($Name -eq 'malformed-json') { return 'support matrix is not strict JSON' }
    if ($Name -match '^(unknown-(top|claim)-key|missing-windows-row|duplicate-windows-row|unknown-windows-row|missing-outlook-target|invariant-removed|duplicate-json-key)$') { return 'P0-SUPPORT-INVENTORY-001' }
    if ($Name -match '^(owner-|support-meaning|acceptance-criterion|supported-row|capability-|gate-pass|completed-ac)') { return 'P0-SUPPORT-CLAIMS-001' }
    if ($Name -match '^(windows-|deferred-windows|remote-runtime)') { return 'P0-SUPPORT-WINDOWS-001' }
    if ($Name -match '^(outlook-|new-outlook|web-|mac-promoted)') { return 'P0-SUPPORT-OUTLOOK-001' }
    if ($Name -match '^(addin-|shared-permission|sync-authority)') { return 'P0-SUPPORT-ADDIN-001' }
    if ($Name -match '^(account-|personal-|shared-mailbox|multi-account)') { return 'P0-SUPPORT-ACCOUNT-001' }
    if ($Name -match '^(authority-|graph-|government-|china-)') { return 'P0-SUPPORT-CLOUD-001' }
    if ($Name -match '^(provider-|local-|loopback-|cloud-|custom-private)') { return 'P0-SUPPORT-MODEL-001' }
    if ($Name -match '^(freshness-|official-source)') { return 'P0-SUPPORT-FRESHNESS-001' }
    throw "Negative support-matrix case has no expected failure mapping: $Name"
}
function Expect-Rejected([string]$Name, [scriptblock]$Mutation) {
    $path = Join-Path $tempRoot ($Name + '.json')
    $manifest = Get-Content $source -Raw | ConvertFrom-Json
    & $Mutation $manifest
    $manifest | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $path -Encoding utf8NoBOM
    $result = Invoke-Check $path -SemanticTestMode
    $expected = Get-ExpectedFailure $Name
    if ($result.ExitCode -eq 0) { throw "Negative support-matrix case was accepted: $Name" }
    if ($result.Output -notmatch [regex]::Escape($expected)) { throw "Negative support-matrix case failed for the wrong reason: $Name (expected $expected)" }
    $script:passed++
}
function Expect-RawRejected([string]$Name, [scriptblock]$Mutation) {
    $path = Join-Path $tempRoot ($Name + '.json')
    $changed = & $Mutation (Get-Content $source -Raw)
    Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM
    $result = Invoke-Check $path -SemanticTestMode
    $expected = Get-ExpectedFailure $Name
    if ($result.ExitCode -eq 0) { throw "Negative support-matrix case was accepted: $Name" }
    if ($result.Output -notmatch [regex]::Escape($expected)) { throw "Negative support-matrix case failed for the wrong reason: $Name (expected $expected)" }
    $script:passed++
}

try {
    if ((Invoke-Check $source).ExitCode -ne 0) { throw 'Baseline support matrix failed.' }

    $fingerprintPath = Join-Path $tempRoot 'fingerprint-only.json'
    $fingerprintManifest = Get-Content $source -Raw | ConvertFrom-Json
    $fingerprintManifest.matrix_disposition.meaning += ' '
    $fingerprintManifest | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $fingerprintPath -Encoding utf8NoBOM
    $fingerprintResult = Invoke-Check $fingerprintPath
    if ($fingerprintResult.ExitCode -eq 0 -or $fingerprintResult.Output -notmatch 'P0-SUPPORT-INVENTORY-001') {
        throw 'Fingerprint-only rejection did not report P0-SUPPORT-INVENTORY-001.'
    }

    Expect-Rejected 'unknown-top-key' { param($m) $m | Add-Member extension @{} }
    Expect-Rejected 'unknown-claim-key' { param($m) $m.claim_state | Add-Member future_support @() }
    Expect-Rejected 'owner-disposition-reopened' { param($m) $m.matrix_disposition.status = 'proposed' }
    Expect-Rejected 'owner-date-drift' { param($m) $m.matrix_disposition.approved_on = '2026-07-20' }
    Expect-Rejected 'support-meaning-overclaim' { param($m) $m.matrix_disposition.meaning = 'supported now' }
    Expect-Rejected 'owner-set-drift' { param($m) $m.owner_decisions = @('OWN-01') }
    Expect-Rejected 'acceptance-criterion-added' { param($m) $m.acceptance_criteria = @('AC-20') }
    Expect-Rejected 'supported-row-added' { param($m) $m.claim_state.supported_rows = @('windows_11_25h2_x64') }
    Expect-Rejected 'capability-enabled-claim' { param($m) $m.claim_state.enabled_capabilities = @('outlook_addin') }
    Expect-Rejected 'capability-advertised-claim' { param($m) $m.claim_state.advertised_capabilities = @('work_school_core') }
    Expect-Rejected 'gate-pass-claim' { param($m) $m.claim_state.gates_passed = @('G-ADDIN') }
    Expect-Rejected 'completed-ac-claim' { param($m) $m.claim_state.acceptance_criteria_completed = @('AC-20') }

    Expect-Rejected 'missing-windows-row' { param($m) $m.matrices.windows = @($m.matrices.windows | Select-Object -Skip 1) }
    Expect-Rejected 'duplicate-windows-row' { param($m) $m.matrices.windows += $m.matrices.windows[0] }
    Expect-Rejected 'unknown-windows-row' { param($m) $m.matrices.windows += [pscustomobject]@{ id='future'; disposition='target'; current_state='disabled'; advertised=$false; boundary=@{}; gates=@(); required_evidence=@('test'); fallback='none'; sources=@('SRC-WIN-RELEASE') } }
    Expect-Rejected 'windows-version-drift' { param($m) $m.matrices.windows[0].boundary.version = '23H2' }
    Expect-Rejected 'windows-architecture-drift' { param($m) $m.matrices.windows[1].boundary.architecture = 'ARM64' }
    Expect-Rejected 'windows-servicing-removed' { param($m) $m.matrices.windows[1].boundary.servicing_rule = 'any build' }
    Expect-Rejected 'windows-target-enabled' { param($m) $m.matrices.windows[1].current_state = 'enabled' }
    Expect-Rejected 'deferred-windows-enabled' { param($m) $m.matrices.windows[2].current_state = 'supported' }
    Expect-Rejected 'remote-runtime-targeted' { param($m) $m.matrices.windows[5].disposition = 'approved_validation_target' }

    Expect-Rejected 'missing-outlook-target' { param($m) $m.matrices.outlook = @($m.matrices.outlook | Where-Object id -ne 'new_outlook_windows') }
    Expect-Rejected 'outlook-target-enabled' { param($m) $m.matrices.outlook[0].current_state = 'enabled' }
    Expect-Rejected 'outlook-addin-gate-removed' { param($m) $m.matrices.outlook[0].gates = @('G-PRIV') }
    Expect-Rejected 'new-outlook-no-item-overclaim' { param($m) $m.matrices.outlook[1].boundary.activation = 'always available dashboard' }
    Expect-Rejected 'web-remote-companion-overclaim' { param($m) $m.matrices.outlook[2].boundary.activation = 'remote companion allowed' }
    Expect-Rejected 'web-browser-drift' { param($m) $m.matrices.outlook[2].boundary.browsers += 'Safari' }
    Expect-Rejected 'mac-promoted' { param($m) $m.matrices.outlook[4].disposition = 'approved_validation_target' }

    Expect-Rejected 'addin-permission-broadened' { param($m) $m.addin_contract.manifest_minimum.permission = 'ReadWriteMailbox' }
    Expect-Rejected 'addin-requirement-drift' { param($m) $m.addin_contract.manifest_minimum.version = '1.12' }
    Expect-Rejected 'shared-permission-removed-from-prohibition' { param($m) $m.addin_contract.prohibited_permissions_or_authority = @($m.addin_contract.prohibited_permissions_or_authority | Where-Object { $_ -ne 'Mailbox.SharedFolder' }) }
    Expect-Rejected 'sync-authority-removed-from-prohibition' { param($m) $m.addin_contract.prohibited_permissions_or_authority = @($m.addin_contract.prohibited_permissions_or_authority | Where-Object { $_ -ne 'synchronization_authority' }) }

    Expect-Rejected 'account-count-expanded' { param($m) $m.matrices.accounts[0].boundary.account_count = 2 }
    Expect-Rejected 'personal-signin-only' { param($m) $m.matrices.accounts[1].required_evidence = @('signin') }
    Expect-Rejected 'personal-calendar-gate-removed' { param($m) $m.matrices.accounts[1].gates = @($m.matrices.accounts[1].gates | Where-Object { $_ -ne 'G-CAL' }) }
    Expect-Rejected 'shared-mailbox-enabled' { param($m) $m.matrices.accounts[2].current_state = 'enabled' }
    Expect-Rejected 'multi-account-promoted' { param($m) $m.matrices.accounts[3].disposition = 'approved_validation_target' }

    Expect-Rejected 'authority-origin-drift' { param($m) $m.matrices.clouds[0].boundary.authority_origin = 'https://example.invalid' }
    Expect-Rejected 'graph-origin-drift' { param($m) $m.matrices.clouds[0].boundary.graph_origin = 'https://graph.microsoft.us' }
    Expect-Rejected 'government-cloud-enabled' { param($m) $m.matrices.clouds[1].current_state = 'enabled' }
    Expect-Rejected 'china-cloud-promoted' { param($m) $m.matrices.clouds[2].disposition = 'approved_validation_target' }

    Expect-Rejected 'provider-default-network' { param($m) $m.matrices.model_providers[0].current_state = 'provider_requests_allowed' }
    Expect-Rejected 'local-provider-enabled' { param($m) $m.matrices.model_providers[1].current_state = 'enabled' }
    Expect-Rejected 'local-model-gate-removed' { param($m) $m.matrices.model_providers[1].gates = @('G-PRIV') }
    Expect-Rejected 'loopback-locality-overclaim' { param($m) $m.matrices.model_providers[1].boundary.locality = 'loopback proves local' }
    Expect-Rejected 'local-only-proof-removed' { param($m) $m.matrices.model_providers[1].required_evidence = @($m.matrices.model_providers[1].required_evidence | Where-Object { $_ -ne 'local_only_proof' }) }
    Expect-Rejected 'local-cloud-fallback' { param($m) $m.matrices.model_providers[1].fallback = 'use cloud instead' }
    Expect-Rejected 'cloud-origin-drift' { param($m) $m.matrices.model_providers[2].boundary.api_base = 'https://api.example.invalid' }
    Expect-Rejected 'cloud-origin-editable' { param($m) $m.matrices.model_providers[2].boundary.editable = $true }
    Expect-Rejected 'cloud-structured-output-overclaim' { param($m) $m.matrices.model_providers[2].boundary.structured_output = 'server schema guaranteed' }
    Expect-Rejected 'custom-private-target' { param($m) $m.matrices.model_providers[3].boundary.origin = 'private addresses allowed' }

    Expect-Rejected 'freshness-disabled' { param($m) $m.freshness_policy.release_recheck_required = $false }
    Expect-Rejected 'freshness-subject-removed' { param($m) $m.freshness_policy.recheck_subjects = @($m.freshness_policy.recheck_subjects | Select-Object -Skip 1) }
    Expect-Rejected 'freshness-enables-on-change' { param($m) $m.freshness_policy.stale_or_changed_result = 'keep enabled' }
    Expect-Rejected 'official-source-removed' { param($m) $m.sources = @($m.sources | Where-Object id -ne 'SRC-OUTLOOK-NOITEM') }
    Expect-Rejected 'official-source-downgraded' { param($m) ($m.sources | Where-Object id -eq 'SRC-OLLAMA-CLOUD').location = 'http://example.invalid' }
    Expect-Rejected 'invariant-removed' { param($m) $m.cross_matrix_invariants = @($m.cross_matrix_invariants | Select-Object -Skip 1) }

    Expect-RawRejected 'duplicate-json-key' { param($raw) $raw -replace '"schema_version": 1,', '"schema_version": 1, "schema_version": 1,' }
    Expect-RawRejected 'malformed-json' { param($raw) $raw.Substring(0, $raw.Length - 2) }

    Write-Host "OpenLoops support-matrix negative suite passed ($passed synthetic rejection cases)."
} finally {
    if (Test-Path -LiteralPath $tempRoot) { Remove-Item -LiteralPath $tempRoot -Recurse -Force }
}
