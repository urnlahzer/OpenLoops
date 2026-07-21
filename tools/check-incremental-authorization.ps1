[CmdletBinding()]
param(
    [string]$ManifestPath = 'contracts/identity/permission-boundary.json',
    [string]$AdrPath = 'docs/adr/ADR-003-incremental-authorization.md',
    [string]$TraceabilityPath = 'docs/prd-traceability.md',
    [string]$GovernancePath = 'contracts/governance/capabilities.json',
    [string]$AuthenticationPath = 'contracts/identity/authentication-boundary.json',
    [string]$PrivacyPath = 'contracts/privacy/persistence-boundary.json',
    [string]$SupportPath = 'contracts/support/support-matrix.json',
    [string]$BuildPath = 'contracts/build-skeleton/skeleton.json',
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) { throw 'Run this script inside the repository.' }

function Resolve-Input([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}
function Add-Failure([string]$Id) {
    if (-not $script:failureSet.ContainsKey($Id)) { $script:failureSet[$Id] = $true; $script:failures.Add($Id) }
}
function Test-ExactSet([object[]]$Actual, [object[]]$Expected, [string]$Id) {
    $a = @($Actual | ForEach-Object { [string]$_ }); $e = @($Expected | ForEach-Object { [string]$_ })
    $au = @($a | Sort-Object -Unique); $eu = @($e | Sort-Object -Unique)
    if ($a.Count -ne $au.Count -or $au.Count -ne $eu.Count -or
        (Compare-Object -ReferenceObject $eu -DifferenceObject $au)) { Add-Failure $Id }
}
function Test-Empty([object[]]$Values, [string]$Id) { if (@($Values).Count -ne 0) { Add-Failure $Id } }

$script:failures = [Collections.Generic.List[string]]::new(); $script:failureSet = @{}
$expectedHash = 'bbf3a5009e56e4976a616006945597dbaea22251d1c04ee2003b7e55e4c4d5e5'
$expectedIds = @('PERM-SIGNIN-001','PERM-MAIL-DIAG-001','PERM-MAIL-DETECT-001','PERM-MAIL-MUTATION-DIAG-001','PERM-TODO-DIAG-001','PERM-TODO-WRITE-001','PERM-CALENDAR-001','PERM-SELFMAIL-001','PERM-ADDIN-001','PERM-ADDIN-REQ-001','PERM-LOCAL-001')
$expectedTrace = @('P0-AUTHZ-INVENTORY-001','P0-AUTHZ-FEATURE-SCOPE-001','P0-AUTHZ-INCREMENTAL-001','P0-AUTHZ-FOLDERS-001','P0-AUTHZ-OFFICE-MANIFEST-001','P0-AUTHZ-CROSS-CONTRACT-001','P0-AUTHZ-CLAIMS-001','P0-AUTHZ-FRESH-CHECKER-001')
$expectedRequirements = @('OL-GOV-001','OL-AUTH-003','OL-SYNC-005','OL-REM-001','OL-REM-002','OL-SUM-002')
$expectedPermissions = [ordered]@{
    'PERM-SIGNIN-001' = 'unresolved_sign_in_refresh_scopes:G-ID'
    'PERM-MAIL-DIAG-001' = 'Mail.ReadBasic'
    'PERM-MAIL-DETECT-001' = 'Mail.Read'
    'PERM-MAIL-MUTATION-DIAG-001' = 'Mail.ReadWrite'
    'PERM-TODO-DIAG-001' = 'Tasks.Read'
    'PERM-TODO-WRITE-001' = 'Tasks.ReadWrite'
    'PERM-CALENDAR-001' = 'unresolved_least_delegated_calendar_scope:G-CAL'
    'PERM-SELFMAIL-001' = 'Mail.Send'
    'PERM-ADDIN-001' = 'unresolved_office_manifest_permission:G-ADDIN'
    'PERM-ADDIN-REQ-001' = 'unresolved_office_requirement_set:G-ADDIN'
    'PERM-LOCAL-001' = 'none'
}
$expectedGates = [ordered]@{
    'PERM-SIGNIN-001' = @('G-ID','G-PRIV')
    'PERM-MAIL-DIAG-001' = @('G-ID','G-MAIL','G-PRIV')
    'PERM-MAIL-DETECT-001' = @('G-ID','G-MAIL','G-PRIV')
    'PERM-MAIL-MUTATION-DIAG-001' = @('G-ID','G-MAIL','G-PRIV')
    'PERM-TODO-DIAG-001' = @('G-ID','G-TODO','G-PRIV')
    'PERM-TODO-WRITE-001' = @('G-ID','G-TODO','G-PRIV')
    'PERM-CALENDAR-001' = @('G-ID','G-CAL','G-PRIV')
    'PERM-SELFMAIL-001' = @('G-ID','G-SELFMAIL','G-PRIV')
    'PERM-ADDIN-001' = @('G-ADDIN','G-PRIV')
    'PERM-ADDIN-REQ-001' = @('G-ADDIN','G-RELEASE')
    'PERM-LOCAL-001' = @('G-STATE','G-PRIV')
}

try {
    $manifest = Get-Content -Raw -LiteralPath (Resolve-Input $ManifestPath) | ConvertFrom-Json
    $canonical = $manifest | ConvertTo-Json -Depth 100 -Compress
    $hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
    if ($hash -ne $expectedHash) { Add-Failure 'P0-AUTHZ-INVENTORY-001' }
} catch { [Console]::Error.WriteLine('BLOCKED: P0-AUTHZ-INVENTORY-001 manifest parse failed.'); exit 1 }

if ($manifest.schema_version -ne 1 -or $manifest.work_item -ne 'P0-WI-06' -or $manifest.adr -ne 'ADR-003' -or
    $manifest.decision_status -ne 'accepted_contract_permissions_unrequested') { Add-Failure 'P0-AUTHZ-INVENTORY-001' }
Test-ExactSet @($manifest.owner_decisions) @('OWN-02') 'P0-AUTHZ-INVENTORY-001'
if (@($manifest.referenced_owner_decisions).Count -ne 3) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }
$expectedOwnerRefs = [ordered]@{
    'OWN-04' = @{ feature = 'calendar_reminders'; adrs = @('ADR-009') }
    'OWN-05' = @{ feature = 'calendar_invitation_loops'; adrs = @('ADR-004','ADR-006','ADR-009') }
    'OWN-10' = @{ feature = 'self_email_summary'; adrs = @('ADR-013') }
}
foreach ($ownerId in $expectedOwnerRefs.Keys) {
    $ownerRef = @($manifest.referenced_owner_decisions | Where-Object id -eq $ownerId)
    if ($ownerRef.Count -ne 1) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001'; continue }
    Test-ExactSet @($ownerRef[0].features) @($expectedOwnerRefs[$ownerId].feature) 'P0-AUTHZ-CROSS-CONTRACT-001'
    Test-ExactSet @($ownerRef[0].governing_adrs) @($expectedOwnerRefs[$ownerId].adrs) 'P0-AUTHZ-CROSS-CONTRACT-001'
}
Test-ExactSet @($manifest.requirements) $expectedRequirements 'P0-AUTHZ-INVENTORY-001'
$rows = @($manifest.permission_rows)
Test-ExactSet @($rows | ForEach-Object { $_.id }) $expectedIds 'P0-AUTHZ-INVENTORY-001'
foreach ($row in $rows) {
    if (-not $expectedPermissions.Contains([string]$row.id) -or [string]$row.permission -ne [string]$expectedPermissions[[string]$row.id] -or
        $row.state -ne 'disabled' -or $row.advertised -ne $false -or
        [string]::IsNullOrWhiteSpace([string]$row.activation) -or [string]::IsNullOrWhiteSpace([string]$row.denial_fallback) -or
        [string]::IsNullOrWhiteSpace([string]$row.revocation_fallback)) { Add-Failure 'P0-AUTHZ-FEATURE-SCOPE-001' }
    if ($expectedGates.Contains([string]$row.id)) { Test-ExactSet @($row.gates) @($expectedGates[[string]$row.id]) 'P0-AUTHZ-FEATURE-SCOPE-001' }
}

$mailDetect = $rows | Where-Object id -eq 'PERM-MAIL-DETECT-001' | Select-Object -First 1
$mailDiag = $rows | Where-Object id -eq 'PERM-MAIL-DIAG-001' | Select-Object -First 1
$mailMutationDiag = $rows | Where-Object id -eq 'PERM-MAIL-MUTATION-DIAG-001' | Select-Object -First 1
$todoWrite = $rows | Where-Object id -eq 'PERM-TODO-WRITE-001' | Select-Object -First 1
$todoDiag = $rows | Where-Object id -eq 'PERM-TODO-DIAG-001' | Select-Object -First 1
if ($null -eq $mailDetect -or $null -eq $mailDiag -or $null -eq $mailMutationDiag -or $null -eq $todoWrite -or $null -eq $todoDiag -or
    $mailDetect.permission -ne 'Mail.Read' -or $mailDiag.kind -ne 'graph_delegated_validation_only' -or
    $mailMutationDiag.kind -ne 'graph_delegated_validation_only' -or $mailMutationDiag.activation -ne 'disposable_tenant_contract_test_only' -or
    $mailMutationDiag.use -ne 'synthetic mutation-without-Mail.Send contract testing only; never product mail mutation' -or
    $todoWrite.permission -ne 'Tasks.ReadWrite' -or $todoDiag.kind -ne 'graph_delegated_validation_only') {
    Add-Failure 'P0-AUTHZ-FEATURE-SCOPE-001'
}
if ($manifest.incremental_consent_policy.initial_combined_grant -ne 'prohibited' -or
    $manifest.incremental_consent_policy.request_trigger -ne 'explicit user action for exactly one feature delta' -or
    $manifest.incremental_consent_policy.returned_scope_superset -ne 'never activates an unrequested feature' -or
    $manifest.incremental_consent_policy.missing_scope -ne 'reject dependent use before any API call' -or
    $manifest.incremental_consent_policy.denial -ne 'disable only dependent feature; no broader request or weaker authentication flow') {
    Add-Failure 'P0-AUTHZ-INCREMENTAL-001'
}
if ($manifest.mail_folder_policy.authorization_disclosure -ne 'Mail.Read is mailbox-wide delegated authorization; configured folders are application processing policy, not an OAuth restriction' -or
    $manifest.mail_folder_policy.account -ne 'explicitly bound supported primary mailbox only' -or
    $manifest.mail_folder_policy.additional_folders -ne 'explicit per-folder opt-in of owned folders only after G-MAIL' -or
    $manifest.mail_folder_policy.automatic_expansion -ne 'prohibited' -or
    $manifest.mail_folder_policy.shared_or_delegated_folders -ne 'prohibited' -or
    $manifest.mail_folder_policy.history_window_days.default -ne 30 -or
    $manifest.mail_folder_policy.history_window_days.minimum -ne 1 -or
    $manifest.mail_folder_policy.history_window_days.maximum -ne 365 -or
    $manifest.mail_folder_policy.folder_removal -ne 'stop new processing; delete the application-owned folder checkpoint under ADR-005 and ADR-PRIV-001; preserve only separately governed active-loop lineage; disclose coverage loss; no consent-revocation claim') { Add-Failure 'P0-AUTHZ-FOLDERS-001' }
Test-ExactSet @($manifest.mail_folder_policy.default_folders) @('Inbox','Sent Items') 'P0-AUTHZ-FOLDERS-001'

if ($manifest.addin_manifest_boundary.production_permission_status -ne 'unresolved_pending_G-ADDIN' -or
    $manifest.addin_manifest_boundary.validation_permission_target -ne 'ReadItem' -or
    $manifest.addin_manifest_boundary.production_requirement_set_status -ne 'unresolved_pending_G-ADDIN' -or
    $manifest.addin_manifest_boundary.validation_requirement_set -ne 'Mailbox' -or
    $manifest.addin_manifest_boundary.validation_requirement_version -ne '1.13' -or
    $manifest.addin_manifest_boundary.support_claim -ne $false -or
    $manifest.addin_manifest_boundary.manifest_activation -ne 'disabled' -or
    $manifest.addin_manifest_boundary.graph_scope_equivalence -ne $false) { Add-Failure 'P0-AUTHZ-OFFICE-MANIFEST-001' }
Test-ExactSet @($manifest.addin_manifest_boundary.prohibited_authority) @('ReadWriteMailbox','Mailbox.SharedFolder','mailbox_token_ownership','synchronization_authority','event_based_loop_creation') 'P0-AUTHZ-OFFICE-MANIFEST-001'

foreach ($name in @('permissions_requested','network_contacts','gates_passed','acceptance_criteria_completed','capabilities_enabled','capabilities_advertised','supported_rows')) { Test-Empty @($manifest.claims.$name) 'P0-AUTHZ-CLAIMS-001' }

$governance = Get-Content -Raw -LiteralPath (Resolve-Input $GovernancePath) | ConvertFrom-Json
$adr003 = @($governance.adrs | Where-Object id -eq 'ADR-003')
if ($adr003.Count -ne 1 -or $adr003[0].status -ne 'accepted' -or
    @($governance.gates | Where-Object status -ne 'unrun').Count -ne 0 -or
    @($governance.capabilities | Where-Object { $_.state -ne 'disabled' -or $_.advertised -ne $false }).Count -ne 0 -or
    @($governance.product_acceptance_criteria_completed).Count -ne 0) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001'; Add-Failure 'P0-AUTHZ-CLAIMS-001' }
foreach ($ownerId in $expectedOwnerRefs.Keys) {
    $governanceOwner = @($governance.owner_decisions | Where-Object id -eq $ownerId)
    if ($governanceOwner.Count -ne 1 -or $governanceOwner[0].status -notlike 'accepted*') { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001'; continue }
    Test-ExactSet @($governanceOwner[0].adrs) @($expectedOwnerRefs[$ownerId].adrs) 'P0-AUTHZ-CROSS-CONTRACT-001'
}
$expectedCapabilities = [ordered]@{
    'calendar_reminders' = @{ owner = 'OWN-04'; gates = @('G-CAL','G-PRIV'); permissions = @('unresolved_least_delegated_calendar_scope:G-CAL') }
    'calendar_invitation_loops' = @{ owner = 'OWN-05'; gates = @('G-CAL','G-MAIL','G-PRIV'); permissions = @('Mail.Read','unresolved_least_delegated_calendar_scope:G-CAL') }
    'self_email_summary' = @{ owner = 'OWN-10'; gates = @('G-SELFMAIL','G-PRIV'); permissions = @('Mail.Send') }
}
foreach ($capabilityId in $expectedCapabilities.Keys) {
    $capability = @($governance.capabilities | Where-Object id -eq $capabilityId)
    if ($capability.Count -ne 1 -or $capability[0].state -ne 'disabled' -or $capability[0].advertised -ne $false) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001'; continue }
    Test-ExactSet @($capability[0].owner_decisions) @($expectedCapabilities[$capabilityId].owner) 'P0-AUTHZ-CROSS-CONTRACT-001'
    Test-ExactSet @($capability[0].gates) @($expectedCapabilities[$capabilityId].gates) 'P0-AUTHZ-CROSS-CONTRACT-001'
    Test-ExactSet @($capability[0].permission_contracts) @($expectedCapabilities[$capabilityId].permissions) 'P0-AUTHZ-CROSS-CONTRACT-001'
}

$authn = Get-Content -Raw -LiteralPath (Resolve-Input $AuthenticationPath) | ConvertFrom-Json
if ($authn.separate_decisions.feature_scopes_and_office_permissions -ne 'ADR-003' -or
    @($authn.claims.permissions_requested).Count -ne 0 -or $authn.flow.runtime_status -ne 'unimplemented') { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }
$support = Get-Content -Raw -LiteralPath (Resolve-Input $SupportPath) | ConvertFrom-Json
if ($support.addin_contract.manifest_minimum.requirement_set -ne 'Mailbox' -or
    $support.addin_contract.manifest_minimum.version -ne '1.13' -or
    $support.addin_contract.manifest_minimum.permission -ne 'ReadItem' -or
    @($support.claim_state.supported_rows).Count -ne 0) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }
$build = Get-Content -Raw -LiteralPath (Resolve-Input $BuildPath) | ConvertFrom-Json
if ($build.runtime_boundary.oauth -ne $false -or @($build.runtime_boundary.network_origins).Count -ne 0 -or @($build.runtime_boundary.accepted_secrets).Count -ne 0) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001'; Add-Failure 'P0-AUTHZ-CLAIMS-001' }
$privacy = Get-Content -Raw -LiteralPath (Resolve-Input $PrivacyPath) | ConvertFrom-Json
$accountBinding = @($privacy.record_types | Where-Object id -eq 'account_binding')
$checkpoint = @($privacy.record_types | Where-Object id -eq 'sync_checkpoint')
if ($accountBinding.Count -ne 1 -or $checkpoint.Count -ne 1) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }
else {
    $accountFields = @($accountBinding[0].field_groups | ForEach-Object { @($_.fields) })
    foreach ($field in @('requested_scope_codes','granted_scope_codes')) { if ($accountFields -notcontains $field) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' } }
    foreach ($group in @($checkpoint[0].field_groups)) {
        if ($group.retention -ne 'checkpoint_until_scope_removed' -or $group.deletion -ne 'delete_local_only' -or $group.reset -ne 'scope_removal_or_disconnect') { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }
    }
}
$cargoFiles = @(Get-ChildItem -LiteralPath $repoRoot -Filter 'Cargo.toml' -Recurse -File | Where-Object { $_.FullName -notmatch '[\\/]target[\\/]' })
$sourceFiles = @(Get-ChildItem -LiteralPath (Join-Path $repoRoot 'crates') -Recurse -File -Include '*.rs') + @(Get-ChildItem -LiteralPath (Join-Path $repoRoot 'outlook-addin') -Recurse -File -Include '*.ts','*.tsx','*.js' | Where-Object { $_.FullName -notmatch '[\\/]\.generated[\\/]' })
if (Select-String -LiteralPath @($cargoFiles.FullName) -Pattern '(?i)\b(reqwest|oauth2|hyper|ureq|tokio)\b' -Quiet -ErrorAction SilentlyContinue) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }
if ($sourceFiles.Count -gt 0 -and (Select-String -LiteralPath @($sourceFiles.FullName) -Pattern '(?i)\b(TcpStream|HttpClient|XMLHttpRequest)\b|\bfetch\s*\(' -Quiet -ErrorAction SilentlyContinue)) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }

$trace = Get-Content -Raw -LiteralPath (Resolve-Input $TraceabilityPath)
$traceIds = @([regex]::Matches($trace, '(?m)^\| (P0-AUTHZ-[A-Z-]+-001) \|') | ForEach-Object { $_.Groups[1].Value })
Test-ExactSet $traceIds $expectedTrace 'P0-AUTHZ-INVENTORY-001'
$adr = (Get-Content -Raw -LiteralPath (Resolve-Input $AdrPath)) -replace '\s+',' '
foreach ($phrase in @('Status:** Accepted','P0-WI-06','Owner decisions:** OWN-02','requests no permission','`Mail.ReadWrite` is permitted only for the disposable-tenant mutation-without-`Mail.Send` experiment named by the validation plan; it is never a product permission or capability','product mail-mutation','No gate, capability, support row, or acceptance criterion advances','exactly these eight checks','P0-AUTHZ-INVENTORY-001','P0-AUTHZ-FEATURE-SCOPE-001','P0-AUTHZ-INCREMENTAL-001','P0-AUTHZ-FOLDERS-001','P0-AUTHZ-OFFICE-MANIFEST-001','P0-AUTHZ-CROSS-CONTRACT-001','P0-AUTHZ-CLAIMS-001','P0-AUTHZ-FRESH-CHECKER-001')) {
    if ($adr -notmatch [regex]::Escape($phrase)) { Add-Failure 'P0-AUTHZ-CROSS-CONTRACT-001' }
}

$addinManifests = @(Get-ChildItem -LiteralPath (Join-Path $repoRoot 'outlook-addin') -Recurse -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -match '(?i)^manifest.*\.(?:xml|json)$' })
if ($addinManifests.Count -ne 0) { Add-Failure 'P0-AUTHZ-OFFICE-MANIFEST-001'; Add-Failure 'P0-AUTHZ-CLAIMS-001' }

if ($failures.Count -gt 0) { [Console]::Error.WriteLine("BLOCKED: incremental-authorization checks failed: $($failures -join ', ')"); exit 1 }
if (-not $Quiet) { Write-Host 'OpenLoops incremental-authorization checks passed (8 P0-WI-06 assertions; zero permission or gate advanced).' }
