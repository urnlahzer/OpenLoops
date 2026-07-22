[CmdletBinding()]
param(
 [string]$ManifestPath='contracts/selfemail/self-email-boundary.json',
 [string]$AdrPath='docs/adr/ADR-013-self-email.md',
 [string]$ThreatPath='docs/threat-model/self-email.md',
 [string]$TraceabilityPath='docs/prd-traceability.md',
 [string]$ProductSpecPath='docs/product-spec.md',
 [string]$ImplementationPlanPath='docs/implementation-plan.md',
 [string]$GovernancePath='contracts/governance/capabilities.json',
 [string]$Adr003Path='docs/adr/ADR-003-incremental-authorization.md',
 [string]$Adr004Path='docs/adr/ADR-004-synchronization.md',
 [string]$Adr005Path='docs/adr/ADR-005-persistence-and-cryptography.md',
 [string]$Adr009Path='docs/adr/ADR-009-reminder-adapters.md',
 [string]$AdrPrivPath='docs/adr/ADR-PRIV-001-derived-metadata-and-source-boundary.md',
 [string]$SupportPath='contracts/support/support-matrix.json',
 [string]$BuildPath='contracts/build-skeleton/skeleton.json',
 [string]$PolicyPath='contracts/domain/policy-state-boundary.json',
 [string]$ReminderPath='contracts/reminder/adapter-boundary.json',
 [string]$AddinPath='contracts/addin/bridge-boundary.json',
 [string]$AutomationPath='contracts/automation/evaluation-boundary.json',
 [string]$DistributionPath='contracts/distribution/registration-boundary.json',
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
function Has([string]$t,[string[]]$p,[string]$id){$tn=($t-replace'\s+',' ');foreach($x in $p){$xn=($x-replace'\s+',' ');if($tn-notmatch[regex]::Escape($xn)){Fail $id}}}
function NHash([string]$t){$c=(($t-replace"`r`n","`n").TrimEnd()+"`n");[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($c))).ToLowerInvariant()}
$script:fail=[Collections.Generic.List[string]]::new();$script:seen=@{}
$checks=@('P0-SELFMAIL-INVENTORY-001','P0-SELFMAIL-RECIPIENT-001','P0-SELFMAIL-CONSENT-001','P0-SELFMAIL-MARKER-001','P0-SELFMAIL-RECURSION-001','P0-SELFMAIL-OPERATION-001','P0-SELFMAIL-SCHEDULE-001','P0-SELFMAIL-PRIVACY-001','P0-SELFMAIL-CROSS-CONTRACT-001','P0-SELFMAIL-CLAIMS-001','P0-SELFMAIL-FRESH-CHECKER-001')

$m=Json $ManifestPath 'P0-SELFMAIL-INVENTORY-001';if($null-eq$m){[Console]::Error.WriteLine('BLOCKED: self-email manifest parse failed.');exit 1}
$canonical=$m|ConvertTo-Json -Depth 100 -Compress;$hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
if($hash-ne'71997cf04b34323b89f56bca7746c37053a362b9df57f0c24863541fca71aad6'){Fail 'P0-SELFMAIL-INVENTORY-001'}
if($m.schema_version-ne 1-or$m.work_item-ne'P0-WI-16'-or$m.adr-ne'ADR-013'-or$m.decision_status-ne'accepted_contract_runtime_unimplemented'-or$m.snapshot_date-ne'2026-07-21'){Fail 'P0-SELFMAIL-INVENTORY-001'}
Ordered @($m.owner_decisions) @('OWN-10') 'P0-SELFMAIL-INVENTORY-001'
Ordered @($m.owned_requirements) @('OL-SUM-002','OL-SUM-003','OL-SUM-004') 'P0-SELFMAIL-INVENTORY-001'
Empty @($m.consumed_requirements) 'P0-SELFMAIL-INVENTORY-001'
Empty @($m.primary_acceptance_criteria) 'P0-SELFMAIL-INVENTORY-001'
Empty @($m.dependent_acceptance_criteria) 'P0-SELFMAIL-INVENTORY-001'
Empty @($m.primary_scenarios) 'P0-SELFMAIL-INVENTORY-001'
Empty @($m.scenario_dependencies) 'P0-SELFMAIL-INVENTORY-001'
Ordered @($m.blocking_gates) @('G-SELFMAIL','G-PRIV') 'P0-SELFMAIL-INVENTORY-001'
Ordered @($m.input_authorities.owner) @('ADR-003','ADR-004','ADR-005','ADR-009','ADR-PRIV-001') 'P0-SELFMAIL-INVENTORY-001'
Ordered @($m.sources.id) @('SRC-SPEC-OWN-10','SRC-SPEC-SUM','SRC-SPEC-AMBIENT','SRC-SPEC-GATES','SRC-PLAN-PHASE1','SRC-PLAN-PHASE6','SRC-PLAN-ADR','SRC-PLAN-RISK','SRC-ADR-003','SRC-ADR-004','SRC-ADR-005','SRC-ADR-009','SRC-ADR-PRIV-001','SRC-THREAT-INDEX') 'P0-SELFMAIL-INVENTORY-001'
$sd=$m.separate_decisions
if($sd.canonical_self_recipient_source-ne'unresolved_pending_G-SELFMAIL'-or$sd.marker_mechanism-ne'unresolved_pending_G-SELFMAIL'-or$sd.durable_operation_ledger_mechanics-ne'ADR-005'-or$sd.operation_replay_and_ambiguous_write_protocol-ne'ADR-009'-or$sd.sent_copy_observation_and_delivery_wording-ne'ADR-004'-or$sd.generated_summary_persistence_prohibition-ne'ADR-PRIV-001'-or$sd.mail_send_incremental_consent_boundary-ne'ADR-003'){Fail 'P0-SELFMAIL-INVENTORY-001'}

# Requirement double-ownership: OL-SUM-002/003/004 owned exactly here; other owned_requirements manifests must not claim them.
foreach($pair in @(@($PolicyPath,'policy'),@($ReminderPath,'reminder'),@($AddinPath,'addin'),@($AutomationPath,'automation'),@($DistributionPath,'distribution'))){
 $other=Json $pair[0] 'P0-SELFMAIL-INVENTORY-001'
 if($null-ne$other){if(@($other.owned_requirements|Where-Object{$_-in@('OL-SUM-002','OL-SUM-003','OL-SUM-004')}).Count-ne 0){Fail 'P0-SELFMAIL-INVENTORY-001'}}
}

$c=$m.catalogs
Ordered @($c.canonical_recipient_source_candidate_code) @('graph_me_mail_field','graph_me_user_principal_name_field','primary_smtp_proxy_address','authenticated_id_token_email_claim') 'P0-SELFMAIL-RECIPIENT-001'
Ordered @($c.marker_mechanism_candidate_code) @('custom_internet_message_header','graph_single_value_extended_property','graph_multi_value_extended_property','proven_equivalent_field') 'P0-SELFMAIL-MARKER-001'
Ordered @($c.recipient_policy_code) @('canonical_contract_tested_address_only','no_arbitrary_or_additional_recipient','no_user_input_sourced_recipient','no_cc_or_bcc','no_reply_to_redirection','no_distribution_list') 'P0-SELFMAIL-RECIPIENT-001'
Ordered @($c.consent_policy_code) @('separate_explicit_feature_enablement','separate_incremental_mail_send_consent_after_G-SELFMAIL','disablement_stops_sends_before_request','consent_loss_stops_sends_before_request') 'P0-SELFMAIL-CONSENT-001'
Ordered @($c.generation_policy_code) @('transient_reconstruction_under_OL-SUM-003','no_stored_summary_copy','no_stored_draft_or_template_output','content_rules_inherit_privacy_boundary') 'P0-SELFMAIL-PRIVACY-001'
Ordered @($c.marker_policy_code) @('candidate_mechanism_unresolved_pending_G-SELFMAIL','survives_full_round_trip_to_saved_sent_copy','verifiable_on_saved_sent_copy','excludes_only_openloops_generated_summaries','never_excludes_user_authored_mail','never_carries_content_identifier_or_secret','never_marker_by_subject_text') 'P0-SELFMAIL-MARKER-001'
Ordered @($c.recursion_suppression_code) @('detected_marker_prevents_loop_creation','detected_marker_prevents_summary_of_summary_inclusion','detected_marker_prevents_re_summarization','suppression_failure_fails_closed_to_no_send') 'P0-SELFMAIL-RECURSION-001'
Ordered @($c.operation_protocol_code) @('durable_pending_operation_before_request','one_request_per_recorded_attempt','ambiguous_outcome_reconciles_against_ledger_and_sent_items_before_retry','lost_response_never_blindly_retried','definitive_failure_surfaces_visibly') 'P0-SELFMAIL-OPERATION-001'
Ordered @($c.schedule_policy_code) @('runs_only_while_enabled_consented_and_authenticated','missed_schedules_coalesce','no_catch_up_burst') 'P0-SELFMAIL-SCHEDULE-001'
Ordered @($c.failure_boundary_code) @('secure_store_loss_causes_zero_requests','rollback_suspicion_causes_zero_requests','account_mismatch_causes_zero_requests','marker_verification_failure_causes_zero_requests') 'P0-SELFMAIL-OPERATION-001'

$rp=$m.recipient_policy
if($rp.legal_recipient-ne'the only legal recipient is the canonical contract-tested address of the authenticated account'-or$rp.canonical_source-ne'unresolved_pending_G-SELFMAIL; candidate sources may be listed but not selected'){Fail 'P0-SELFMAIL-RECIPIENT-001'}
Ordered @($rp.prohibited) @('arbitrary or additional recipients','a recipient sourced from user input','Cc','Bcc','reply-to redirection','distribution lists') 'P0-SELFMAIL-RECIPIENT-001'

$cp=$m.consent_policy
Has ($cp|ConvertTo-Json -Depth 10 -Compress) @('separate explicit enablement of the feature','separate incremental Mail.Send consent granted only after G-SELFMAIL passes','disablement or consent loss stops sends before any request','not exempted') 'P0-SELFMAIL-CONSENT-001'

$gp=$m.generation_policy
Has ($gp|ConvertTo-Json -Depth 10 -Compress) @('reconstructed transiently under OL-SUM-003','no summary copy, draft, or template output is stored','content rules inherit the ADR-PRIV-001 privacy boundary in full','no new persistence exception') 'P0-SELFMAIL-PRIVACY-001'

$mp=$m.marker_policy
Has ($mp|ConvertTo-Json -Depth 10 -Compress) @('unresolved_pending_G-SELFMAIL','full round trip from send through the Sent Items saved copy','verifiable on the saved sent copy without relying on subject text','excludes from loop detection only messages OpenLoops itself generated','never excludes a user-authored message','never carries summary content, an identifier, or a secret','recoverable only through a keyed derivation, never a plaintext content signal') 'P0-SELFMAIL-MARKER-001'

$rs=$m.recursion_suppression
Ordered @($rs.prevented_paths) @('loop creation','summary-of-summary inclusion','re-summarization') 'P0-SELFMAIL-RECURSION-001'
Has ($rs|ConvertTo-Json -Depth 10 -Compress) @('detection logic must check the marker before any other loop-creation or summarization path','a suppression failure is a no-send, not a best-effort filter') 'P0-SELFMAIL-RECURSION-001'

$sop=$m.send_operation_protocol
Has ($sop|ConvertTo-Json -Depth 10 -Compress) @('a pending encrypted operation-ledger entry is committed before any send request','exactly one request is issued per recorded attempt','never blindly retried','reconciles against both the durable ledger entry and the Sent Items delta observation before any retry decision','surfaces visibly to the user rather than being silently swallowed or silently retried') 'P0-SELFMAIL-OPERATION-001'

$schp=$m.schedule_policy
Has ($schp|ConvertTo-Json -Depth 10 -Compress) @('the feature is enabled, Mail.Send consent is current, and the account remains authenticated','missed scheduled sends coalesce into the next eligible run','no catch-up burst that reissues every missed occurrence') 'P0-SELFMAIL-SCHEDULE-001'

$fb=$m.failure_boundary
Ordered @($fb.triggers) @('secure-store loss','rollback suspicion','account mismatch','marker-verification failure') 'P0-SELFMAIL-OPERATION-001'
Has ([string]$fb.result) @('each independently causes zero send requests for the affected operation','none is a partial or best-effort success') 'P0-SELFMAIL-OPERATION-001'

Ordered @($m.'unresolved_pending_G-SELFMAIL') @('canonical self-recipient source','marker mechanism') 'P0-SELFMAIL-PRIVACY-001'

$rt=$m.runtime_boundary
if($rt.mail_send_requested-ne$false-or$rt.self_email_sent-ne$false-or$rt.marker_mechanism_selected-ne$false-or$rt.scheduled_send_active-ne$false){Fail 'P0-SELFMAIL-CLAIMS-001'}
foreach($n in @('acceptance_criteria_completed','scenarios_completed','gates_passed')){Empty @($rt.$n) 'P0-SELFMAIL-CLAIMS-001'}
foreach($n in @('capabilities_enabled','capabilities_advertised','recipient_sources_advertised','marker_mechanisms_advertised')){Empty @($m.claims.$n) 'P0-SELFMAIL-CLAIMS-001'}

# Cross-contract reconciliation.
$adr003=Text $Adr003Path 'P0-SELFMAIL-CROSS-CONTRACT-001'
Has $adr003 @('Self-email adds `Mail.Send` only through separate explicit','consent after G-SELFMAIL and may address only the contract-tested authenticated','self recipient') 'P0-SELFMAIL-CROSS-CONTRACT-001'
$adr004=Text $Adr004Path 'P0-SELFMAIL-CROSS-CONTRACT-001'
Has $adr004 @('A saved sent copy may be analyzed once, but drafts and compose activity never activate loops','the product says','sent copy observed','never','delivered') 'P0-SELFMAIL-CROSS-CONTRACT-001'
$adr005=Text $Adr005Path 'P0-SELFMAIL-CROSS-CONTRACT-001'
Has $adr005 @('pending operation identity precedes an external request; ambiguous writes reconcile before retry') 'P0-SELFMAIL-CROSS-CONTRACT-001'
$adr009=Text $Adr009Path 'P0-SELFMAIL-CROSS-CONTRACT-001'
Has $adr009 @('Every remote mutation begins with a pending encrypted operation-ledger record','committed under ADR-005 before a request') 'P0-SELFMAIL-CROSS-CONTRACT-001'
$adrPriv=Text $AdrPrivPath 'P0-SELFMAIL-CROSS-CONTRACT-001'
Has $adrPriv @('generated summaries or explanations') 'P0-SELFMAIL-CROSS-CONTRACT-001'

$gov=Json $GovernancePath 'P0-SELFMAIL-CROSS-CONTRACT-001'
if($null-ne$gov){
 $a13=@($gov.adrs|Where-Object{$_.id-eq'ADR-013'})
 if($a13.Count-ne 1-or$a13[0].status-ne'accepted'){Fail 'P0-SELFMAIL-CROSS-CONTRACT-001'}
 if(@($gov.gates|Where-Object{$_.id-in$m.blocking_gates-and$_.status-ne'unrun'}).Count-ne 0){Fail 'P0-SELFMAIL-CROSS-CONTRACT-001'}
 $sem=@($gov.capabilities|Where-Object{$_.id-eq'self_email_summary'})
 if($sem.Count-ne 1-or$sem[0].state-ne'disabled'-or$sem[0].advertised-ne$false){Fail 'P0-SELFMAIL-CROSS-CONTRACT-001'}
 Ordered @($sem[0].owner_decisions) @('OWN-10') 'P0-SELFMAIL-CROSS-CONTRACT-001'
 Ordered @($sem[0].gates) @('G-SELFMAIL','G-PRIV') 'P0-SELFMAIL-CROSS-CONTRACT-001'
 Ordered @($sem[0].permission_contracts) @('Mail.Send') 'P0-SELFMAIL-CROSS-CONTRACT-001'
 $own10=@($gov.owner_decisions|Where-Object{$_.id-eq'OWN-10'})
 if($own10.Count-ne 1-or$own10[0].status-ne'accepted'){Fail 'P0-SELFMAIL-CROSS-CONTRACT-001'}
 Ordered @($own10[0].adrs) @('ADR-013') 'P0-SELFMAIL-CROSS-CONTRACT-001'
 Ordered @($own10[0].gates) @('G-SELFMAIL') 'P0-SELFMAIL-CROSS-CONTRACT-001'
}
$support=Json $SupportPath 'P0-SELFMAIL-CROSS-CONTRACT-001'
if($null-ne$support){foreach($n in @('supported_rows','enabled_capabilities','advertised_capabilities','gates_passed','acceptance_criteria_completed')){Empty @($support.claim_state.$n) 'P0-SELFMAIL-CROSS-CONTRACT-001'}}
$build=Json $BuildPath 'P0-SELFMAIL-CROSS-CONTRACT-001'
if($null-ne$build){
 if($build.runtime_boundary.graph_transport-ne$false-or$build.runtime_boundary.oauth-ne$false-or$build.runtime_boundary.model_provider-ne$false-or$build.runtime_boundary.durable_state-ne$false-or$build.runtime_boundary.addin_manifest-ne$false){Fail 'P0-SELFMAIL-CROSS-CONTRACT-001'}
 Empty @($build.runtime_boundary.network_origins) 'P0-SELFMAIL-CROSS-CONTRACT-001'
 Empty @($build.gates_passed) 'P0-SELFMAIL-CROSS-CONTRACT-001'
 Empty @($build.capabilities_enabled) 'P0-SELFMAIL-CROSS-CONTRACT-001'
}

$adr=Text $AdrPath 'P0-SELFMAIL-CROSS-CONTRACT-001';$threat=Text $ThreatPath 'P0-SELFMAIL-CROSS-CONTRACT-001';$spec=Text $ProductSpecPath 'P0-SELFMAIL-INVENTORY-001';$plan=Text $ImplementationPlanPath 'P0-SELFMAIL-INVENTORY-001';$trace=Text $TraceabilityPath 'P0-SELFMAIL-INVENTORY-001'
if((NHash $adr)-ne'72ab899003f74468b73e85d3117afb4b85db6dea3bb0361551d69431e08f463b'){Fail 'P0-SELFMAIL-CROSS-CONTRACT-001'}
if((NHash $threat)-ne'8e0354bec777b277b7ba8c4a2f7e8095034d33f987f40da6c5e0a515ea4f21d0'){Fail 'P0-SELFMAIL-CROSS-CONTRACT-001'}
if((NHash $spec)-ne'c0a7718d78bde3804142e5b79a937a8d66a372bfe35406e2c8b2ce96d4c7ed54'){Fail 'P0-SELFMAIL-INVENTORY-001'}
if((NHash $plan)-ne'03c11ed8980703bbc2649a460162577d4a4a898528c63867c789afbd8b6c735a'){Fail 'P0-SELFMAIL-INVENTORY-001'}
if((NHash $trace)-ne'50c627612d5a6c6d4429bab069901ec04c3b8c067c00e7d0e06bef7b2a810d23'){Fail 'P0-SELFMAIL-INVENTORY-001'}
Has $adr @('Status:** Accepted','P0-WI-16','OWN-10','G-SELFMAIL, G-PRIV','the only legal recipient of a self-email is the canonical contract-tested','address of the authenticated account','separate explicit enablement of the feature and a','separate incremental `Mail.Send` consent','no summary copy, draft, or template output is stored','a verified OpenLoops-generated marker must accompany every self-email','never carries summary content, an identifier, or a secret','a suppression failure is a no-send, not a best-effort filter','a pending encrypted operation-ledger entry is committed before any send request','no catch-up burst','unresolved_pending_G-SELFMAIL') 'P0-SELFMAIL-CROSS-CONTRACT-001'
Has $threat @('Self-email','wrong-recipient','duplicate summary after a lost response','recursion loop','marker forgery by hostile inbound mail','consent creep','summary content leak','scheduled send while disconnected') 'P0-SELFMAIL-CROSS-CONTRACT-001'
$ids=[regex]::Matches($trace,'(?m)^\| (P0-SELFMAIL-[A-Z-]+-001) \|')|ForEach-Object{$_.Groups[1].Value};Exact @($ids) $checks 'P0-SELFMAIL-INVENTORY-001';foreach($id in $checks){if(([regex]::Matches($trace,'\b'+[regex]::Escape($id)+'\b')).Count-ne 1){Fail 'P0-SELFMAIL-INVENTORY-001'}}
$fresh=[regex]::Match($trace,'(?m)^\| P0-SELFMAIL-FRESH-CHECKER-001 \|.*$').Value
Has $fresh @('Fresh-context send-safety, privacy, governance, and adversarial judges','no prompt, transcript, or model output stored','Passed at P0-WI-16 closure') 'P0-SELFMAIL-FRESH-CHECKER-001'
if($fresh-match'Passed without independent review'){Fail 'P0-SELFMAIL-FRESH-CHECKER-001'}

if($script:fail.Count-gt 0){[Console]::Error.WriteLine(('FAIL: self-email boundary: '+(($script:fail|Sort-Object)-join', ')));exit 1};if(-not$Quiet){Write-Output ('PASS: self-email boundary ({0} checks; recipient, consent, marker, recursion, operation, schedule, and send disabled)' -f $checks.Count)}
