[CmdletBinding()]
param([string]$CheckerPath = 'tools/check-policy-state-boundary.ps1')

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) { throw 'Run inside the repository.' }

function Resolve-Repo([string]$Path) { if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }; return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path)) }
$checker = Resolve-Repo $CheckerPath
$inputs = @(
    'contracts/domain/policy-state-boundary.json','docs/adr/ADR-008-policy-and-state-model.md',
    'docs/threat-model/policy-state-boundary.md','docs/prd-traceability.md',
    'docs/product-spec.md','docs/implementation-plan.md',
    'contracts/governance/capabilities.json','contracts/privacy/persistence-boundary.json',
    'contracts/persistence/protected-state-boundary.json','contracts/evidence/identity-boundary.json',
    'contracts/model/provider-boundary.json','contracts/synchronization/mail-sync-boundary.json',
    'contracts/support/support-matrix.json','contracts/build-skeleton/skeleton.json'
)
$script:caseCount = 0
$script:failures = [Collections.Generic.List[string]]::new()

function New-SyntheticRoot {
    $root = Join-Path ([IO.Path]::GetTempPath()) ('openloops-policy-state-' + [guid]::NewGuid().ToString('N'))
    [IO.Directory]::CreateDirectory($root) | Out-Null
    foreach ($relative in $inputs) {
        $target = Join-Path $root $relative
        [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target)) | Out-Null
        Copy-Item -LiteralPath (Join-Path $repoRoot $relative) -Destination $target
    }
    return $root
}

function Invoke-SyntheticChecker([string]$Root) {
    $args = @('-NoProfile','-File',$checker,
        '-ManifestPath',(Join-Path $Root 'contracts/domain/policy-state-boundary.json'),
        '-AdrPath',(Join-Path $Root 'docs/adr/ADR-008-policy-and-state-model.md'),
        '-ThreatPath',(Join-Path $Root 'docs/threat-model/policy-state-boundary.md'),
        '-TraceabilityPath',(Join-Path $Root 'docs/prd-traceability.md'),
        '-ProductSpecPath',(Join-Path $Root 'docs/product-spec.md'),
        '-ImplementationPlanPath',(Join-Path $Root 'docs/implementation-plan.md'),
        '-GovernancePath',(Join-Path $Root 'contracts/governance/capabilities.json'),
        '-PrivacyPath',(Join-Path $Root 'contracts/privacy/persistence-boundary.json'),
        '-PersistencePath',(Join-Path $Root 'contracts/persistence/protected-state-boundary.json'),
        '-EvidencePath',(Join-Path $Root 'contracts/evidence/identity-boundary.json'),
        '-ModelPath',(Join-Path $Root 'contracts/model/provider-boundary.json'),
        '-SyncPath',(Join-Path $Root 'contracts/synchronization/mail-sync-boundary.json'),
        '-SupportPath',(Join-Path $Root 'contracts/support/support-matrix.json'),
        '-BuildPath',(Join-Path $Root 'contracts/build-skeleton/skeleton.json'),'-Quiet')
    $output = @(& pwsh @args 2>&1 | ForEach-Object { [string]$_ })
    return [pscustomobject]@{ Code=$LASTEXITCODE; Output=($output -join "`n") }
}

function JsonCase([string]$Name,[string]$ExpectedId,[string]$RelativePath,[scriptblock]$Mutate) {
    $script:caseCount++; $root=New-SyntheticRoot
    try {
        $path=Join-Path $root $RelativePath; $value=Get-Content -Raw -LiteralPath $path|ConvertFrom-Json -Depth 100
        & $Mutate $value
        $value|ConvertTo-Json -Depth 100|Set-Content -LiteralPath $path -Encoding utf8NoBOM
        $result=Invoke-SyntheticChecker $root
        if($result.Code-eq 0-or$result.Output-notmatch[regex]::Escape($ExpectedId)){$script:failures.Add("$Name (expected $ExpectedId; exit $($result.Code))")}
    } finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}
function TextCase([string]$Name,[string]$ExpectedId,[string]$RelativePath,[scriptblock]$Mutate) {
    $script:caseCount++; $root=New-SyntheticRoot
    try {
        $path=Join-Path $root $RelativePath; $changed=& $Mutate (Get-Content -Raw -LiteralPath $path)
        Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM
        $result=Invoke-SyntheticChecker $root
        if($result.Code-eq 0-or$result.Output-notmatch[regex]::Escape($ExpectedId)){$script:failures.Add("$Name (expected $ExpectedId; exit $($result.Code))")}
    } finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

$baseline=& pwsh -NoProfile -File $checker 2>&1
if($LASTEXITCODE-ne 0){throw ('Baseline policy/state checker failed: '+(@($baseline)-join"`n"))}

JsonCase 'work item drift' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.work_item='P0-WI-SYNTHETIC'}
JsonCase 'owner removed' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.owner_decisions=@()}
JsonCase 'requirement removed' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.owned_requirements=@($c.owned_requirements|Where-Object{$_-ne'OL-DUE-009'})}
JsonCase 'OL-REM-010 consumed requirement removed' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.consumed_requirements=@($c.consumed_requirements|Where-Object{$_-ne'OL-REM-010'})}
JsonCase 'OL-REM-017 consumed requirement removed' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.consumed_requirements=@($c.consumed_requirements|Where-Object{$_-ne'OL-REM-017'})}
JsonCase 'acceptance criterion added' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.acceptance_criteria+='AC-SYNTHETIC'}
JsonCase 'dependency marked passed' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.scenario_dependencies[0].status='passed'}
JsonCase 'gate removed' 'P0-POLICY-INVENTORY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.blocking_gates=@('G-AUTO')}

JsonCase 'loop field removed' 'P0-POLICY-SCHEMA-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.record_contracts[0].exact_fields_in_order=@($c.record_contracts[0].exact_fields_in_order|Where-Object{$_-ne'policy_version'})}
JsonCase 'deadline field added' 'P0-POLICY-SCHEMA-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.record_contracts[1].exact_fields_in_order+='readable_text'}
JsonCase 'nullable field widened' 'P0-POLICY-SCHEMA-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.record_contracts[0].nullable_fields+='loop_id'}
JsonCase 'collection bound widened' 'P0-POLICY-SCHEMA-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.record_contracts[0].collection_bounds.transition_ref_ids=4097}
JsonCase 'privacy schema mapping drift' 'P0-POLICY-SCHEMA-001' 'contracts/privacy/persistence-boundary.json' {param($c)($c.record_types|Where-Object id -eq 'loop').field_groups[0].fields=@('loop_id')}

JsonCase 'obligation enum extended' 'P0-POLICY-FACETS-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.facet_catalogs.obligation_state_code+='paused'}
JsonCase 'resolution reordered' 'P0-POLICY-FACETS-001' 'contracts/domain/policy-state-boundary.json' {param($c)[array]::Reverse($c.facet_catalogs.resolution_code)}
JsonCase 'precedence reversed' 'P0-POLICY-FACETS-001' 'contracts/domain/policy-state-boundary.json' {param($c)[array]::Reverse($c.facet_catalogs.primary_label_precedence)}

JsonCase 'legality rule removed' 'P0-POLICY-LEGALITY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.legality_rules=@($c.legality_rules|Select-Object -Skip 1)}
JsonCase 'candidate aging allowed' 'P0-POLICY-LEGALITY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.legality_rules[4]='candidate may age'}
JsonCase 'reminder terminalizes' 'P0-POLICY-LEGALITY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.legality_rules[8]='reminder completion closes obligation'}

JsonCase 'candidate auto-promotes' 'P0-POLICY-PROMOTION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.establishment_policy[5].state='open automatically'}
JsonCase 'history gains authority' 'P0-POLICY-PROMOTION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.establishment_policy[2].promise_rule='artifact authority'}
JsonCase 'model confidence promotes' 'P0-POLICY-LEGALITY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.legality_rules[10]='model confidence promotes'}

JsonCase 'deadline precision added' 'P0-POLICY-DEADLINE-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.facet_catalogs.deadline_precision_code+='quarter'}
JsonCase 'soft deadline ages' 'P0-POLICY-DEADLINE-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.deadline_policy.aging_rules[4]='soft ages immediately'}
JsonCase 'event relative ages unresolved' 'P0-POLICY-DEADLINE-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.deadline_policy.aging_rules[3]='event_relative always ages'}
JsonCase 'date mention mutates' 'P0-POLICY-DEADLINE-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.deadline_policy.operative_rules[3]='a date mention changes deadline'}

JsonCase 'command removed' 'P0-POLICY-COMMANDS-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.command_policy=@($c.command_policy|Where-Object id -ne 'keep_open')}
JsonCase 'fulfilled from candidate' 'P0-POLICY-CONFIRMATION-001' 'contracts/domain/policy-state-boundary.json' {param($c)($c.command_policy|Where-Object id -eq 'confirm_fulfilled').allowed_from=@('candidate','open')}
JsonCase 'decline becomes closed' 'P0-POLICY-COMMANDS-001' 'contracts/domain/policy-state-boundary.json' {param($c)($c.command_policy|Where-Object id -eq 'confirm_declined').result='terminal closed'}
JsonCase 'reminder changes obligation' 'P0-POLICY-CONFIRMATION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.reminder_boundary.obligation_projection='open becomes terminal'}

JsonCase 'idempotency reordered' 'P0-POLICY-IDEMPOTENCY-001' 'contracts/domain/policy-state-boundary.json' {param($c)[array]::Reverse($c.transition_policy.user_key_algorithm)}
JsonCase 'collision allowed' 'P0-POLICY-IDEMPOTENCY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.transition_policy.user_key_algorithm[2]='same key different payload applies'}
JsonCase 'stale command applies' 'P0-POLICY-IDEMPOTENCY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.transition_policy.user_key_algorithm[3]='stale expected version applies'}
JsonCase 'replay mutates' 'P0-POLICY-IDEMPOTENCY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.transition_policy.automated_key='replay appends transition'}

JsonCase 'suppression broadens' 'P0-POLICY-STALE-REOPEN-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.hypothesis_projection.identity='loop only'}
JsonCase 'later source suppressed' 'P0-POLICY-STALE-REOPEN-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.hypothesis_projection.suppression='keep_open suppresses all future evidence'}
JsonCase 'reopen loses history' 'P0-POLICY-STALE-REOPEN-001' 'contracts/domain/policy-state-boundary.json' {param($c)($c.command_policy|Where-Object id -eq 'reopen').result='open empty history'}
JsonCase 'old key reterminalizes' 'P0-POLICY-STALE-REOPEN-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.transition_policy.reopen_generation='old keys apply'}

JsonCase 'free text correction allowed' 'P0-POLICY-CORRECTION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.correction_policy.free_text='allowed'}
JsonCase 'correction trains model' 'P0-POLICY-CORRECTION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.correction_policy.future_rule='use correction for model training'}
JsonCase 'correction replay automatic' 'P0-POLICY-CORRECTION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.correction_policy.replay='automatic historical mutation'}
JsonCase 'duplicate merge lossy' 'P0-POLICY-CORRECTION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.correction_policy.duplicate='discard loser'}

JsonCase 'privacy prohibition removed' 'P0-POLICY-PRIVACY-001' 'contracts/privacy/persistence-boundary.json' {param($c)$c.forbidden_classes=@($c.forbidden_classes|Where-Object{$_-ne'prompt'})}
JsonCase 'diagnostic content allowed' 'P0-POLICY-PRIVACY-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.privacy_boundary.diagnostics='readable content allowed'}
JsonCase 'policy runtime enabled' 'P0-POLICY-CLAIMS-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.runtime_boundary.policy_runtime=$true}
JsonCase 'persistent record claimed' 'P0-POLICY-CLAIMS-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.runtime_boundary.persistent_records_created=@('loop')}
JsonCase 'capability claimed' 'P0-POLICY-CLAIMS-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.claims.capabilities_enabled=@('automatic')}

JsonCase 'ADR-009 ownership drift' 'P0-POLICY-DEFERRED-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.separate_decisions.reminder_adapters_operations_ownership_conflicts='ADR-008'}
JsonCase 'Graph operation added' 'P0-POLICY-DEFERRED-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.reminder_boundary.graph_office_permissions_calls_operations=@('synthetic')}
JsonCase 'automatic mutation enabled' 'P0-POLICY-CONFIRMATION-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.reminder_boundary.automatic_mutation='enabled'}

JsonCase 'governance gate passed' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' {param($c)($c.gates|Where-Object id -eq 'G-AUTO').status='passed'}
JsonCase 'automatic capability enabled' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' {param($c)($c.capabilities|Where-Object id -eq 'hybrid_reminder_mode').state='enabled'}
JsonCase 'logical runtime schema approved' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/persistence/protected-state-boundary.json' {param($c)$c.logical_schema_boundary.logical_runtime_schema_approved=$true}
JsonCase 'evidence unavailable closes' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/evidence/identity-boundary.json' {param($c)$c.unavailable_projection.closure_rule='unavailable proves failure'}
JsonCase 'model gains transition authority' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/model/provider-boundary.json' {param($c)$c.semantic_policy.never_authority_for=@($c.semantic_policy.never_authority_for|Where-Object{$_-ne'lifecycle transition'})}
JsonCase 'sync network contact claimed' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/synchronization/mail-sync-boundary.json' {param($c)$c.claims.network_contacts=@('synthetic.invalid')}
JsonCase 'support row advertised' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/support/support-matrix.json' {param($c)$c.claim_state.supported_rows=@('synthetic')}
JsonCase 'build durable state enabled' 'P0-POLICY-CROSS-CONTRACT-001' 'contracts/build-skeleton/skeleton.json' {param($c)$c.runtime_boundary.durable_state=$true}

TextCase 'ADR runtime activation' 'P0-POLICY-CROSS-CONTRACT-001' 'docs/adr/ADR-008-policy-and-state-model.md' {param($t)$t-replace'P0-WI-11 implements no policy runtime','P0-WI-11 implements policy runtime'}
TextCase 'threat control removed' 'P0-POLICY-CROSS-CONTRACT-001' 'docs/threat-model/policy-state-boundary.md' {param($t)$t-replace'Model output terminalizes a loop','Model output proposes a loop'}
TextCase 'trace row removed' 'P0-POLICY-INVENTORY-001' 'docs/prd-traceability.md' {param($t)$t-replace'(?m)^\| P0-POLICY-DEADLINE-001 \|.*\r?\n',''}
TextCase 'OL-REM-010 authoritative rule drift' 'P0-POLICY-INVENTORY-001' 'docs/product-spec.md' {param($t)$t-replace'development, test mode, and limited preview confirmation-first until G-AUTO passes','development and preview may enable hybrid before G-AUTO'}
TextCase 'OL-REM-017 prospective boundary drift' 'P0-POLICY-INVENTORY-001' 'docs/product-spec.md' {param($t)$t-replace'A mode change is prospective\.','A mode change rewrites historical reminders.'}
TextCase 'implementation-plan mode trace removed' 'P0-POLICY-INVENTORY-001' 'docs/implementation-plan.md' {param($t)$t-replace'Exact OL-REM-010/OL-REM-017 mode policy','Exact reminder mode policy'}
TextCase 'implementation plan generic runtime contradiction appended' 'P0-POLICY-INVENTORY-001' 'docs/implementation-plan.md' {param($t)$t+"`nP0-WI-11 has shipped policy runtime and automatic reminder writes.`n"}
TextCase 'product spec contradictory activation appended' 'P0-POLICY-INVENTORY-001' 'docs/product-spec.md' {param($t)$t+"`nContrary rule: hybrid is enabled before G-AUTO; automatic is the development default; historical reminders and inferred closure mutate automatically.`n"}
TextCase 'ADR contradictory runtime appended' 'P0-POLICY-CROSS-CONTRACT-001' 'docs/adr/ADR-008-policy-and-state-model.md' {param($t)$t+"`nContrary decision: P0-WI-11 implements policy runtime, auto-closes inferred loops, and performs reminder mutations.`n"}
TextCase 'trace contradictory claims appended' 'P0-POLICY-INVENTORY-001' 'docs/prd-traceability.md' {param($t)$t+"`nContrary trace: P0-POLICY-CLAIMS-001 is complete because policy runtime, automatic closure, reminder writes, support rows, and gates are active.`n"}
TextCase 'trace generic runtime contradiction appended' 'P0-POLICY-INVENTORY-001' 'docs/prd-traceability.md' {param($t)$t+"`nP0-WI-11 has shipped policy runtime and automatic reminder writes.`n"}
TextCase 'fresh checker closure weakened' 'P0-POLICY-FRESH-CHECKER-001' 'docs/prd-traceability.md' {param($t)$t-replace'Passed at P0-WI-11 closure','Passed without fresh judges'}

if($script:failures.Count-gt 0){[Console]::Error.WriteLine(('FAIL: policy/state synthetic mutations: '+($script:failures-join'; ')));exit 1}
Write-Output ('PASS: policy/state synthetic mutations ({0} rejection cases; OS temp only)' -f $script:caseCount)
