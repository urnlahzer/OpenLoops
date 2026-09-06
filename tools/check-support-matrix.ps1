[CmdletBinding()]
param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot '..\contracts\support\support-matrix.json'),
    [switch]$SemanticTestMode
)

$ErrorActionPreference = 'Stop'
$failures = [System.Collections.Generic.HashSet[string]]::new()
function Fail([string]$Id) { [void]$failures.Add($Id) }
function Exact([object[]]$Actual, [object[]]$Expected, [string]$Id) {
    $a = @($Actual | ForEach-Object { [string]$_ })
    $e = @($Expected | ForEach-Object { [string]$_ })
    if ($a.Count -eq 0 -and $e.Count -eq 0) { return }
    if ($a.Count -eq 0 -or $e.Count -eq 0 -or
        $a.Count -ne (@($a | Sort-Object -Unique)).Count -or
        $e.Count -ne (@($e | Sort-Object -Unique)).Count -or
        (Compare-Object ($a | Sort-Object) ($e | Sort-Object))) { Fail $Id }
}
function Test-JsonObject([System.Text.Json.JsonElement]$Element) {
    if ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Object) {
        $names = @($Element.EnumerateObject() | ForEach-Object Name)
        if ($names.Count -ne (@($names | Sort-Object -Unique)).Count) { Fail 'P0-SUPPORT-INVENTORY-001' }
        foreach ($property in $Element.EnumerateObject()) { Test-JsonObject $property.Value }
    } elseif ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Array) {
        foreach ($item in $Element.EnumerateArray()) { Test-JsonObject $item }
    }
}

$raw = Get-Content -LiteralPath $ManifestPath -Raw
try {
    $document = [System.Text.Json.JsonDocument]::Parse($raw)
    Test-JsonObject $document.RootElement
    $manifest = $raw | ConvertFrom-Json
} catch {
    [Console]::Error.WriteLine('BLOCKED: support matrix is not strict JSON.')
    exit 1
} finally {
    if ($document) { $document.Dispose() }
}

$canonical = $manifest | ConvertTo-Json -Depth 100 -Compress
$fingerprint = [Convert]::ToHexString(
    [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))
).ToLowerInvariant()
if (-not $SemanticTestMode -and $fingerprint -ne 'a87465f7972f30a98d05cbe51ad9e04589728082a3f5bd684d4f6f94e65161c4') {
    Fail 'P0-SUPPORT-INVENTORY-001'
}

$topKeys = 'schema_version','work_item','matrix_disposition','owner_decisions','requirements','acceptance_criteria','blocking_gates','claim_state','freshness_policy','matrices','addin_contract','cross_matrix_invariants','sources'
Exact @($manifest.PSObject.Properties.Name) $topKeys 'P0-SUPPORT-INVENTORY-001'
if ($manifest.schema_version -ne 1 -or $manifest.work_item -ne 'P0-WI-03') { Fail 'P0-SUPPORT-CLAIMS-001' }
Exact @($manifest.matrix_disposition.PSObject.Properties.Name) @('id','status','approved_on','meaning') 'P0-SUPPORT-INVENTORY-001'
if ($manifest.matrix_disposition.id -ne 'P0-MATRIX-001' -or $manifest.matrix_disposition.status -ne 'accepted' -or
    $manifest.matrix_disposition.approved_on -ne '2026-07-19' -or $manifest.matrix_disposition.meaning -notmatch 'no support, capability, or release claim') { Fail 'P0-SUPPORT-CLAIMS-001' }
Exact @($manifest.owner_decisions) @('OWN-00','OWN-01','OWN-02','OWN-08') 'P0-SUPPORT-CLAIMS-001'
Exact @($manifest.acceptance_criteria) @() 'P0-SUPPORT-CLAIMS-001'
Exact @($manifest.claim_state.PSObject.Properties.Name) @('supported_rows','enabled_capabilities','advertised_capabilities','gates_passed','acceptance_criteria_completed') 'P0-SUPPORT-INVENTORY-001'
foreach ($property in $manifest.claim_state.PSObject.Properties) {
    Exact @($property.Value) @() 'P0-SUPPORT-CLAIMS-001'
}

$knownGates = 'G-ID','G-MAIL','G-TODO','G-CAL','G-ADDIN','G-STATE','G-MODEL','G-AUTO','G-AUTO-FULL','G-SELFMAIL','G-PRIV','G-SEC-AUDIT','G-RELEASE'
Exact @($manifest.blocking_gates) @('G-ID','G-MAIL','G-TODO','G-CAL','G-ADDIN','G-STATE','G-MODEL','G-PRIV','G-SEC-AUDIT','G-RELEASE') 'P0-SUPPORT-CLAIMS-001'
Exact @($manifest.matrices.PSObject.Properties.Name) @('windows','outlook','accounts','clouds','model_providers') 'P0-SUPPORT-INVENTORY-001'

$expectedRows = @{
    windows = @('windows_11_24h2_x64','windows_11_25h2_x64','windows_11_26h1_or_arm64','windows_10_or_x86','macos_or_linux','remote_container_nas_headless')
    outlook = @('classic_outlook_windows_m365','new_outlook_windows','outlook_web_same_windows','classic_outlook_perpetual','outlook_mac_mobile','outlook_onprem_hybrid_or_sovereign')
    accounts = @('commercial_global_work_school_primary','personal_microsoft_account','shared_or_delegated_mailbox','multiple_accounts','guest_without_supported_mailbox','non_microsoft_mailbox')
    clouds = @('commercial_global','government_clouds','china_21vianet')
    model_providers = @('disabled','ollama_local','ollama_cloud','approved_https')
}
$rowKeys = @('id','disposition','current_state','advertised','boundary','gates','required_evidence','fallback','sources')
foreach ($matrixName in $expectedRows.Keys) {
    $rows = @($manifest.matrices.$matrixName)
    Exact @($rows.id) $expectedRows[$matrixName] 'P0-SUPPORT-INVENTORY-001'
    foreach ($row in $rows) {
        Exact @($row.PSObject.Properties.Name) $rowKeys 'P0-SUPPORT-INVENTORY-001'
        if ($row.advertised -ne $false) { Fail 'P0-SUPPORT-CLAIMS-001' }
        if ([string]::IsNullOrWhiteSpace($row.disposition) -or [string]::IsNullOrWhiteSpace($row.current_state) -or
            [string]::IsNullOrWhiteSpace($row.fallback) -or @($row.required_evidence).Count -eq 0 -or @($row.sources).Count -eq 0) { Fail 'P0-SUPPORT-INVENTORY-001' }
        foreach ($gate in $row.gates) { if ($knownGates -notcontains $gate) { Fail 'P0-SUPPORT-CLAIMS-001' } }
    }
}

$windows = $manifest.matrices.windows
$approvedWindows = @($windows | Where-Object disposition -eq 'approved_validation_target')
Exact @($approvedWindows.id) @('windows_11_24h2_x64','windows_11_25h2_x64') 'P0-SUPPORT-WINDOWS-001'
foreach ($row in $approvedWindows) {
    $expectedVersion = if ($row.id -eq 'windows_11_24h2_x64') { '24H2' } else { '25H2' }
    if ($row.current_state -ne 'unverified_disabled' -or $row.boundary.os -ne 'Windows 11' -or
        $row.boundary.version -ne $expectedVersion -or $row.boundary.architecture -ne 'x64' -or $row.boundary.servicing_rule -notmatch 'Microsoft servicing' -or
        $row.boundary.servicing_rule -notmatch 'release tests') { Fail 'P0-SUPPORT-WINDOWS-001' }
}
if (@($windows | Where-Object { $_.id -notin $approvedWindows.id -and $_.current_state -ne 'unavailable' }).Count -ne 0) { Fail 'P0-SUPPORT-WINDOWS-001' }

$outlook = $manifest.matrices.outlook
$approvedOutlook = @($outlook | Where-Object disposition -eq 'approved_validation_target')
Exact @($approvedOutlook.id) @('classic_outlook_windows_m365','new_outlook_windows','outlook_web_same_windows') 'P0-SUPPORT-OUTLOOK-001'
foreach ($row in $approvedOutlook) {
    if ($row.current_state -ne 'disabled_pending_G-ADDIN' -or @($row.gates) -notcontains 'G-ADDIN' -or @($row.gates) -notcontains 'G-PRIV') { Fail 'P0-SUPPORT-OUTLOOK-001' }
}
if ($outlook[1].boundary.activation -ne 'selected-item activation; no always-available no-item dashboard claim' -or
    $outlook[2].boundary.activation -notmatch 'selected-item activation' -or $outlook[2].boundary.activation -notmatch 'no remote-companion claim') { Fail 'P0-SUPPORT-OUTLOOK-001' }
Exact @($outlook[2].boundary.browsers) @('current Microsoft Edge','current Google Chrome','current Mozilla Firefox') 'P0-SUPPORT-OUTLOOK-001'

$addin = $manifest.addin_contract
Exact @($addin.PSObject.Properties.Name) @('manifest_minimum','prohibited_permissions_or_authority','client_behavior','gate','fallback') 'P0-SUPPORT-ADDIN-001'
Exact @($addin.manifest_minimum.PSObject.Properties.Name) @('requirement_set','version','permission','form_factor') 'P0-SUPPORT-ADDIN-001'
if ($addin.manifest_minimum.requirement_set -ne 'Mailbox' -or $addin.manifest_minimum.version -ne '1.13' -or
    $addin.manifest_minimum.permission -ne 'ReadItem' -or $addin.manifest_minimum.form_factor -ne 'desktop' -or
    $addin.gate -ne 'G-ADDIN') { Fail 'P0-SUPPORT-ADDIN-001' }
Exact @($addin.prohibited_permissions_or_authority) @('ReadWriteMailbox','Mailbox.SharedFolder','mailbox_token_ownership','synchronization_authority','event_based_loop_creation') 'P0-SUPPORT-ADDIN-001'
if ($addin.client_behavior.new_outlook_windows -notmatch 'selected-item only' -or $addin.client_behavior.outlook_web_same_windows -notmatch 'selected-item only') { Fail 'P0-SUPPORT-ADDIN-001' }

$accounts = $manifest.matrices.accounts
$coreAccount = @($accounts | Where-Object disposition -eq 'approved_validation_target')
Exact @($coreAccount.id) @('commercial_global_work_school_primary') 'P0-SUPPORT-ACCOUNT-001'
if ($coreAccount[0].current_state -ne 'disabled_pending_feature_gates' -or $coreAccount[0].boundary.account_count -ne 1 -or $coreAccount[0].boundary.os_user_count -ne 1) { Fail 'P0-SUPPORT-ACCOUNT-001' }
$personal = $accounts | Where-Object id -eq 'personal_microsoft_account'
Exact @($personal.gates) @('G-ID','G-MAIL','G-TODO','G-CAL','G-ADDIN','G-PRIV') 'P0-SUPPORT-ACCOUNT-001'
if ($personal.current_state -ne 'disabled' -or $personal.required_evidence -notcontains 'complete_identity_mail_todo_calendar_addin_matrix_not_signin_only') { Fail 'P0-SUPPORT-ACCOUNT-001' }
if (@($accounts | Where-Object { $_.id -notin @('commercial_global_work_school_primary','personal_microsoft_account') -and $_.current_state -ne 'unavailable' }).Count -ne 0) { Fail 'P0-SUPPORT-ACCOUNT-001' }

$clouds = $manifest.matrices.clouds
$coreCloud = @($clouds | Where-Object disposition -eq 'approved_validation_target')
Exact @($coreCloud.id) @('commercial_global') 'P0-SUPPORT-CLOUD-001'
if ($coreCloud[0].boundary.authority_origin -ne 'https://login.microsoftonline.com' -or $coreCloud[0].boundary.graph_origin -ne 'https://graph.microsoft.com') { Fail 'P0-SUPPORT-CLOUD-001' }
if (@($clouds | Where-Object { $_.id -ne 'commercial_global' -and $_.current_state -ne 'unavailable' }).Count -ne 0) { Fail 'P0-SUPPORT-CLOUD-001' }

$providers = $manifest.matrices.model_providers
$providerDefault = $providers | Where-Object id -eq 'disabled'
if ($providerDefault.disposition -ne 'safe_default' -or $providerDefault.current_state -ne 'no_provider_requests' -or @($providerDefault.gates).Count -ne 0) { Fail 'P0-SUPPORT-MODEL-001' }
foreach ($id in 'ollama_local','ollama_cloud','approved_https') {
    $provider = $providers | Where-Object id -eq $id
    if ($provider.current_state -ne 'disabled' -or @($provider.gates) -notcontains 'G-MODEL' -or @($provider.gates) -notcontains 'G-PRIV') { Fail 'P0-SUPPORT-MODEL-001' }
}
$local = $providers | Where-Object id -eq 'ollama_local'
if ($local.boundary.locality -notmatch 'loopback alone is not proof' -or $local.required_evidence -notcontains 'local_only_proof' -or $local.fallback -notmatch 'never fall through to cloud') { Fail 'P0-SUPPORT-MODEL-001' }
$cloud = $providers | Where-Object id -eq 'ollama_cloud'
if ($cloud.boundary.api_base -ne 'https://ollama.com/api' -or $cloud.boundary.editable -ne $false -or
    $cloud.boundary.structured_output -notmatch 'no server-side schema claim') { Fail 'P0-SUPPORT-MODEL-001' }
$approvedHttps = $providers | Where-Object id -eq 'approved_https'
if ($approvedHttps.boundary.origin -ne 'one user-approved canonical absolute public HTTPS origin plus adapter-fixed path' -or
    $approvedHttps.boundary.transport -notmatch 'private' -or $approvedHttps.boundary.transport -notmatch 'metadata targets prohibited') { Fail 'P0-SUPPORT-MODEL-001' }

Exact @($manifest.freshness_policy.PSObject.Properties.Name) @('official_source_snapshot','release_recheck_required','recheck_subjects','stale_or_changed_result') 'P0-SUPPORT-FRESHNESS-001'
if ($manifest.freshness_policy.official_source_snapshot -ne '2026-07-19' -or $manifest.freshness_policy.release_recheck_required -ne $true -or
    @($manifest.freshness_policy.recheck_subjects).Count -ne 9 -or $manifest.freshness_policy.stale_or_changed_result -notmatch 'remains disabled and unadvertised') { Fail 'P0-SUPPORT-FRESHNESS-001' }

$expectedSources = @('SRC-SPEC-OWN-01','SRC-SPEC-OWN-02','SRC-SPEC-OWN-08','SRC-SPEC-G-ADDIN','SRC-SPEC-MODEL','SRC-RESEARCH-CONNECTION','SRC-RESEARCH-SECURITY','SRC-RESEARCH-VALIDATION','SRC-WIN-RELEASE','SRC-WIN-10-EOS','SRC-WIN-ARM','SRC-OUTLOOK-OVERVIEW','SRC-OUTLOOK-NEW','SRC-OUTLOOK-SETS','SRC-OUTLOOK-NOITEM','SRC-OUTLOOK-BROWSERS','SRC-IDENTITY-ACCOUNTS','SRC-GRAPH-CLOUDS','SRC-OLLAMA-API','SRC-OLLAMA-AUTH','SRC-OLLAMA-CLOUD','SRC-OLLAMA-FAQ','SRC-OLLAMA-STRUCTURED','SRC-OLLAMA-TAGS')
Exact @($manifest.sources.id) $expectedSources 'P0-SUPPORT-FRESHNESS-001'
foreach ($source in $manifest.sources) {
    Exact @($source.PSObject.Properties.Name) @('id','kind','location') 'P0-SUPPORT-INVENTORY-001'
    if ($source.kind -eq 'official_current' -and $source.location -notmatch '^https://') { Fail 'P0-SUPPORT-FRESHNESS-001' }
    if ($source.kind -match '_local$') {
        $relative = ($source.location -split '#')[0]
        if (-not (Test-Path (Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..')).Path $relative))) { Fail 'P0-SUPPORT-FRESHNESS-001' }
    }
}
foreach ($matrix in $manifest.matrices.PSObject.Properties.Value) {
    foreach ($row in $matrix) {
        foreach ($sourceId in $row.sources) { if ($expectedSources -notcontains $sourceId) { Fail 'P0-SUPPORT-FRESHNESS-001' } }
    }
}

Exact @($manifest.cross_matrix_invariants.id) (1..13 | ForEach-Object { 'SUP-X-{0:d3}' -f $_ }) 'P0-SUPPORT-INVENTORY-001'
foreach ($invariant in $manifest.cross_matrix_invariants) { Exact @($invariant.PSObject.Properties.Name) @('id','rule') 'P0-SUPPORT-INVENTORY-001' }

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$governance = Get-Content (Join-Path $repoRoot 'contracts\governance\capabilities.json') -Raw | ConvertFrom-Json
if (@($governance.product_acceptance_criteria_completed).Count -ne 0 -or
    @($governance.gates | Where-Object status -ne 'unrun').Count -ne 0 -or
    @($governance.capabilities | Where-Object { $_.state -ne 'disabled' -or $_.advertised }).Count -ne 0) { Fail 'P0-SUPPORT-CLAIMS-001' }
$requiredCapabilities = @('work_school_core','personal_accounts','outlook_addin','external_model_providers')
foreach ($id in $requiredCapabilities) { if (@($governance.capabilities | Where-Object id -eq $id).Count -ne 1) { Fail 'P0-SUPPORT-CLAIMS-001' } }

foreach ($relative in 'docs\support-matrix.md','docs\product-spec.md','docs\implementation-plan.md','docs\prd-traceability.md','research\microsoft-graph\product-decisions.md') {
    if (-not (Test-Path (Join-Path $repoRoot $relative))) { Fail 'P0-SUPPORT-CLAIMS-001' }
}
$docText = (Get-Content (Join-Path $repoRoot 'docs\support-matrix.md') -Raw) + (Get-Content (Join-Path $repoRoot 'docs\product-spec.md') -Raw) + (Get-Content (Join-Path $repoRoot 'research\microsoft-graph\product-decisions.md') -Raw)
foreach ($required in 'P0-MATRIX-001','Windows 11 x64 24H2/25H2','Mailbox 1.13','ReadItem','Current support claim:** none','loopback does not prove') {
    if ($docText -notmatch [regex]::Escape($required)) { Fail 'P0-SUPPORT-CLAIMS-001' }
}
$trace = Get-Content (Join-Path $repoRoot 'docs\prd-traceability.md') -Raw
foreach ($id in 'P0-SUPPORT-INVENTORY-001','P0-SUPPORT-WINDOWS-001','P0-SUPPORT-OUTLOOK-001','P0-SUPPORT-ADDIN-001','P0-SUPPORT-ACCOUNT-001','P0-SUPPORT-CLOUD-001','P0-SUPPORT-MODEL-001','P0-SUPPORT-FRESHNESS-001','P0-SUPPORT-CLAIMS-001','P0-SUPPORT-FRESH-CHECKER-001') {
    if (([regex]::Matches($trace, [regex]::Escape($id))).Count -ne 1) { Fail 'P0-SUPPORT-CLAIMS-001' }
}

if ($failures.Count) {
    [Console]::Error.WriteLine(('BLOCKED support-matrix checks: ' + (($failures | Sort-Object) -join ', ')))
    exit 1
}
Write-Host 'OpenLoops support-matrix checks passed (10 P0-WI-03 assertions; validation targets only).'
