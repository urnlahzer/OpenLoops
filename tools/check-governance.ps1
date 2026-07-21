[CmdletBinding()]
param(
    [string]$RegistryPath = 'contracts/governance/capabilities.json',
    [string]$ResearchDecisionPath = 'research/microsoft-graph/product-decisions.md',
    [string]$AdrIndexPath = 'docs/adr/README.md',
    [string]$Adr001Path = 'docs/adr/ADR-001-runtime-and-self-hosting-boundary.md',
    [string]$ThreatIndexPath = 'docs/threat-model/README.md',
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) {
    throw 'Run this script from inside the OpenLoops Git repository.'
}

function Resolve-RepositoryInput {
    param([Parameter(Mandatory)][string]$Path)

    if ([IO.Path]::IsPathRooted($Path)) {
        return [IO.Path]::GetFullPath($Path)
    }
    return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Add-Failure {
    param([Parameter(Mandatory)][string]$Id)

    if (-not $script:failureSet.ContainsKey($Id)) {
        $script:failureSet[$Id] = $true
        $script:failures.Add($Id)
    }
}

function Test-ExactSet {
    param(
        [Parameter(Mandatory)][string[]]$Actual,
        [Parameter(Mandatory)][string[]]$Expected,
        [Parameter(Mandatory)][string]$FailureId
    )

    $actualUnique = @($Actual | Sort-Object -Unique)
    $expectedUnique = @($Expected | Sort-Object -Unique)
    if ($Actual.Count -ne $actualUnique.Count -or
        $actualUnique.Count -ne $expectedUnique.Count -or
        (Compare-Object -ReferenceObject $expectedUnique -DifferenceObject $actualUnique)) {
        Add-Failure -Id $FailureId
    }
}

$expectedOwners = 0..10 | ForEach-Object { 'OWN-{0:D2}' -f $_ }
$expectedAdrs = @(
    'ADR-001', 'ADR-002', 'ADR-003', 'ADR-PRIV-001', 'ADR-004', 'ADR-005',
    'ADR-006', 'ADR-007', 'ADR-008', 'ADR-009', 'ADR-010', 'ADR-011',
    'ADR-012', 'ADR-013'
)
$expectedGates = @(
    'G-ID', 'G-MAIL', 'G-TODO', 'G-CAL', 'G-ADDIN', 'G-STATE', 'G-MODEL',
    'G-AUTO', 'G-AUTO-FULL', 'G-SELFMAIL', 'G-PRIV', 'G-SEC-AUDIT',
    'G-RELEASE'
)
$expectedCapabilities = @(
    'work_school_core', 'personal_accounts', 'calendar_reminders',
    'calendar_invitation_loops', 'outlook_addin', 'encrypted_local_state',
    'external_model_providers', 'hybrid_reminder_mode',
    'fully_automatic_reminder_mode', 'self_email_summary',
    'shared_project_registration', 'microsoft_hosted_state_adapter'
)
$expectedOwnerContracts = [ordered]@{
    'OWN-00' = @{ status = 'accepted'; adrs = @('ADR-001', 'ADR-002'); gates = @('G-ID', 'G-ADDIN') }
    'OWN-01' = @{ status = 'accepted'; adrs = @('ADR-001', 'ADR-010', 'ADR-012'); gates = @('G-ADDIN', 'G-RELEASE') }
    'OWN-02' = @{ status = 'accepted'; adrs = @('ADR-002', 'ADR-003'); gates = @('G-ID', 'G-MAIL', 'G-TODO', 'G-CAL', 'G-ADDIN') }
    'OWN-03' = @{ status = 'accepted'; adrs = @('ADR-008', 'ADR-009', 'ADR-011'); gates = @('G-AUTO', 'G-AUTO-FULL') }
    'OWN-04' = @{ status = 'accepted'; adrs = @('ADR-009'); gates = @('G-TODO', 'G-CAL') }
    'OWN-05' = @{ status = 'accepted'; adrs = @('ADR-004', 'ADR-006', 'ADR-009'); gates = @('G-CAL', 'G-MAIL') }
    'OWN-06' = @{ status = 'accepted_with_security_condition'; adrs = @('ADR-PRIV-001', 'ADR-005'); gates = @('G-STATE', 'G-PRIV', 'G-SEC-AUDIT') }
    'OWN-07' = @{ status = 'accepted'; adrs = @('ADR-PRIV-001', 'ADR-006', 'ADR-009'); gates = @('G-STATE', 'G-PRIV') }
    'OWN-08' = @{ status = 'accepted'; adrs = @('ADR-007'); gates = @('G-MODEL', 'G-PRIV') }
    'OWN-09' = @{ status = 'accepted'; adrs = @('ADR-004'); gates = @('G-MAIL') }
    'OWN-10' = @{ status = 'accepted'; adrs = @('ADR-013'); gates = @('G-SELFMAIL') }
}
$expectedGateFallbacks = [ordered]@{
    'G-ID' = 'identity-dependent capabilities remain disabled'
    'G-MAIL' = 'mail detection and evidence capabilities remain disabled'
    'G-TODO' = 'To Do mutations remain disabled'
    'G-CAL' = 'calendar reminders and invitation loops remain disabled'
    'G-ADDIN' = 'Outlook add-in capability remains disabled; native UI is not a silent substitute'
    'G-STATE' = 'durable state remains disabled; session-only or fail-closed behavior applies'
    'G-MODEL' = 'model transmission and model-dependent analysis remain disabled'
    'G-AUTO' = 'hybrid remains unavailable and confirmation-first remains the only mutation mode'
    'G-AUTO-FULL' = 'fully automatic mode remains unavailable'
    'G-SELFMAIL' = 'self-email remains unavailable; add-in summary is the approved direction'
    'G-PRIV' = 'affected persistence, transmission, diagnostics, and packaging paths remain disabled'
    'G-SEC-AUDIT' = 'broad public release remains blocked'
    'G-RELEASE' = 'publishing and official distribution remain blocked'
}
$expectedThreatFlows = [ordered]@{
    oauth_token_state = @{
        label = 'OAuth transactions and token state'; adrs = @('ADR-002', 'ADR-003', 'ADR-005'); gates = @('G-ID', 'G-PRIV')
        prohibited_behavior = 'no secrets, embedded credentials, application permissions, plaintext cache, device-code downgrade, or content-bearing authentication logs'
        fallback = 'identity-dependent capabilities remain disabled'
    }
    graph_mail_content = @{
        label = 'Graph mail content and synchronization'; adrs = @('ADR-003', 'ADR-004', 'ADR-006'); gates = @('G-MAIL', 'G-PRIV')
        prohibited_behavior = 'no real content in fixtures, logs, or state; no unbounded fetch or complete-event-observation claim'
        fallback = 'mail detection remains disabled'
    }
    model_transmission = @{
        label = 'Model transmission'; adrs = @('ADR-007', 'ADR-PRIV-001'); gates = @('G-MODEL', 'G-PRIV')
        prohibited_behavior = 'no implicit external transmission, redirect, ambient proxy route, key logging, transcript persistence, or model mutation authority'
        fallback = 'model-dependent analysis remains unavailable'
    }
    addin_bridge = @{
        label = 'Outlook add-in and local bridge'; adrs = @('ADR-010', 'ADR-PRIV-001'); gates = @('G-ADDIN', 'G-PRIV')
        prohibited_behavior = 'no browser persistence, bearer secret in a URL, unpaired command, machine-wide trust, or add-in synchronization authority'
        fallback = 'Outlook add-in remains disabled; native recovery only'
    }
    local_state = @{
        label = 'Local state and cryptographic keys'; adrs = @('ADR-PRIV-001', 'ADR-005'); gates = @('G-STATE', 'G-PRIV', 'G-SEC-AUDIT')
        prohibited_behavior = 'no human-readable mailbox text, unapproved derived field, plaintext fallback, rollback-blind mutation, or cross-user access'
        fallback = 'durable state remains disabled; session-only or fail-closed behavior applies'
    }
    microsoft_artifacts = @{
        label = 'Microsoft reminder artifacts'; adrs = @('ADR-009', 'ADR-011', 'ADR-013'); gates = @('G-TODO', 'G-CAL', 'G-AUTO', 'G-AUTO-FULL', 'G-SELFMAIL')
        prohibited_behavior = 'no unconfirmed or ungated write, other recipient or attendee, blind retry, automatic delete, or reminder-driven loop closure'
        fallback = 'confirmation-first applies where separately gated; otherwise the capability remains disabled'
    }
    diagnostics_release_artifacts = @{
        label = 'Diagnostics, crashes, support, and CI'; adrs = @('ADR-PRIV-001', 'ADR-012'); gates = @('G-PRIV', 'G-RELEASE', 'G-SEC-AUDIT')
        prohibited_behavior = 'no content, identifier, token, raw URL, prompt, model output, transcript, automatic upload, or non-allowlisted diagnostic field'
        fallback = 'diagnostics, export, and release remain blocked'
    }
    installation_update = @{
        label = 'Installation and update'; adrs = @('ADR-012'); gates = @('G-RELEASE', 'G-SEC-AUDIT')
        prohibited_behavior = 'no unsigned package or metadata, downgrade, unverified staging, hidden credential, source map, or uninspected artifact'
        fallback = 'publishing and update remain disabled'
    }
    disconnect_uninstall = @{
        label = 'Disconnect and uninstall'; adrs = @('ADR-002', 'ADR-005', 'ADR-010', 'ADR-012'); gates = @('G-ID', 'G-ADDIN', 'G-STATE', 'G-PRIV', 'G-RELEASE')
        prohibited_behavior = 'no global-logout claim, orphaned bridge trust or secret, silent artifact deletion, or undisclosed residual state'
        fallback = 'cleanup failure blocks supported release'
    }
}
$expectedCapabilityContracts = [ordered]@{
    work_school_core = @{
        owners = @('OWN-00', 'OWN-01', 'OWN-02'); gates = @('G-ID', 'G-MAIL', 'G-TODO', 'G-ADDIN', 'G-PRIV')
        permissions = @('unresolved_sign_in_scopes:G-ID', 'Mail.Read', 'Tasks.ReadWrite', 'unresolved_office_manifest:G-ADDIN')
        processing_scope = 'Inbox and Sent Items by default; other owned folders opt-in after G-MAIL'
        fallback = 'no Microsoft 365 capability is enabled'
    }
    personal_accounts = @{
        owners = @('OWN-02'); gates = @('G-ID', 'G-MAIL', 'G-TODO', 'G-CAL', 'G-ADDIN')
        permissions = @('complete_personal_account_matrix_required'); processing_scope = 'none while disabled'
        fallback = 'commercial-global work/school remains the only target matrix'
    }
    calendar_reminders = @{
        owners = @('OWN-04'); gates = @('G-CAL', 'G-PRIV'); permissions = @('unresolved_least_delegated_calendar_scope:G-CAL')
        processing_scope = 'none while disabled'
        fallback = 'To Do-first preview may proceed only after its own gates; complete PRD MVP remains blocked'
    }
    calendar_invitation_loops = @{
        owners = @('OWN-05'); gates = @('G-CAL', 'G-MAIL', 'G-PRIV')
        permissions = @('Mail.Read', 'unresolved_least_delegated_calendar_scope:G-CAL'); processing_scope = 'none while disabled'
        fallback = 'invitation-loop claims remain unavailable'
    }
    outlook_addin = @{
        owners = @('OWN-00', 'OWN-01'); gates = @('G-ADDIN', 'G-PRIV'); permissions = @('unresolved_office_manifest:G-ADDIN')
        processing_scope = 'no durable browser storage and no companion bridge while disabled'
        fallback = 'native recovery surface only; no silent replacement of the Outlook add-in'
    }
    encrypted_local_state = @{
        owners = @('OWN-06', 'OWN-07'); gates = @('G-STATE', 'G-PRIV', 'G-SEC-AUDIT'); permissions = @('no_graph_scope')
        processing_scope = 'session-only or fail-closed until ADR-PRIV-001 and ADR-005 are accepted and gates pass'
        fallback = 'no plaintext persistence'
    }
    external_model_providers = @{
        owners = @('OWN-08'); gates = @('G-MODEL', 'G-PRIV'); permissions = @('no_graph_scope', 'exact_provider_endpoint_consent_required')
        processing_scope = 'no mailbox content transmission while disabled'
        fallback = 'model-dependent analysis remains unavailable; no implicit provider selection'
    }
    hybrid_reminder_mode = @{
        owners = @('OWN-03'); gates = @('G-AUTO', 'G-TODO', 'G-PRIV'); permissions = @('Tasks.ReadWrite')
        processing_scope = 'no automatic mutations while disabled'
        fallback = 'confirmation-first remains the only available mutation mode'
    }
    fully_automatic_reminder_mode = @{
        owners = @('OWN-03'); gates = @('G-AUTO-FULL', 'G-TODO', 'G-PRIV'); permissions = @('Tasks.ReadWrite')
        processing_scope = 'no automatic mutations while disabled'
        fallback = 'feature remains unavailable even if hybrid later passes'
    }
    self_email_summary = @{
        owners = @('OWN-10'); gates = @('G-SELFMAIL', 'G-PRIV'); permissions = @('Mail.Send')
        processing_scope = 'no mail send while disabled'
        fallback = 'in-add-in summary direction only; it remains subject to G-ADDIN'
    }
    shared_project_registration = @{
        owners = @('OWN-00', 'OWN-01', 'OWN-02'); gates = @('G-ID', 'G-RELEASE')
        permissions = @('delegated_public_client_only', 'publisher_governance_required'); processing_scope = 'none while disabled'
        fallback = 'placeholder-only BYO public-client registration remains the planned source-build path'
    }
    microsoft_hosted_state_adapter = @{
        owners = @('OWN-06'); gates = @('G-STATE', 'G-PRIV'); permissions = @('unresolved_feasibility_only')
        processing_scope = 'none while disabled'
        fallback = 'does not replace the gated encrypted-local baseline'
    }
}

$script:failures = [System.Collections.Generic.List[string]]::new()
$script:failureSet = @{}

try {
    $resolvedRegistry = Resolve-RepositoryInput -Path $RegistryPath
    $registry = Get-Content -Raw -LiteralPath $resolvedRegistry | ConvertFrom-Json
} catch {
    [Console]::Error.WriteLine('BLOCKED: P0-TRACE-001 registry parse failed.')
    exit 1
}

if ($registry.schema_version -ne 1 -or $registry.work_item -ne 'P0-WI-01') {
    Add-Failure -Id 'P0-TRACE-001'
}

$ownerIds = @($registry.owner_decisions | ForEach-Object { [string]$_.id })
$adrIds = @($registry.adrs | ForEach-Object { [string]$_.id })
$gateIds = @($registry.gates | ForEach-Object { [string]$_.id })
Test-ExactSet -Actual $ownerIds -Expected $expectedOwners -FailureId 'P0-ADR-OWN-001'
Test-ExactSet -Actual $adrIds -Expected $expectedAdrs -FailureId 'P0-TRACE-001'
Test-ExactSet -Actual $gateIds -Expected $expectedGates -FailureId 'P0-TRACE-001'

foreach ($owner in @($registry.owner_decisions)) {
    $ownerId = [string]$owner.id
    if (-not $expectedOwnerContracts.Contains($ownerId)) {
        Add-Failure -Id 'P0-ADR-OWN-001'
        continue
    }
    $expectedOwner = $expectedOwnerContracts[$ownerId]
    if ([string]$owner.status -ne [string]$expectedOwner.status) {
        Add-Failure -Id 'P0-ADR-OWN-001'
    }
    Test-ExactSet -Actual @($owner.adrs) -Expected @($expectedOwner.adrs) -FailureId 'P0-ADR-OWN-001'
    Test-ExactSet -Actual @($owner.gates) -Expected @($expectedOwner.gates) -FailureId 'P0-ADR-OWN-001'
}

$acceptedAdrs = @($registry.adrs | Where-Object { $_.status -eq 'accepted' } | ForEach-Object { [string]$_.id })
Test-ExactSet -Actual $acceptedAdrs -Expected @('ADR-001', 'ADR-002', 'ADR-003', 'ADR-004', 'ADR-005', 'ADR-006', 'ADR-007', 'ADR-008', 'ADR-009', 'ADR-010', 'ADR-011', 'ADR-012', 'ADR-PRIV-001') -FailureId 'P0-TRACE-001'
if (@($registry.adrs | Where-Object { $_.status -notin @('accepted', 'planned') }).Count -gt 0) {
    Add-Failure -Id 'P0-TRACE-001'
}

foreach ($gate in @($registry.gates)) {
    $gateId = [string]$gate.id
    if ($gate.status -ne 'unrun' -or $gate.owner_on_failure -ne 'product_owner' -or
        -not $expectedGateFallbacks.Contains($gateId) -or
        [string]$gate.fallback -ne [string]$expectedGateFallbacks[$gateId]) {
        Add-Failure -Id 'P0-CAP-001'
    }
}

$threatFlowIds = @($registry.threat_flows | ForEach-Object { [string]$_.id })
Test-ExactSet -Actual $threatFlowIds -Expected @($expectedThreatFlows.Keys) -FailureId 'P0-THREAT-001'
foreach ($flow in @($registry.threat_flows)) {
    $flowId = [string]$flow.id
    if (-not $expectedThreatFlows.Contains($flowId)) {
        Add-Failure -Id 'P0-THREAT-001'
        continue
    }
    $expectedFlow = $expectedThreatFlows[$flowId]
    if ([string]$flow.label -ne [string]$expectedFlow.label -or
        $flow.owner_on_failure -ne 'product_owner' -or
        [string]$flow.prohibited_behavior -ne [string]$expectedFlow.prohibited_behavior -or
        [string]$flow.fallback -ne [string]$expectedFlow.fallback) {
        Add-Failure -Id 'P0-THREAT-001'
    }
    Test-ExactSet -Actual @($flow.adrs) -Expected @($expectedFlow.adrs) -FailureId 'P0-THREAT-001'
    Test-ExactSet -Actual @($flow.gates) -Expected @($expectedFlow.gates) -FailureId 'P0-THREAT-001'
}

if (@($registry.product_acceptance_criteria_completed).Count -ne 0) {
    Add-Failure -Id 'P0-TRACE-001'
}

$capabilityIds = @($registry.capabilities | ForEach-Object { [string]$_.id })
Test-ExactSet -Actual $capabilityIds -Expected $expectedCapabilities -FailureId 'P0-CAP-001'

foreach ($capability in @($registry.capabilities)) {
    $capabilityId = [string]$capability.id
    if (-not $expectedCapabilityContracts.Contains($capabilityId)) {
        Add-Failure -Id 'P0-CAP-001'
        continue
    }
    $expectedCapability = $expectedCapabilityContracts[$capabilityId]
    if ($capability.state -ne 'disabled' -or $capability.advertised -ne $false -or
        $capability.owner_on_failure -ne 'product_owner' -or
        [string]$capability.fallback -ne [string]$expectedCapability.fallback -or
        [string]$capability.processing_scope -ne [string]$expectedCapability.processing_scope) {
        Add-Failure -Id 'P0-CAP-001'
    }
    Test-ExactSet -Actual @($capability.owner_decisions) -Expected @($expectedCapability.owners) -FailureId 'P0-CAP-001'
    Test-ExactSet -Actual @($capability.gates) -Expected @($expectedCapability.gates) -FailureId 'P0-CAP-001'
    Test-ExactSet -Actual @($capability.permission_contracts) -Expected @($expectedCapability.permissions) -FailureId 'P0-CAP-001'
}

$resolvedResearchDecisionPath = Resolve-RepositoryInput -Path $ResearchDecisionPath
$researchText = Get-Content -Raw -LiteralPath $resolvedResearchDecisionPath
$researchRows = @([regex]::Matches($researchText, '(?m)^\| (OWN-\d{2}) \| ([^|]+) \| ([^|]+) \| ([^|]+) \|$'))
$researchOwners = @($researchRows | ForEach-Object { $_.Groups[1].Value })
Test-ExactSet -Actual $researchOwners -Expected $expectedOwners -FailureId 'P0-ADR-OWN-001'
foreach ($row in $researchRows) {
    $ownerId = $row.Groups[1].Value
    $consequence = $row.Groups[2].Value.Trim()
    $researchAdrs = @($row.Groups[3].Value -split ',' | ForEach-Object { $_.Trim() })
    $researchGates = @($row.Groups[4].Value -split ',' | ForEach-Object { $_.Trim() })
    $registryOwner = @($registry.owner_decisions | Where-Object { $_.id -eq $ownerId })
    if ($registryOwner.Count -ne 1 -or [string]::IsNullOrWhiteSpace($consequence) -or
        $consequence -match '(?i)\b(?:reopen|undecided)\b') {
        Add-Failure -Id 'P0-ADR-OWN-001'
        continue
    }
    Test-ExactSet -Actual $researchAdrs -Expected @($registryOwner[0].adrs) -FailureId 'P0-ADR-OWN-001'
    Test-ExactSet -Actual $researchGates -Expected @($registryOwner[0].gates) -FailureId 'P0-ADR-OWN-001'
}

$resolvedAdrIndexPath = Resolve-RepositoryInput -Path $AdrIndexPath
$adrIndexText = Get-Content -Raw -LiteralPath $resolvedAdrIndexPath
$indexAdrs = @([regex]::Matches($adrIndexText, '(?m)^\| (?:\[)?(ADR-(?:PRIV-)?\d{3})') | ForEach-Object { $_.Groups[1].Value })
Test-ExactSet -Actual $indexAdrs -Expected $expectedAdrs -FailureId 'P0-TRACE-001'

$resolvedAdr001Path = Resolve-RepositoryInput -Path $Adr001Path
$adr001Text = Get-Content -Raw -LiteralPath $resolvedAdr001Path
foreach ($required in @('OWN-00', 'OWN-01', 'pure Rust', 'Office.js/TypeScript', 'system browser', 'PKCE', 'session-only', 'fail-closed')) {
    if ($adr001Text -notmatch [regex]::Escape($required)) { Add-Failure -Id 'P0-ADR-OWN-001' }
}

$resolvedThreatIndexPath = Resolve-RepositoryInput -Path $ThreatIndexPath
$threatDocLines = @(Get-Content -LiteralPath $resolvedThreatIndexPath | Where-Object { $_ -match '^\| [a-z][a-z_]+ \|' })
$threatDocIds = [System.Collections.Generic.List[string]]::new()
foreach ($line in $threatDocLines) {
    $cells = @(($line.Trim().Trim('|') -split '\|') | ForEach-Object { $_.Trim() })
    if ($cells.Count -ne 7) {
        Add-Failure -Id 'P0-THREAT-001'
        continue
    }
    $flowId = $cells[0]
    $threatDocIds.Add($flowId)
    $registryFlow = @($registry.threat_flows | Where-Object { $_.id -eq $flowId })
    if ($registryFlow.Count -ne 1 -or $cells[1] -ne [string]$registryFlow[0].label -or
        $cells[2] -ne [string]$registryFlow[0].prohibited_behavior -or
        $cells[5] -ne [string]$registryFlow[0].owner_on_failure -or
        $cells[6] -ne [string]$registryFlow[0].fallback) {
        Add-Failure -Id 'P0-THREAT-001'
        continue
    }
    $docAdrs = @($cells[3] -split ',' | ForEach-Object { $_.Trim() })
    $docGates = @($cells[4] -split ',' | ForEach-Object { $_.Trim() })
    Test-ExactSet -Actual $docAdrs -Expected @($registryFlow[0].adrs) -FailureId 'P0-THREAT-001'
    Test-ExactSet -Actual $docGates -Expected @($registryFlow[0].gates) -FailureId 'P0-THREAT-001'
}
Test-ExactSet -Actual @($threatDocIds) -Expected @($expectedThreatFlows.Keys) -FailureId 'P0-THREAT-001'

if ($failures.Count -gt 0) {
    [Console]::Error.WriteLine("BLOCKED: governance checks failed: $($failures -join ', ')")
    exit 1
}

if (-not $Quiet) {
    Write-Host 'OpenLoops governance checks passed (P0-ADR-OWN-001, P0-CAP-001, P0-THREAT-001, P0-TRACE-001).'
}
