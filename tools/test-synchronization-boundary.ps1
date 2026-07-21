[CmdletBinding()]param()
Set-StrictMode -Version Latest;$ErrorActionPreference='Stop'
$repoRoot=(& git rev-parse --show-toplevel 2>$null).Trim();if($LASTEXITCODE-ne0){throw'Run inside repository.'}
$checker=Join-Path $repoRoot 'tools/check-synchronization-boundary.ps1';$baseline=Get-Content -Raw (Join-Path $repoRoot 'contracts/synchronization/mail-sync-boundary.json')|ConvertFrom-Json
$tempBase=[IO.Path]::GetFullPath([IO.Path]::GetTempPath());$tempRoot=[IO.Path]::GetFullPath((Join-Path $tempBase ('openloops-sync-'+[guid]::NewGuid().ToString('N'))));if(-not$tempRoot.StartsWith($tempBase,[StringComparison]::OrdinalIgnoreCase)){throw'Unsafe temp path.'};[void](New-Item -ItemType Directory $tempRoot);$caseNumber=0
function Case([string]$Name,[string]$Expected,[scriptblock]$Mutate){$script:caseNumber++;$c=$baseline|ConvertTo-Json -Depth 100|ConvertFrom-Json;&$Mutate $c;$path=Join-Path $tempRoot "case-$caseNumber.json";$c|ConvertTo-Json -Depth 100|Set-Content $path -Encoding utf8NoBOM;$out=(&pwsh -NoProfile -File $checker -ManifestPath $path -Quiet 2>&1|Out-String);if($LASTEXITCODE-eq0-or$out-notmatch[regex]::Escape($Expected)){throw"Synthetic sync case failed to trigger $Expected`: $Name"}}
function DocCase([string]$Name,[string]$Expected,[string]$Source,[string]$Arg,[scriptblock]$Mutate){$script:caseNumber++;$t=Get-Content -Raw (Join-Path $repoRoot $Source);$t=&$Mutate $t;$path=Join-Path $tempRoot ("doc-$caseNumber"+[IO.Path]::GetExtension($Source));Set-Content $path $t -Encoding utf8NoBOM;$out=(&pwsh -NoProfile -File $checker $Arg $path -Quiet 2>&1|Out-String);if($LASTEXITCODE-eq0-or$out-notmatch[regex]::Escape($Expected)){throw"Synthetic sync document case failed to trigger $Expected`: $Name"}}
try{
 &pwsh -NoProfile -File $checker -Quiet;if($LASTEXITCODE){throw'Valid sync boundary failed.'}
 Case 'unknown field' 'P0-SYNC-INVENTORY-001' {param($c)$c|Add-Member extra synthetic}
 Case 'work item drift' 'P0-SYNC-INVENTORY-001' {param($c)$c.work_item='P0-WI-99'}
 Case 'owner omitted' 'P0-SYNC-INVENTORY-001' {param($c)$c.owner_decisions=@('OWN-09')}
 Case 'requirement omitted' 'P0-SYNC-INVENTORY-001' {param($c)$c.requirements=@($c.requirements|Select-Object -Skip 1)}
 Case 'collection omitted' 'P0-SYNC-INVENTORY-001' {param($c)$c.collection_contracts=@($c.collection_contracts|Select-Object -Skip 1)}
 Case 'collection enabled' 'P0-SYNC-INVENTORY-001' {param($c)$c.collection_contracts[0].state='enabled'}
 Case 'collection advertised' 'P0-SYNC-INVENTORY-001' {param($c)$c.collection_contracts[1].advertised=$true}
 Case 'checkpoint field omitted' 'P0-SYNC-CHECKPOINT-001' {param($c)$c.checkpoint_contract.fields=@($c.checkpoint_contract.fields|Where-Object{$_-ne'query_fingerprint_hmac'})}
 Case 'checkpoint shared' 'P0-SYNC-CHECKPOINT-001' {param($c)$c.checkpoint_contract.cardinality='one shared checkpoint'}
 Case 'cursor reconstructed' 'P0-SYNC-CHECKPOINT-001' {param($c)$c.checkpoint_contract.continuation='reconstruct URL'}
 Case 'cursor plaintext' 'P0-SYNC-DELTA-001' {param($c)$c.delta_protocol.plaintext_cursor_persistence='allowed'}
 Case 'cursor logged' 'P0-SYNC-DELTA-001' {param($c)$c.delta_protocol.cursor_logging='allowed'}
 Case 'mailbox-wide delta' 'P0-SYNC-DELTA-001' {param($c)$c.poll_policy.mode='mailbox_wide'}
 Case 'webhook fallback' 'P0-SYNC-DELTA-001' {param($c)$c.poll_policy.webhooks='fallback'}
 Case 'ordering assumed' 'P0-SYNC-DELTA-001' {param($c)$c.delta_protocol.ordering_assumption='chronological'}
 Case 'unbounded budgets' 'P0-SYNC-DELTA-001' {param($c)$c.cycle_budgets.status='unbounded'}
 Case 'last complete on partial page' 'P0-SYNC-DELTA-001' {param($c)$c.delta_protocol.completion='nextLink advances last_complete_at'}
 Case 'arbitrary error triggers resync' 'P0-SYNC-DELTA-001' {param($c)$c.delta_protocol.cursor_invalid_trigger='any HTTP 400 triggers resync'}
 Case 'tombstone closes loop' 'P0-SYNC-DELTA-001' {param($c)$c.delta_protocol.tombstones='close loop'}
 Case 'initial unread analyzed' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.initial_or_backfill.incoming_unread='analyze now'}
 Case 'initial already-read omitted' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.initial_or_backfill.incoming_already_read='ignore'}
 Case 'initial sent copy omitted' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.initial_or_backfill.saved_sent_copy='ignore'}
 Case 'backfill automatic mutation' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.initial_or_backfill.automatic_mutation='allowed'}
 Case 'new unread analyzed' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.post_baseline_incoming.first_observed_unread='analyze'}
 Case 'unread-to-read omitted' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.post_baseline_incoming.observed_unread_then_read='ignore'}
 Case 'first observed read ignored' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.post_baseline_incoming.first_observed_already_read='ignore'}
 Case 'read means attention' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.post_baseline_incoming.isRead_meaning='proof of review'}
 Case 'draft activates' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.post_baseline_outgoing.draft_or_compose='activate'}
 Case 'saved sent first observation omitted' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.post_baseline_outgoing.saved_sent_copy_first_observed='ignore'}
 Case 'sent means delivered' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.post_baseline_outgoing.claim_language='delivered'}
 Case 'idempotency account binding omitted' 'P0-SYNC-ELIGIBILITY-001' {param($c)$c.eligibility_policy.idempotency='digest locator only'}
 Case 'recoverable quarantine removed' 'P0-SYNC-TRANSACTION-001' {param($c)$c.page_transaction.durable_safe_outcomes=@($c.page_transaction.durable_safe_outcomes|Where-Object{$_-ne'recoverable_quarantine'})}
 Case 'ambiguous write terminal' 'P0-SYNC-TRANSACTION-001' {param($c)$c.page_transaction.nonterminal_outcomes=@($c.page_transaction.nonterminal_outcomes|Where-Object{$_-ne'ambiguous_write'})}
 Case 'cursor advances early' 'P0-SYNC-TRANSACTION-001' {param($c)$c.page_transaction.cursor_advance_rule='advance before items'}
 Case 'atomic step reordered' 'P0-SYNC-TRANSACTION-001' {param($c)$c.page_transaction.steps[5]='replace checkpoint before item outcomes'}
 Case 'whole page failure advances' 'P0-SYNC-TRANSACTION-001' {param($c)$c.page_transaction.whole_page_failure='advance checkpoint'}
 Case 'crash loses page' 'P0-SYNC-TRANSACTION-001' {param($c)$c.page_transaction.crash_rule='skip page'}
 Case 'Retry-After ignored' 'P0-SYNC-RESILIENCE-001' {param($c)$c.retry_policy.retry_after='ignore'}
 Case 'no jitter' 'P0-SYNC-RESILIENCE-001' {param($c)$c.retry_policy.missing_or_invalid_retry_after='fixed immediate retry'}
 Case 'HTTP 400 made retryable' 'P0-SYNC-RESILIENCE-001' {param($c)$c.retry_policy.retryable_classes+=@('http_400')}
 Case 'approved retryable omitted' 'P0-SYNC-RESILIENCE-001' {param($c)$c.retry_policy.retryable_classes=@($c.retry_policy.retryable_classes|Where-Object{$_-ne'http_503'})}
 Case 'nonretryable omitted' 'P0-SYNC-RESILIENCE-001' {param($c)$c.retry_policy.nonretryable_classes=@($c.retry_policy.nonretryable_classes|Where-Object{$_-ne'account_mismatch'})}
 Case 'permission denial made retryable' 'P0-SYNC-RESILIENCE-001' {param($c)$c.retry_policy.nonretryable_classes=@($c.retry_policy.nonretryable_classes|Where-Object{$_-ne'permission_denied'});$c.retry_policy.retryable_classes+=@('permission_denied')}
 Case 'raw error logging' 'P0-SYNC-RESILIENCE-001' {param($c)$c.retry_policy.raw_error_url_header_body_logging='allowed'}
 Case 'quarantine locator omitted' 'P0-SYNC-RESILIENCE-001' {param($c)$c.quarantine_policy.approved_fields=@($c.quarantine_policy.approved_fields|Where-Object{$_-ne'quarantined_locator'})}
 Case 'cursor skips quarantine' 'P0-SYNC-RESILIENCE-001' {param($c)$c.quarantine_policy.before_cursor_advance='advance anyway'}
 Case 'failed bounded accepted' 'P0-SYNC-RESILIENCE-001' {param($c)$c.quarantine_policy.failed_bounded_without_recoverability='allowed'}
 Case 'resolved health left dangling' 'P0-SYNC-RESILIENCE-001' {param($c)$c.quarantine_policy.resolution_cleanup='retain job_health quarantine_ref'}
 Case 'disconnect retains quarantine' 'P0-SYNC-RESILIENCE-001' {param($c)$c.quarantine_policy.disconnect_cleanup='return to observation retention'}
 Case 'complete coverage claimed' 'P0-SYNC-COVERAGE-001' {param($c)$c.reconciliation_policy.completeness_claim='complete'}
 Case 'rename by display name' 'P0-SYNC-COVERAGE-001' {param($c)$c.reconciliation_policy.folder_rename='match display name'}
 Case 'folder checkpoint retained' 'P0-SYNC-COVERAGE-001' {param($c)$c.reconciliation_policy.folder_missing_or_deleted='retain checkpoint'}
 Case 'known loss omitted' 'P0-SYNC-COVERAGE-001' {param($c)$c.reconciliation_policy.known_unrecoverable=@($c.reconciliation_policy.known_unrecoverable|Select-Object -Skip 1)}
 Case 'replay trigger omitted' 'P0-SYNC-REPLAY-001' {param($c)$c.replay_policy.triggers=@($c.replay_policy.triggers|Select-Object -Skip 1)}
 Case 'replay automatic reminder' 'P0-SYNC-REPLAY-001' {param($c)$c.replay_policy.automatic_reminders='allowed'}
 Case 'silent replay mutation' 'P0-SYNC-REPLAY-001' {param($c)$c.replay_policy.silent_retroactive_mutation='allowed'}
 Case 'overlapping scheduler' 'P0-SYNC-SCHEDULING-001' {param($c)$c.poll_policy.single_flight='overlap allowed'}
 Case 'cadence drift' 'P0-SYNC-SCHEDULING-001' {param($c)$c.poll_policy.cadence_seconds.default=10}
 Case 'inactive background claim' 'P0-SYNC-SCHEDULING-001' {param($c)$c.poll_policy.background_boundary='always covered'}
 Case 'Graph call claim' 'P0-SYNC-CLAIMS-001' {param($c)$c.claims.graph_calls=@('synthetic')}
 Case 'configured folder claim' 'P0-SYNC-CLAIMS-001' {param($c)$c.claims.configured_collections=@('synthetic')}
 Case 'runtime transport enabled' 'P0-SYNC-CLAIMS-001' {param($c)$c.runtime_boundary.graph_transport=$true}
 Case 'network origin configured' 'P0-SYNC-CLAIMS-001' {param($c)$c.runtime_boundary.network_origins=@('synthetic-origin')}
 Case 'deferred transaction owner drift' 'P0-SYNC-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.encryption_and_transaction_implementation='ADR-006'}
 Case 'deferred permission owner drift' 'P0-SYNC-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.permissions_and_folder_selection='ADR-004'}
 Case 'deferred identity owner drift' 'P0-SYNC-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.message_identity_exact_fields_and_anchors='ADR-004'}
 Case 'deferred mutation owner drift' 'P0-SYNC-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.reminder_mutation_and_reconciliation='ADR-004'}
 Case 'deferred calendar gate drift' 'P0-SYNC-CROSS-CONTRACT-001' {param($c)$c.separate_decisions.calendar_invitation_correlation='ADR-006 only'}
 DocCase 'privacy quarantine lifecycle drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/privacy/persistence-boundary.json' '-PrivacyPath' {param($t)$t-replace'"retention": "unresolved_quarantine"','"retention": "observation_window_plus_15"'}
 DocCase 'governance ADR drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"id": "ADR-004", "status": "accepted"','"id": "ADR-004", "status": "planned"'}
 DocCase 'governance owner mapping drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"id": "OWN-09", "status": "accepted", "adrs": \["ADR-004"\]','"id": "OWN-09", "status": "accepted", "adrs": ["ADR-999"]'}
 DocCase 'OWN-09 gate drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"id": "OWN-09", "status": "accepted", "adrs": \["ADR-004"\], "gates": \["G-MAIL"\]','"id": "OWN-09", "status": "accepted", "adrs": ["ADR-004"], "gates": ["G-CAL"]'}
 DocCase 'OWN-05 ADR drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"id": "OWN-05", "status": "accepted", "adrs": \["ADR-004", "ADR-006", "ADR-009"\]','"id": "OWN-05", "status": "accepted", "adrs": ["ADR-006", "ADR-009"]'}
 DocCase 'OWN-05 gate drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"id": "OWN-05", "status": "accepted", "adrs": \["ADR-004", "ADR-006", "ADR-009"\], "gates": \["G-CAL", "G-MAIL"\]','"id": "OWN-05", "status": "accepted", "adrs": ["ADR-004", "ADR-006", "ADR-009"], "gates": ["G-MAIL"]'}
 DocCase 'mail threat ADR drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"adrs": \["ADR-003", "ADR-004", "ADR-006"\]','"adrs": ["ADR-003", "ADR-006"]'}
 DocCase 'mail threat gate drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'"gates": \["G-MAIL", "G-PRIV"\]','"gates": ["G-MAIL"]'}
 DocCase 'mail threat prohibition drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' {param($t)$t-replace'no real content in fixtures, logs, or state; no unbounded fetch or complete-event-observation claim','complete observation may be claimed'}
 DocCase 'build transport drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/build-skeleton/skeleton.json' '-BuildPath' {param($t)$t-replace'"graph_transport": false','"graph_transport": true'}
 DocCase 'ADR retention language drift' 'P0-SYNC-CROSS-CONTRACT-001' 'docs/adr/ADR-004-synchronization.md' '-AdrPath' {param($t)$t-replace'deleted immediately if that\s+window elapsed','retained indefinitely'}
 DocCase 'privacy resolved lifecycle drift' 'P0-SYNC-CROSS-CONTRACT-001' 'contracts/privacy/persistence-boundary.json' '-PrivacyPath' {param($t)$t-replace'"when": "retry_succeeded_dismissed_or_recovery_complete", "retention": "observation_window_plus_15"','"when": "retry_succeeded_dismissed_or_recovery_complete", "retention": "retain_forever"'}
 DocCase 'trace check removed' 'P0-SYNC-INVENTORY-001' 'docs/prd-traceability.md' '-TraceabilityPath' {param($t)$t-replace'(?m)^\| P0-SYNC-DELTA-001 \|.*\r?\n',''}
 Write-Host "OpenLoops synchronization negative suite passed ($caseNumber synthetic rejection cases)."
}finally{if([IO.Directory]::Exists($tempRoot)){[IO.Directory]::Delete($tempRoot,$true)}}
