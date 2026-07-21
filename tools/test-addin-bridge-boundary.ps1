[CmdletBinding()]param([string]$CheckerPath='tools/check-addin-bridge-boundary.ps1')
Set-StrictMode -Version Latest;$ErrorActionPreference='Stop'
$repoRoot=(& git rev-parse --show-toplevel 2>$null).Trim();if($LASTEXITCODE-ne 0-or[string]::IsNullOrWhiteSpace($repoRoot)){throw'Run inside the repository.'}
function Resolve-Repo([string]$p){if([IO.Path]::IsPathRooted($p)){[IO.Path]::GetFullPath($p)}else{[IO.Path]::GetFullPath((Join-Path $repoRoot $p))}}
$checker=Resolve-Repo $CheckerPath
$inputs=@('contracts/addin/bridge-boundary.json','docs/adr/ADR-010-add-in-bridge.md','docs/threat-model/addin-bridge.md','docs/prd-traceability.md','docs/product-spec.md','docs/implementation-plan.md','contracts/governance/capabilities.json','contracts/persistence/protected-state-boundary.json','contracts/evidence/identity-boundary.json','contracts/reminder/adapter-boundary.json','contracts/privacy/persistence-boundary.json','contracts/support/support-matrix.json','contracts/build-skeleton/skeleton.json')
$script:count=0;$script:fail=[Collections.Generic.List[string]]::new()
function New-Root{$root=Join-Path ([IO.Path]::GetTempPath()) ('openloops-bridge-'+[guid]::NewGuid().ToString('N'));[IO.Directory]::CreateDirectory($root)|Out-Null;foreach($rel in $inputs){$target=Join-Path $root $rel;[IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($target))|Out-Null;Copy-Item -LiteralPath (Join-Path $repoRoot $rel) -Destination $target};$root}
function Check([string]$root){$a=@('-NoProfile','-File',$checker,'-ManifestPath',(Join-Path $root 'contracts/addin/bridge-boundary.json'),'-AdrPath',(Join-Path $root 'docs/adr/ADR-010-add-in-bridge.md'),'-ThreatPath',(Join-Path $root 'docs/threat-model/addin-bridge.md'),'-TraceabilityPath',(Join-Path $root 'docs/prd-traceability.md'),'-ProductSpecPath',(Join-Path $root 'docs/product-spec.md'),'-ImplementationPlanPath',(Join-Path $root 'docs/implementation-plan.md'),'-GovernancePath',(Join-Path $root 'contracts/governance/capabilities.json'),'-PersistencePath',(Join-Path $root 'contracts/persistence/protected-state-boundary.json'),'-EvidencePath',(Join-Path $root 'contracts/evidence/identity-boundary.json'),'-ReminderPath',(Join-Path $root 'contracts/reminder/adapter-boundary.json'),'-PrivacyPath',(Join-Path $root 'contracts/privacy/persistence-boundary.json'),'-SupportPath',(Join-Path $root 'contracts/support/support-matrix.json'),'-BuildPath',(Join-Path $root 'contracts/build-skeleton/skeleton.json'),'-Quiet');$o=@(& pwsh @a 2>&1|ForEach-Object{[string]$_});[pscustomobject]@{Code=$LASTEXITCODE;Output=($o-join"`n")}}
function JsonCase([string]$name,[string]$id,[string]$rel,[scriptblock]$mutate){$script:count++;$root=New-Root;try{$path=Join-Path $root $rel;$v=Get-Content -Raw -LiteralPath $path|ConvertFrom-Json -Depth 100;&$mutate $v;$v|ConvertTo-Json -Depth 100|Set-Content -LiteralPath $path -Encoding utf8NoBOM;$r=Check $root;if($r.Code-eq 0-or$r.Output-notmatch[regex]::Escape($id)){$script:fail.Add("$name (expected $id; exit $($r.Code))")}}finally{Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue}}
function TextCase([string]$name,[string]$id,[string]$rel,[scriptblock]$mutate){$script:count++;$root=New-Root;try{$path=Join-Path $root $rel;$changed=&$mutate (Get-Content -Raw -LiteralPath $path);Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM;$r=Check $root;if($r.Code-eq 0-or$r.Output-notmatch[regex]::Escape($id)){$script:fail.Add("$name (expected $id; exit $($r.Code))")}}finally{Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue}}
$base=& pwsh -NoProfile -File $checker 2>&1;if($LASTEXITCODE-ne 0){throw('Baseline bridge checker failed: '+(@($base)-join"`n"))}

JsonCase 'work item drift' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.work_item='P0-WI-X'}
JsonCase 'owner removed' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.owner_decisions=@()}
JsonCase 'owned requirement removed' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.owned_requirements=@($c.owned_requirements|Where-Object{$_-ne'OL-UX-007'})}
JsonCase 'OL-REM-016 double-owned' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.owned_requirements+='OL-REM-016'}
JsonCase 'dependent AC invented' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.dependent_acceptance_criteria=@('AC-10')}
JsonCase 'scenario invented' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.primary_scenarios=@('AS-11')}
JsonCase 'gate removed' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.blocking_gates=@($c.blocking_gates|Where-Object{$_-ne'G-PRIV'})}
JsonCase 'separate decision reassigned' 'P0-BRIDGE-INVENTORY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.separate_decisions.exact_transport_selection_and_loopback_mechanics='ADR-010'}

JsonCase 'transport candidate widened to LAN' 'P0-BRIDGE-TRANSPORT-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.catalogs.transport_candidate_code+='lan_web_bridge'}
JsonCase 'named pipe assumed reachable from Office.js' 'P0-BRIDGE-TRANSPORT-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.transport_boundary.prohibited_assumptions=@($c.transport_boundary.prohibited_assumptions|Where-Object{$_-ne'a named pipe is reachable from Office.js'})}
JsonCase 'transport selection guessed' 'P0-BRIDGE-TRANSPORT-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.transport_boundary.'unresolved_pending_G-ADDIN'=@($c.transport_boundary.'unresolved_pending_G-ADDIN'|Where-Object{$_-ne'exact transport selection'})}
JsonCase 'remote reachable service permitted' 'P0-BRIDGE-TRANSPORT-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.transport_boundary.addin_candidate_transport='a remotely reachable hosted bridge is acceptable for the Office add-in'}

JsonCase 'bootstrap missing nonce' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.catalogs.bootstrap_binding_field=@($c.catalogs.bootstrap_binding_field|Where-Object{$_-ne'single_use_nonce'})}
JsonCase 'bootstrap missing expiry' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.bound_fields=@($c.bootstrap_boundary.bound_fields|Where-Object{$_-ne'expiry'})}
JsonCase 'bootstrap allows URL secret' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.prohibited_locations=@($c.bootstrap_boundary.prohibited_locations|Where-Object{$_-ne'URL parameters'})}
JsonCase 'bootstrap allows localStorage secret' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.prohibited_locations=@($c.bootstrap_boundary.prohibited_locations|Where-Object{$_-ne'localStorage'})}
JsonCase 'bootstrap allows roaming settings secret' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.prohibited_locations=@($c.bootstrap_boundary.prohibited_locations|Where-Object{$_-ne'Office roaming settings'})}
JsonCase 'bootstrap no explicit user action' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.bound_fields=@($c.bootstrap_boundary.bound_fields|Where-Object{$_-ne'explicit_user_verifiable_action'})}
JsonCase 'race allows silent second binding' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.race_defense='both concurrent attempts may bind successfully'}
JsonCase 'replay accepted after expiry' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.replay_defense='a nonce may be reused after its first successful pairing'}
JsonCase 'unbounded verification attempts' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.brute_force_defense='pairing verification allows unlimited attempts'}
JsonCase 'unpaired command allowed' 'P0-BRIDGE-BOOTSTRAP-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.bootstrap_boundary.unpaired_command_rule='a limited status command may run before pairing completes'}

JsonCase 'session becomes generic authority' 'P0-BRIDGE-SESSION-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.session_boundary.prohibited_authority=@()}
JsonCase 'session persisted to disk' 'P0-BRIDGE-SESSION-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.session_boundary.persistence='session secrets are written to the encrypted database for durability across restarts'}
JsonCase 'session without rotation' 'P0-BRIDGE-SESSION-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.session_boundary.rotation='the session never rotates and persists across reconnects'}
JsonCase 'stale session reused' 'P0-BRIDGE-SESSION-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.session_boundary.stale_rule='a session bound to a prior account is silently reused after account change'}
JsonCase 'session revocation catalog entry removed' 'P0-BRIDGE-SESSION-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.catalogs.session_property=@($c.catalogs.session_property|Where-Object{$_-ne'revoked_on_account_change'})}
JsonCase 'same-user malware claimed defended' 'P0-BRIDGE-SESSION-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.session_boundary.residual_limitation='this boundary defends against any code running as the same authenticated OS user'}

JsonCase 'machine-wide trust permitted' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.catalogs.certificate_prohibition=@($c.catalogs.certificate_prohibition|Where-Object{$_-ne'machine_wide_trust'})}
JsonCase 'exportable key permitted' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.certificate_boundary.prohibited=@($c.certificate_boundary.prohibited|Where-Object{$_-ne'an exportable private key'})}
JsonCase 'retained signing root permitted' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.certificate_boundary.prohibited=@($c.certificate_boundary.prohibited|Where-Object{$_-ne'a general-purpose trusted root with a retained signing key'})}
JsonCase 'certificate cleanup optional' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.certificate_boundary.cleanup='best-effort removal on uninstall; failure does not block the add-in'}
JsonCase 'LAN binding permitted' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.network_boundary.binding='loopback and LAN addresses; wildcard bind permitted for discovery'}
JsonCase 'DNS-rebinding ignored' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.network_boundary.dns_rebinding='rebound hostnames are accepted once the Origin header is present'}
JsonCase 'missing CSRF rule' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.network_boundary.csrf_defense='not required because the bridge is loopback-only'}
JsonCase 'missing Host/Origin validation rule' 'P0-BRIDGE-CERTIFICATE-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.network_boundary.host_origin_validation='any Host or Origin header is accepted from a loopback-bound socket'}

JsonCase 'review link authority granted' 'P0-BRIDGE-REVIEW-LINK-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.review_link_boundary.payload='a bearer token that authorizes read access to the loop'}
JsonCase 'review link prohibited list weakened' 'P0-BRIDGE-REVIEW-LINK-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.review_link_boundary.prohibited=@($c.review_link_boundary.prohibited|Where-Object{$_-ne'bearer token'})}
JsonCase 'unsupported client embeds authority' 'P0-BRIDGE-REVIEW-LINK-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.review_link_boundary.unsupported_client_behavior='embed a bearer token in the link so unsupported clients can still resolve it'}

JsonCase 'mailbox content permitted in localStorage' 'P0-BRIDGE-CONTENT-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.content_boundary.prohibited_locations=@($c.content_boundary.prohibited_locations|Where-Object{$_-ne'browser localStorage'})}
JsonCase 'content prohibition catalog narrowed' 'P0-BRIDGE-CONTENT-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.catalogs.content_prohibition=@($c.catalogs.content_prohibition|Where-Object{$_-ne'indexeddb'})}

JsonCase 'native fallback silently substitutes' 'P0-BRIDGE-FALLBACK-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.native_fallback_boundary.prohibited='native UI may silently substitute for a failed G-ADDIN capability without product-owner routing'}
JsonCase 'gate failure not routed to OWN-01' 'P0-BRIDGE-FALLBACK-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.native_fallback_boundary.gate_failure_routing='if G-ADDIN fails the feature quietly ships with native UI only'}
JsonCase 'fallback trigger narrowed' 'P0-BRIDGE-FALLBACK-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.catalogs.native_fallback_trigger=@($c.catalogs.native_fallback_trigger|Where-Object{$_-ne'gate_failed'})}

JsonCase 'new database record introduced' 'P0-BRIDGE-PRIVACY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.privacy_boundary.approved_record_types=@('bridge_pairing_record')}
JsonCase 'privacy revision requirement removed' 'P0-BRIDGE-PRIVACY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.privacy_boundary.revision_requirement='ADR-010 may add persisted bridge records directly without a privacy revision'}
JsonCase 'pairing moved outside DPAPI' 'P0-BRIDGE-PRIVACY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.privacy_boundary.pairing_state_location='a plaintext configuration file next to the executable'}
JsonCase 'session claimed persisted' 'P0-BRIDGE-PRIVACY-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.privacy_boundary.session_state_location='session secrets are written to the encrypted SQLite database'}

JsonCase 'runtime enabled' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.bridge_runtime=$true}
JsonCase 'manifest deployed claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.manifest_deployed=$true}
JsonCase 'listener active claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.listener_active=$true}
JsonCase 'pipe active claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.pipe_active=$true}
JsonCase 'certificate issued claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.certificate_issued=$true}
JsonCase 'pairing created claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.pairing_created=$true}
JsonCase 'permission requested claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.permissions_requested=@('unresolved_office_manifest:G-ADDIN')}
JsonCase 'gate passed claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.runtime_boundary.gates_passed=@('G-ADDIN')}
JsonCase 'capability enabled claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.claims.capabilities_enabled=@('outlook_addin')}
JsonCase 'transport selected claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.claims.transport_selected=@('loopback_web_bridge_addin_candidate')}
JsonCase 'certificate mechanics proven claimed' 'P0-BRIDGE-CLAIMS-001' 'contracts/addin/bridge-boundary.json' {param($c)$c.claims.certificate_mechanics_proven=@('loopback_cert_v1')}

JsonCase 'ADR-010 planned again' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' {param($c)($c.adrs|Where-Object{$_.id -eq 'ADR-010'}).status='planned'}
JsonCase 'ADR-011 prematurely accepted' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' {param($c)($c.adrs|Where-Object{$_.id -eq 'ADR-011'}).status='accepted'}
JsonCase 'pairing-root ownership drift' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/persistence/protected-state-boundary.json' {param($c)($c.secret_inventory|Where-Object{$_.id -eq 'pairing_root_secret'}).owner='ADR-005'}
JsonCase 'session secret becomes a database value' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/persistence/protected-state-boundary.json' {param($c)($c.secret_inventory|Where-Object{$_.id -eq 'session_secret'}).database_value='opaque_reference'}
JsonCase 'office link boundary weakened' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/evidence/identity-boundary.json' {param($c)$c.office_and_link_contract.review_link='may contain a session or pairing value for unsupported clients'}
JsonCase 'reminder review-link deferral removed' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/reminder/adapter-boundary.json' {param($c)$c.review_link_boundary.activation='available now via the bridge'}
JsonCase 'support matrix client floor raised silently' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/support/support-matrix.json' {param($c)$c.addin_contract.manifest_minimum.permission='ReadWriteMailbox'}
JsonCase 'build skeleton addin manifest active' 'P0-BRIDGE-CROSS-CONTRACT-001' 'contracts/build-skeleton/skeleton.json' {param($c)$c.runtime_boundary.addin_manifest=$true}

TextCase 'spec contradiction appended' 'P0-BRIDGE-INVENTORY-001' 'docs/product-spec.md' {param($t)$t+"`nContrary rule: the add-in bridge may bind to any LAN interface once paired.`n"}
TextCase 'plan contradiction appended' 'P0-BRIDGE-INVENTORY-001' 'docs/implementation-plan.md' {param($t)$t+"`nP0-WI-13 enables the Outlook add-in bridge listener and issues a loopback certificate.`n"}
TextCase 'ADR contradiction appended' 'P0-BRIDGE-CROSS-CONTRACT-001' 'docs/adr/ADR-010-add-in-bridge.md' {param($t)$t+"`nContrary decision: install a machine-wide trusted root and embed the bearer token in the review link.`n"}
TextCase 'trace contradiction appended' 'P0-BRIDGE-INVENTORY-001' 'docs/prd-traceability.md' {param($t)$t+"`nP0-WI-13 passed G-ADDIN and ships the Outlook add-in bridge.`n"}
TextCase 'fresh checker preapproved' 'P0-BRIDGE-FRESH-CHECKER-001' 'docs/prd-traceability.md' {param($t)$t-replace'\| P0-BRIDGE-FRESH-CHECKER-001 \|([^\n]*)Passed at P0-WI-13 closure','| P0-BRIDGE-FRESH-CHECKER-001 |$1Passed without independent review'}

if($script:fail.Count-gt 0){[Console]::Error.WriteLine(('FAIL: add-in bridge synthetic mutations: '+($script:fail-join'; ')));exit 1};Write-Output ('PASS: add-in bridge synthetic mutations ({0} rejection cases; OS temp only)' -f $script:count)
