[CmdletBinding()]
param(
    [string]$ManifestPath = 'contracts/identity/authentication-boundary.json',
    [string]$AdrPath = 'docs/adr/ADR-002-rust-authentication.md',
    [string]$ThreatPath = 'docs/threat-model/oauth-token-state.md',
    [string]$TraceabilityPath = 'docs/prd-traceability.md',
    [string]$GovernancePath = 'contracts/governance/capabilities.json',
    [string]$BuildManifestPath = 'contracts/build-skeleton/skeleton.json',
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) {
    throw 'Run this script from inside the OpenLoops Git repository.'
}

function Resolve-RepositoryInput([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Add-Failure([string]$Id) {
    if (-not $script:failureSet.ContainsKey($Id)) {
        $script:failureSet[$Id] = $true
        $script:failures.Add($Id)
    }
}

function Test-ExactSet([object[]]$Actual, [object[]]$Expected, [string]$FailureId) {
    $actualStrings = @($Actual | ForEach-Object { [string]$_ })
    $expectedStrings = @($Expected | ForEach-Object { [string]$_ })
    $actualUnique = @($actualStrings | Sort-Object -Unique)
    $expectedUnique = @($expectedStrings | Sort-Object -Unique)
    if ($actualStrings.Count -ne $actualUnique.Count -or
        $actualUnique.Count -ne $expectedUnique.Count -or
        (Compare-Object -ReferenceObject $expectedUnique -DifferenceObject $actualUnique)) {
        Add-Failure $FailureId
    }
}

function Test-Empty([object[]]$Values, [string]$FailureId) {
    if (@($Values).Count -ne 0) { Add-Failure $FailureId }
}

$script:failures = [Collections.Generic.List[string]]::new()
$script:failureSet = @{}
$expectedCanonicalHash = '6cc6ea196c063d28c264d0d3d8ca47a365875c994d199480793d9f561efe439a'
$expectedTraceIds = @(
    'P0-AUTH-INVENTORY-001', 'P0-AUTH-FLOW-001', 'P0-AUTH-REDIRECT-001',
    'P0-AUTH-ACCOUNT-001', 'P0-AUTH-TOKEN-001', 'P0-AUTH-CONCURRENCY-001',
    'P0-AUTH-DISCONNECT-001', 'P0-AUTH-DEPENDENCIES-001',
    'P0-AUTH-CLAIMS-001', 'P0-AUTH-FRESH-CHECKER-001'
)

try {
    $manifest = Get-Content -Raw -LiteralPath (Resolve-RepositoryInput $ManifestPath) | ConvertFrom-Json
    $canonical = $manifest | ConvertTo-Json -Depth 100 -Compress
    $actualHash = [Convert]::ToHexString(
        [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))
    ).ToLowerInvariant()
    if ($actualHash -ne $expectedCanonicalHash) { Add-Failure 'P0-AUTH-INVENTORY-001' }
} catch {
    [Console]::Error.WriteLine('BLOCKED: P0-AUTH-INVENTORY-001 manifest parse failed.')
    exit 1
}

if ($manifest.schema_version -ne 1 -or $manifest.work_item -ne 'P0-WI-05' -or
    $manifest.adr -ne 'ADR-002' -or
    $manifest.decision_status -ne 'accepted_contract_runtime_unimplemented') {
    Add-Failure 'P0-AUTH-INVENTORY-001'
}
Test-ExactSet @($manifest.owner_decisions) @('OWN-00', 'OWN-02') 'P0-AUTH-INVENTORY-001'
Test-ExactSet @($manifest.requirements) @(
    'OL-GOV-001', 'OL-AUTH-001', 'OL-AUTH-002', 'OL-AUTH-004', 'OL-AUTH-005',
    'OL-AUTH-006', 'OL-NFR-005', 'OL-NFR-006', 'OL-NFR-011', 'OL-NFR-012'
) 'P0-AUTH-INVENTORY-001'

if ($manifest.flow.client_type -ne 'delegated_public_client' -or
    $manifest.flow.authorization_surface -ne 'system_browser' -or
    $manifest.flow.grant -ne 'authorization_code' -or
    $manifest.flow.pkce_method -ne 'S256' -or
    $manifest.flow.confidential_client_material -ne 'prohibited' -or
    $manifest.flow.runtime_status -ne 'unimplemented') {
    Add-Failure 'P0-AUTH-FLOW-001'
}
Test-ExactSet @($manifest.flow.blocking_gates) @('G-ID', 'G-PRIV') 'P0-AUTH-FLOW-001'
Test-ExactSet @($manifest.prohibited_flows) @(
    'application_permissions', 'client_credentials', 'device_code_fallback',
    'embedded_credential_collection', 'embedded_webview', 'implicit', 'ropc',
    'tenant_wide_access', 'wam_mvp_dependency'
) 'P0-AUTH-FLOW-001'

if ($manifest.redirect_contract.binding -ne 'same_machine_loopback_only' -or
    $manifest.redirect_contract.host_selection -ne 'unresolved_pending_G-ID' -or
    $manifest.redirect_contract.port -ne 'ephemeral_os_assigned' -or
    $manifest.redirect_contract.lan_binding -ne 'prohibited' -or
    $manifest.redirect_contract.wildcard_redirect -ne 'prohibited' -or
    $manifest.redirect_contract.remote_callback -ne 'prohibited' -or
    $manifest.redirect_contract.response_mode -ne 'query_only_pending_G-ID_validation' -or
    $manifest.redirect_contract.http_method -ne 'GET_only_pending_G-ID_validation' -or
    $manifest.redirect_contract.form_post -ne 'prohibited_unless_future_owner_review_and_G-ID_revision' -or
    $manifest.redirect_contract.fragment_response -ne 'prohibited' -or
    $manifest.redirect_contract.host_authority_validation -ne 'exact_pending_listener_authority_required' -or
    $manifest.redirect_contract.duplicate_parameters -ne 'reject' -or
    $manifest.redirect_contract.malformed_parameters -ne 'reject' -or
    $manifest.redirect_contract.unexpected_parameters -ne 'ignore_bounded_without_logging_persistence_or_exposure' -or
    $manifest.redirect_contract.mixed_success_error -ne 'reject' -or
    $manifest.redirect_contract.success_shape -ne 'exactly_one_code_and_state_no_error' -or
    $manifest.redirect_contract.error_shape -ne 'exactly_one_error_and_state_no_code') {
    Add-Failure 'P0-AUTH-REDIRECT-001'
}
Test-ExactSet @($manifest.redirect_contract.host_candidates) @('127.0.0.1', 'localhost') 'P0-AUTH-REDIRECT-001'

if ($manifest.transaction_contract.pending_transactions -ne 1 -or
    $manifest.transaction_contract.duplicate_callback -ne 'reject' -or
    $manifest.transaction_contract.replayed_callback -ne 'reject' -or
    $manifest.transaction_contract.expired_callback -ne 'reject' -or
    $manifest.transaction_contract.second_process -ne 'fail_closed' -or
    $manifest.transaction_contract.second_same_process_attempt -ne 'reject_without_mutating_incumbent' -or
    $manifest.transaction_contract.unsolicited_or_malformed_callback -ne 'reject_without_mutating_or_replacing_incumbent') {
    Add-Failure 'P0-AUTH-CONCURRENCY-001'
}

if ($manifest.account_contract.core_target -ne 'commercial_global_work_school_primary_mailbox' -or
    $manifest.account_contract.accounts_per_os_user -ne 1 -or
    $manifest.account_contract.personal_accounts -ne 'disabled_pending_complete_matrix' -or
    $manifest.account_contract.sovereign_clouds -ne 'unsupported' -or
    $manifest.account_contract.multiple_accounts -ne 'unsupported' -or
    $manifest.account_contract.cross_account_state_inheritance -ne 'prohibited') {
    Add-Failure 'P0-AUTH-ACCOUNT-001'
}

if ($manifest.token_boundary.persistence_status -ne 'ADR-005_contract_accepted_runtime_unimplemented_pending_G-ID_G-PRIV' -or
    $manifest.token_boundary.secure_store_failure -ne 'session_only_or_fail_closed_no_plaintext_fallback') {
    Add-Failure 'P0-AUTH-TOKEN-001'
}
Test-ExactSet @($manifest.token_boundary.prohibited_locations) @(
    'application_controlled_browser_storage', 'cli_arguments', 'diagnostics',
    'fixtures', 'logs', 'plaintext_files', 'repository', 'telemetry'
) 'P0-AUTH-TOKEN-001'
Test-ExactSet @($manifest.token_boundary.allowed_locations) @(
    'process_memory', 'transient_system_browser_protocol_url',
    'transient_loopback_callback_url', 'future_os_protected_store_after_gates'
) 'P0-AUTH-TOKEN-001'
if ($manifest.token_boundary.protocol_url_policy -ne
    'only opaque random state in the authorization request and opaque code plus exact state in the callback; never content, account data, return URLs, logging, copying, or application-controlled persistence') {
    Add-Failure 'P0-AUTH-TOKEN-001'
}
Test-Empty @($manifest.token_boundary.diagnostic_allowlist) 'P0-AUTH-TOKEN-001'

foreach ($claimName in @('acceptance_criteria_completed', 'gates_passed', 'capabilities_enabled',
        'capabilities_advertised', 'permissions_requested', 'network_contacts')) {
    Test-Empty @($manifest.claims.$claimName) 'P0-AUTH-CLAIMS-001'
}

if ($manifest.disconnect_contract.global_logout_claim -ne $false -or
    $manifest.disconnect_contract.global_token_revocation_claim -ne $false -or
    $manifest.disconnect_contract.browser_session_revocation_claim -ne $false -or
    $manifest.disconnect_contract.tenant_consent_revocation_claim -ne $false -or
    $manifest.disconnect_contract.cache_delete_failure -ne 'identity_and_reconnect_remain_disabled_owner_approved_recovery_required' -or
    $manifest.disconnect_contract.cache_delete_failure_disclosure -ne 'application-controlled protected cache cleanup is incomplete and residual Microsoft browser session or consent may also remain') {
    Add-Failure 'P0-AUTH-DISCONNECT-001'
}

$dependencyKeys = @($manifest.dependency_decisions | ForEach-Object { "$($_.crate)@$($_.version)" })
Test-ExactSet $dependencyKeys @(
    'httparse@1.10.1', 'oauth2@5.0.0', 'reqwest@0.12.28', 'tokio@1.53.0',
    'url@2.5.8', 'webbrowser@1.2.1'
) 'P0-AUTH-DEPENDENCIES-001'
$expectedDependencyContracts = [ordered]@{
    httparse = @{ version = '1.10.1'; license = 'MIT OR Apache-2.0'; features = @('std') }
    oauth2 = @{ version = '5.0.0'; license = 'MIT OR Apache-2.0'; features = @('reqwest', 'timing-resistant-secret-traits') }
    reqwest = @{ version = '0.12.28'; license = 'MIT OR Apache-2.0'; features = @('rustls-tls-native-roots') }
    tokio = @{ version = '1.53.0'; license = 'MIT'; features = @('io-util', 'net', 'rt', 'sync', 'time') }
    url = @{ version = '2.5.8'; license = 'MIT OR Apache-2.0'; features = @('std') }
    webbrowser = @{ version = '1.2.1'; license = 'MIT OR Apache-2.0'; features = @('hardened') }
}
foreach ($dependency in @($manifest.dependency_decisions)) {
    $crate = [string]$dependency.crate
    if (-not $expectedDependencyContracts.Contains($crate)) {
        Add-Failure 'P0-AUTH-DEPENDENCIES-001'
        continue
    }
    $expectedDependency = $expectedDependencyContracts[$crate]
    if ([string]$dependency.version -ne [string]$expectedDependency.version -or
        [string]$dependency.license -ne [string]$expectedDependency.license -or
        $dependency.default_features -ne $false) {
        Add-Failure 'P0-AUTH-DEPENDENCIES-001'
    }
    Test-ExactSet @($dependency.features) @($expectedDependency.features) 'P0-AUTH-DEPENDENCIES-001'
}
if ($manifest.dependency_policy.activation -ne 'selected_not_activated' -or
    $manifest.dependency_policy.cargo_lock_status -ne 'required_before_activation' -or
    $manifest.dependency_policy.git_dependencies -ne 'prohibited' -or
    $manifest.dependency_policy.wildcard_versions -ne 'prohibited' -or
    $manifest.dependency_policy.ad_hoc_oauth_or_crypto -ne 'prohibited') {
    Add-Failure 'P0-AUTH-DEPENDENCIES-001'
}
$cargoLockText = Get-Content -Raw -LiteralPath (Join-Path $repoRoot 'Cargo.lock')
foreach ($inactivePackage in @('httparse', 'oauth2', 'reqwest', 'tokio', 'webbrowser')) {
    if ($cargoLockText -match "(?m)^name = `"$([regex]::Escape($inactivePackage))`"$") {
        Add-Failure 'P0-AUTH-DEPENDENCIES-001'
    }
}

$adrText = Get-Content -Raw -LiteralPath (Resolve-RepositoryInput $AdrPath)
$adrNormalized = $adrText -replace '\s+', ' '
foreach ($required in @('Status:** Accepted', 'P0-WI-05', 'OWN-00, OWN-02', 'PKCE S256',
        'OL-AUTH-001–002, OL-AUTH-004–006', 'unresolved_pending_G-ID',
        'selected_not_activated', 'requests no permission',
        'completes no acceptance criterion')) {
    if ($adrNormalized -notmatch [regex]::Escape($required)) { Add-Failure 'P0-AUTH-INVENTORY-001' }
}

$threatText = Get-Content -Raw -LiteralPath (Resolve-RepositoryInput $ThreatPath)
$threatIds = @([regex]::Matches($threatText, '(?m)^\| (TM-OAUTH-\d{3}) \|') | ForEach-Object { $_.Groups[1].Value })
Test-ExactSet $threatIds (1..11 | ForEach-Object { 'TM-OAUTH-{0:D3}' -f $_ }) 'P0-AUTH-FLOW-001'

$traceText = Get-Content -Raw -LiteralPath (Resolve-RepositoryInput $TraceabilityPath)
$traceIds = @([regex]::Matches($traceText, '(?m)^\| (P0-AUTH-[A-Z-]+-001) \|') | ForEach-Object { $_.Groups[1].Value })
Test-ExactSet $traceIds $expectedTraceIds 'P0-AUTH-INVENTORY-001'

$governance = Get-Content -Raw -LiteralPath (Resolve-RepositoryInput $GovernancePath) | ConvertFrom-Json
$adr002 = @($governance.adrs | Where-Object { $_.id -eq 'ADR-002' })
if ($adr002.Count -ne 1 -or $adr002[0].status -ne 'accepted' -or
    @($governance.gates | Where-Object { $_.status -ne 'unrun' }).Count -ne 0 -or
    @($governance.capabilities | Where-Object { $_.state -ne 'disabled' -or $_.advertised -ne $false }).Count -ne 0 -or
    @($governance.product_acceptance_criteria_completed).Count -ne 0) {
    Add-Failure 'P0-AUTH-CLAIMS-001'
}

$buildManifest = Get-Content -Raw -LiteralPath (Resolve-RepositoryInput $BuildManifestPath) | ConvertFrom-Json
if ($buildManifest.runtime_boundary.oauth -ne $false -or
    @($buildManifest.runtime_boundary.network_origins).Count -ne 0 -or
    @($buildManifest.runtime_boundary.accepted_secrets).Count -ne 0) {
    Add-Failure 'P0-AUTH-CLAIMS-001'
}

if ($failures.Count -gt 0) {
    [Console]::Error.WriteLine("BLOCKED: authentication-boundary checks failed: $($failures -join ', ')")
    exit 1
}
if (-not $Quiet) {
    Write-Host 'OpenLoops authentication-boundary checks passed (10 P0-WI-05 assertions; OAuth runtime and G-ID remain unimplemented).'
}
