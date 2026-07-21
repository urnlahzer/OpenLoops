[CmdletBinding()]
param([string]$CheckerPath = 'tools/check-model-boundary.ps1')

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) { throw 'Run inside the repository.' }

function Resolve-Repo([string]$Path) {
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

$checker = Resolve-Repo $CheckerPath
$inputs = @(
    'contracts/model/provider-boundary.json',
    'contracts/model/analysis-output.schema.json',
    'docs/adr/ADR-007-model-boundary.md',
    'docs/threat-model/model-provider-boundary.md',
    'docs/prd-traceability.md',
    'contracts/governance/capabilities.json',
    'contracts/support/support-matrix.json',
    'contracts/build-skeleton/skeleton.json',
    'contracts/privacy/persistence-boundary.json',
    'contracts/persistence/protected-state-boundary.json'
)
$script:caseCount = 0
$script:failures = [Collections.Generic.List[string]]::new()

function New-SyntheticRoot {
    $root = Join-Path ([IO.Path]::GetTempPath()) ('openloops-model-boundary-' + [guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($root) | Out-Null
    foreach ($relative in $inputs) {
        $target = Join-Path $root $relative
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target)) | Out-Null
        Copy-Item -LiteralPath (Join-Path $repoRoot $relative) -Destination $target
    }
    return $root
}

function Invoke-SyntheticChecker([string]$Root) {
    $arguments = @(
        '-NoProfile','-File',$checker,
        '-ManifestPath',(Join-Path $Root 'contracts/model/provider-boundary.json'),
        '-SchemaPath',(Join-Path $Root 'contracts/model/analysis-output.schema.json'),
        '-AdrPath',(Join-Path $Root 'docs/adr/ADR-007-model-boundary.md'),
        '-ThreatPath',(Join-Path $Root 'docs/threat-model/model-provider-boundary.md'),
        '-TraceabilityPath',(Join-Path $Root 'docs/prd-traceability.md'),
        '-GovernancePath',(Join-Path $Root 'contracts/governance/capabilities.json'),
        '-SupportPath',(Join-Path $Root 'contracts/support/support-matrix.json'),
        '-BuildPath',(Join-Path $Root 'contracts/build-skeleton/skeleton.json'),
        '-PrivacyPath',(Join-Path $Root 'contracts/privacy/persistence-boundary.json'),
        '-ProtectedStatePath',(Join-Path $Root 'contracts/persistence/protected-state-boundary.json'),
        '-Quiet'
    )
    $output = @(& pwsh @arguments 2>&1 | ForEach-Object { [string]$_ })
    return [pscustomobject]@{ Code = $LASTEXITCODE; Output = ($output -join "`n") }
}

function JsonCase([string]$Name, [string]$ExpectedId, [string]$RelativePath, [scriptblock]$Mutate) {
    $script:caseCount++
    $root = New-SyntheticRoot
    try {
        $path = Join-Path $root $RelativePath
        $value = Get-Content -Raw -LiteralPath $path | ConvertFrom-Json -Depth 100
        & $Mutate $value
        $value | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $path -Encoding utf8NoBOM
        $result = Invoke-SyntheticChecker $root
        if ($result.Code -eq 0 -or $result.Output -notmatch [regex]::Escape($ExpectedId)) {
            $script:failures.Add("$Name (expected $ExpectedId; exit $($result.Code))")
        }
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

function TextCase([string]$Name, [string]$ExpectedId, [string]$RelativePath, [scriptblock]$Mutate) {
    $script:caseCount++
    $root = New-SyntheticRoot
    try {
        $path = Join-Path $root $RelativePath
        $text = Get-Content -Raw -LiteralPath $path
        $changed = & $Mutate $text
        Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM
        $result = Invoke-SyntheticChecker $root
        if ($result.Code -eq 0 -or $result.Output -notmatch [regex]::Escape($ExpectedId)) {
            $script:failures.Add("$Name (expected $ExpectedId; exit $($result.Code))")
        }
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

$baseline = & pwsh -NoProfile -File $checker 2>&1
if ($LASTEXITCODE -ne 0) { throw ('Baseline model-boundary checker failed: ' + (@($baseline) -join "`n")) }

JsonCase 'work item drift' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) $c.work_item = 'P0-WI-SYNTHETIC' }
JsonCase 'owner omitted' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) $c.owner_decisions = @() }
JsonCase 'requirement omitted' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) $c.requirements = @($c.requirements | Where-Object { $_ -ne 'OL-MODEL-015' }) }
JsonCase 'sync recovery requirement omitted' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) $c.requirements = @($c.requirements | Where-Object { $_ -ne 'OL-SYNC-013' }) }
JsonCase 'scenario dependency omitted' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) $c.scenario_dependencies = @($c.scenario_dependencies | Where-Object { $_.id -ne 'AS-16' }) }
JsonCase 'scenario marked passed' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) ($c.scenario_dependencies | Where-Object id -eq 'AS-22').status = 'passed' }
JsonCase 'scenario owner widened' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) ($c.scenario_dependencies | Where-Object id -eq 'AS-12').owners += 'ADR-SYNTHETIC' }
JsonCase 'blocking gate omitted' 'P0-MODEL-INVENTORY-001' 'contracts/model/provider-boundary.json' { param($c) $c.blocking_gates = @($c.blocking_gates | Where-Object { $_ -ne 'G-PRIV' }) }

JsonCase 'default provider selected' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) $c.default_provider = 'ollama_local' }
JsonCase 'profile advertised' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.profiles | Where-Object id -eq 'ollama_cloud').advertised = $true }
JsonCase 'local authority widened' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.profiles | Where-Object id -eq 'ollama_local').authority = 'http://localhost:11434' }
JsonCase 'local credential accepted' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.profiles | Where-Object id -eq 'ollama_local').credential = 'optional' }
JsonCase 'local cloud rejection removed' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.profiles | Where-Object id -eq 'ollama_local').locality = 'loopback is local' }
JsonCase 'cloud authority editable' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.profiles | Where-Object id -eq 'ollama_cloud').authority = 'https://synthetic.invalid' }
JsonCase 'cloud schema trusted' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.profiles | Where-Object id -eq 'ollama_cloud').structured_output = 'provider enforcement is authoritative' }
JsonCase 'approved HTTPS substitutes hosted' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.profiles | Where-Object id -eq 'approved_https').substitution = 'allowed' }
JsonCase 'preflight sends mailbox content' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) $c.provider_preflight.mailbox_content = 'allowed' }
JsonCase 'preflight raw response persists' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) $c.provider_preflight.ollama_model_listing = 'persist complete tags response' }
JsonCase 'local cloud-disable proof removed' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) $c.provider_preflight.local_only_release_proof = 'loopback reachability passes' }
JsonCase 'cloud key validation sends content' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) $c.provider_preflight.cloud_key_validation = 'send sample mailbox projection' }
JsonCase 'preflight fallback broadens' 'P0-MODEL-PROFILES-001' 'contracts/model/provider-boundary.json' { param($c) $c.provider_preflight.failure = 'try another provider' }

JsonCase 'request streams' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.stream = $true }
JsonCase 'request top-level field added' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.ollama_top_level_fields_in_order += 'tools' }
JsonCase 'request message field added' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.ollama_message_fields_in_order += 'images' }
JsonCase 'assistant role added' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.ollama_roles_in_order += 'assistant' }
JsonCase 'provider format enabled' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.provider_schema_request = 'send format field' }
JsonCase 'tool calls enabled' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.tool_calls = 'allowed' }
JsonCase 'context bound widened' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.maximum_context_messages = 5 }
JsonCase 'request byte bound widened' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.maximum_request_bytes = 524289 }
JsonCase 'URL component allowed' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.allowed_components += 'url' }
JsonCase 'attachment bytes no longer prohibited' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.prohibited = @($c.request_contract.prohibited | Where-Object { $_ -ne 'attachment bytes' }) }
JsonCase 'untrusted framing weakened' 'P0-MODEL-REQUEST-001' 'contracts/model/provider-boundary.json' { param($c) $c.request_contract.untrusted_delimiting = 'plain concatenation' }

JsonCase 'response byte bound widened' 'P0-MODEL-RESPONSE-001' 'contracts/model/provider-boundary.json' { param($c) $c.response_contract.maximum_response_bytes = 262145 }
JsonCase 'response schema path changed' 'P0-MODEL-RESPONSE-001' 'contracts/model/provider-boundary.json' { param($c) $c.response_contract.schema_path = 'synthetic.json' }
JsonCase 'unknown fields accepted' 'P0-MODEL-RESPONSE-001' 'contracts/model/provider-boundary.json' { param($c) $c.response_contract.unknown_fields = 'accept' }
JsonCase 'validation reordered' 'P0-MODEL-RESPONSE-001' 'contracts/model/provider-boundary.json' { param($c) [array]::Reverse($c.response_contract.required_validation_order) }
JsonCase 'provider schema authoritative' 'P0-MODEL-RESPONSE-001' 'contracts/model/provider-boundary.json' { param($c) $c.response_contract.provider_schema_enforcement = 'authoritative' }
JsonCase 'invalid result mutates' 'P0-MODEL-RESPONSE-001' 'contracts/model/provider-boundary.json' { param($c) $c.response_contract.invalid_result = 'create reminder' }
JsonCase 'schema root opens' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.additionalProperties = $true }
JsonCase 'schema claim count widens' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.properties.claims.maxItems = 65 }
JsonCase 'schema command property added' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.properties | Add-Member command ([pscustomobject]@{ type = 'string' }) }
JsonCase 'schema evidence opens' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.'$defs'.evidence_range.additionalProperties = $true }
JsonCase 'schema URL component added' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.'$defs'.evidence_range.properties.component.enum += 'url' }
JsonCase 'schema empty evidence allowed' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.'$defs'.claim.properties.evidence.minItems = 0 }
JsonCase 'schema confidence widens' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.'$defs'.claim.properties.confidence_micros.maximum = 1000001 }
JsonCase 'schema mutation claim added' 'P0-MODEL-RESPONSE-001' 'contracts/model/analysis-output.schema.json' { param($c) $c.'$defs'.claim.properties.claim_type.enum += 'send_message' }

JsonCase 'redirect accepted' 'P0-MODEL-NETWORK-001' 'contracts/model/provider-boundary.json' { param($c) $c.network_policy.redirects = 'follow' }
JsonCase 'proxy inherited' 'P0-MODEL-NETWORK-001' 'contracts/model/provider-boundary.json' { param($c) $c.network_policy.proxy_inheritance = 'system default' }
JsonCase 'connected peer not revalidated' 'P0-MODEL-NETWORK-001' 'contracts/model/provider-boundary.json' { param($c) $c.network_policy.dns = 'validate first answer only' }
JsonCase 'authorization forwarded' 'P0-MODEL-NETWORK-001' 'contracts/model/provider-boundary.json' { param($c) $c.network_policy.auth_header_policy = 'forward on redirect' }
JsonCase 'custom CA allowed' 'P0-MODEL-NETWORK-001' 'contracts/model/provider-boundary.json' { param($c) $c.network_policy.tls = 'custom CA allowed' }
JsonCase 'content retry enabled' 'P0-MODEL-NETWORK-001' 'contracts/model/provider-boundary.json' { param($c) $c.network_policy.retries = 'three automatic retries' }
JsonCase 'oversize response buffered' 'P0-MODEL-NETWORK-001' 'contracts/model/provider-boundary.json' { param($c) $c.network_policy.response = 'read to completion' }

JsonCase 'consent field omitted' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.consent_contract.required_before_content = @($c.consent_contract.required_before_content | Where-Object { $_ -ne 'provider privacy and retention responsibility' }) }
JsonCase 'authority change keeps consent' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.consent_contract.invalidated_by = @($c.consent_contract.invalidated_by | Where-Object { $_ -ne 'authority change' }) }
JsonCase 'key returned to UI' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.consent_contract.credential_display = 'return to UI' }
JsonCase 'replacement skips validation' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.consent_contract.key_validation = 'replacement accepted without request' }
JsonCase 'deletion triggers replay' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.consent_contract.key_validation = 'deletion triggers replay' }
JsonCase 'sensitivity inventory removed' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.PSObject.Properties.Remove('settings_disclosure') }
JsonCase 'request sensitivity default drift' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) ($c.settings_disclosure.sensitivities | Where-Object id -eq 'request_sensitivity').default = 'high' }
JsonCase 'review threshold default broadened' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.settings_disclosure.review_thresholds.safe_default = 'automatic' }
JsonCase 'settings change starts replay' 'P0-MODEL-CONSENT-001' 'contracts/model/provider-boundary.json' { param($c) $c.settings_disclosure.global_change_behavior = 'replay immediately' }

JsonCase 'model becomes lifecycle authority' 'P0-MODEL-AUTHORITY-001' 'contracts/model/provider-boundary.json' { param($c) $c.semantic_policy.never_authority_for = @($c.semantic_policy.never_authority_for | Where-Object { $_ -ne 'lifecycle transition' }) }
JsonCase 'schema valid marked safe' 'P0-MODEL-AUTHORITY-001' 'contracts/model/provider-boundary.json' { param($c) $c.semantic_policy.schema_valid_is_not_safe = $false }
JsonCase 'automatic eligibility enabled' 'P0-MODEL-AUTHORITY-001' 'contracts/model/provider-boundary.json' { param($c) $c.semantic_policy.automatic_eligibility = 'enabled' }
JsonCase 'drift keeps calibration' 'P0-MODEL-AUTHORITY-001' 'contracts/model/provider-boundary.json' { param($c) $c.semantic_policy.drift = 'keep calibration' }

JsonCase 'prompt persistence enabled' 'P0-MODEL-PRIVACY-001' 'contracts/model/provider-boundary.json' { param($c) $c.privacy_boundary.persistent_prompt_request_response_output_rationale_transcript_embedding = 'allowed' }
JsonCase 'content diagnostic added' 'P0-MODEL-PRIVACY-001' 'contracts/model/provider-boundary.json' { param($c) $c.privacy_boundary.diagnostics += 'provider_body' }
JsonCase 'diagnostic host allowed' 'P0-MODEL-PRIVACY-001' 'contracts/model/provider-boundary.json' { param($c) $c.privacy_boundary.diagnostic_values = 'host and path allowed' }
JsonCase 'SDK logging enabled' 'P0-MODEL-PRIVACY-001' 'contracts/model/provider-boundary.json' { param($c) $c.privacy_boundary.provider_sdk_logging = 'verbose' }
JsonCase 'canary tolerated' 'P0-MODEL-PRIVACY-001' 'contracts/model/provider-boundary.json' { param($c) $c.privacy_boundary.canary_requirement = 'one occurrence allowed' }

JsonCase 'source omitted' 'P0-MODEL-SOURCES-001' 'contracts/model/provider-boundary.json' { param($c) $c.sources = @($c.sources | Where-Object id -ne 'SRC-OLLAMA-TAGS') }
JsonCase 'source verification date drift' 'P0-MODEL-SOURCES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.sources | Where-Object id -eq 'SRC-OLLAMA-FAQ').verified = '2000-01-01' }
JsonCase 'source claim reordered' 'P0-MODEL-SOURCES-001' 'contracts/model/provider-boundary.json' { param($c) [array]::Reverse($c.source_claims) }
JsonCase 'cloud limitation claim weakened' 'P0-MODEL-SOURCES-001' 'contracts/model/provider-boundary.json' { param($c) ($c.source_claims | Where-Object source -eq 'SRC-OLLAMA-STRUCTURED').claim = 'cloud enforces structured outputs' }
JsonCase 'cloud disable claim removed' 'P0-MODEL-SOURCES-001' 'contracts/model/provider-boundary.json' { param($c) $c.source_claims = @($c.source_claims | Where-Object source -ne 'SRC-OLLAMA-FAQ') }

JsonCase 'provider transport enabled' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.runtime_boundary.provider_transport = $true }
JsonCase 'profile configured' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.runtime_boundary.configured_profiles = @('ollama_local') }
JsonCase 'origin configured' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.runtime_boundary.network_origins = @('https://synthetic.invalid') }
JsonCase 'credential accepted' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.runtime_boundary.credentials_accepted = $true }
JsonCase 'scenario completed' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.runtime_boundary.acceptance_scenarios_completed = @('AS-22') }
JsonCase 'provider call claimed' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.claims.provider_calls = @('synthetic') }
JsonCase 'permission claimed' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.claims.permissions_requested = @('Synthetic.Scope') }
JsonCase 'support advertised' 'P0-MODEL-CLAIMS-001' 'contracts/model/provider-boundary.json' { param($c) $c.claims.support_rows_advertised = @('ollama_cloud') }

JsonCase 'governance model gate passed' 'P0-MODEL-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' { param($c) ($c.gates | Where-Object id -eq 'G-MODEL').status = 'passed' }
JsonCase 'governance provider enabled' 'P0-MODEL-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' { param($c) ($c.capabilities | Where-Object id -eq 'external_model_providers').state = 'enabled' }
JsonCase 'support provider advertised' 'P0-MODEL-CROSS-CONTRACT-001' 'contracts/support/support-matrix.json' { param($c) ($c.matrices.model_providers | Where-Object id -eq 'ollama_local').advertised = $true }
JsonCase 'support row claimed' 'P0-MODEL-CROSS-CONTRACT-001' 'contracts/support/support-matrix.json' { param($c) $c.claim_state.supported_rows = @('synthetic') }
JsonCase 'build provider enabled' 'P0-MODEL-CROSS-CONTRACT-001' 'contracts/build-skeleton/skeleton.json' { param($c) $c.runtime_boundary.model_provider = $true }
JsonCase 'privacy prompt allowed' 'P0-MODEL-CROSS-CONTRACT-001' 'contracts/privacy/persistence-boundary.json' { param($c) $c.forbidden_classes = @($c.forbidden_classes | Where-Object { $_ -ne 'prompt' }) }
JsonCase 'provider credential owner drift' 'P0-MODEL-CROSS-CONTRACT-001' 'contracts/persistence/protected-state-boundary.json' { param($c) ($c.secret_inventory | Where-Object id -eq 'provider_credential').owner = 'synthetic' }
TextCase 'ADR runtime activation' 'P0-MODEL-CROSS-CONTRACT-001' 'docs/adr/ADR-007-model-boundary.md' { param($t) $t -replace 'implements no adapter, transport, credential store','implements an adapter and transport' }
TextCase 'ADR contradictory activation appended' 'P0-MODEL-CROSS-CONTRACT-001' 'docs/adr/ADR-007-model-boundary.md' { param($t) $t + "`nollama_cloud is enabled and advertised now; G-MODEL passed.`n" }
TextCase 'threat redirect control removed' 'P0-MODEL-CROSS-CONTRACT-001' 'docs/threat-model/model-provider-boundary.md' { param($t) $t -replace 'redirect','synthetic-follow' }
TextCase 'trace row omitted' 'P0-MODEL-INVENTORY-001' 'docs/prd-traceability.md' { param($t) $t -replace '(?m)^\| P0-MODEL-REQUEST-001 \|.*\r?\n','' }
TextCase 'fresh checker closure weakened' 'P0-MODEL-FRESH-CHECKER-001' 'docs/prd-traceability.md' { param($t) $t -replace 'Passed at P0-WI-10 closure','Required before closing P0-WI-10' }

if ($script:failures.Count -gt 0) {
    [Console]::Error.WriteLine(('FAIL: model-boundary synthetic mutations: ' + ($script:failures -join '; ')))
    exit 1
}

Write-Output ('PASS: model-boundary synthetic mutations ({0} rejection cases; OS temp only)' -f $script:caseCount)
