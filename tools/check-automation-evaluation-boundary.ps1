[CmdletBinding()]
param(
 [string]$ManifestPath='contracts/automation/evaluation-boundary.json',
 [string]$AdrPath='docs/adr/ADR-011-automation-and-evaluation.md',
 [string]$ThreatPath='docs/threat-model/automation-and-evaluation.md',
 [string]$TraceabilityPath='docs/prd-traceability.md',
 [string]$ProductSpecPath='docs/product-spec.md',
 [string]$ImplementationPlanPath='docs/implementation-plan.md',
 [string]$GovernancePath='contracts/governance/capabilities.json',
 [string]$ModelPath='contracts/model/provider-boundary.json',
 [string]$PolicyPath='contracts/domain/policy-state-boundary.json',
 [string]$ReminderPath='contracts/reminder/adapter-boundary.json',
 [string]$SupportPath='contracts/support/support-matrix.json',
 [string]$BuildPath='contracts/build-skeleton/skeleton.json',
 [switch]$Quiet
)
Set-StrictMode -Version Latest;$ErrorActionPreference='Stop'
$repoRoot=(& git rev-parse --show-toplevel 2>$null).Trim();if($LASTEXITCODE-ne 0-or[string]::IsNullOrWhiteSpace($repoRoot)){throw'Run inside the repository.'}
function Resolve-Input([string]$p){if([IO.Path]::IsPathRooted($p)){[IO.Path]::GetFullPath($p)}else{[IO.Path]::GetFullPath((Join-Path $repoRoot $p))}}
function Fail([string]$id){if(-not$script:seen.ContainsKey($id)){$script:seen[$id]=$true;$script:fail.Add($id)}}
function Json([string]$p,[string]$id){try{Get-Content -Raw -LiteralPath (Resolve-Input $p)|ConvertFrom-Json -Depth 100}catch{Fail $id;$null}}
function Text([string]$p,[string]$id){try{Get-Content -Raw -LiteralPath (Resolve-Input $p)}catch{Fail $id;''}}
function Exact([object[]]$a,[object[]]$e,[string]$id){$aa=@($a|ForEach-Object{[string]$_});$ee=@($e|ForEach-Object{[string]$_});$au=@($aa|Sort-Object -Unique);$eu=@($ee|Sort-Object -Unique);if($aa.Count-ne$au.Count-or$au.Count-ne$eu.Count-or(Compare-Object $eu $au)){Fail $id}}
function Ordered([object[]]$a,[object[]]$e,[string]$id){$aa=@($a|ForEach-Object{[string]$_});$ee=@($e|ForEach-Object{[string]$_});if($aa.Count-ne$ee.Count){Fail $id;return};for($i=0;$i-lt$ee.Count;$i++){if($aa[$i]-cne$ee[$i]){Fail $id;return}}}
function Empty([object[]]$v,[string]$id){if(@($v).Count-ne 0){Fail $id}}
function Has([string]$t,[string[]]$p,[string]$id){foreach($x in $p){if($t-notmatch[regex]::Escape($x)){Fail $id}}}
function NHash([string]$t){$c=(($t-replace"`r`n","`n").TrimEnd()+"`n");[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($c))).ToLowerInvariant()}
$script:fail=[Collections.Generic.List[string]]::new();$script:seen=@{}
$checks=@('P0-AUTOMATION-INVENTORY-001','P0-AUTOMATION-MODES-001','P0-AUTOMATION-STRATA-001','P0-AUTOMATION-FLAGS-001','P0-AUTOMATION-CORPUS-001','P0-AUTOMATION-CALIBRATION-001','P0-AUTOMATION-ROLLBACK-001','P0-AUTOMATION-PRIVACY-001','P0-AUTOMATION-CROSS-CONTRACT-001','P0-AUTOMATION-CLAIMS-001','P0-AUTOMATION-FRESH-CHECKER-001')

$m=Json $ManifestPath 'P0-AUTOMATION-INVENTORY-001';if($null-eq$m){[Console]::Error.WriteLine('BLOCKED: automation manifest parse failed.');exit 1}
$canonical=$m|ConvertTo-Json -Depth 100 -Compress;$hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
if($hash-ne'f663c8b519f132fd7371b88e7fdf37ce27194ce69e602198942843f5de968a13'){Fail 'P0-AUTOMATION-INVENTORY-001'}
if($m.schema_version-ne 1-or$m.work_item-ne'P0-WI-14'-or$m.adr-ne'ADR-011'-or$m.decision_status-ne'accepted_contract_runtime_unimplemented'-or$m.snapshot_date-ne'2026-07-21'){Fail 'P0-AUTOMATION-INVENTORY-001'}
Ordered @($m.owner_decisions) @('OWN-03') 'P0-AUTOMATION-INVENTORY-001'
Ordered @($m.owned_requirements) @('OL-REM-010','OL-REM-017') 'P0-AUTOMATION-INVENTORY-001'
Empty @($m.consumed_requirements) 'P0-AUTOMATION-INVENTORY-001'
Empty @($m.primary_acceptance_criteria) 'P0-AUTOMATION-INVENTORY-001'
Ordered @($m.dependent_acceptance_criteria) @('AC-10') 'P0-AUTOMATION-INVENTORY-001'
Ordered @($m.primary_scenarios) @('AS-21','AS-23') 'P0-AUTOMATION-INVENTORY-001'
Empty @($m.scenario_dependencies) 'P0-AUTOMATION-INVENTORY-001'
Ordered @($m.blocking_gates) @('G-AUTO','G-AUTO-FULL') 'P0-AUTOMATION-INVENTORY-001'
Ordered @($m.input_authorities.owner) @('ADR-005','ADR-007','ADR-008','ADR-009','ADR-PRIV-001') 'P0-AUTOMATION-INVENTORY-001'
Ordered @($m.sources.id) @('SRC-SPEC-DECISIONS','SRC-SPEC-OWNER','SRC-SPEC-REMINDER','SRC-SPEC-GATES','SRC-PLAN-EVAL','SRC-PLAN-ADR','SRC-ADR-007','SRC-ADR-008','SRC-ADR-009','SRC-THREAT-INDEX') 'P0-AUTOMATION-INVENTORY-001'
$sd=$m.separate_decisions
if($sd.reminder_adapter_operations_and_ownership-ne'ADR-009'-or$sd.local_facet_and_lifecycle_transitions-ne'ADR-008'-or$sd.provider_transport_and_drift_detection-ne'ADR-007'-or$sd.persisted_flag_record_encryption-ne'ADR-005'-or$sd.sealed_corpus_and_flag_record_privacy-ne'ADR-PRIV-001'-or$sd.review_link_and_bridge_activation-ne'ADR-010'-or$sd.self_email_send_recipient_and_operation_protocol-ne'ADR-013'){Fail 'P0-AUTOMATION-INVENTORY-001'}

# Requirement double-ownership: OL-REM-010/017 must be owned exactly once, here, and only consumed elsewhere.
$policyCheck=Json $PolicyPath 'P0-AUTOMATION-INVENTORY-001';$reminderCheck=Json $ReminderPath 'P0-AUTOMATION-INVENTORY-001'
if($null-ne$policyCheck){if(@($policyCheck.owned_requirements|Where-Object{$_-in@('OL-REM-010','OL-REM-017')}).Count-ne 0){Fail 'P0-AUTOMATION-INVENTORY-001'};if(@($policyCheck.consumed_requirements|Where-Object{$_-in@('OL-REM-010','OL-REM-017')}).Count-ne 2){Fail 'P0-AUTOMATION-INVENTORY-001'}}
if($null-ne$reminderCheck){if(@($reminderCheck.owned_requirements|Where-Object{$_-in@('OL-REM-010','OL-REM-017')}).Count-ne 0){Fail 'P0-AUTOMATION-INVENTORY-001'};if(@($reminderCheck.consumed_requirements|Where-Object{$_-in@('OL-REM-010','OL-REM-017')}).Count-ne 2){Fail 'P0-AUTOMATION-INVENTORY-001'}}

$c=$m.catalogs
Ordered @($c.mode_code) @('confirmation_first','hybrid','automatic') 'P0-AUTOMATION-MODES-001'
Ordered @($c.hybrid_create_category_code) @('explicit_promise','explicit_directly_addressed_request','explicit_deterministic_attribution') 'P0-AUTOMATION-MODES-001'
Ordered @($c.automatic_always_review_category_code) @('candidate','ambiguous','identity_ambiguous','delegation_ambiguous','quote_ambiguous','undated','historical_backfill','stale_write_conflict') 'P0-AUTOMATION-STRATA-001'
Ordered @($c.hybrid_update_field_scope_code) @('title','body','due') 'P0-AUTOMATION-MODES-001'
Ordered @($c.flag_code) @('hybrid_enabled','automatic_enabled') 'P0-AUTOMATION-FLAGS-001'
Ordered @($c.calibration_drift_trigger_code) @('provider_change','model_digest_or_label_change','schema_change','prompt_change','policy_change','review_threshold_change','category_change') 'P0-AUTOMATION-CALIBRATION-001'
Ordered @($c.reported_metric_code) @('macro_recall_precision_f2','per_stratum_recall_precision_f2','atomic_split_merge_errors','evidence_span_f1','attribution','deadline_normalization','dedup','association','review_routing','calibration','latency_cost','unintended_mutations','privacy_canaries') 'P0-AUTOMATION-PRIVACY-001'
if(@($c.automatic_always_review_category_code|Where-Object{$_-in@($c.hybrid_create_category_code)}).Count-ne 0){Fail 'P0-AUTOMATION-STRATA-001'}

Has ($m.mode_catalog|ConvertTo-Json -Depth 10 -Compress) @('every reminder creation requires an explicit user action','every evidence-driven service update to a Microsoft artifact requires an explicit user action','read-only reconciliation and local review-state updates still run automatically','live narrow-category cases only','valid resolved deadline, high calibrated confidence, deterministic identity','no quote/delegation/coreference ambiguity','OpenLoops-owned artifact fields only','no user override or conflict exists','every other mutation is proposed for confirmation','explicit opt-in and risk disclosure plus G-AUTO-FULL','every live established loop in the user''s enabled non-ambiguous categories','always require review in every mode','a mode change is prospective only','no mode silently creates or rewrites a historical batch, closes a loop, deletes an artifact, or communicates with another person','inferred closure and external communication are never automatic in any mode') 'P0-AUTOMATION-MODES-001'

$ft=$m.flag_topology
foreach($flag in @($ft.flags)){if($flag.default-ne$false){Fail 'P0-AUTOMATION-FLAGS-001'}}
$hy=@($ft.flags|Where-Object id -eq 'hybrid_enabled');$au=@($ft.flags|Where-Object id -eq 'automatic_enabled')
if($hy.Count-ne 1-or($hy[0].flip_requires-notcontains'G-AUTO passed')-or$au.Count-ne 1-or($au[0].flip_requires-notcontains'G-AUTO-FULL passed')-or($au[0].flip_requires-notcontains'user explicit opt-in and risk acknowledgement')){Fail 'P0-AUTOMATION-FLAGS-001'}
Ordered @($ft.prohibited_flip_paths) @('code default','configuration drift','mode-change UI alone','settings preference alone') 'P0-AUTOMATION-FLAGS-001'
Has ($ft.effective_mode_rule) @('narrower of the user''s selected preference','modes currently permitted by passed-gate evidence and flipped flags') 'P0-AUTOMATION-FLAGS-001'

Has ($m.corpus_governance|ConvertTo-Json -Depth 10 -Compress) @('outside the repository, outside the implementation-LLM''s context, and outside the maker workflow','wholly synthetic; never real mailbox content','at least 30% of held examples','at least 20% of held examples','split by conversation family, never by message row','no family straddles a split','declared before evaluation','a point estimate alone never substitutes for a declared lower bound','immutable once sealed','maximum tuning-attempt budget is pinned before evaluation begins','requires a fresh corpus version, never a silent extra round','no sealed-corpus example, label, or identifier reaches a public development fixture','strictly as a finite-sample observation, never a guarantee') 'P0-AUTOMATION-CORPUS-001'

Has ($m.calibration_invalidation|ConvertTo-Json -Depth 10 -Compress) @('provider change','model digest or label change','schema change','prompt change','policy change','review threshold change','category change','invalidates the corresponding calibration','any already-flipped flag reverts to confirmation_first pending re-evaluation','only an explicit review-only replay is permitted','no automatic terminal, Graph, Office, or reminder mutation follows a drift-triggered reversion') 'P0-AUTOMATION-CALIBRATION-001'

Ordered @($m.rollback_boundary.properties) @('instant','unilateral','lossless','prospective_only') 'P0-AUTOMATION-ROLLBACK-001'
Has ($m.rollback_boundary|ConvertTo-Json -Depth 10 -Compress) @('regardless of flag state, gate status, or pending evaluation','historical batch mutation','recreation of a prior artifact','deletion of a prior artifact','mutation of a prior loop transition','required additional confirmation or owner approval to take effect') 'P0-AUTOMATION-ROLLBACK-001'

Has ($m.evaluation_privacy_boundary|ConvertTo-Json -Depth 10 -Compress) @('corpus content','label','prediction','prompt','transcript','model output','rationale','real fixture sample','repository','package','diagnostic artifact','log','sanitized aggregate counts','stable non-content identifiers','no reported metric implies persistent user profiling') 'P0-AUTOMATION-PRIVACY-001'
Ordered @($m.evaluation_privacy_boundary.prohibited_values) @('corpus content','label','prediction','prompt','transcript','model output','rationale','real fixture sample') 'P0-AUTOMATION-PRIVACY-001'

if($m.mode_change_boundary.prospective_only-ne$true){Fail 'P0-AUTOMATION-STRATA-001'}
Ordered @($m.mode_change_boundary.prohibited) @('retroactive historical batch creation or rewrite','loop closure','artifact deletion','communication with another person') 'P0-AUTOMATION-STRATA-001'
$pg=$m.mode_change_boundary.phase_gating
if($pg.development_test_limited_preview-notmatch'confirmation_first'-or$pg.development_test_limited_preview-notmatch'G-AUTO'-or$pg.full_mvp_new_install_default-notmatch'hybrid'-or$pg.full_mvp_new_install_default-notmatch'G-AUTO'-or$pg.fully_automatic-notmatch'G-AUTO-FULL'-or$pg.fully_automatic-notmatch'opt-in'){Fail 'P0-AUTOMATION-STRATA-001'}
if($m.mode_change_boundary.fallback-notmatch'instantly and unilaterally'){Fail 'P0-AUTOMATION-ROLLBACK-001'}

# Cross-contract reconciliation.
$model=Json $ModelPath 'P0-AUTOMATION-CROSS-CONTRACT-001'
if($null-ne$model){Has ($model.semantic_policy|ConvertTo-Json -Depth 10 -Compress) @('unavailable until separate ADR-011 plus G-AUTO or G-AUTO-FULL','drift invalidates automation calibration and permits review-only replay only') 'P0-AUTOMATION-CROSS-CONTRACT-001';if($model.runtime_boundary.provider_transport-ne$false){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'}}
$policy=Json $PolicyPath 'P0-AUTOMATION-CROSS-CONTRACT-001'
if($null-ne$policy){if($policy.separate_decisions.automation_modes_eligibility_evaluation_and_feature_flags-ne'ADR-011'){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'};Has ($policy.reminder_boundary|ConvertTo-Json -Depth 10 -Compress) @('prohibited pending ADR-009 ADR-011') 'P0-AUTOMATION-CROSS-CONTRACT-001'}
$reminder=Json $ReminderPath 'P0-AUTOMATION-CROSS-CONTRACT-001'
if($null-ne$reminder){if($reminder.separate_decisions.mode_semantics_eligibility_evaluation_flags_and_rollback-ne'ADR-011'){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'};$rmatrix=$reminder.review_link_boundary|ConvertTo-Json -Depth 10 -Compress}
$gov=Json $GovernancePath 'P0-AUTOMATION-CROSS-CONTRACT-001'
if($null-ne$gov){
 $a11=@($gov.adrs|Where-Object{$_.id-eq'ADR-011'});$a12=@($gov.adrs|Where-Object{$_.id-eq'ADR-012'});$planned=@($gov.adrs|Where-Object{$_.id-in@('ADR-013')})
 if($a11.Count-ne 1-or$a11[0].status-ne'accepted'-or$a12.Count-ne 1-or$a12[0].status-ne'accepted'-or@($planned|Where-Object{$_.status-ne'accepted'}).Count-ne 0){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'}
 $own03=@($gov.owner_decisions|Where-Object{$_.id-eq'OWN-03'});if($own03.Count-ne 1-or($own03[0].adrs-notcontains'ADR-011')-or$own03[0].status-ne'accepted'){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'}
 if(@($gov.gates|Where-Object{$_.id-in$m.blocking_gates-and$_.status-ne'unrun'}).Count-ne 0){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'}
 $hybridCap=@($gov.capabilities|Where-Object{$_.id-eq'hybrid_reminder_mode'});$autoCap=@($gov.capabilities|Where-Object{$_.id-eq'fully_automatic_reminder_mode'})
 if($hybridCap.Count-ne 1-or$hybridCap[0].state-ne'disabled'-or$hybridCap[0].advertised-ne$false-or$autoCap.Count-ne 1-or$autoCap[0].state-ne'disabled'-or$autoCap[0].advertised-ne$false){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'}
}
$support=Json $SupportPath 'P0-AUTOMATION-CROSS-CONTRACT-001'
if($null-ne$support){foreach($n in @('supported_rows','enabled_capabilities','advertised_capabilities','gates_passed','acceptance_criteria_completed')){Empty @($support.claim_state.$n) 'P0-AUTOMATION-CROSS-CONTRACT-001'}}
$build=Json $BuildPath 'P0-AUTOMATION-CROSS-CONTRACT-001'
if($null-ne$build){if($build.runtime_boundary.model_provider-ne$false-or$build.runtime_boundary.durable_state-ne$false){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'};Empty @($build.runtime_boundary.network_origins) 'P0-AUTOMATION-CROSS-CONTRACT-001'}

$rt=$m.runtime_boundary
if($rt.hybrid_enabled-ne$false-or$rt.automatic_enabled-ne$false-or$rt.evaluation_run-ne$false-or$rt.corpus_created-ne$false-or$rt.tuning_rounds_used-ne 0){Fail 'P0-AUTOMATION-CLAIMS-001'}
foreach($n in @('flags_flipped','acceptance_criteria_completed','scenarios_completed','gates_passed')){Empty @($rt.$n) 'P0-AUTOMATION-CLAIMS-001'}
foreach($n in @('capabilities_enabled','capabilities_advertised','modes_advertised','corpus_results_reported','calibration_proven_strata')){Empty @($m.claims.$n) 'P0-AUTOMATION-CLAIMS-001'}

$adr=Text $AdrPath 'P0-AUTOMATION-CROSS-CONTRACT-001';$threat=Text $ThreatPath 'P0-AUTOMATION-CROSS-CONTRACT-001';$spec=Text $ProductSpecPath 'P0-AUTOMATION-INVENTORY-001';$plan=Text $ImplementationPlanPath 'P0-AUTOMATION-INVENTORY-001';$trace=Text $TraceabilityPath 'P0-AUTOMATION-INVENTORY-001'
if((NHash $adr)-ne'77dd1396da6f88097bb638ce0677b0f6eba904e0350b8f9fa4e638c3c586baf2'){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'}
if((NHash $threat)-ne'b81bc7f32a8a727e0c840aba2c36a2c45d86f6597b3a5794a419d94bf4bf3100'){Fail 'P0-AUTOMATION-CROSS-CONTRACT-001'}
if((NHash $spec)-ne'c0a7718d78bde3804142e5b79a937a8d66a372bfe35406e2c8b2ce96d4c7ed54'){Fail 'P0-AUTOMATION-INVENTORY-001'}
if((NHash $plan)-ne'bc857151555cf9be61a0e075c40907e5b9750ac62293b9a5bddf13efe6cbe575'){Fail 'P0-AUTOMATION-INVENTORY-001'}
if((NHash $trace)-ne'a5dc80035f1e0b447ec1f1c2f2ed3896eb9d433bd87a515a5c473878c3062fea'){Fail 'P0-AUTOMATION-INVENTORY-001'}
Has $adr @('Status:** Accepted','P0-WI-14','OWN-03','G-AUTO, G-AUTO-FULL','alone owns OL-REM-017 execution, eligibility strata, evaluation, feature','flags, G-AUTO/G-AUTO-FULL evidence, and rollback to confirmation-first','A mode change is prospective only','No mode silently creates or rewrites a','invalidates the corresponding calibration','instantly, unilaterally, and','without running an evaluation, creating a corpus, flipping a flag, or') 'P0-AUTOMATION-CROSS-CONTRACT-001'
Has $threat @('Automation and evaluation','Eligibility creep','Calibration rot after provider/model/schema/prompt/policy/threshold/category drift','Sealed judge-corpus contamination or leakage into maker/implementation-LLM context','Flag flip without independent gate evidence','Retroactive batch mutation on mode change or replay','Rollback to confirmation-first failing or being conditional','Judge-corpus content, labels, or predictions leaking into the repository or diagnostics','Semantic manipulation that passes schema validation but is unsafe','Sample sizes too small for the claimed 95% lower-bound precision','Inferred closure or external communication becoming automatic in any mode') 'P0-AUTOMATION-CROSS-CONTRACT-001'
$ids=[regex]::Matches($trace,'(?m)^\| (P0-AUTOMATION-[A-Z-]+-001) \|')|ForEach-Object{$_.Groups[1].Value};Exact @($ids) $checks 'P0-AUTOMATION-INVENTORY-001';foreach($id in $checks){if(([regex]::Matches($trace,'\b'+[regex]::Escape($id)+'\b')).Count-ne 1){Fail 'P0-AUTOMATION-INVENTORY-001'}}
$fresh=[regex]::Match($trace,'(?m)^\| P0-AUTOMATION-FRESH-CHECKER-001 \|.*$').Value
Has $fresh @('Fresh-context safety, evaluation-integrity, governance, and adversarial judges','no prompt, transcript, or model output stored','Passed at P0-WI-14 closure') 'P0-AUTOMATION-FRESH-CHECKER-001'

if($script:fail.Count-gt 0){[Console]::Error.WriteLine(('FAIL: automation and evaluation boundary: '+(($script:fail|Sort-Object)-join', ')));exit 1};if(-not$Quiet){Write-Output ('PASS: automation and evaluation boundary ({0} checks; runtime, evaluation, corpus, and claims disabled)' -f $checks.Count)}
