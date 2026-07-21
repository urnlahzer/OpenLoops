[CmdletBinding()]
param(
    [string]$ManifestPath = 'contracts/domain/policy-state-boundary.json',
    [string]$AdrPath = 'docs/adr/ADR-008-policy-and-state-model.md',
    [string]$ThreatPath = 'docs/threat-model/policy-state-boundary.md',
    [string]$TraceabilityPath = 'docs/prd-traceability.md',
    [string]$ProductSpecPath = 'docs/product-spec.md',
    [string]$ImplementationPlanPath = 'docs/implementation-plan.md',
    [string]$GovernancePath = 'contracts/governance/capabilities.json',
    [string]$PrivacyPath = 'contracts/privacy/persistence-boundary.json',
    [string]$PersistencePath = 'contracts/persistence/protected-state-boundary.json',
    [string]$EvidencePath = 'contracts/evidence/identity-boundary.json',
    [string]$ModelPath = 'contracts/model/provider-boundary.json',
    [string]$SyncPath = 'contracts/synchronization/mail-sync-boundary.json',
    [string]$SupportPath = 'contracts/support/support-matrix.json',
    [string]$BuildPath = 'contracts/build-skeleton/skeleton.json',
    [switch]$Quiet
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) { throw 'Run inside the repository.' }

function Resolve-Input([string]$Path) { if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }; return [IO.Path]::GetFullPath((Join-Path $repoRoot $Path)) }
function Fail([string]$Id) { if (-not $script:failureSet.ContainsKey($Id)) { $script:failureSet[$Id] = $true; $script:failures.Add($Id) } }
function Exact([object[]]$Actual,[object[]]$Expected,[string]$Id) { $a=@($Actual|ForEach-Object{[string]$_});$e=@($Expected|ForEach-Object{[string]$_});$au=@($a|Sort-Object -Unique);$eu=@($e|Sort-Object -Unique);if($a.Count-ne$au.Count-or$au.Count-ne$eu.Count-or(Compare-Object $eu $au)){Fail $Id} }
function ExactOrdered([object[]]$Actual,[object[]]$Expected,[string]$Id) { $a=@($Actual|ForEach-Object{[string]$_});$e=@($Expected|ForEach-Object{[string]$_});if($a.Count-ne$e.Count){Fail $Id;return};for($i=0;$i-lt$e.Count;$i++){if($a[$i]-cne$e[$i]){Fail $Id;return}} }
function Empty([object[]]$Values,[string]$Id) { if(@($Values).Count-ne 0){Fail $Id} }
function Has([string]$Text,[string[]]$Phrases,[string]$Id) { foreach($phrase in $Phrases){if($Text-notmatch[regex]::Escape($phrase)){Fail $Id}} }
function Normalized-Hash([string]$Text) { $canonical=(($Text-replace"`r`n","`n").TrimEnd()+"`n");return [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant() }
function Read-Json([string]$Path,[string]$Id) { try{return Get-Content -Raw -LiteralPath (Resolve-Input $Path)|ConvertFrom-Json -Depth 100}catch{Fail $Id;return $null} }
function Read-Text([string]$Path,[string]$Id) { try{return Get-Content -Raw -LiteralPath (Resolve-Input $Path)}catch{Fail $Id;return ''} }

$script:failures=[Collections.Generic.List[string]]::new();$script:failureSet=@{}
$checks=@('P0-POLICY-INVENTORY-001','P0-POLICY-SCHEMA-001','P0-POLICY-FACETS-001','P0-POLICY-LEGALITY-001','P0-POLICY-PROMOTION-001','P0-POLICY-DEADLINE-001','P0-POLICY-COMMANDS-001','P0-POLICY-CONFIRMATION-001','P0-POLICY-IDEMPOTENCY-001','P0-POLICY-STALE-REOPEN-001','P0-POLICY-CORRECTION-001','P0-POLICY-PRIVACY-001','P0-POLICY-DEFERRED-001','P0-POLICY-CROSS-CONTRACT-001','P0-POLICY-CLAIMS-001','P0-POLICY-FRESH-CHECKER-001')
$m=Read-Json $ManifestPath 'P0-POLICY-INVENTORY-001'
if($null-eq$m){[Console]::Error.WriteLine('BLOCKED: P0-POLICY-INVENTORY-001 manifest parse failed.');exit 1}
$canonical=$m|ConvertTo-Json -Depth 100 -Compress
$hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
if($hash-ne'90a57be4fd97e3d1a1832b592cca9f491577d209d000b70a3a50ba36189b0203'){Fail 'P0-POLICY-INVENTORY-001'}

if($m.schema_version-ne 1-or$m.work_item-ne'P0-WI-11'-or$m.adr-ne'ADR-008'-or$m.decision_status-ne'accepted_contract_runtime_unimplemented'-or$m.snapshot_date-ne'2026-07-20'){Fail 'P0-POLICY-INVENTORY-001'}
ExactOrdered @($m.owner_decisions) @('OWN-03') 'P0-POLICY-INVENTORY-001'
ExactOrdered @($m.owned_requirements) @('OL-GOV-001','OL-DUE-001','OL-DUE-002','OL-DUE-003','OL-DUE-004','OL-DUE-005','OL-DUE-006','OL-DUE-007','OL-DUE-008','OL-DUE-009','OL-CLOSE-003','OL-CLOSE-004','OL-CLOSE-005','OL-CLOSE-006','OL-CLOSE-007','OL-CLOSE-008','OL-DELEG-001','OL-DELEG-002') 'P0-POLICY-INVENTORY-001'
ExactOrdered @($m.consumed_requirements) @('OL-DET-001','OL-DET-002','OL-DET-003','OL-DET-004','OL-DET-005','OL-DET-006','OL-DET-007','OL-CLOSE-001','OL-CLOSE-002','OL-REM-010','OL-REM-017') 'P0-POLICY-INVENTORY-001'
ExactOrdered @($m.acceptance_criteria) @('AC-08','AC-09','AC-13','AC-15','AC-16','AC-17') 'P0-POLICY-INVENTORY-001'
ExactOrdered @($m.primary_scenarios) @('AS-03','AS-04','AS-06','AS-08','AS-09','AS-20') 'P0-POLICY-INVENTORY-001'
ExactOrdered @($m.scenario_dependencies.id) @('AS-07','AS-10','AS-13','AS-15','AS-16','AS-17','AS-19','AS-21','AS-23') 'P0-POLICY-INVENTORY-001'
if(@($m.scenario_dependencies|Where-Object{$_.status-notlike'unpassed_*'}).Count-ne 0){Fail 'P0-POLICY-INVENTORY-001'}
ExactOrdered @($m.blocking_gates) @('G-AUTO','G-AUTO-FULL') 'P0-POLICY-INVENTORY-001'
ExactOrdered @($m.input_authorities.owner) @('ADR-004','ADR-006','ADR-007','ADR-PRIV-001','ADR-005') 'P0-POLICY-INVENTORY-001'
ExactOrdered @($m.sources.id) @('SRC-SPEC-DOMAIN','SRC-SPEC-DEADLINE','SRC-SPEC-CLOSURE','SRC-SPEC-REMINDER','SRC-SPEC-FLOWS','SRC-PLAN-INVARIANTS','SRC-PLAN-AUTOMATION','SRC-OWN-03','SRC-RESEARCH-SECURITY','SRC-RESEARCH-VALIDATION') 'P0-POLICY-INVENTORY-001'
$specText=Read-Text $ProductSpecPath 'P0-POLICY-INVENTORY-001';$planText=Read-Text $ImplementationPlanPath 'P0-POLICY-INVENTORY-001'
if((Normalized-Hash $specText)-ne'c0a7718d78bde3804142e5b79a937a8d66a372bfe35406e2c8b2ce96d4c7ed54'){Fail 'P0-POLICY-INVENTORY-001'}
if((Normalized-Hash $planText)-ne'24d7f6a978f62b7d7a2eca0314361df84628ca515ee957593fa77f83f3b2e93f'){Fail 'P0-POLICY-INVENTORY-001'}
Has $specText @('OL-REM-010 MUST','development, test mode, and limited preview confirmation-first until G-AUTO passes','Fully automatic mode MUST remain unavailable until G-AUTO-FULL passes','Inferred closure and external communication are never automatic in any mode.','OL-REM-017 MUST','`confirmation_first`','`hybrid`','`automatic`','A mode change is prospective.','No mode silently creates or rewrites a historical batch, closes a loop, deletes an artifact, or communicates with another person.') 'P0-POLICY-INVENTORY-001'
Has $planText @('Exact OL-REM-010/OL-REM-017 mode policy and separate feature flags','keep hybrid disabled until G-AUTO and automatic disabled until G-AUTO-FULL','make hybrid the full-MVP new-install default under OWN-03') 'P0-POLICY-INVENTORY-001'

$records=@($m.record_contracts)
ExactOrdered @($records.id) @('loop','deadline_evidence','transition') 'P0-POLICY-SCHEMA-001'
$loop=@($records|Where-Object id -eq 'loop');$deadline=@($records|Where-Object id -eq 'deadline_evidence');$transition=@($records|Where-Object id -eq 'transition')
if($loop.Count-ne 1-or$deadline.Count-ne 1-or$transition.Count-ne 1){Fail 'P0-POLICY-SCHEMA-001'}
if($loop.Count-eq 1){
 ExactOrdered @($loop[0].exact_fields_in_order) @('loop_id','account_ref','origin_code','provenance_codes','obligation_state_code','review_flag_codes','deadline_state_code','closure_review_state_code','reminder_state_code','analysis_state_code','source_state_code','resolution_code','confidence_bucket','source_ref_ids_by_role','operative_deadline_ref_id','operative_deadline_value_or_range','reminder_ref_ids','transition_ref_ids','policy_version','schema_version','model_label_code','loop_version','last_evaluated_source_ref_id') 'P0-POLICY-SCHEMA-001'
 Exact @($loop[0].nullable_fields) @('operative_deadline_ref_id','operative_deadline_value_or_range','last_evaluated_source_ref_id') 'P0-POLICY-SCHEMA-001'
 if($loop[0].unknown_fields-ne'reject'-or$loop[0].collection_bounds.provenance_codes-ne 6-or$loop[0].collection_bounds.review_flag_codes-ne 7-or$loop[0].collection_bounds.source_ref_ids_by_role-ne 256-or$loop[0].collection_bounds.reminder_ref_ids-ne 16-or$loop[0].collection_bounds.transition_ref_ids-ne 4096){Fail 'P0-POLICY-SCHEMA-001'}
}
if($deadline.Count-eq 1){ExactOrdered @($deadline[0].exact_fields_in_order) @('deadline_evidence_id','loop_id','source_ref_id','source_type_code','precision_code','interpretation_code','operative_selection_code','value_or_range','timezone_id') 'P0-POLICY-SCHEMA-001';Has ($deadline[0]|ConvertTo-Json -Compress) @('required except for explicit user_supplied','1 through 128 bytes','no generic string map JSON or extension member') 'P0-POLICY-SCHEMA-001'}
if($transition.Count-eq 1){ExactOrdered @($transition[0].exact_fields_in_order) @('transition_id','loop_id','user_action_id','prior_facet_codes','new_facet_codes','reason_code','actor_code','source_ref_ids','occurred_at','loop_version','policy_version') 'P0-POLICY-SCHEMA-001';if($transition[0].source_ref_bound-ne 32-or$transition[0].unknown_fields-ne'reject'){Fail 'P0-POLICY-SCHEMA-001'};Has ($transition[0]|ConvertTo-Json -Compress) @('required for actor user and prohibited for actor system','complete prior and new facet tuples','no copied rationale free text') 'P0-POLICY-SCHEMA-001'}

$f=$m.facet_catalogs
ExactOrdered @($f.origin_code) @('email_evidence','calendar_invitation','user_authored_microsoft_artifact') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.provenance_codes) @('requested','acknowledged','promised','attributed','inferred','ambiguous') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.obligation_state_code) @('candidate','open','terminal') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.review_flag_codes) @('needs_review','historical_backfill','identity_ambiguous','association_ambiguous','quote_ambiguous','deadline_ambiguous','delegation_ambiguous') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.deadline_state_code) @('unresolved','undated_confirmed','scheduled','approaching','overdue') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.closure_review_state_code) @('none','possible','kept_open','evidence_requested') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.reminder_state_code) @('none','proposed','pending_write','linked','changed','completed_needs_evidence','missing','conflict','ambiguous_write') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.analysis_state_code) @('current','queued','unavailable','quarantined','stale') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.source_state_code) @('available','partially_available','unavailable','changed') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.resolution_code) @('none','closed','declined','delegated','moot','dismissed','completed_outside_email') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.confidence_bucket) @('high','medium','low') 'P0-POLICY-FACETS-001'
ExactOrdered @($f.primary_label_precedence) @('terminal_resolution','possible_closure','needs_review','overdue','approaching_deadline','needs_deadline','candidate','active') 'P0-POLICY-FACETS-001'

if(@($m.legality_rules).Count-ne 12){Fail 'P0-POLICY-LEGALITY-001'}
Has (($m.legality_rules|ConvertTo-Json -Compress)) @('resolution none if and only if','never produce terminal','require an open obligation','require an open obligation plus a resolved operational boundary','candidate never ages','undated_confirmed requires an explicit user transition','quarantined requires an encrypted re-fetchable locator','freezes evidence-dependent automation','reminder completion deletion missing conflict or ambiguity never changes obligation state','later evidence on a terminal loop','model confidence alone never promotes','user correction dominates') 'P0-POLICY-LEGALITY-001'
$legal=0
foreach($o in @($f.obligation_state_code)){foreach($r in @($f.resolution_code)){foreach($c in @($f.closure_review_state_code)){foreach($d in @($f.deadline_state_code)){
 $ok=(($o-eq'terminal')-eq($r-ne'none'))
 if($c-ne'none'-and$o-ne'open'){$ok=$false}
 if($d-in@('approaching','overdue')-and$o-ne'open'){$ok=$false}
 if($ok){$legal++}
}}}}
if($legal-ne 41){Fail 'P0-POLICY-LEGALITY-001'}

ExactOrdered @($m.establishment_policy.case) @('validated explicit outgoing promise; medium or high; no core ambiguity','validated direct incoming request or question; deterministic identity; medium or high; no core ambiguity','validated acknowledgement of associated request','validated explicit deterministic attribution; high; no transfer ambiguity','calendar invitation','soft implied social contextual role low-confidence or core-ambiguous','user confirms candidate mine and actionable','invalid or ungrounded hypothesis') 'P0-POLICY-PROMOTION-001'
ExactOrdered @($m.establishment_policy.state) @('open','open','open','open','open_only_after_G_CAL_and_G_MAIL_input','candidate','open','unchanged') 'P0-POLICY-PROMOTION-001'
Has (($m.establishment_policy|ConvertTo-Json -Compress)) @('review_only_no_artifact_authority','add promised only with supported future-act evidence','open_only_after_G_CAL_and_G_MAIL_input','needs_review plus exact ambiguity flags','append transition preserve provenance','state":"unchanged') 'P0-POLICY-PROMOTION-001'

$dp=$m.deadline_policy
ExactOrdered @($f.deadline_source_type_code) @('requested','promised','inferred','user_supplied') 'P0-POLICY-DEADLINE-001'
ExactOrdered @($f.deadline_precision_code) @('instant','date','business_day','week','event_relative','soft','unspecified') 'P0-POLICY-DEADLINE-001'
ExactOrdered @($f.deadline_interpretation_code) @('resolved','ambiguous','needs_user_input') 'P0-POLICY-DEADLINE-001'
ExactOrdered @($f.operative_selection_code) @('not_selected','selected','superseded') 'P0-POLICY-DEADLINE-001'
if(@($dp.operative_rules).Count-ne 8-or@($dp.aging_rules).Count-ne 6){Fail 'P0-POLICY-DEADLINE-001'}
ExactOrdered @($dp.aging_rules) @('instant ages against its evidence-derived instant','date and business_day age against a configured policy boundary without upgrading evidence precision','week ages against retained range end and displays the range','event_relative ages only after unique event correlation','soft unspecified and undated_confirmed never age','terminal and candidate do not newly age; open alone projects approaching or overdue') 'P0-POLICY-DEADLINE-001'
ExactOrdered @($dp.prompt_commands) @('set_deadline','no_deadline','defer_reminder','dismiss','not_mine') 'P0-POLICY-DEADLINE-001'
Has ($dp|ConvertTo-Json -Depth 10 -Compress) @('remain distinct','17:00 user-local','requires confirmation','a date mention without directive','unresolved event-relative','soft urgency is scheduled without an aging boundary','without upgrading evidence precision','terminal and candidate do not newly age','suppresses the current prompt version only') 'P0-POLICY-DEADLINE-001'

$commands=@($m.command_policy)
ExactOrdered @($commands.id) @('confirm_mine_actionable','confirm_fulfilled','confirm_declined','confirm_moot','confirm_transfer','keep_open','request_evidence','remap_evidence','select_different_evidence','completed_outside_email','dismiss_classify','reopen') 'P0-POLICY-COMMANDS-001'
Has (($commands|ConvertTo-Json -Compress)) @('one or more validated closure evidence refs','terminal declined distinct from closed dismissed and moot','terminal delegated only for transferred','shared assisted retained or unclear stays open','exact current hypothesis identity','atomic relation detach and attach','without fabricated evidence','terminal dismissed except typed transferred or moot outcomes','prior history retained') 'P0-POLICY-COMMANDS-001'
$terminalCommands=@($commands|Where-Object{$_.result-like'terminal*'-or$_.id-eq'reopen'})
if($terminalCommands.Count-ne 7){Fail 'P0-POLICY-CONFIRMATION-001'}
foreach($confirmation in @(
 @{id='confirm_fulfilled';phrase='explicit user action'},
 @{id='confirm_declined';phrase='manual decline confirmation'},
 @{id='confirm_moot';phrase='explicit manual choice'},
 @{id='confirm_transfer';phrase='manual confirmation of full transfer'},
 @{id='completed_outside_email';phrase='explicit user confirmation'},
 @{id='dismiss_classify';phrase='explicit user action'},
 @{id='reopen';phrase='explicit user action'}
)){
 $command=@($commands|Where-Object id -eq $confirmation.id)
 if($command.Count-ne 1-or$command[0].required-notlike('*'+$confirmation.phrase+'*')){Fail 'P0-POLICY-CONFIRMATION-001'}
}
$fulfilled=@($commands|Where-Object id -eq 'confirm_fulfilled');$declined=@($commands|Where-Object id -eq 'confirm_declined');$transfer=@($commands|Where-Object id -eq 'confirm_transfer')
if($fulfilled.Count-ne 1-or(Compare-Object @('open') @($fulfilled[0].allowed_from))-or$declined.Count-ne 1-or$transfer.Count-ne 1){Fail 'P0-POLICY-CONFIRMATION-001'}
Has (($m.reminder_boundary|ConvertTo-Json -Compress)) @('changes reminder facet only','candidate stays candidate open stays open and terminal stays terminal','commits independently','remote failure never rolls back','automatic_mutation','prohibited pending ADR-009 ADR-011') 'P0-POLICY-CONFIRMATION-001'

$tp=$m.transition_policy
ExactOrdered @($tp.user_key_algorithm) @('lookup opaque command key before version comparison','same key and same canonical payload returns original result with zero mutation','same key and different payload is command_key_collision','unseen key with stale expected loop version is visible_refresh_conflict','validate current state evidence and command preconditions','append exactly one transition and increment loop version atomically') 'P0-POLICY-IDEMPOTENCY-001'
Has ($tp.automated_key) @('validated source identity and source version','policy version','target loop','replay is a no-op') 'P0-POLICY-IDEMPOTENCY-001'
Has (($m.hypothesis_projection|ConvertTo-Json -Compress)) @('target loop plus hypothesis kind','ordered validated source references including source versions plus policy version','possible if any unsuppressed current hypothesis exists','evidence_requested if no possible hypothesis','kept_open if no possible or requested hypothesis','causally later source version or a different loop kind source tuple or policy version remains reviewable') 'P0-POLICY-STALE-REOPEN-001'
Has (($commands|Where-Object id -eq 'reopen'|ConvertTo-Json -Compress)) @('open resolution none closure review none needs_review added prior history retained','explicit user action') 'P0-POLICY-STALE-REOPEN-001'
Has $tp.reopen_generation @('old command keys remain historical no-ops','cannot re-terminal') 'P0-POLICY-STALE-REOPEN-001'

$cp=$m.correction_policy
ExactOrdered @($cp.reason_codes) @('not_mine','false_detection','duplicate','delegated','shared','assisted','retained','moot','no_longer_relevant','wrong_deadline','wrong_client','other') 'P0-POLICY-CORRECTION-001'
Has ($cp|ConvertTo-Json -Compress) @('free_text":"prohibited','separate commands and separate confirmations','never model training personalization or hidden profile','bounded review-only replay','never historical reminder mutation','version-check survivor and loser','validated non-conflicting evidence','loser becomes dismissed duplicate') 'P0-POLICY-CORRECTION-001'

$privacy=Read-Json $PrivacyPath 'P0-POLICY-CROSS-CONTRACT-001'
if($null-ne$privacy){
 foreach($record in $records){$pr=@($privacy.record_types|Where-Object id -eq $record.id);if($pr.Count-ne 1){Fail 'P0-POLICY-SCHEMA-001';continue};$fields=@($pr[0].field_groups|ForEach-Object{$_.fields}|ForEach-Object{$_});Exact @($record.exact_fields_in_order) $fields 'P0-POLICY-SCHEMA-001'}
 ExactOrdered @($m.privacy_boundary.approved_record_types) @('loop','deadline_evidence','transition') 'P0-POLICY-PRIVACY-001'
 Exact @($privacy.forbidden_classes|Where-Object{$_-in@('prompt','model_request','model_response','model_output','model_rationale','embedding','transcript')}) @('prompt','model_request','model_response','model_output','model_rationale','embedding','transcript') 'P0-POLICY-PRIVACY-001'
}
Has (($m.privacy_boundary|ConvertTo-Json -Compress)) @('human-readable task person client subject body attachment link or deadline phrase','free-text reason rationale correction explanation or note','raw Microsoft Graph Office account tenant message event task or URL identifier','prompt model request response output rationale embedding transcript or vector','generic JSON map extension bag open enum unbounded string bytes array or blob','fixed non-content codes and bounded counters only','no facet history source identifier temporal value or command payload','disabled') 'P0-POLICY-PRIVACY-001'

$sd=$m.separate_decisions
if($sd.reminder_adapters_operations_ownership_conflicts-ne'ADR-009'-or$sd.automation_modes_eligibility_evaluation_and_feature_flags-ne'ADR-011'-or$sd.evidence_identity_and_reanchoring-ne'ADR-006'-or$sd.persistence_encryption_and_atomic_commit-ne'ADR-005'-or$sd.provider_transport_and_hypothesis_validation-ne'ADR-007'){Fail 'P0-POLICY-DEFERRED-001'}
if($m.reminder_boundary.local_policy_only-ne$true){Fail 'P0-POLICY-DEFERRED-001'}
Empty @($m.reminder_boundary.graph_office_permissions_calls_operations) 'P0-POLICY-DEFERRED-001'

$gov=Read-Json $GovernancePath 'P0-POLICY-CROSS-CONTRACT-001';$persistence=Read-Json $PersistencePath 'P0-POLICY-CROSS-CONTRACT-001';$evidence=Read-Json $EvidencePath 'P0-POLICY-CROSS-CONTRACT-001';$model=Read-Json $ModelPath 'P0-POLICY-CROSS-CONTRACT-001';$sync=Read-Json $SyncPath 'P0-POLICY-CROSS-CONTRACT-001';$support=Read-Json $SupportPath 'P0-POLICY-CROSS-CONTRACT-001';$build=Read-Json $BuildPath 'P0-POLICY-CROSS-CONTRACT-001'
if($null-ne$gov){$owner=@($gov.owner_decisions|Where-Object id -eq 'OWN-03');$adr=@($gov.adrs|Where-Object id -eq 'ADR-008');$gates=@($gov.gates|Where-Object id -in @('G-AUTO','G-AUTO-FULL'));$hybrid=@($gov.capabilities|Where-Object id -eq 'hybrid_reminder_mode');$automatic=@($gov.capabilities|Where-Object id -eq 'fully_automatic_reminder_mode');if($owner.Count-ne 1-or$owner[0].status-ne'accepted'-or$adr.Count-ne 1-or$adr[0].status-ne'accepted'-or$gates.Count-ne 2-or@($gates|Where-Object status -ne 'unrun').Count-ne 0-or$hybrid.Count-ne 1-or$automatic.Count-ne 1-or$hybrid[0].state-ne'disabled'-or$automatic[0].state-ne'disabled'-or$hybrid[0].advertised-ne$false-or$automatic[0].advertised-ne$false){Fail 'P0-POLICY-CROSS-CONTRACT-001'}}
if($null-ne$persistence){if($persistence.logical_schema_boundary.logical_runtime_schema_approved-ne$false-or$persistence.separate_decisions.loop_state_and_policy_catalogs-ne'ADR-008'-or$persistence.separate_decisions.reminder_ownership_and_operation_catalogs-ne'ADR-009'-or$persistence.separate_decisions.automation_mode_and_evaluation-ne'ADR-011'){Fail 'P0-POLICY-CROSS-CONTRACT-001'}}
if($null-ne$evidence){Has (($evidence.unavailable_projection|ConvertTo-Json -Compress)) @('never closes resolves dismisses or proves failure') 'P0-POLICY-CROSS-CONTRACT-001'}
if($null-ne$model){if($model.runtime_boundary.provider_transport-ne$false){Fail 'P0-POLICY-CROSS-CONTRACT-001'};Has (($model.semantic_policy|ConvertTo-Json -Compress)) @('lifecycle transition','automatic closure','unavailable until separate ADR-011') 'P0-POLICY-CROSS-CONTRACT-001'}
if($null-ne$sync){Empty @($sync.claims.graph_calls) 'P0-POLICY-CROSS-CONTRACT-001';Empty @($sync.claims.network_contacts) 'P0-POLICY-CROSS-CONTRACT-001';Empty @($sync.claims.capabilities_enabled) 'P0-POLICY-CROSS-CONTRACT-001'}
if($null-ne$support){Empty @($support.claim_state.supported_rows) 'P0-POLICY-CROSS-CONTRACT-001';Empty @($support.claim_state.enabled_capabilities) 'P0-POLICY-CROSS-CONTRACT-001';Empty @($support.claim_state.advertised_capabilities) 'P0-POLICY-CROSS-CONTRACT-001';Empty @($support.claim_state.gates_passed) 'P0-POLICY-CROSS-CONTRACT-001';Empty @($support.claim_state.acceptance_criteria_completed) 'P0-POLICY-CROSS-CONTRACT-001'}
if($null-ne$build){if($build.runtime_boundary.durable_state-ne$false-or$build.runtime_boundary.graph_transport-ne$false-or$build.runtime_boundary.model_provider-ne$false){Fail 'P0-POLICY-CROSS-CONTRACT-001'};Empty @($build.runtime_boundary.network_origins) 'P0-POLICY-CROSS-CONTRACT-001';Empty @($build.runtime_boundary.accepted_secrets) 'P0-POLICY-CROSS-CONTRACT-001'}

$runtime=$m.runtime_boundary
if($runtime.policy_runtime-ne$false-or$runtime.logical_records_accepted-ne$false){Fail 'P0-POLICY-CLAIMS-001'}
foreach($name in @('persistent_records_created','network_calls','graph_office_mutations','model_calls','acceptance_criteria_completed','scenarios_completed','gates_passed')){Empty @($runtime.$name) 'P0-POLICY-CLAIMS-001'}
foreach($name in @('capabilities_enabled','capabilities_advertised','support_rows_advertised','permissions_requested','dependencies_activated')){Empty @($m.claims.$name) 'P0-POLICY-CLAIMS-001'}

$adrText=Read-Text $AdrPath 'P0-POLICY-CROSS-CONTRACT-001';$threatText=Read-Text $ThreatPath 'P0-POLICY-CROSS-CONTRACT-001';$traceText=Read-Text $TraceabilityPath 'P0-POLICY-INVENTORY-001'
if((Normalized-Hash $adrText)-ne'f1007955f1704d536bd827fc8d8ef41b7bcde892bf5db4e22592b681294b5b73'){Fail 'P0-POLICY-CROSS-CONTRACT-001'}
if((Normalized-Hash $traceText)-ne'90bd38ff7a1872e7550f40a1e01e966f2bc95d3f1e45eb0119478977505f776a'){Fail 'P0-POLICY-INVENTORY-001'}
Has $adrText @('ADR-008','Status:** Accepted','P0-WI-11','OWN-03','Consumed requirements:** OL-REM-010, OL-REM-017','G-AUTO','G-AUTO-FULL','Several current closure','same key and canonical payload','Candidate loops may be explicitly declined','terminal loops do not newly age','ADR-009 exclusively owns','ADR-011 exclusively owns','implements no policy runtime','neither implements nor completes either requirement') 'P0-POLICY-CROSS-CONTRACT-001'
Has $threatText @('Policy and state boundary threat model','Model output terminalizes a loop','Multiple closure hypotheses','Reminder completion/deletion closes an obligation','Accepted OWN-03 is mistaken','Local terminal change causes an implicit Graph operation') 'P0-POLICY-CROSS-CONTRACT-001'
$traceIds=[regex]::Matches($traceText,'(?m)^\| (P0-POLICY-[A-Z-]+-001) \|')|ForEach-Object{$_.Groups[1].Value}
Exact @($traceIds) $checks 'P0-POLICY-INVENTORY-001'
foreach($checkId in $checks){if(([regex]::Matches($traceText,'\b'+[regex]::Escape($checkId)+'\b')).Count-ne 1){Fail 'P0-POLICY-INVENTORY-001'}}
$freshRow=[regex]::Match($traceText,'(?m)^\| P0-POLICY-FRESH-CHECKER-001 \|.*$').Value
Has $freshRow @('Fresh-context state, privacy, scope, and adversarial judges','no transcript or model output stored','Passed at P0-WI-11 closure') 'P0-POLICY-FRESH-CHECKER-001'

if($script:failures.Count-gt 0){[Console]::Error.WriteLine(('FAIL: policy/state boundary: '+(($script:failures|Sort-Object)-join', ')));exit 1}
if(-not$Quiet){Write-Output ('PASS: policy/state boundary ({0} checks; 420 core tuples / 41 legal; runtime and claims disabled)' -f $checks.Count)}
