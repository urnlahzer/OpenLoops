[CmdletBinding()]
param(
    [string]$ManifestPath = 'contracts/model/provider-boundary.json',
    [string]$SchemaPath = 'contracts/model/analysis-output.schema.json',
    [string]$AdrPath = 'docs/adr/ADR-007-model-boundary.md',
    [string]$ThreatPath = 'docs/threat-model/model-provider-boundary.md',
    [string]$TraceabilityPath = 'docs/prd-traceability.md',
    [string]$GovernancePath = 'contracts/governance/capabilities.json',
    [string]$SupportPath = 'contracts/support/support-matrix.json',
    [string]$BuildPath = 'contracts/build-skeleton/skeleton.json',
    [string]$PrivacyPath = 'contracts/privacy/persistence-boundary.json',
    [string]$ProtectedStatePath = 'contracts/persistence/protected-state-boundary.json',
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) {
    throw 'Run inside the repository.'
}

function Resolve-Input([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Fail([string]$Id) {
    if (-not $script:failureSet.ContainsKey($Id)) {
        $script:failureSet[$Id] = $true
        $script:failures.Add($Id)
    }
}

function Exact([object[]]$Actual, [object[]]$Expected, [string]$Id) {
    $a = @($Actual | ForEach-Object { [string]$_ })
    $e = @($Expected | ForEach-Object { [string]$_ })
    $au = @($a | Sort-Object -Unique)
    $eu = @($e | Sort-Object -Unique)
    if ($a.Count -ne $au.Count -or $au.Count -ne $eu.Count -or (Compare-Object $eu $au)) { Fail $Id }
}

function ExactOrdered([object[]]$Actual, [object[]]$Expected, [string]$Id) {
    $a = @($Actual | ForEach-Object { [string]$_ })
    $e = @($Expected | ForEach-Object { [string]$_ })
    if ($a.Count -ne $e.Count) { Fail $Id; return }
    for ($i = 0; $i -lt $e.Count; $i++) {
        if ($a[$i] -cne $e[$i]) { Fail $Id; return }
    }
}

function Empty([object[]]$Values, [string]$Id) {
    if (@($Values).Count -ne 0) { Fail $Id }
}

function Has([string]$Text, [string[]]$Phrases, [string]$Id) {
    foreach ($phrase in $Phrases) {
        if ($Text -notmatch [regex]::Escape($phrase)) { Fail $Id }
    }
}

function Read-Json([string]$Path, [string]$Id) {
    try { return Get-Content -Raw -LiteralPath (Resolve-Input $Path) | ConvertFrom-Json -Depth 100 }
    catch { Fail $Id; return $null }
}

function Read-Text([string]$Path, [string]$Id) {
    try { return Get-Content -Raw -LiteralPath (Resolve-Input $Path) }
    catch { Fail $Id; return '' }
}

$script:failures = [Collections.Generic.List[string]]::new()
$script:failureSet = @{}
$checks = @(
    'P0-MODEL-INVENTORY-001',
    'P0-MODEL-PROFILES-001',
    'P0-MODEL-REQUEST-001',
    'P0-MODEL-RESPONSE-001',
    'P0-MODEL-NETWORK-001',
    'P0-MODEL-CONSENT-001',
    'P0-MODEL-AUTHORITY-001',
    'P0-MODEL-PRIVACY-001',
    'P0-MODEL-SOURCES-001',
    'P0-MODEL-CROSS-CONTRACT-001',
    'P0-MODEL-CLAIMS-001',
    'P0-MODEL-FRESH-CHECKER-001'
)

$m = Read-Json $ManifestPath 'P0-MODEL-INVENTORY-001'
if ($null -eq $m) {
    [Console]::Error.WriteLine('BLOCKED: P0-MODEL-INVENTORY-001 manifest parse failed.')
    exit 1
}

$canonical = $m | ConvertTo-Json -Depth 100 -Compress
$hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
if ($hash -ne '139792d20b43595232a8532ba91b4f71f800f5b538cd8f425353eea6a4bcfcdd') { Fail 'P0-MODEL-INVENTORY-001' }

$requirements = @('OL-SYNC-012','OL-SYNC-013','OL-TEST-003','OL-MODEL-001','OL-MODEL-002','OL-MODEL-003','OL-MODEL-004','OL-MODEL-005','OL-MODEL-006','OL-MODEL-007','OL-MODEL-008','OL-MODEL-009','OL-MODEL-010','OL-MODEL-011','OL-MODEL-012','OL-MODEL-013','OL-MODEL-014','OL-MODEL-015','OL-NFR-005','OL-NFR-007','OL-NFR-011')
if ($m.schema_version -ne 1 -or $m.work_item -ne 'P0-WI-10' -or $m.adr -ne 'ADR-007' -or $m.decision_status -ne 'accepted_contract_runtime_unimplemented' -or $m.snapshot_date -ne '2026-07-20') { Fail 'P0-MODEL-INVENTORY-001' }
ExactOrdered @($m.owner_decisions) @('OWN-08') 'P0-MODEL-INVENTORY-001'
ExactOrdered @($m.requirements) $requirements 'P0-MODEL-INVENTORY-001'
ExactOrdered @($m.acceptance_scenarios) @('AS-12','AS-16','AS-22') 'P0-MODEL-INVENTORY-001'
ExactOrdered @($m.blocking_gates) @('G-MODEL','G-PRIV','G-SEC-AUDIT','G-RELEASE') 'P0-MODEL-INVENTORY-001'
$scenarios = @($m.scenario_dependencies)
ExactOrdered @($scenarios.id) @('AS-12','AS-16','AS-22') 'P0-MODEL-INVENTORY-001'
$as12 = @($scenarios | Where-Object id -eq 'AS-12'); $as16 = @($scenarios | Where-Object id -eq 'AS-16'); $as22 = @($scenarios | Where-Object id -eq 'AS-22')
if ($as12.Count -ne 1 -or $as16.Count -ne 1 -or $as22.Count -ne 1) { Fail 'P0-MODEL-INVENTORY-001' }
if ($as12.Count -eq 1) {
    if ($as12[0].status -ne 'unpassed_runtime_and_adversarial_evidence_required') { Fail 'P0-MODEL-INVENTORY-001' }
    ExactOrdered @($as12[0].requirements) @('OL-MODEL-006','OL-MODEL-011','OL-NFR-005') 'P0-MODEL-INVENTORY-001'; ExactOrdered @($as12[0].owners) @('ADR-007','ADR-PRIV-001') 'P0-MODEL-INVENTORY-001'; ExactOrdered @($as12[0].gates) @('G-MODEL','G-PRIV') 'P0-MODEL-INVENTORY-001'
}
if ($as16.Count -eq 1) {
    if ($as16[0].status -ne 'unpassed_runtime_persistence_and_provider_recovery_evidence_required') { Fail 'P0-MODEL-INVENTORY-001' }
    ExactOrdered @($as16[0].requirements) @('OL-SYNC-012','OL-SYNC-013','OL-NFR-007') 'P0-MODEL-INVENTORY-001'; ExactOrdered @($as16[0].owners) @('ADR-004','ADR-005','ADR-007') 'P0-MODEL-INVENTORY-001'; ExactOrdered @($as16[0].gates) @('G-MAIL','G-STATE','G-MODEL','G-PRIV') 'P0-MODEL-INVENTORY-001'
}
if ($as22.Count -eq 1) {
    if ($as22[0].status -ne 'unpassed_local_and_hosted_provider_contract_evidence_required') { Fail 'P0-MODEL-INVENTORY-001' }
    ExactOrdered @($as22[0].requirements) @('OL-MODEL-003','OL-MODEL-004','OL-MODEL-010','OL-MODEL-011','OL-MODEL-013','OL-MODEL-014','OL-MODEL-015') 'P0-MODEL-INVENTORY-001'; ExactOrdered @($as22[0].owners) @('ADR-005','ADR-007') 'P0-MODEL-INVENTORY-001'; ExactOrdered @($as22[0].gates) @('G-MODEL','G-PRIV','G-SEC-AUDIT','G-RELEASE') 'P0-MODEL-INVENTORY-001'
}

$profiles = @($m.profiles)
if ($m.default_provider -ne 'disabled') { Fail 'P0-MODEL-PROFILES-001' }
ExactOrdered @($profiles.id) @('disabled','ollama_local','ollama_cloud','approved_https') 'P0-MODEL-PROFILES-001'
if ($profiles.Count -ne 4) { Fail 'P0-MODEL-PROFILES-001' }
foreach ($profile in $profiles) { if ($profile.advertised -ne $false) { Fail 'P0-MODEL-PROFILES-001' } }
$disabled = @($profiles | Where-Object id -eq 'disabled')
$local = @($profiles | Where-Object id -eq 'ollama_local')
$cloud = @($profiles | Where-Object id -eq 'ollama_cloud')
$https = @($profiles | Where-Object id -eq 'approved_https')
if ($disabled.Count -ne 1 -or $disabled[0].state -ne 'safe_default' -or $disabled[0].network_requests -ne 0 -or $disabled[0].credential -ne 'none' -or $disabled[0].fallback -ne 'analysis_unavailable; preserve existing loops; zero mutation') { Fail 'P0-MODEL-PROFILES-001' }
if ($local.Count -ne 1 -or $local[0].state -ne 'disabled_pending_gates' -or $local[0].authority -ne 'http://127.0.0.1:11434' -or $local[0].chat_path -ne '/api/chat' -or $local[0].tags_path -ne '/api/tags' -or $local[0].credential -ne 'prohibited') { Fail 'P0-MODEL-PROFILES-001' }
Has (($local[0] | ConvertTo-Json -Compress)) @('direct IPv4 loopback only','no DNS proxy redirect LAN tunnel or alternate port','loopback is not proof of local processing','G-MODEL must prove cloud disabled and reject cloud models','always validate again in application') 'P0-MODEL-PROFILES-001'
if ($cloud.Count -ne 1 -or $cloud[0].state -ne 'disabled_pending_gates' -or $cloud[0].authority -ne 'https://ollama.com' -or $cloud[0].chat_path -ne '/api/chat' -or $cloud[0].tags_path -ne '/api/tags') { Fail 'P0-MODEL-PROFILES-001' }
Has (($cloud[0] | ConvertTo-Json -Compress)) @('provider-credential.dpapi','Authorization Bearer only to exact authority','no proxy redirect or cross-origin authorization','does not currently enforce structured outputs','application validation is authoritative') 'P0-MODEL-PROFILES-001'
if ($https.Count -ne 1 -or $https[0].state -ne 'optional_disabled_pending_adapter_gates' -or $https[0].authority -ne 'one canonical user-consented public HTTPS origin' -or $https[0].chat_path -ne 'adapter-fixed and separately reviewed') { Fail 'P0-MODEL-PROFILES-001' }
Has (($https[0] | ConvertTo-Json -Compress)) @('write-only OS-protected adapter credential','no arbitrary headers','no proxy redirect userinfo query fragment IP literal private link-local reserved metadata or cross-origin authorization','cannot replace the required tested ollama_cloud path') 'P0-MODEL-PROFILES-001'
$preflight = $m.provider_preflight
if ($preflight.mailbox_content -ne 'prohibited') { Fail 'P0-MODEL-PROFILES-001' }
Has (($preflight | ConvertTo-Json -Compress)) @('GET the fixed /api/tags path only after authority validation','bound and strictly parse name model digest and details','ignore unknown members','never persist the raw response','exact provider profile plus exact model label plus provider-reported digest when available plus adapter schema and policy versions','starts Ollama with OLLAMA_NO_CLOUD=1','confirms the content-free cloud-disabled status','confirms a locally resident selected digest','rejects every cloud model and any cloud-capable fallback','proves zero non-loopback connection','ordinary loopback reachability alone never passes','content-free bounded GET /api/tags with the write-only key','only after exact https://ollama.com authority validation','Ollama API is not strictly versioned','response shape capability model digest or documented behavior drift disables the profile pending review','provider remains disabled','no mailbox projection is sent','no alternate origin model provider or proxy is attempted') 'P0-MODEL-PROFILES-001'

$request = $m.request_contract
if ($request.method -ne 'POST' -or $request.content_type -ne 'application/json' -or $request.stream -ne $false -or $request.maximum_context_messages -ne 4 -or $request.maximum_blocks_per_message -ne 64 -or $request.maximum_scalars_per_block -ne 8192 -or $request.maximum_participants_per_message -ne 500 -or $request.maximum_attachment_names_per_message -ne 256 -or $request.maximum_link_labels_per_message -ne 256 -or $request.maximum_request_bytes -ne 524288) { Fail 'P0-MODEL-REQUEST-001' }
ExactOrdered @($request.ollama_top_level_fields_in_order) @('model','messages','stream') 'P0-MODEL-REQUEST-001'
ExactOrdered @($request.ollama_message_fields_in_order) @('role','content') 'P0-MODEL-REQUEST-001'
ExactOrdered @($request.ollama_roles_in_order) @('system','user') 'P0-MODEL-REQUEST-001'
ExactOrdered @($request.allowed_components) @('subject','body_block','quote_block','sender','to','cc','attachment_name','link_label') 'P0-MODEL-REQUEST-001'
ExactOrdered @($request.prohibited) @('whole mailbox','whole thread by default','attachment bytes','linked content','URLs or href values','credentials','unrelated recipients','Graph locators','provider key') 'P0-MODEL-REQUEST-001'
Has (($request | ConvertTo-Json -Compress)) @('no provider-side format or tools field is sent in the MVP contract','strict application validation of analysis-output-v1 is authoritative','tool_calls','prohibited','tools functions images attachments linked-document contents and remote retrieval fields are absent','one changed message projection plus at most four relevance-selected context projections supplied by deterministic code','length-framed untrusted data','cannot alter policy prompt schema scopes tools endpoint or model') 'P0-MODEL-REQUEST-001'

$response = $m.response_contract
if ($response.maximum_response_bytes -ne 262144 -or $response.maximum_wall_time_seconds -ne 60 -or $response.maximum_connect_time_seconds -ne 5 -or $response.schema_id -ne 'openloops-analysis-v1' -or $response.schema_path -ne 'contracts/model/analysis-output.schema.json' -or $response.schema_dialect -ne 'https://json-schema.org/draft/2020-12/schema' -or $response.unknown_fields -ne 'reject' -or $response.duplicate_json_members -ne 'reject before schema validation' -or $response.maximum_claims -ne 64) { Fail 'P0-MODEL-RESPONSE-001' }
ExactOrdered @($response.required_validation_order) @('strict UTF-8 and one JSON value','duplicate-member and unknown-field rejection','schema and numeric/catalog bounds','supplied opaque-handle membership','canonical block and Unicode-scalar range bounds','transient evidence text correspondence','participant-position membership','deterministic date reparse and timezone resolution','internal claim and relation consistency','semantic adversarial and deterministic positive-policy checks') 'P0-MODEL-RESPONSE-001'
Has (($response | ConvertTo-Json -Compress)) @('analysis_unavailable or needs_review','zero Graph Office reminder lifecycle or provider mutation','never persisted or evidence','reconstructed from validated templates and evidence','defense in depth only','never substitutes for application validation') 'P0-MODEL-RESPONSE-001'

$schema = Read-Json $SchemaPath 'P0-MODEL-RESPONSE-001'
if ($null -ne $schema) {
    $schemaCanonical = $schema | ConvertTo-Json -Depth 100 -Compress
    $schemaHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($schemaCanonical))).ToLowerInvariant()
    if ($schemaHash -ne '7af8982ef3c9cc8b19cd4adeea668da55528d84ea9ce0e43a68a135229ea3337' -or $schema.'$schema' -ne 'https://json-schema.org/draft/2020-12/schema' -or $schema.'$id' -ne 'https://openloops.invalid/contracts/model/analysis-output-v1' -or $schema.title -ne 'OpenLoops transient model hypotheses v1' -or $schema.type -ne 'object' -or $schema.additionalProperties -ne $false) { Fail 'P0-MODEL-RESPONSE-001' }
    ExactOrdered @($schema.required) @('schema_version','claims') 'P0-MODEL-RESPONSE-001'
    Exact @($schema.properties.PSObject.Properties.Name) @('schema_version','claims') 'P0-MODEL-RESPONSE-001'
    if ($schema.properties.schema_version.const -ne 1 -or $schema.properties.claims.type -ne 'array' -or $schema.properties.claims.maxItems -ne 64 -or $schema.properties.claims.items.'$ref' -ne '#/$defs/claim') { Fail 'P0-MODEL-RESPONSE-001' }
    $defs = $schema.'$defs'
    Exact @($defs.PSObject.Properties.Name) @('opaque_handle','evidence_range','temporal_hypothesis','claim') 'P0-MODEL-RESPONSE-001'
    $opaque = $defs.opaque_handle
    if ($opaque.type -ne 'string' -or $opaque.minLength -ne 1 -or $opaque.maxLength -ne 128 -or $opaque.pattern -ne '^[A-Za-z0-9_-]+$') { Fail 'P0-MODEL-RESPONSE-001' }
    $evidence = $defs.evidence_range
    if ($evidence.type -ne 'object' -or $evidence.additionalProperties -ne $false) { Fail 'P0-MODEL-RESPONSE-001' }
    ExactOrdered @($evidence.required) @('source_handle','component','block_ordinal','range_start','range_end') 'P0-MODEL-RESPONSE-001'
    Exact @($evidence.properties.PSObject.Properties.Name) @('source_handle','component','block_ordinal','range_start','range_end') 'P0-MODEL-RESPONSE-001'
    ExactOrdered @($evidence.properties.component.enum) @('subject','body_block','quote_block','sender','to','cc','attachment_name','link_label') 'P0-MODEL-RESPONSE-001'
    if ($evidence.properties.source_handle.'$ref' -ne '#/$defs/opaque_handle' -or $evidence.properties.block_ordinal.type -ne 'integer' -or $evidence.properties.block_ordinal.minimum -ne 0 -or $evidence.properties.block_ordinal.maximum -ne 65535 -or $evidence.properties.range_start.type -ne 'integer' -or $evidence.properties.range_start.minimum -ne 0 -or $evidence.properties.range_start.maximum -ne 4294967295 -or $evidence.properties.range_end.type -ne 'integer' -or $evidence.properties.range_end.minimum -ne 1 -or $evidence.properties.range_end.maximum -ne 4294967295) { Fail 'P0-MODEL-RESPONSE-001' }
    $temporal = $defs.temporal_hypothesis
    if ($temporal.type -ne 'object' -or $temporal.additionalProperties -ne $false) { Fail 'P0-MODEL-RESPONSE-001' }
    ExactOrdered @($temporal.required) @('text_evidence_index','kind','value') 'P0-MODEL-RESPONSE-001'
    Exact @($temporal.properties.PSObject.Properties.Name) @('text_evidence_index','kind','value') 'P0-MODEL-RESPONSE-001'
    ExactOrdered @($temporal.properties.kind.enum) @('date','local_datetime','relative','event_relative','soft_window') 'P0-MODEL-RESPONSE-001'
    if ($temporal.properties.text_evidence_index.minimum -ne 0 -or $temporal.properties.text_evidence_index.maximum -ne 7 -or $temporal.properties.value.type -ne 'string' -or $temporal.properties.value.minLength -ne 1 -or $temporal.properties.value.maxLength -ne 256) { Fail 'P0-MODEL-RESPONSE-001' }
    $claim = $defs.claim
    if ($claim.type -ne 'object' -or $claim.additionalProperties -ne $false) { Fail 'P0-MODEL-RESPONSE-001' }
    ExactOrdered @($claim.required) @('claim_type','evidence','waiting_party_handle','related_loop_handles','temporal','confidence_micros','ambiguity_codes') 'P0-MODEL-RESPONSE-001'
    Exact @($claim.properties.PSObject.Properties.Name) @('claim_type','evidence','waiting_party_handle','related_loop_handles','temporal','confidence_micros','ambiguity_codes') 'P0-MODEL-RESPONSE-001'
    ExactOrdered @($claim.properties.claim_type.enum) @('request','promise','attribution','question','deadline_change','possible_closure','delegation','modification') 'P0-MODEL-RESPONSE-001'
    ExactOrdered @($claim.properties.ambiguity_codes.items.enum) @('quote_scope','identity','delegation','deadline','relation','cross_message','insufficient_context','semantic_conflict') 'P0-MODEL-RESPONSE-001'
    if ($claim.properties.evidence.type -ne 'array' -or $claim.properties.evidence.minItems -ne 1 -or $claim.properties.evidence.maxItems -ne 8 -or $claim.properties.evidence.items.'$ref' -ne '#/$defs/evidence_range' -or $claim.properties.related_loop_handles.type -ne 'array' -or $claim.properties.related_loop_handles.maxItems -ne 8 -or $claim.properties.related_loop_handles.uniqueItems -ne $true -or $claim.properties.confidence_micros.minimum -ne 0 -or $claim.properties.confidence_micros.maximum -ne 1000000 -or $claim.properties.ambiguity_codes.maxItems -ne 8 -or $claim.properties.ambiguity_codes.uniqueItems -ne $true) { Fail 'P0-MODEL-RESPONSE-001' }
    Exact @($claim.properties.waiting_party_handle.oneOf | ForEach-Object { if ($_.PSObject.Properties['$ref']) { $_.'$ref' } else { $_.type } }) @('#/$defs/opaque_handle','null') 'P0-MODEL-RESPONSE-001'
    Exact @($claim.properties.temporal.oneOf | ForEach-Object { if ($_.PSObject.Properties['$ref']) { $_.'$ref' } else { $_.type } }) @('#/$defs/temporal_hypothesis','null') 'P0-MODEL-RESPONSE-001'
}

$network = $m.network_policy
Has (($network | ConvertTo-Json -Compress)) @('disabled at client and any redirect response rejects','HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY system proxy PAC WPAD and provider SDK defaults are ignored','all answers are validated before connect','connected peer address is revalidated','private loopback link-local multicast reserved documentation benchmark carrier-grade NAT unspecified or metadata address rejects','construct only after final authority validation','never forward on redirect retry authority edit or DNS-policy failure','system trust and hostname verification required externally','no custom CA insecure switch or certificate bypass','zero automatic retry after any request with content','revalidates consent origin model and evidence','cancels immediately over byte or time limit') 'P0-MODEL-NETWORK-001'

$consent = $m.consent_contract
ExactOrdered @($consent.required_before_content) @('provider profile','exact authority','model label','transmitted field categories','maximum context count','provider privacy and retention responsibility','local process boundary or external transmission') 'P0-MODEL-CONSENT-001'
ExactOrdered @($consent.invalidated_by) @('provider profile change','authority change','path or adapter change','model label or digest change','transmitted-field change','schema or policy version change','provider capability drift') 'P0-MODEL-CONSENT-001'
Has (($consent | ConvertTo-Json -Compress)) @('write-only','never returned to add-in UI diagnostics logs errors command line repository or package','content-free request only where supported','replacement requires content-free revalidation','deletion or secure-store loss disables active use','neither replacement nor deletion triggers replay','failure sends no mailbox content') 'P0-MODEL-CONSENT-001'
$settingsProperty = $m.PSObject.Properties['settings_disclosure']
if ($null -eq $settingsProperty) {
    Fail 'P0-MODEL-CONSENT-001'
}
else {
    $settings = $settingsProperty.Value
    if ($settings.policy_version -ne 'model-sensitivity-v1' -or $settings.ui_visibility -ne 'show provider profile exact endpoint model label external-data disclosure and every confidence or detection sensitivity before provider use and whenever settings are reviewed' -or $settings.global_change_behavior -ne 'a sensitivity or threshold change never starts replay automatically; existing loops remain intact') { Fail 'P0-MODEL-CONSENT-001' }
    $sensitivities = @($settings.sensitivities)
    ExactOrdered @($sensitivities.id) @('request_sensitivity','deadline_inference_sensitivity','closure_sensitivity') 'P0-MODEL-CONSENT-001'
    $requestSensitivity = @($sensitivities | Where-Object id -eq 'request_sensitivity')
    $deadlineSensitivity = @($sensitivities | Where-Object id -eq 'deadline_inference_sensitivity')
    $closureSensitivity = @($sensitivities | Where-Object id -eq 'closure_sensitivity')
    if ($requestSensitivity.Count -ne 1 -or $deadlineSensitivity.Count -ne 1 -or $closureSensitivity.Count -ne 1) { Fail 'P0-MODEL-CONSENT-001' }
    if ($requestSensitivity.Count -eq 1) {
        ExactOrdered @($requestSensitivity[0].allowed_values) @('low','standard','high') 'P0-MODEL-CONSENT-001'
        if ($requestSensitivity[0].default -ne 'standard' -or $requestSensitivity[0].default_semantics -ne 'high-recall standard' -or $requestSensitivity[0].prerequisite -ne 'provider configured for inferred cases' -or $requestSensitivity[0].change_behavior -ne 'offers bounded replay with review-only results') { Fail 'P0-MODEL-CONSENT-001' }
    }
    if ($deadlineSensitivity.Count -eq 1) {
        ExactOrdered @($deadlineSensitivity[0].allowed_values) @('explicit-only','standard','high') 'P0-MODEL-CONSENT-001'
        if ($deadlineSensitivity[0].default -ne 'standard' -or $deadlineSensitivity[0].prerequisite -ne 'provider optional' -or $deadlineSensitivity[0].change_behavior -ne 'offers bounded replay and never overwrites user resolution automatically') { Fail 'P0-MODEL-CONSENT-001' }
    }
    if ($closureSensitivity.Count -eq 1) {
        ExactOrdered @($closureSensitivity[0].allowed_values) @('conservative','standard','high') 'P0-MODEL-CONSENT-001'
        if ($closureSensitivity[0].default -ne 'conservative' -or $closureSensitivity[0].prerequisite -ne 'provider configured for semantic association' -or $closureSensitivity[0].change_behavior -ne 'offers bounded replay and every result remains possible closure') { Fail 'P0-MODEL-CONSENT-001' }
    }
    $reviewThresholds = $settings.review_thresholds
    ExactOrdered @($reviewThresholds.claim_types) @('request','promise','attribution','question','deadline_change','possible_closure','delegation','modification') 'P0-MODEL-CONSENT-001'
    if ($reviewThresholds.threshold_kind -ne 'per claim type calibrated confidence bucket' -or $reviewThresholds.safe_default -ne 'review_required' -or $reviewThresholds.pinned_by -ne 'model-sensitivity-v1' -or $reviewThresholds.gate -ne 'G-MODEL' -or $reviewThresholds.change_behavior -ne 'offers bounded policy-version replay with review-only results and no automatic terminal Graph Office or reminder mutation') { Fail 'P0-MODEL-CONSENT-001' }
}

$semantic = $m.semantic_policy
if ($semantic.model_role -ne 'untrusted transient hypothesis producer only' -or $semantic.schema_valid_is_not_safe -ne $true) { Fail 'P0-MODEL-AUTHORITY-001' }
ExactOrdered @($semantic.never_authority_for) @('Graph or Office mutation','lifecycle transition','automatic closure','scope or prompt change','endpoint selection','tool execution') 'P0-MODEL-AUTHORITY-001'
Has (($semantic | ConvertTo-Json -Compress)) @('unavailable until separate ADR-011 plus G-AUTO or G-AUTO-FULL','independently held semantic adversarial suite','deterministic positive constraints','drift invalidates automation calibration','review-only replay only') 'P0-MODEL-AUTHORITY-001'

$privacy = $m.privacy_boundary
ExactOrdered @($privacy.diagnostics) @('provider_disabled','analysis_unavailable','timeout','transport_policy_rejected','response_too_large','invalid_utf8','invalid_json','invalid_schema','invalid_evidence','semantic_rejected') 'P0-MODEL-PRIVACY-001'
Has (($privacy | ConvertTo-Json -Compress)) @('persistent_prompt_request_response_output_rationale_transcript_embedding','prohibited','fixed codes and bounded non-content counters only','no endpoint path host key header body prompt response model text or source identifier','disabled and source-sanitized before application exceptions','zero in every application-controlled durable temporary diagnostic crash browser package CI and repository artifact','exact enabled provider request is the only test-time exception') 'P0-MODEL-PRIVACY-001'

ExactOrdered @($m.sources.id) @('SRC-SPEC-OWN-08','SRC-SPEC-MODEL','SRC-OLLAMA-API','SRC-OLLAMA-AUTH','SRC-OLLAMA-CLOUD','SRC-OLLAMA-FAQ','SRC-OLLAMA-STRUCTURED','SRC-OLLAMA-TAGS') 'P0-MODEL-SOURCES-001'
ExactOrdered @($m.sources.location) @('docs/product-spec.md#31-approved-owner-decisions','docs/product-spec.md#611-model-and-policy-contract','https://docs.ollama.com/api/introduction','https://docs.ollama.com/api/authentication','https://docs.ollama.com/cloud','https://docs.ollama.com/faq','https://docs.ollama.com/capabilities/structured-outputs','https://docs.ollama.com/api/tags') 'P0-MODEL-SOURCES-001'
$remoteSources = @($m.sources | Where-Object { $_.location -like 'https://*' })
if ($remoteSources.Count -ne 6 -or @($remoteSources | Where-Object { $_.verified -ne '2026-07-20' }).Count -ne 0) { Fail 'P0-MODEL-SOURCES-001' }
ExactOrdered @($m.source_claims.source) @('SRC-OLLAMA-API','SRC-OLLAMA-AUTH','SRC-OLLAMA-CLOUD','SRC-OLLAMA-FAQ','SRC-OLLAMA-STRUCTURED','SRC-OLLAMA-TAGS') 'P0-MODEL-SOURCES-001'
ExactOrdered @($m.source_claims.claim) @(
    'documented local base is http://localhost:11434/api, cloud base is https://ollama.com/api, and the API is not strictly versioned',
    'local localhost API requires no authentication; direct ollama.com API uses an API-key Bearer authorization header',
    'cloud models may be reached through a signed-in local Ollama process or directly at ollama.com; P0-WI-10 permits only the separately gated direct cloud profile and never treats loopback as local-only proof',
    'OLLAMA_NO_CLOUD=1 disables Ollama cloud features and default serving binds loopback; G-MODEL must prove this in a disposable process rather than trusting reachability',
    'Ollama Cloud currently does not support structured outputs; application validation is authoritative',
    'GET /api/tags returns a model list including name model and digest fields; raw responses remain transient'
) 'P0-MODEL-SOURCES-001'

$runtime = $m.runtime_boundary
if ($runtime.provider_transport -ne $false -or $runtime.credentials_accepted -ne $false) { Fail 'P0-MODEL-CLAIMS-001' }
Empty @($runtime.configured_profiles) 'P0-MODEL-CLAIMS-001'
Empty @($runtime.network_origins) 'P0-MODEL-CLAIMS-001'
Empty @($runtime.records_created) 'P0-MODEL-CLAIMS-001'
Empty @($runtime.acceptance_scenarios_completed) 'P0-MODEL-CLAIMS-001'
Empty @($runtime.gates_passed) 'P0-MODEL-CLAIMS-001'
Empty @($m.claims.provider_calls) 'P0-MODEL-CLAIMS-001'
Empty @($m.claims.permissions_requested) 'P0-MODEL-CLAIMS-001'
Empty @($m.claims.capabilities_enabled) 'P0-MODEL-CLAIMS-001'
Empty @($m.claims.support_rows_advertised) 'P0-MODEL-CLAIMS-001'
Empty @($m.claims.acceptance_criteria_completed) 'P0-MODEL-CLAIMS-001'

$governance = Read-Json $GovernancePath 'P0-MODEL-CROSS-CONTRACT-001'
$support = Read-Json $SupportPath 'P0-MODEL-CROSS-CONTRACT-001'
$build = Read-Json $BuildPath 'P0-MODEL-CROSS-CONTRACT-001'
$privacyContract = Read-Json $PrivacyPath 'P0-MODEL-CROSS-CONTRACT-001'
$protected = Read-Json $ProtectedStatePath 'P0-MODEL-CROSS-CONTRACT-001'
if ($null -ne $governance) {
    $owner = @($governance.owner_decisions | Where-Object id -eq 'OWN-08')
    $adr = @($governance.adrs | Where-Object id -eq 'ADR-007')
    $gates = @($governance.gates | Where-Object id -in @('G-MODEL','G-PRIV','G-SEC-AUDIT','G-RELEASE'))
    $capability = @($governance.capabilities | Where-Object id -eq 'external_model_providers')
    $threat = @($governance.threat_flows | Where-Object id -eq 'model_transmission')
    if ($owner.Count -ne 1 -or $owner[0].status -ne 'accepted' -or $adr.Count -ne 1 -or $adr[0].status -ne 'accepted' -or $gates.Count -ne 4 -or @($gates | Where-Object status -ne 'unrun').Count -ne 0 -or $capability.Count -ne 1 -or $capability[0].state -ne 'disabled' -or $capability[0].advertised -ne $false -or $threat.Count -ne 1) { Fail 'P0-MODEL-CROSS-CONTRACT-001' }
    Exact @($capability[0].gates) @('G-MODEL','G-PRIV') 'P0-MODEL-CROSS-CONTRACT-001'
    Has (($capability[0] | ConvertTo-Json -Compress)) @('no_graph_scope','exact_provider_endpoint_consent_required','no mailbox content transmission while disabled','no implicit provider selection') 'P0-MODEL-CROSS-CONTRACT-001'
}
if ($null -ne $support) {
    $supportProfiles = @($support.matrices.model_providers)
    ExactOrdered @($supportProfiles.id) @('disabled','ollama_local','ollama_cloud','approved_https') 'P0-MODEL-CROSS-CONTRACT-001'
    if (@($supportProfiles | Where-Object { $_.advertised -ne $false }).Count -ne 0 -or @($supportProfiles | Where-Object { $_.id -ne 'disabled' -and $_.current_state -ne 'disabled' }).Count -ne 0) { Fail 'P0-MODEL-CROSS-CONTRACT-001' }
    Empty @($support.claim_state.supported_rows) 'P0-MODEL-CROSS-CONTRACT-001'
    Empty @($support.claim_state.enabled_capabilities) 'P0-MODEL-CROSS-CONTRACT-001'
    Empty @($support.claim_state.advertised_capabilities) 'P0-MODEL-CROSS-CONTRACT-001'
    Empty @($support.claim_state.gates_passed) 'P0-MODEL-CROSS-CONTRACT-001'
}
if ($null -ne $build) {
    if ($build.runtime_boundary.model_provider -ne $false) { Fail 'P0-MODEL-CROSS-CONTRACT-001' }
    Empty @($build.runtime_boundary.network_origins) 'P0-MODEL-CROSS-CONTRACT-001'
    Empty @($build.runtime_boundary.accepted_secrets) 'P0-MODEL-CROSS-CONTRACT-001'
}
if ($null -ne $privacyContract) {
    if ($privacyContract.claim_state.capability_enabled -ne $false) { Fail 'P0-MODEL-CROSS-CONTRACT-001' }
    Empty @($privacyContract.claim_state.gates_passed) 'P0-MODEL-CROSS-CONTRACT-001'
    Empty @($privacyContract.claim_state.acceptance_criteria_completed) 'P0-MODEL-CROSS-CONTRACT-001'
    Exact @($privacyContract.forbidden_classes | Where-Object { $_ -in @('prompt','model_request','model_response','model_output','model_rationale','embedding','transcript') }) @('prompt','model_request','model_response','model_output','model_rationale','embedding','transcript') 'P0-MODEL-CROSS-CONTRACT-001'
}
if ($null -ne $protected) {
    $credential = @($protected.secret_inventory | Where-Object id -eq 'provider_credential')
    if ($credential.Count -ne 1 -or $credential[0].owner -ne 'ADR-005 foundation and ADR-007 use policy' -or $credential[0].location -ne 'provider-credential.dpapi inside the protected blob store or process memory only' -or $credential[0].failure -ne 'external provider remains unavailable and no content is transmitted' -or $protected.separate_decisions.model_provider_use_and_transport -ne 'ADR-007') { Fail 'P0-MODEL-CROSS-CONTRACT-001' }
}

$adrText = Read-Text $AdrPath 'P0-MODEL-CROSS-CONTRACT-001'
$threatText = Read-Text $ThreatPath 'P0-MODEL-CROSS-CONTRACT-001'
$traceText = Read-Text $TraceabilityPath 'P0-MODEL-INVENTORY-001'
$adrCanonical = (($adrText -replace "`r`n", "`n").TrimEnd() + "`n")
$adrHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($adrCanonical))).ToLowerInvariant()
if ($adrHash -ne '0d5c48eff0d9fc1ff33a0dc8effa952dd1e8232eb51dec2c9e7451ff9ac044a3') { Fail 'P0-MODEL-CROSS-CONTRACT-001' }
Has $adrText @('ADR-007','OWN-08','Accepted','implements no adapter, transport, credential store','G-MODEL','G-PRIV','ollama_local','ollama_cloud','approved_https','model-sensitivity-v1','Request sensitivity','Deadline inference sensitivity','Closure sensitivity','review_required','No settings change starts replay automatically','No tool surface exists','Prompts, requests, responses, outputs, rationales, transcripts','are never persisted') 'P0-MODEL-CROSS-CONTRACT-001'
Has $threatText @('Model-provider boundary threat model','No provider adapter, origin, credential, request','redirect','proxy','DNS','consent','canary','untrusted','zero mutation') 'P0-MODEL-CROSS-CONTRACT-001'
$traceIds = [regex]::Matches($traceText, '(?m)^\| (P0-MODEL-[A-Z-]+-001) \|') | ForEach-Object { $_.Groups[1].Value }
Exact @($traceIds) $checks 'P0-MODEL-INVENTORY-001'
$freshRow = [regex]::Match($traceText, '(?m)^\| P0-MODEL-FRESH-CHECKER-001 \|.*$').Value
Has $freshRow @('Fresh-context privacy/security, transport, and adversarial judges','no transcript or model output stored','Passed at P0-WI-10 closure') 'P0-MODEL-FRESH-CHECKER-001'

if ($script:failures.Count -gt 0) {
    [Console]::Error.WriteLine(('FAIL: model provider boundary: ' + (($script:failures | Sort-Object) -join ', ')))
    exit 1
}

if (-not $Quiet) {
    Write-Output ('PASS: model provider boundary ({0} checks; runtime and claims disabled)' -f $checks.Count)
}
