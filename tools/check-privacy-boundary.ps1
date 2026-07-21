[CmdletBinding()]
param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot '..\contracts\privacy\persistence-boundary.json')
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
        if ($names.Count -ne (@($names | Sort-Object -Unique)).Count) { Fail 'P0-PRIV-CLOSED-001' }
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
    [Console]::Error.WriteLine('BLOCKED: privacy manifest is not strict JSON.')
    exit 1
} finally {
    if ($document) { $document.Dispose() }
}

# This decision fingerprint makes every classification, purpose, threat,
# disclosure, protection, lifecycle, and invariant value independently exact.
# Structural checks below provide stable, actionable assertion IDs as well.
$canonical = $manifest | ConvertTo-Json -Depth 100 -Compress
$fingerprint = [Convert]::ToHexString(
    [Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))
).ToLowerInvariant()
if ($fingerprint -ne '8eba3b0080578cd540676e64bda6e9cf7424dbf0b6f5b14bdae69b2e966c83c6') {
    Fail 'P0-PRIV-INVENTORY-001'
}

$topKeys = 'schema_version','work_item','adr','owner_decisions','requirements','acceptance_criteria','blocking_gates','claim_state','semantic_types','retention_policies','operation_policies','lifecycle_transitions','record_types','source_variants','diagnostic_events','forbidden_classes','cross_record_invariants'
Exact @($manifest.PSObject.Properties.Name) $topKeys 'P0-PRIV-CLOSED-001'
Exact @($manifest.adr.PSObject.Properties.Name) @('id','status') 'P0-PRIV-CLOSED-001'
Exact @($manifest.claim_state.PSObject.Properties.Name) @('capability_enabled','logical_runtime_schema_approved','gates_passed','acceptance_criteria_completed') 'P0-PRIV-CLOSED-001'
if ($manifest.schema_version -ne 1 -or $manifest.work_item -ne 'P0-WI-02' -or
    $manifest.adr.id -ne 'ADR-PRIV-001' -or $manifest.adr.status -ne 'accepted') { Fail 'P0-PRIV-TRACE-001' }
Exact @($manifest.owner_decisions) @('OWN-06','OWN-07') 'P0-PRIV-TRACE-001'
Exact @($manifest.blocking_gates) @('G-STATE','G-PRIV','G-SEC-AUDIT') 'P0-PRIV-TRACE-001'
Exact @($manifest.acceptance_criteria) @() 'P0-PRIV-TRACE-001'
if ($manifest.claim_state.capability_enabled -ne $false) { Fail 'P0-PRIV-TRACE-001' }
if ($manifest.claim_state.logical_runtime_schema_approved -ne $false) { Fail 'P0-PRIV-CLOSED-001' }
Exact @($manifest.claim_state.gates_passed) @() 'P0-PRIV-TRACE-001'
Exact @($manifest.claim_state.acceptance_criteria_completed) @() 'P0-PRIV-TRACE-001'

$expectedRequirements = @(
 'OL-GOV-001','OL-CLOSE-006','OL-CLOSE-007','OL-REM-004','OL-REM-006','OL-REM-007','OL-REM-011','OL-REM-013','OL-REM-016',
 'OL-UX-006','OL-UX-009','OL-UX-010','OL-TEST-003','OL-METRIC-001','OL-METRIC-002','OL-METRIC-003','OL-METRIC-004',
 'OL-MODEL-001','OL-MODEL-003','OL-MODEL-004','OL-MODEL-006','OL-MODEL-009','OL-MODEL-011','OL-MODEL-013',
 'OL-NFR-005','OL-NFR-006','OL-NFR-010','OL-NFR-012'
)
Exact @($manifest.requirements) $expectedRequirements 'P0-PRIV-TRACE-001'

$semanticIds = 'opaque_local_id','encrypted_locator','keyed_digest','closed_enum','boolean','bounded_integer','canonical_timestamp','canonical_duration','unicode_range','version_label','encrypted_structured_temporal','encrypted_user_setting','secure_store_reference','opaque_reference_set','counter_value','fixed_crypto_bytes','authenticated_ciphertext','local_schedule','timezone_identifier','coarse_time_bucket'
$retentionIds = 'account_until_disconnect','checkpoint_until_scope_removed','observation_window_plus_15','unresolved_quarantine','active_loop_lineage','terminal_loop_lineage','completed_operation_90','unresolved_operation','job_health_until_resolved','settings_until_replaced','counter_rolling_30','enclosed_record_policy'
$operationIds = 'delete_local_only','compact_and_retire_best_effort','reset_counter_only','reset_setting_only','disconnect_user_choice','export_disabled','diagnostic_export_gated'
Exact @($manifest.semantic_types.id) $semanticIds 'P0-PRIV-CLOSED-001'
Exact @($manifest.retention_policies.id) $retentionIds 'P0-PRIV-RETENTION-001'
Exact @($manifest.operation_policies.id) $operationIds 'P0-PRIV-OPERATIONS-001'
foreach ($entry in @($manifest.semantic_types) + @($manifest.retention_policies) + @($manifest.operation_policies)) {
    Exact @($entry.PSObject.Properties.Name) @('id','rule') 'P0-PRIV-CLOSED-001'
}

$retentionText = ($manifest.retention_policies.rule -join ' ')
foreach ($required in '1 through 365 days plus exactly 15 days','retain only the required encrypted message-observation and content-free health fields','success dismissal or recovery returns the observation to observation-window retention and deletes it immediately when that window has elapsed','disconnect immediately deletes the local observation and related health record regardless of age with zero Graph mutations','default 30 days','0 through 365 days','exactly 90 days','rolling 30-day aggregate','explicitly abandoned') {
    if ($retentionText -notmatch [regex]::Escape($required)) { Fail 'P0-PRIV-RETENTION-001' }
}
$healthRetention = @($manifest.retention_policies | Where-Object id -eq 'job_health_until_resolved')
if ($healthRetention.Count -ne 1 -or $healthRetention[0].rule -ne 'unresolved health state only until retry success, dismissal, recovery completion, or disconnect; resolution atomically deletes the job_health record before the related observation returns to ordinary retention') {
    Fail 'P0-PRIV-RETENTION-001'
}
$operationText = ($manifest.operation_policies.rule -join ' ')
foreach ($required in 'zero Graph mutations','no Microsoft artifact deletion','no physical-erasure guarantee','never bulk-deletes Microsoft artifacts','product-state and metric export are unavailable') {
    if ($operationText -notmatch [regex]::Escape($required)) { Fail 'P0-PRIV-OPERATIONS-001' }
}

Exact @($manifest.lifecycle_transitions.id) @('loop_lineage_retention','operation_retention','quarantine_retention') 'P0-PRIV-RETENTION-001'
$loopLifecycle = $manifest.lifecycle_transitions | Where-Object id -eq 'loop_lineage_retention'
$operationLifecycle = $manifest.lifecycle_transitions | Where-Object id -eq 'operation_retention'
$quarantineLifecycle = $manifest.lifecycle_transitions | Where-Object id -eq 'quarantine_retention'
foreach ($lifecycle in $manifest.lifecycle_transitions) {
    Exact @($lifecycle.PSObject.Properties.Name) @('id','record_types','states') 'P0-PRIV-CLOSED-001'
    foreach ($state in $lifecycle.states) {
        Exact @($state.PSObject.Properties.Name) @('when','retention') 'P0-PRIV-CLOSED-001'
        if ($retentionIds -notcontains $state.retention) { Fail 'P0-PRIV-RETENTION-001' }
    }
}
Exact @($quarantineLifecycle.record_types) @('message_observation') 'P0-PRIV-RETENTION-001'
if ($quarantineLifecycle.states.Count -ne 2 -or
    $quarantineLifecycle.states[0].when -ne 'recoverable_quarantine_unresolved' -or
    $quarantineLifecycle.states[0].retention -ne 'unresolved_quarantine' -or
    $quarantineLifecycle.states[1].when -ne 'retry_succeeded_dismissed_or_recovery_complete' -or
    $quarantineLifecycle.states[1].retention -ne 'observation_window_plus_15') { Fail 'P0-PRIV-RETENTION-001' }
Exact @($loopLifecycle.record_types) @('email_evidence_ref','calendar_invitation_ref','user_authored_artifact_ref','loop','deadline_evidence','transition','reminder_link','client_association') 'P0-PRIV-RETENTION-001'
if ($loopLifecycle.states[0].when -ne 'candidate_or_open' -or $loopLifecycle.states[0].retention -ne 'active_loop_lineage' -or
    $loopLifecycle.states[1].when -ne 'terminal' -or $loopLifecycle.states[1].retention -ne 'terminal_loop_lineage') { Fail 'P0-PRIV-RETENTION-001' }
Exact @($operationLifecycle.record_types) @('operation_ledger') 'P0-PRIV-RETENTION-001'
if ($operationLifecycle.states[0].when -ne 'pending_or_ambiguous' -or $operationLifecycle.states[0].retention -ne 'unresolved_operation' -or
    $operationLifecycle.states[1].when -ne 'completed' -or $operationLifecycle.states[1].retention -ne 'completed_operation_90') { Fail 'P0-PRIV-RETENTION-001' }

$expectedRecords = [ordered]@{
 persistence_envelope='record_id,account_binding_aad,record_type,schema_version,ciphertext_version,key_id,nonce,authentication_tag,ciphertext'
 account_binding='record_id,account_ref,cloud_code,enabled_feature_codes,requested_scope_codes,granted_scope_codes,oauth_token_state_ref'
 sync_checkpoint='record_id,account_ref,collection_locator,delta_or_next_locator,query_fingerprint_hmac,collection_kind,status_code,last_complete_at'
 message_observation='record_id,account_ref,source_version_hmac,observed_version_hmac,direction_code,read_observation_code,first_observation_code,job_outcome_code,failure_class,retry_state,policy_version,quarantined_locator,retry_generation,observed_at'
 email_evidence_ref='source_ref_id,account_ref,message_locator,fallback_locator,conversation_ref,selected_folder_context,component_code,relation_code,block_ordinal,range_start,range_end,content_digest_hmac,prefix_digest_hmac,suffix_digest_hmac,anchor_version,locator_version,observed_at'
 calendar_invitation_ref='source_ref_id,account_ref,invitation_email_ref_id,event_locator,response_locator,event_version_hmac,response_state_code,relation_code,observed_at'
 user_authored_artifact_ref='source_ref_id,account_ref,adapter_locator,list_locator,artifact_locator,title_digest_hmac,body_digest_hmac,due_digest_hmac,title_version_hmac,body_version_hmac,due_version_hmac,artifact_type_code,title_ownership_code,body_ownership_code,due_ownership_code,title_version_state,body_version_state,due_version_state,origin_state_code,source_availability_code,observed_at'
 loop='loop_id,account_ref,operative_deadline_ref_id,last_evaluated_source_ref_id,origin_code,provenance_codes,obligation_state_code,review_flag_codes,deadline_state_code,closure_review_state_code,reminder_state_code,analysis_state_code,source_state_code,resolution_code,confidence_bucket,source_ref_ids_by_role,reminder_ref_ids,transition_ref_ids,loop_version,policy_version,schema_version,model_label_code,operative_deadline_value_or_range'
 deadline_evidence='deadline_evidence_id,loop_id,source_ref_id,source_type_code,precision_code,interpretation_code,operative_selection_code,value_or_range,timezone_id'
 transition='transition_id,loop_id,user_action_id,prior_facet_codes,new_facet_codes,reason_code,actor_code,source_ref_ids,occurred_at,loop_version,policy_version'
 reminder_link='reminder_link_id,loop_id,list_locator,artifact_locator,adapter_code,operation_state_code,reconciliation_state_code,title_ownership_code,body_ownership_code,due_ownership_code,title_digest_hmac,body_digest_hmac,due_digest_hmac,artifact_version_hmac,observed_remote_version_hmac,remote_correlation_hmac,reminder_due_override,last_reconciled_at'
 operation_ledger='operation_key_hmac,record_id,account_ref,operation_id,loop_id,source_ref_id,operation_kind_code,adapter_code,state_code,reconciliation_code,field_mask_codes,ownership_codes,ambiguity_or_failure_code,destination_locator,expected_remote_version_hmac,remote_correlation_hmac,attempt_count,retry_at,completed_at'
 job_health='record_id,account_ref,quarantine_ref,error_class_code,attempt_count,quarantine_generation,retry_at,stale_since'
 user_setting='settings_id,account_ref,identity_address_values,identity_name_values,identity_alias_values,identity_nickname_values,identity_initial_values,identity_username_values,identity_role_values,identity_team_values,identity_context_rules,client_mapping_refs,excluded_sender_values,excluded_domain_values,typed_correction_rules,provider_profile,exact_consented_origin,provider_model_label,consented_transmitted_field_codes,selected_folder_locators,reminder_creation_mode,candidate_visibility_codes,reminder_eligibility_codes,request_sensitivity_code,deadline_inference_sensitivity_code,closure_sensitivity_code,review_threshold_codes,week_end_policy_code,date_only_policy_code,reminder_type_code,no_deadline_behavior_code,ignored_category_codes,daily_summary_destination_code,reward_mode_code,reward_wording_code,reward_frequency_code,model_provider_code,model_label,telemetry_code,history_window_days,poll_cadence_seconds,eod_local_time,daily_summary_schedule,reminder_lead_rule,reward_emoji_enabled,aggregate_indicators_enabled,timezone_id'
 client_association='client_association_id,loop_id,evidence_ref_ids,configured_mapping_ref,confidence_bucket,ambiguity_flag'
 product_counter='counter_code,count,coarse_window_bucket,window_version'
}
Exact @($manifest.record_types.id) @($expectedRecords.Keys) 'P0-PRIV-INVENTORY-001'
$groupKeys = 'classification','semantic_type','purpose','necessity','threat','disclosure','protection','retention','deletion','reset','export','reconstruction','source','fields'
foreach ($record in $manifest.record_types) {
    Exact @($record.PSObject.Properties.Name) @('id','field_groups') 'P0-PRIV-CLOSED-001'
    if (@($record.field_groups).Count -eq 0) { Fail 'P0-PRIV-INVENTORY-001' }
    $fields = @()
    foreach ($group in $record.field_groups) {
        Exact @($group.PSObject.Properties.Name) $groupKeys 'P0-PRIV-INVENTORY-001'
        foreach ($key in $groupKeys | Where-Object { $_ -ne 'fields' }) {
            if (-not ($group.$key -is [string]) -or [string]::IsNullOrWhiteSpace($group.$key)) { Fail 'P0-PRIV-INVENTORY-001' }
        }
        if ($semanticIds -notcontains $group.semantic_type) { Fail 'P0-PRIV-CLOSED-001' }
        if ($retentionIds -notcontains $group.retention) { Fail 'P0-PRIV-RETENTION-001' }
        if ($group.export -ne 'export_disabled') { Fail 'P0-PRIV-OPERATIONS-001' }
        $fields += @($group.fields)
    }
    Exact $fields ($expectedRecords[$record.id] -split ',') 'P0-PRIV-INVENTORY-001'
}

$sources = $manifest.source_variants
Exact @($sources.id) @('email_evidence','calendar_invitation','user_authored_microsoft_artifact') 'P0-PRIV-SOURCE-001'
$email = $sources | Where-Object id -eq 'email_evidence'
Exact @($email.PSObject.Properties.Name) @('id','record_type','component_codes','relation_codes','availability') 'P0-PRIV-CLOSED-001'
Exact @($email.component_codes) @('subject','body_block','quote_block','sender','to','cc','attachment_name','link_label','calendar_response') 'P0-PRIV-SOURCE-001'
Exact @($email.relation_codes) @('task','ownership','waiting_party','deadline','modification','closure','delegation','supersession','context') 'P0-PRIV-SOURCE-001'
if ($email.record_type -ne 'email_evidence_ref' -or $email.availability -ne 'shape_approved_use_blocked_by_G-MAIL_G-PRIV') { Fail 'P0-PRIV-SOURCE-001' }
$calendar = $sources | Where-Object id -eq 'calendar_invitation'
Exact @($calendar.PSObject.Properties.Name) @('id','record_type','component_codes','relation_codes','availability') 'P0-PRIV-CLOSED-001'
Exact @($calendar.component_codes) @() 'P0-PRIV-SOURCE-001'
Exact @($calendar.relation_codes) @('task','ownership','waiting_party','deadline','modification','closure','delegation','supersession','context') 'P0-PRIV-SOURCE-001'
if ($calendar.record_type -ne 'calendar_invitation_ref' -or $calendar.availability -ne 'shape_approved_use_blocked_by_G-CAL_G-MAIL_G-PRIV') { Fail 'P0-PRIV-SOURCE-001' }
$manual = $sources | Where-Object id -eq 'user_authored_microsoft_artifact'
Exact @($manual.PSObject.Properties.Name) @('id','record_type','artifact_type_codes','authoritative_fields','ownership_code','authoritative_readable_source_location','availability') 'P0-PRIV-CLOSED-001'
Exact @($manual.artifact_type_codes) @('todo','calendar') 'P0-PRIV-MANUAL-001'
Exact @($manual.authoritative_fields) @('title','body','due') 'P0-PRIV-MANUAL-001'
if ($manual.record_type -ne 'user_authored_artifact_ref' -or $manual.ownership_code -ne 'user_authoritative' -or
    $manual.authoritative_readable_source_location -ne 'microsoft_artifact_only' -or
    $manual.availability -ne 'shape_approved_use_blocked_by_G-TODO_or_G-CAL_and_G-PRIV') { Fail 'P0-PRIV-MANUAL-001' }

$forbidden = 'mail_subject','mail_body','body_preview','quote_text','generated_summary','generated_explanation','task_title','task_body','deadline_phrase','participant_value','email_address','display_name','attachment_name','attachment_bytes','link_label','url','raw_graph_id','tenant_id','account_id','object_id','workspace_id','raw_delta_url','raw_request','raw_response','authorization_header','token','key_material','secret','prompt','model_request','model_response','model_output','model_rationale','embedding','vector','test_sample','test_label','test_prediction','free_text_correction','exception_text','error_body','log','transcript','browser_cache','diagnostic_bundle','source_map','screenshot','har_capture'
Exact @($manifest.forbidden_classes) $forbidden 'P0-PRIV-NOTEXT-001'
if (@($manifest.diagnostic_events).Count -ne 0) { Fail 'P0-PRIV-DIAG-001' }
Exact @($manifest.cross_record_invariants.id) (1..13 | ForEach-Object { 'PRIV-XREC-{0:d3}' -f $_ }) 'P0-PRIV-MANUAL-001'
foreach ($invariant in $manifest.cross_record_invariants) {
    Exact @($invariant.PSObject.Properties.Name) @('id','rule') 'P0-PRIV-CLOSED-001'
}
$invariantText = $manifest.cross_record_invariants.rule -join ' '
foreach ($required in 'immediately before encryption','immediately after decryption or migration','exactly one authoritative artifact','cannot overwrite','produces source unavailable','zero Graph mutations','no product-state or metric export','never claim physical erasure','atomically deletes the related job_health record','a quarantine_ref never dangles') {
    if ($invariantText -notmatch [regex]::Escape($required)) { Fail 'P0-PRIV-MANUAL-001' }
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$governance = Get-Content (Join-Path $repoRoot 'contracts\governance\capabilities.json') -Raw | ConvertFrom-Json
$privacyAdr = @($governance.adrs | Where-Object id -eq 'ADR-PRIV-001')
if ($privacyAdr.Count -ne 1 -or $privacyAdr[0].status -ne 'accepted' -or @($governance.product_acceptance_criteria_completed).Count -ne 0) { Fail 'P0-PRIV-TRACE-001' }
if (@($governance.gates | Where-Object status -ne 'unrun').Count -ne 0 -or @($governance.capabilities | Where-Object { $_.state -ne 'disabled' -or $_.advertised }).Count -ne 0) { Fail 'P0-PRIV-TRACE-001' }
foreach ($relative in 'docs\adr\ADR-PRIV-001-derived-metadata-and-source-boundary.md','docs\threat-model\privacy-and-local-state.md','docs\prd-traceability.md') {
    if (-not (Test-Path (Join-Path $repoRoot $relative))) { Fail 'P0-PRIV-TRACE-001' }
}
$trace = Get-Content (Join-Path $repoRoot 'docs\prd-traceability.md') -Raw
foreach ($id in 'P0-PRIV-INVENTORY-001','P0-PRIV-SOURCE-001','P0-PRIV-CLOSED-001','P0-PRIV-NOTEXT-001','P0-PRIV-ID-001','P0-PRIV-MANUAL-001','P0-PRIV-RETENTION-001','P0-PRIV-OPERATIONS-001','P0-PRIV-DIAG-001','P0-PRIV-TRACE-001','P0-PRIV-FRESH-CHECKER-001') {
    if (([regex]::Matches($trace, [regex]::Escape($id))).Count -ne 1) { Fail 'P0-PRIV-TRACE-001' }
}

if ($failures.Count) {
    [Console]::Error.WriteLine(('BLOCKED privacy boundary checks: ' + (($failures | Sort-Object) -join ', ')))
    exit 1
}
Write-Host 'OpenLoops privacy-boundary checks passed (11 P0-WI-02 assertions; no gate or capability advanced).'
