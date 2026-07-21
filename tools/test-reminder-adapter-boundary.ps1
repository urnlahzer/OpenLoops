[CmdletBinding()]param([string]$CheckerPath='tools/check-reminder-adapter-boundary.ps1')
Set-StrictMode -Version Latest;$ErrorActionPreference='Stop'
$repoRoot=(& git rev-parse --show-toplevel 2>$null).Trim();if($LASTEXITCODE-ne 0-or[string]::IsNullOrWhiteSpace($repoRoot)){throw'Run inside the repository.'}
function Resolve-Repo([string]$p){if([IO.Path]::IsPathRooted($p)){[IO.Path]::GetFullPath($p)}else{[IO.Path]::GetFullPath((Join-Path $repoRoot $p))}}
$checker=Resolve-Repo $CheckerPath
$inputs=@('contracts/reminder/adapter-boundary.json','docs/adr/ADR-009-reminder-adapters.md','docs/threat-model/reminder-adapter-boundary.md','docs/prd-traceability.md','docs/product-spec.md','docs/implementation-plan.md','contracts/governance/capabilities.json','contracts/identity/permission-boundary.json','contracts/privacy/persistence-boundary.json','contracts/persistence/protected-state-boundary.json','contracts/evidence/identity-boundary.json','contracts/domain/policy-state-boundary.json','contracts/synchronization/mail-sync-boundary.json','contracts/support/support-matrix.json','contracts/build-skeleton/skeleton.json')
$script:count=0;$script:fail=[Collections.Generic.List[string]]::new()
function New-Root{$root=Join-Path ([IO.Path]::GetTempPath()) ('openloops-reminder-'+[guid]::NewGuid().ToString('N'));[IO.Directory]::CreateDirectory($root)|Out-Null;foreach($rel in $inputs){$target=Join-Path $root $rel;[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target))|Out-Null;Copy-Item -LiteralPath (Join-Path $repoRoot $rel) -Destination $target};$root}
function Check([string]$root){$a=@('-NoProfile','-File',$checker,'-ManifestPath',(Join-Path $root 'contracts/reminder/adapter-boundary.json'),'-AdrPath',(Join-Path $root 'docs/adr/ADR-009-reminder-adapters.md'),'-ThreatPath',(Join-Path $root 'docs/threat-model/reminder-adapter-boundary.md'),'-TraceabilityPath',(Join-Path $root 'docs/prd-traceability.md'),'-ProductSpecPath',(Join-Path $root 'docs/product-spec.md'),'-ImplementationPlanPath',(Join-Path $root 'docs/implementation-plan.md'),'-GovernancePath',(Join-Path $root 'contracts/governance/capabilities.json'),'-PermissionPath',(Join-Path $root 'contracts/identity/permission-boundary.json'),'-PrivacyPath',(Join-Path $root 'contracts/privacy/persistence-boundary.json'),'-PersistencePath',(Join-Path $root 'contracts/persistence/protected-state-boundary.json'),'-EvidencePath',(Join-Path $root 'contracts/evidence/identity-boundary.json'),'-PolicyPath',(Join-Path $root 'contracts/domain/policy-state-boundary.json'),'-SyncPath',(Join-Path $root 'contracts/synchronization/mail-sync-boundary.json'),'-SupportPath',(Join-Path $root 'contracts/support/support-matrix.json'),'-BuildPath',(Join-Path $root 'contracts/build-skeleton/skeleton.json'),'-Quiet');$o=@(& pwsh @a 2>&1|ForEach-Object{[string]$_});[pscustomobject]@{Code=$LASTEXITCODE;Output=($o-join"`n")}}
function JsonCase([string]$name,[string]$id,[string]$rel,[scriptblock]$mutate){$script:count++;$root=New-Root;try{$path=Join-Path $root $rel;$v=Get-Content -Raw -LiteralPath $path|ConvertFrom-Json -Depth 100;&$mutate $v;$v|ConvertTo-Json -Depth 100|Set-Content -LiteralPath $path -Encoding utf8NoBOM;$r=Check $root;if($r.Code-eq 0-or$r.Output-notmatch[regex]::Escape($id)){$script:fail.Add("$name (expected $id; exit $($r.Code))")}}finally{Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue}}
function TextCase([string]$name,[string]$id,[string]$rel,[scriptblock]$mutate){$script:count++;$root=New-Root;try{$path=Join-Path $root $rel;$changed=&$mutate (Get-Content -Raw -LiteralPath $path);Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM;$r=Check $root;if($r.Code-eq 0-or$r.Output-notmatch[regex]::Escape($id)){$script:fail.Add("$name (expected $id; exit $($r.Code))")}}finally{Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue}}
$base=& pwsh -NoProfile -File $checker 2>&1;if($LASTEXITCODE-ne 0){throw('Baseline reminder checker failed: '+(@($base)-join"`n"))}

JsonCase 'work item drift' 'P0-REMINDER-INVENTORY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.work_item='P0-WI-X'}
JsonCase 'owner removed' 'P0-REMINDER-INVENTORY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.owner_decisions=@('OWN-03')}
JsonCase 'owned requirement removed' 'P0-REMINDER-INVENTORY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.owned_requirements=@($c.owned_requirements|Where-Object{$_-ne'OL-REM-005'})}
JsonCase 'dependent AC completed' 'P0-REMINDER-INVENTORY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.dependent_acceptance_criteria+='AC-X'}
JsonCase 'scenario dependency passed' 'P0-REMINDER-INVENTORY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.scenario_dependencies[0].status='passed'}
JsonCase 'gate removed' 'P0-REMINDER-INVENTORY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.blocking_gates=@($c.blocking_gates|Where-Object{$_-ne'G-PRIV'})}

JsonCase 'record added' 'P0-REMINDER-SCHEMA-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.record_contracts += ($c.record_contracts[0]|Select-Object *)}
JsonCase 'raw title field persisted' 'P0-REMINDER-SCHEMA-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.record_contracts[0].exact_fields_in_order+='title'}
JsonCase 'unknown fields allowed' 'P0-REMINDER-SCHEMA-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.record_contracts[1].unknown_fields='allow'}
JsonCase 'attempt bound widened' 'P0-REMINDER-SCHEMA-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.record_contracts[1].bounds.attempt_count_max=99}
JsonCase 'privacy mapping drift' 'P0-REMINDER-SCHEMA-001' 'contracts/privacy/persistence-boundary.json' {param($c)($c.record_types|Where-Object{$_.id -eq 'reminder_link'}).field_groups[0].fields=@('reminder_link_id')}
JsonCase 'invented reminder time value' 'P0-REMINDER-DIRECT-EDIT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.record_contracts[0].exact_fields_in_order+='reminder_time'}
JsonCase 'invented reminder time digest' 'P0-REMINDER-DIRECT-EDIT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.record_contracts[0].exact_fields_in_order+='reminder_time_digest_hmac'}
JsonCase 'invented reminder time override' 'P0-REMINDER-DIRECT-EDIT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.record_contracts[0].exact_fields_in_order+='reminder_time_override'}

JsonCase 'generic operation kind' 'P0-REMINDER-CATALOGS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.catalogs.operation_kind_code+='generic'}
JsonCase 'send operation kind' 'P0-REMINDER-CATALOGS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.catalogs.operation_kind_code+='send'}
JsonCase 'delete operation kind' 'P0-REMINDER-CATALOGS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.catalogs.operation_kind_code+='delete'}
JsonCase 'invitation response kind' 'P0-REMINDER-CATALOGS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.catalogs.operation_kind_code+='respond'}

JsonCase 'product permission becomes Tasks.Read' 'P0-REMINDER-TODO-BASELINE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.todo_boundary.permission='Tasks.Read'}
JsonCase 'diagnostic combines scopes' 'P0-REMINDER-TODO-BASELINE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.todo_boundary.read_diagnostic='combine Tasks.Read and Tasks.ReadWrite'}
JsonCase 'delta required' 'P0-REMINDER-TODO-BASELINE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.todo_boundary.reconciliation_baseline='delta required'}
JsonCase 'guessed default list' 'P0-REMINDER-TODO-BASELINE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.todo_boundary.collection_selection='guess Tasks'}
JsonCase 'permission enabled' 'P0-REMINDER-TODO-BASELINE-001' 'contracts/identity/permission-boundary.json' {param($c)($c.permission_rows|Where-Object{$_.id -eq 'PERM-TODO-WRITE-001'}).state='enabled'}

JsonCase 'operation key misses account' 'P0-REMINDER-OPERATION-PROTOCOL-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.operation_protocol.intent_hmac_input_order=@($c.operation_protocol.intent_hmac_input_order|Where-Object{$_-ne'account_ref'})}
JsonCase 'operation key misses loop version' 'P0-REMINDER-OPERATION-PROTOCOL-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.operation_protocol.intent_hmac_input_order=@($c.operation_protocol.intent_hmac_input_order|Where-Object{$_-ne'expected_loop_version'})}
JsonCase 'operation key misses auth generation' 'P0-REMINDER-OPERATION-PROTOCOL-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.operation_protocol.intent_hmac_input_order=@($c.operation_protocol.intent_hmac_input_order|Where-Object{$_-ne'authorization_generation'})}
JsonCase 'operation key misses recreate generation' 'P0-REMINDER-OPERATION-PROTOCOL-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.operation_protocol.intent_hmac_input_order=@($c.operation_protocol.intent_hmac_input_order|Where-Object{$_-ne'recreate_generation'})}
JsonCase 'request before ledger' 'P0-REMINDER-OPERATION-PROTOCOL-001' 'contracts/reminder/adapter-boundary.json' {param($c)[array]::Reverse($c.operation_protocol.durability_order)}
JsonCase 'ambiguous cursor advances' 'P0-REMINDER-OPERATION-PROTOCOL-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.operation_protocol.cursor_rule='advance cursor'}
JsonCase 'manual draft persisted' 'P0-REMINDER-OPERATION-PROTOCOL-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.operation_protocol.manual_draft_rule='persist plaintext and resume'}

JsonCase 'raw marker persisted' 'P0-REMINDER-AMBIGUOUS-WRITE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.remote_marker_protocol.persisted_verifier='raw marker'}
JsonCase 'random unrecoverable marker' 'P0-REMINDER-AMBIGUOUS-WRITE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.remote_marker_protocol.derivation='random value unavailable after crash'}
JsonCase 'title identity matching' 'P0-REMINDER-AMBIGUOUS-WRITE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.remote_marker_protocol.enumeration_rule='match title and body'}
JsonCase 'incomplete means zero' 'P0-REMINDER-AMBIGUOUS-WRITE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.remote_marker_protocol.enumeration_rule='incomplete paging is zero'}
JsonCase 'ambiguous auto retry' 'P0-REMINDER-AMBIGUOUS-WRITE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.remote_marker_protocol.outcomes.complete_many='retry automatically'}
JsonCase 'automatic recreate' 'P0-REMINDER-AMBIGUOUS-WRITE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.remote_marker_protocol.recreate_rule='automatically recreate same operation'}

JsonCase 'service overwrites edited title' 'P0-REMINDER-OWNERSHIP-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.ownership_matrix[0].direct_edit='service_projection';$c.ownership_matrix[0].service_update='allowed'}
JsonCase 'manual title service-owned' 'P0-REMINDER-OWNERSHIP-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.ownership_matrix[6].initial='service_projection'}
JsonCase 'stale PATCH allowed' 'P0-REMINDER-CONFLICT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.direct_edit_and_conflict.stale_or_changed='PATCH latest values'}
JsonCase 'full object overwrite' 'P0-REMINDER-CONFLICT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.direct_edit_and_conflict.partial_update='replace whole artifact'}
JsonCase 'completion closes loop' 'P0-REMINDER-DIRECT-EDIT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.direct_edit_and_conflict.completion='close loop automatically'}
JsonCase 'automatic delete enabled' 'P0-REMINDER-DIRECT-EDIT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.direct_edit_and_conflict.automatic_delete='enabled'}
JsonCase 'reminder time persisted' 'P0-REMINDER-DIRECT-EDIT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.reminder_time_boundary.persistence='persist reminder time'}

JsonCase 'manual source before proof' 'P0-REMINDER-MANUAL-SOURCE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.manual_artifact_protocol.atomic_success='create loop before request'}
JsonCase 'failed manual loop persists' 'P0-REMINDER-MANUAL-SOURCE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.manual_artifact_protocol.failure='retain draft and loop'}
JsonCase 'ambiguous manual auto recreate' 'P0-REMINDER-MANUAL-SOURCE-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.manual_artifact_protocol.ambiguous='recreate automatically'}

JsonCase 'calendar guessed scope' 'P0-REMINDER-CALENDAR-SAFETY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.calendar_boundary.permission='Calendars.ReadWrite requested now'}
JsonCase 'calendar attendees' 'P0-REMINDER-CALENDAR-SAFETY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.calendar_boundary.reminder_event='invite attendees and request responses'}
JsonCase 'invitation mutated' 'P0-REMINDER-CALENDAR-SAFETY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.calendar_boundary.invitation_event='accept and update invitation'}
JsonCase 'calendar completion invented' 'P0-REMINDER-CALENDAR-SAFETY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.calendar_boundary.completion='complete event'}

JsonCase 'review link bearer token' 'P0-REMINDER-REVIEW-LINK-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.review_link_boundary.payload='bearer token'}
JsonCase 'review link authority' 'P0-REMINDER-REVIEW-LINK-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.review_link_boundary.authority='Graph mutation authority'}
JsonCase 'review link activated' 'P0-REMINDER-REVIEW-LINK-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.review_link_boundary.activation='enabled'}

JsonCase 'privacy record removed' 'P0-REMINDER-PRIVACY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.privacy_boundary.approved_record_types=@('reminder_link')}
JsonCase 'prompt permitted' 'P0-REMINDER-PRIVACY-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.privacy_boundary.prohibited_values=@($c.privacy_boundary.prohibited_values|Where-Object{$_-notlike'prompt model*'})}
JsonCase 'runtime enabled' 'P0-REMINDER-CLAIMS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.runtime_boundary.adapter_runtime=$true}
JsonCase 'persistent record claimed' 'P0-REMINDER-CLAIMS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.runtime_boundary.persistent_records_created=@('operation_ledger')}
JsonCase 'Graph call claimed' 'P0-REMINDER-CLAIMS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.runtime_boundary.graph_office_calls=@('todo.create')}
JsonCase 'permission claimed' 'P0-REMINDER-CLAIMS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.runtime_boundary.permissions_requested=@('Tasks.ReadWrite')}
JsonCase 'gate claimed' 'P0-REMINDER-CLAIMS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.runtime_boundary.gates_passed=@('G-TODO')}
JsonCase 'support claimed' 'P0-REMINDER-CLAIMS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.claims.todo_collections_supported=@('Tasks')}
JsonCase 'conditional write claimed' 'P0-REMINDER-CLAIMS-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.claims.conditional_writes_proven=@('todo')}

JsonCase 'ADR-009 planned again' 'P0-REMINDER-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' {param($c)($c.adrs|Where-Object{$_.id -eq 'ADR-009'}).status='planned'}
JsonCase 'persistence runtime accepted' 'P0-REMINDER-CROSS-CONTRACT-001' 'contracts/persistence/protected-state-boundary.json' {param($c)$c.logical_schema_boundary.logical_runtime_schema_approved=$true}
JsonCase 'policy gains Graph operation' 'P0-REMINDER-CROSS-CONTRACT-001' 'contracts/domain/policy-state-boundary.json' {param($c)$c.reminder_boundary.graph_office_permissions_calls_operations=@('todo.create')}

TextCase 'spec contradiction appended' 'P0-REMINDER-INVENTORY-001' 'docs/product-spec.md' {param($t)$t+"`nContrary rule: ambiguous reminder writes retry automatically and reminder completion closes the loop.`n"}
TextCase 'plan contradiction appended' 'P0-REMINDER-INVENTORY-001' 'docs/implementation-plan.md' {param($t)$t+"`nP0-WI-12 enables Graph reminder runtime and Calendar invitations.`n"}
TextCase 'ADR contradiction appended' 'P0-REMINDER-CROSS-CONTRACT-001' 'docs/adr/ADR-009-reminder-adapters.md' {param($t)$t+"`nContrary decision: persist manual drafts and overwrite user edits.`n"}
TextCase 'trace contradiction appended' 'P0-REMINDER-INVENTORY-001' 'docs/prd-traceability.md' {param($t)$t+"`nP0-WI-12 passed every gate and ships reminder writes.`n"}
TextCase 'fresh checker preapproved' 'P0-REMINDER-FRESH-CHECKER-001' 'docs/prd-traceability.md' {param($t)$t-replace'Pending closure','Passed without independent review'}

if($script:fail.Count-gt 0){[Console]::Error.WriteLine(('FAIL: reminder adapter synthetic mutations: '+($script:fail-join'; ')));exit 1};Write-Output ('PASS: reminder adapter synthetic mutations ({0} rejection cases; OS temp only)' -f $script:count)
