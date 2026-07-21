[CmdletBinding()]
param(
 [string]$ManifestPath='contracts/addin/bridge-boundary.json',
 [string]$AdrPath='docs/adr/ADR-010-add-in-bridge.md',
 [string]$ThreatPath='docs/threat-model/addin-bridge.md',
 [string]$TraceabilityPath='docs/prd-traceability.md',
 [string]$ProductSpecPath='docs/product-spec.md',
 [string]$ImplementationPlanPath='docs/implementation-plan.md',
 [string]$GovernancePath='contracts/governance/capabilities.json',
 [string]$PersistencePath='contracts/persistence/protected-state-boundary.json',
 [string]$EvidencePath='contracts/evidence/identity-boundary.json',
 [string]$ReminderPath='contracts/reminder/adapter-boundary.json',
 [string]$PrivacyPath='contracts/privacy/persistence-boundary.json',
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
$checks=@('P0-BRIDGE-INVENTORY-001','P0-BRIDGE-TRANSPORT-001','P0-BRIDGE-BOOTSTRAP-001','P0-BRIDGE-SESSION-001','P0-BRIDGE-CERTIFICATE-001','P0-BRIDGE-REVIEW-LINK-001','P0-BRIDGE-CONTENT-001','P0-BRIDGE-FALLBACK-001','P0-BRIDGE-PRIVACY-001','P0-BRIDGE-CROSS-CONTRACT-001','P0-BRIDGE-CLAIMS-001','P0-BRIDGE-FRESH-CHECKER-001')

$m=Json $ManifestPath 'P0-BRIDGE-INVENTORY-001';if($null-eq$m){[Console]::Error.WriteLine('BLOCKED: bridge manifest parse failed.');exit 1}
$canonical=$m|ConvertTo-Json -Depth 100 -Compress;$hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
if($hash-ne'a8e86c9cba3a1f2a8c1ebf1a382503e3e43283b0bdd2877894511accc42cae8a'){Fail 'P0-BRIDGE-INVENTORY-001'}
if($m.schema_version-ne 1-or$m.work_item-ne'P0-WI-13'-or$m.adr-ne'ADR-010'-or$m.decision_status-ne'accepted_contract_runtime_unimplemented'-or$m.snapshot_date-ne'2026-07-21'){Fail 'P0-BRIDGE-INVENTORY-001'}
Ordered @($m.owner_decisions) @('OWN-01') 'P0-BRIDGE-INVENTORY-001'
Ordered @($m.owned_requirements) @('OL-GOV-001','OL-UX-006','OL-UX-007') 'P0-BRIDGE-INVENTORY-001'
Ordered @($m.consumed_requirements) @('OL-REM-016') 'P0-BRIDGE-INVENTORY-001'
Ordered @($m.primary_acceptance_criteria) @('AC-11','AC-12') 'P0-BRIDGE-INVENTORY-001';Empty @($m.dependent_acceptance_criteria) 'P0-BRIDGE-INVENTORY-001';Empty @($m.primary_scenarios) 'P0-BRIDGE-INVENTORY-001';Empty @($m.scenario_dependencies) 'P0-BRIDGE-INVENTORY-001'
Ordered @($m.blocking_gates) @('G-ADDIN','G-PRIV') 'P0-BRIDGE-INVENTORY-001'
Ordered @($m.input_authorities.owner) @('ADR-001','ADR-003','ADR-005','ADR-006','ADR-009','ADR-PRIV-001') 'P0-BRIDGE-INVENTORY-001'
Ordered @($m.sources.id) @('SRC-SPEC-ADDIN','SRC-SPEC-UX','SRC-PLAN-ADDIN','SRC-PLAN-ADR','SRC-SUPPORT-MATRIX','SRC-THREAT-INDEX','SRC-RESEARCH-CONNECTION','SRC-RESEARCH-SECURITY') 'P0-BRIDGE-INVENTORY-001'
$sd=$m.separate_decisions;if($sd.exact_transport_selection_and_loopback_mechanics-ne'G-ADDIN'-or$sd.asset_hosting_manifest_and_client_matrix-ne'G-ADDIN'-or$sd.certificate_issuance_rotation_and_expiry_mechanics-ne'G-ADDIN'-or$sd.automation_modes_eligibility_evaluation_and_flags-ne'ADR-011'-or$sd.self_email_send_recipient_and_operation_protocol-ne'ADR-013'-or$sd.persistent_field_allowlist_and_retention-ne'ADR-PRIV-001'-or$sd.storage_encryption_atomicity_and_rollback-ne'ADR-005'){Fail 'P0-BRIDGE-INVENTORY-001'}

$c=$m.catalogs
Ordered @($c.transport_candidate_code) @('named_pipe_companion_internal','loopback_web_bridge_addin_candidate') 'P0-BRIDGE-TRANSPORT-001'
Ordered @($c.bootstrap_binding_field) @('expected_packaged_companion','office_addin_origin_and_client','os_user','opaque_account_reference','single_use_nonce','expiry','explicit_user_verifiable_action') 'P0-BRIDGE-BOOTSTRAP-001'
Ordered @($c.session_property) @('least_authority','command_scoped','in_memory_only','rotated_on_reconnect','revoked_on_account_change') 'P0-BRIDGE-SESSION-001'
Ordered @($c.certificate_prohibition) @('machine_wide_trust','general_purpose_trusted_root_with_retained_signing_key','exportable_private_key','broad_or_inherited_acl') 'P0-BRIDGE-CERTIFICATE-001'
Ordered @($c.review_link_activation_property) @('random_non_secret_opaque_handle','authenticated_per_user_session','account_bound_resolution','no_standalone_authority') 'P0-BRIDGE-REVIEW-LINK-001'
Ordered @($c.content_prohibition) @('browser_local_storage','indexeddb','service_worker_cache','url_query_or_fragment','console_output','analytics','crash_report','office_roaming_settings') 'P0-BRIDGE-CONTENT-001'
Ordered @($c.native_fallback_trigger) @('bridge_unavailable','addin_unavailable','unsupported_client','disconnect','uninstall','gate_failed') 'P0-BRIDGE-FALLBACK-001'

Has ($m.transport_boundary|ConvertTo-Json -Depth 10 -Compress) @('named pipes for companion-internal and native-UI traffic','sole candidate transport for the Office add-in','only if the runnable packaged G-ADDIN spike proves','a named pipe is reachable from Office.js','Graph delegated scopes authorize add-in operations','a remotely reachable service','LAN or non-loopback binding','exact transport selection','loopback host and port','certificate mechanics','discovery mechanism','per-client behavior') 'P0-BRIDGE-TRANSPORT-001'

Has ($m.bootstrap_boundary|ConvertTo-Json -Depth 10 -Compress) @('URL parameters','Office roaming settings','localStorage','repository configuration','at most one bound pairing','fail-closed rejection','single use is consumed or its expiry passes','rate- and attempt-bounded','fails closed rather than degrading into an open window','no unpaired command exists','requires a bound, live session') 'P0-BRIDGE-BOOTSTRAP-001'
Ordered @($m.bootstrap_boundary.bound_fields) @('expected_packaged_companion','office_addin_origin_and_client','os_user','opaque_account_reference','single_use_nonce','expiry','explicit_user_verifiable_action') 'P0-BRIDGE-BOOTSTRAP-001'

Has ($m.session_boundary|ConvertTo-Json -Depth 10 -Compress) @('least-authority command-scoped in-memory sessions','review, evidence navigation, settings, and status','held in memory only','never written to the database, a DPAPI blob, disk, or a log','rotates on reconnect','revoked on account change','rejected, not silently reused','same-user boundary only','not against code already running as the same authenticated OS user') 'P0-BRIDGE-SESSION-001'
Ordered @($m.session_boundary.prohibited_authority) @('generic authority','synchronization authority') 'P0-BRIDGE-SESSION-001'

Has ($m.certificate_boundary|ConvertTo-Json -Depth 10 -Compress) @('current-user only','limited to the exact tested loopback names','non-exportable private key','user and application','explicit issue, rotation, expiry, and replacement','completely removed on disconnect and uninstall','blocks the add-in') 'P0-BRIDGE-CERTIFICATE-001'
Ordered @($m.certificate_boundary.prohibited) @('machine-wide certificate trust','a general-purpose trusted root with a retained signing key','an exportable private key','a broad or inherited ACL') 'P0-BRIDGE-CERTIFICATE-001'
Has ($m.network_boundary|ConvertTo-Json -Depth 10 -Compress) @('exact Host and Origin values','closed allowlist','CSRF defense independent of cookies','loopback addresses only','no LAN, wildcard, or 0.0.0.0 bind','rate and size limits are enforced','sessions expire','resolve to a loopback address after the fact') 'P0-BRIDGE-CERTIFICATE-001'

Has ($m.review_link_boundary|ConvertTo-Json -Depth 10 -Compress) @('one random, non-secret, opaque loop handle and no authority','exactly the ADR-009 payload','authenticated, per-user, account-bound resolution','omit the link or show a fixed safe-fallback instruction','never fall back to embedding authority in the link') 'P0-BRIDGE-REVIEW-LINK-001'
Ordered @($m.review_link_boundary.prohibited) @('bearer token','session token','pairing token','Graph or Office ID','account identifier','mailbox content','raw evidence link') 'P0-BRIDGE-REVIEW-LINK-001'

Has ($m.content_boundary|ConvertTo-Json -Depth 10 -Compress) @('every add-in surface, not only the review link') 'P0-BRIDGE-CONTENT-001'
Ordered @($m.content_boundary.prohibited_locations) @('browser localStorage','IndexedDB','service-worker cache','a URL','console output','analytics','a crash report','Office roaming settings') 'P0-BRIDGE-CONTENT-001'

Has ($m.native_fallback_boundary|ConvertTo-Json -Depth 10 -Compress) @("companion's native status/review/recovery surface is the fallback",'blocked and routed to OWN-01 for a product-owner decision','never a silent substitute for the blocked add-in capability','no claim advertises add-in support in its place') 'P0-BRIDGE-FALLBACK-001'

Has ($m.privacy_boundary|ConvertTo-Json -Depth 10 -Compress) @('none; this ADR introduces no new persisted database record','already-reserved pairing-root.dpapi blob','ADR-005 storage constraints','session secrets remain memory-only','never a database record','requires a separate ADR-PRIV-001 revision') 'P0-BRIDGE-PRIVACY-001'
Empty @($m.privacy_boundary.approved_record_types) 'P0-BRIDGE-PRIVACY-001'

$rt=$m.runtime_boundary
if($rt.bridge_runtime-ne$false-or$rt.manifest_deployed-ne$false-or$rt.listener_active-ne$false-or$rt.pipe_active-ne$false-or$rt.certificate_issued-ne$false-or$rt.pairing_created-ne$false){Fail 'P0-BRIDGE-CLAIMS-001'}
foreach($n in @('permissions_requested','persistent_records_created','network_calls','acceptance_criteria_completed','scenarios_completed','gates_passed')){Empty @($rt.$n) 'P0-BRIDGE-CLAIMS-001'}
foreach($n in @('capabilities_enabled','capabilities_advertised','support_rows_advertised','client_matrix_supported','certificate_mechanics_proven','transport_selected')){Empty @($m.claims.$n) 'P0-BRIDGE-CLAIMS-001'}

$persist=Json $PersistencePath 'P0-BRIDGE-CROSS-CONTRACT-001'
if($null-ne$persist){
 $pairing=@($persist.secret_inventory|Where-Object{$_.id-eq'pairing_root_secret'});$session=@($persist.secret_inventory|Where-Object{$_.id-eq'session_secret'})
 if($pairing.Count-ne 1-or$pairing[0].owner-notlike'ADR-010*'-or$pairing[0].location-notmatch'pairing-root\.dpapi'-or$session.Count-ne 1-or$session[0].owner-ne'ADR-010'-or$session[0].database_value-ne'prohibited'){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
 $files=@($persist.protected_blob_store.canonical_files|Where-Object{$_.name-eq'pairing-root.dpapi'});if($files.Count-ne 1){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
}
$evidence=Json $EvidencePath 'P0-BRIDGE-CROSS-CONTRACT-001'
if($null-ne$evidence){Has ($evidence.office_and_link_contract|ConvertTo-Json -Depth 10 -Compress) @('no Graph Office account mailbox content session pairing or bearer value') 'P0-BRIDGE-CROSS-CONTRACT-001';if($evidence.separate_decisions.add_in_bridge_pairing_and_client_support-ne'ADR-010 and G-ADDIN'){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}}
$reminder=Json $ReminderPath 'P0-BRIDGE-CROSS-CONTRACT-001'
if($null-ne$reminder){Has ($reminder.review_link_boundary|ConvertTo-Json -Depth 10 -Compress) @('one random non-secret opaque loop handle only','ADR-010 authenticated per-user account-bound session','unavailable pending ADR-010 and G-ADDIN') 'P0-BRIDGE-CROSS-CONTRACT-001'}
$gov=Json $GovernancePath 'P0-BRIDGE-CROSS-CONTRACT-001'
if($null-ne$gov){
 $a10=@($gov.adrs|Where-Object{$_.id-eq'ADR-010'});$a11=@($gov.adrs|Where-Object{$_.id-eq'ADR-011'});$a12=@($gov.adrs|Where-Object{$_.id-eq'ADR-012'});$planned=@($gov.adrs|Where-Object{$_.id-in@('ADR-013')})
 if($a10.Count-ne 1-or$a10[0].status-ne'accepted'-or$a11.Count-ne 1-or$a11[0].status-ne'accepted'-or$a12.Count-ne 1-or$a12[0].status-ne'accepted'-or@($planned|Where-Object{$_.status-ne'planned'}).Count-ne 0){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
 if(@($gov.gates|Where-Object{$_.id-in$m.blocking_gates-and$_.status-ne'unrun'}).Count-ne 0){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
}
$support=Json $SupportPath 'P0-BRIDGE-CROSS-CONTRACT-001'
if($null-ne$support){
 $outlook=@($support.matrices.outlook|Where-Object{$_.disposition-eq'approved_validation_target'})
 if(@($outlook|Where-Object{$_.current_state-ne'disabled_pending_G-ADDIN'}).Count-ne 0){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
 $floor=$support.addin_contract.manifest_minimum
 if($floor.requirement_set-ne'Mailbox'-or$floor.version-ne'1.13'-or$floor.permission-ne'ReadItem'){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
 foreach($n in @('supported_rows','enabled_capabilities','advertised_capabilities','gates_passed','acceptance_criteria_completed')){Empty @($support.claim_state.$n) 'P0-BRIDGE-CROSS-CONTRACT-001'}
}
$build=Json $BuildPath 'P0-BRIDGE-CROSS-CONTRACT-001'
if($null-ne$build){if($build.runtime_boundary.addin_manifest-ne$false-or$build.runtime_boundary.graph_transport-ne$false-or$build.runtime_boundary.durable_state-ne$false){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'};Empty @($build.runtime_boundary.network_origins) 'P0-BRIDGE-CROSS-CONTRACT-001'}

$adr=Text $AdrPath 'P0-BRIDGE-CROSS-CONTRACT-001';$threat=Text $ThreatPath 'P0-BRIDGE-CROSS-CONTRACT-001';$spec=Text $ProductSpecPath 'P0-BRIDGE-INVENTORY-001';$plan=Text $ImplementationPlanPath 'P0-BRIDGE-INVENTORY-001';$trace=Text $TraceabilityPath 'P0-BRIDGE-INVENTORY-001'
if((NHash $adr)-ne'4522e71c18247b157fa15997b00ff60e4ee10b5ad1ea307ca832c14ba3ee8ba1'){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
if((NHash $threat)-ne'5f476f5096703aab139505a4f01ef20a2e1fd3c0fd358661152d3da1e8515987'){Fail 'P0-BRIDGE-CROSS-CONTRACT-001'}
if((NHash $spec)-ne'c0a7718d78bde3804142e5b79a937a8d66a372bfe35406e2c8b2ce96d4c7ed54'){Fail 'P0-BRIDGE-INVENTORY-001'}
if((NHash $plan)-ne'24d7f6a978f62b7d7a2eca0314361df84628ca515ee957593fa77f83f3b2e93f'){Fail 'P0-BRIDGE-INVENTORY-001'}
if((NHash $trace)-ne'90bd38ff7a1872e7550f40a1e01e966f2bc95d3f1e45eb0119478977505f776a'){Fail 'P0-BRIDGE-INVENTORY-001'}
Has $adr @('Status:** Accepted','P0-WI-13','OWN-01','Named pipes are preferred','same-user loopback web bridge','no unpaired command exists','least-authority and command-scoped','It MUST NOT install machine-wide certificate trust','one random, non-secret,','opaque loop handle and no authority','No mailbox content, identifier, token, or URL carrying','never a silent substitute','introduces no new persisted database record') 'P0-BRIDGE-CROSS-CONTRACT-001'
Has $threat @('Add-in bridge','Pairing bootstrap race between two first-pair attempts','Replayed pairing nonce','DNS-rebinding a hostname to 127.0.0.1','CSRF from the add-in WebView or another page','Certificate/trust abuse','Uninstall/disconnect cleanup failure','Same-user malware') 'P0-BRIDGE-CROSS-CONTRACT-001'
$ids=[regex]::Matches($trace,'(?m)^\| (P0-BRIDGE-[A-Z-]+-001) \|')|ForEach-Object{$_.Groups[1].Value};Exact @($ids) $checks 'P0-BRIDGE-INVENTORY-001';foreach($id in $checks){if(([regex]::Matches($trace,'\b'+[regex]::Escape($id)+'\b')).Count-ne 1){Fail 'P0-BRIDGE-INVENTORY-001'}}
$fresh=[regex]::Match($trace,'(?m)^\| P0-BRIDGE-FRESH-CHECKER-001 \|.*$').Value
Has $fresh @('Fresh-context security, privacy, governance, and adversarial judges','no prompt, transcript, or model output stored','Passed at P0-WI-13 closure') 'P0-BRIDGE-FRESH-CHECKER-001'

if($script:fail.Count-gt 0){[Console]::Error.WriteLine(('FAIL: add-in bridge boundary: '+(($script:fail|Sort-Object)-join', ')));exit 1};if(-not$Quiet){Write-Output ('PASS: add-in bridge boundary ({0} checks; runtime, calls, permissions, records, and claims disabled)' -f $checks.Count)}
