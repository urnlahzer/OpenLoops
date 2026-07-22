[CmdletBinding()]
param(
 [string]$ManifestPath='contracts/distribution/registration-boundary.json',
 [string]$AdrPath='docs/adr/ADR-012-distribution-and-registration.md',
 [string]$ThreatPath='docs/threat-model/distribution-and-update.md',
 [string]$TraceabilityPath='docs/prd-traceability.md',
 [string]$ProductSpecPath='docs/product-spec.md',
 [string]$ImplementationPlanPath='docs/implementation-plan.md',
 [string]$GovernancePath='contracts/governance/capabilities.json',
 [string]$Adr001Path='docs/adr/ADR-001-runtime-and-self-hosting-boundary.md',
 [string]$Adr005Path='docs/adr/ADR-005-persistence-and-cryptography.md',
 [string]$Adr010Path='docs/adr/ADR-010-add-in-bridge.md',
 [string]$AdrPrivPath='docs/adr/ADR-PRIV-001-derived-metadata-and-source-boundary.md',
 [string]$SupportPath='contracts/support/support-matrix.json',
 [string]$BuildPath='contracts/build-skeleton/skeleton.json',
 [string]$PolicyPath='contracts/domain/policy-state-boundary.json',
 [string]$ReminderPath='contracts/reminder/adapter-boundary.json',
 [string]$AddinPath='contracts/addin/bridge-boundary.json',
 [string]$AutomationPath='contracts/automation/evaluation-boundary.json',
 [string]$CargoPath='Cargo.toml',
 [string]$PackageJsonPath='package.json',
 [string]$AgentsPath='AGENTS.md',
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
$checks=@('P0-DIST-INVENTORY-001','P0-DIST-REGISTRATION-001','P0-DIST-SHARED-GOVERNANCE-001','P0-DIST-UPDATE-TRUST-001','P0-DIST-ARTIFACTS-001','P0-DIST-INSTALLER-001','P0-DIST-DIAGNOSTICS-001','P0-DIST-PRIVACY-001','P0-DIST-CROSS-CONTRACT-001','P0-DIST-CLAIMS-001','P0-DIST-FRESH-CHECKER-001')

$m=Json $ManifestPath 'P0-DIST-INVENTORY-001';if($null-eq$m){[Console]::Error.WriteLine('BLOCKED: distribution manifest parse failed.');exit 1}
$canonical=$m|ConvertTo-Json -Depth 100 -Compress;$hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
if($hash-ne'016abaddda7040a6b70d52748bd10eb5cdd4fa026f7169061cc0ab7cd721c096'){Fail 'P0-DIST-INVENTORY-001'}
if($m.schema_version-ne 1-or$m.work_item-ne'P0-WI-15'-or$m.adr-ne'ADR-012'-or$m.decision_status-ne'accepted_contract_runtime_unimplemented'-or$m.snapshot_date-ne'2026-07-21'){Fail 'P0-DIST-INVENTORY-001'}
Ordered @($m.owner_decisions) @('OWN-00','OWN-01','OWN-02') 'P0-DIST-INVENTORY-001'
Ordered @($m.owned_requirements) @('OL-NFR-010','OL-NFR-011','OL-NFR-012') 'P0-DIST-INVENTORY-001'
Empty @($m.consumed_requirements) 'P0-DIST-INVENTORY-001'
Empty @($m.primary_acceptance_criteria) 'P0-DIST-INVENTORY-001'
Empty @($m.dependent_acceptance_criteria) 'P0-DIST-INVENTORY-001'
Empty @($m.primary_scenarios) 'P0-DIST-INVENTORY-001'
Empty @($m.scenario_dependencies) 'P0-DIST-INVENTORY-001'
Ordered @($m.blocking_gates) @('G-ID','G-RELEASE') 'P0-DIST-INVENTORY-001'
Ordered @($m.input_authorities.owner) @('ADR-001','ADR-005','ADR-010','ADR-PRIV-001') 'P0-DIST-INVENTORY-001'
Ordered @($m.sources.id) @('SRC-SPEC-NFR','SRC-SPEC-GATES','SRC-SPEC-DISCONNECT','SRC-PLAN-ADR','SRC-PLAN-PHASE7','SRC-ADR-001','SRC-ADR-005','SRC-ADR-010','SRC-ADR-PRIV-001','SRC-RESEARCH-CONNECTION','SRC-AGENTS','SRC-THREAT-INDEX') 'P0-DIST-INVENTORY-001'
$sd=$m.separate_decisions
if($sd.'exact_signing_technology_and_certificate_provider'-ne'unresolved_pending_G-RELEASE'-or$sd.'update_transport_and_channel_names'-ne'unresolved_pending_G-RELEASE'-or$sd.bridge_certificate_lifecycle_and_addin_trust-ne'ADR-010'-or$sd.state_migration_encryption_and_rollback_anchor_mechanics-ne'ADR-005'-or$sd.diagnostics_export_schema_and_privacy_gates-ne'ADR-PRIV-001'-or$sd.independent_security_audit_scope-ne'G-SEC-AUDIT'){Fail 'P0-DIST-INVENTORY-001'}

# Requirement double-ownership: OL-NFR-010/011/012 owned exactly here; other owned_requirements manifests must not claim them.
foreach($pair in @(@($PolicyPath,'policy'),@($ReminderPath,'reminder'),@($AddinPath,'addin'),@($AutomationPath,'automation'))){
 $other=Json $pair[0] 'P0-DIST-INVENTORY-001'
 if($null-ne$other){if(@($other.owned_requirements|Where-Object{$_-in@('OL-NFR-010','OL-NFR-011','OL-NFR-012')}).Count-ne 0){Fail 'P0-DIST-INVENTORY-001'}}
}

$c=$m.catalogs
Ordered @($c.registration_mode_code) @('byo_public_client','shared_project_registration') 'P0-DIST-REGISTRATION-001'
Ordered @($c.shared_registration_governance_control_code) @('dedicated_publisher_tenant_and_verified_domain','publisher_verification','accurate_metadata_and_disclosures','two_registration_owners_with_recovery','exact_registered_redirects','incremental_consent_explanations','admin_consent_disabled_tenant_support','sign_in_monitoring_without_content','malicious_fork_client_id_abuse_incident_procedure','signed_builds_with_provenance','byo_documentation_for_rejecting_orgs') 'P0-DIST-SHARED-GOVERNANCE-001'
Ordered @($c.update_trust_field_code) @('channel','version','expiry','package_hash','package_size','signing_key_id','source_revision') 'P0-DIST-UPDATE-TRUST-001'
Ordered @($c.artifact_allowlist_class_code) @('installer_package','update_metadata','sbom','provenance_attestation','release_notes_content_free') 'P0-DIST-ARTIFACTS-001'
Ordered @($c.installer_boundary_property_code) @('per_user','unelevated','complete_uninstall','bridge_trust_removed','residual_risk_disclosed') 'P0-DIST-INSTALLER-001'
Ordered @($c.diagnostics_rule_code) @('allowlist_empty','future_export_requires_named_adrs_and_gates') 'P0-DIST-DIAGNOSTICS-001'

$rb=$m.registration_boundary
if($rb.byo_public_client.status-ne'the only enabled Phase 0/source-build path'-or$rb.shared_project_registration.status-ne'disabled; a separately gated future option'-or$rb.pkce_limitation_preserved-ne$true){Fail 'P0-DIST-REGISTRATION-001'}
Has ($rb.byo_public_client|ConvertTo-Json -Depth 10 -Compress) @('placeholder-only tracked configuration','no real client ID, tenant ID, or other Microsoft identifier','no secret on a command line or in tracked configuration','PKCE does not authenticate the binary requesting a token') 'P0-DIST-REGISTRATION-001'
Ordered @($rb.shared_project_registration.enablement_prohibited_by) @('successful sign-in','development convenience','a passing test') 'P0-DIST-SHARED-GOVERNANCE-001'
Ordered @($rb.shared_project_registration.required_governance_controls) @(
 'dedicated organizational publisher tenant and a verified domain',
 'Microsoft publisher verification',
 'accurate name, logo, homepage, privacy statement, terms, and support contact',
 'at least two controlled registration owners and an owner-recovery procedure',
 'exact registered redirects with no unused platform configuration',
 'incremental consent and plain-language permission explanations',
 'support for tenants that disable user consent or require admin approval',
 'sign-in/consent-failure monitoring without message or task content collection',
 'a malicious-fork/client-ID-abuse and incident-response procedure',
 'signed official builds and updates with published provenance',
 'BYO registration documentation for organizations that reject the shared application'
) 'P0-DIST-SHARED-GOVERNANCE-001'

$ut=$m.update_trust_chain
Has ($ut|ConvertTo-Json -Depth 10 -Compress) @('signed independently of package signing','trusted-key inventory','rotation/revocation procedure','monotonic anti-downgrade','refuses to write a newer schema or ciphertext version','strictly greater than the currently installed version','bound to an exact release channel','explicit expiry','exact package hash and exact package size','atomic and protected','verified completely before staging','single atomic step with a recovery path','verifiable to the exact public source revision','no installation or update step elevates privileges by default') 'P0-DIST-UPDATE-TRUST-001'

$ab=$m.artifact_boundary
Has ($ab|ConvertTo-Json -Depth 10 -Compress) @('explicit allowlist','rejected before release rather than filtered afterward','required for every release','source maps','generated diagnostic artifacts','never print a suspected secret or identifier value','blocks the release rather than being silenced or bypassed') 'P0-DIST-ARTIFACTS-001'
Ordered @($ab.prohibited) @('source maps','generated diagnostic artifacts') 'P0-DIST-ARTIFACTS-001'
Has ([string]$ab.npm_publishability) @('disabled in Phase 0','"private": true','empty files allowlist') 'P0-DIST-ARTIFACTS-001'
Has ([string]$ab.cargo_publishability) @('disabled in Phase 0','publish = false') 'P0-DIST-ARTIFACTS-001'

$ib=$m.installer_boundary
Has ($ib|ConvertTo-Json -Depth 10 -Compress) @('per-user and unelevated','no administrator rights requested by default','repeats disconnect cleanup where possible','removes every local bridge trust, certificate, and protocol registration ADR-010 created','leaving no orphaned trust behind','not a physical-erasure claim','zero Graph mutation and no bulk deletion') 'P0-DIST-INSTALLER-001'

$dr=$m.diagnostics_rule
if($dr.allowlist-ne'empty'){Fail 'P0-DIST-DIAGNOSTICS-001'}
Has ($dr|ConvertTo-Json -Depth 10 -Compress) @('ADR-012, G-PRIV, G-RELEASE, and G-SEC-AUDIT','none of those four is satisfied','none may be represented as satisfied by a partial subset') 'P0-DIST-DIAGNOSTICS-001'

Ordered @($m.'unresolved_pending_G-RELEASE') @('exact signing technology','certificate provider','update transport','release-channel names') 'P0-DIST-PRIVACY-001'

$rt=$m.runtime_boundary
if($rt.shared_registration_enabled-ne$false-or$rt.package_signed-ne$false-or$rt.update_metadata_published-ne$false-or$rt.installer_built-ne$false-or$rt.security_audit_completed-ne$false){Fail 'P0-DIST-CLAIMS-001'}
foreach($n in @('acceptance_criteria_completed','scenarios_completed','gates_passed')){Empty @($rt.$n) 'P0-DIST-CLAIMS-001'}
foreach($n in @('capabilities_enabled','capabilities_advertised','registration_modes_advertised','release_channels_advertised')){Empty @($m.claims.$n) 'P0-DIST-CLAIMS-001'}

# Cross-contract reconciliation.
$adr001=Text $Adr001Path 'P0-DIST-CROSS-CONTRACT-001'
Has $adr001 @('BYO registration is the source-build and development path','registration remains gated by ADR-012, publisher governance, signed') 'P0-DIST-CROSS-CONTRACT-001'
$adr005=Text $Adr005Path 'P0-DIST-CROSS-CONTRACT-001'
Has $adr005 @('older binary refuses to write a newer schema or ciphertext version') 'P0-DIST-CROSS-CONTRACT-001'
$adr010=Text $Adr010Path 'P0-DIST-CROSS-CONTRACT-001'
Has $adr010 @('completely removed on disconnect and uninstall') 'P0-DIST-CROSS-CONTRACT-001'
$adrPriv=Text $AdrPrivPath 'P0-DIST-CROSS-CONTRACT-001'
Has $adrPriv @('Diagnostics have an empty allowlist; any future diagnostic export needs an','exact content-free schema plus ADR-012, G-PRIV, G-RELEASE, and G-SEC-AUDIT') 'P0-DIST-CROSS-CONTRACT-001'

$gov=Json $GovernancePath 'P0-DIST-CROSS-CONTRACT-001'
if($null-ne$gov){
 $a12=@($gov.adrs|Where-Object{$_.id-eq'ADR-012'});$a13=@($gov.adrs|Where-Object{$_.id-eq'ADR-013'})
 if($a12.Count-ne 1-or$a12[0].status-ne'accepted'-or$a13.Count-ne 1-or$a13[0].status-ne'accepted'){Fail 'P0-DIST-CROSS-CONTRACT-001'}
 if(@($gov.gates|Where-Object{$_.id-in$m.blocking_gates-and$_.status-ne'unrun'}).Count-ne 0){Fail 'P0-DIST-CROSS-CONTRACT-001'}
 $shared=@($gov.capabilities|Where-Object{$_.id-eq'shared_project_registration'})
 if($shared.Count-ne 1-or$shared[0].state-ne'disabled'-or$shared[0].advertised-ne$false){Fail 'P0-DIST-CROSS-CONTRACT-001'}
 Ordered @($shared[0].owner_decisions) @('OWN-00','OWN-01','OWN-02') 'P0-DIST-CROSS-CONTRACT-001'
 Ordered @($shared[0].gates) @('G-ID','G-RELEASE') 'P0-DIST-CROSS-CONTRACT-001'
}
$support=Json $SupportPath 'P0-DIST-CROSS-CONTRACT-001'
if($null-ne$support){foreach($n in @('supported_rows','enabled_capabilities','advertised_capabilities','gates_passed','acceptance_criteria_completed')){Empty @($support.claim_state.$n) 'P0-DIST-CROSS-CONTRACT-001'}}
$build=Json $BuildPath 'P0-DIST-CROSS-CONTRACT-001'
if($null-ne$build){
 if($build.runtime_boundary.graph_transport-ne$false-or$build.runtime_boundary.oauth-ne$false-or$build.runtime_boundary.model_provider-ne$false-or$build.runtime_boundary.durable_state-ne$false-or$build.runtime_boundary.addin_manifest-ne$false){Fail 'P0-DIST-CROSS-CONTRACT-001'}
 Empty @($build.runtime_boundary.network_origins) 'P0-DIST-CROSS-CONTRACT-001'
 if($build.build_policy.npm_private-ne$true-or$build.build_policy.source_maps-ne$false-or$build.build_policy.debug_symbols_in_release-ne$false){Fail 'P0-DIST-ARTIFACTS-001'}
 Empty @($build.build_policy.npm_files) 'P0-DIST-ARTIFACTS-001'
}
$cargoText=Text $CargoPath 'P0-DIST-ARTIFACTS-001'
Has $cargoText @('publish = false') 'P0-DIST-ARTIFACTS-001'
$pkgText=Text $PackageJsonPath 'P0-DIST-ARTIFACTS-001'
Has $pkgText @('"private": true','"files": []') 'P0-DIST-ARTIFACTS-001'
$agentsText=Text $AgentsPath 'P0-DIST-ARTIFACTS-001'
Has $agentsText @('Source maps are prohibited by default','use the','`files` field once `package.json` exists, run `npm pack --dry-run`') 'P0-DIST-ARTIFACTS-001'

$adr=Text $AdrPath 'P0-DIST-CROSS-CONTRACT-001';$threat=Text $ThreatPath 'P0-DIST-CROSS-CONTRACT-001';$spec=Text $ProductSpecPath 'P0-DIST-INVENTORY-001';$plan=Text $ImplementationPlanPath 'P0-DIST-INVENTORY-001';$trace=Text $TraceabilityPath 'P0-DIST-INVENTORY-001'
if((NHash $adr)-ne'09ac519ec14fd894706a2010f9d7860658655487a4afee07c0ae044c19e924cc'){Fail 'P0-DIST-CROSS-CONTRACT-001'}
if((NHash $threat)-ne'10143d6884cb28a4449ff6172becd8915ae877eec11fb40535f02b7f23bedcb4'){Fail 'P0-DIST-CROSS-CONTRACT-001'}
if((NHash $spec)-ne'c0a7718d78bde3804142e5b79a937a8d66a372bfe35406e2c8b2ce96d4c7ed54'){Fail 'P0-DIST-INVENTORY-001'}
if((NHash $plan)-ne'03c11ed8980703bbc2649a460162577d4a4a898528c63867c789afbd8b6c735a'){Fail 'P0-DIST-INVENTORY-001'}
if((NHash $trace)-ne'50c627612d5a6c6d4429bab069901ec04c3b8c067c00e7d0e06bef7b2a810d23'){Fail 'P0-DIST-INVENTORY-001'}
Has $adr @('Status:** Accepted','P0-WI-15','OWN-00, OWN-01, OWN-02','G-ID, G-RELEASE','BYO public-client registration is the only enabled Phase 0/source-build path','is a separate, disabled, gated future option','PKCE does not authenticate the binary','signed independently of package signing','older binary refuses to write a newer schema or ciphertext version','publishability remain disabled in','stays empty','unresolved_pending_G-RELEASE') 'P0-DIST-CROSS-CONTRACT-001'
Has $threat @('Distribution and update','Malicious fork reuses the shared client ID or impersonates the publisher','Update downgrade to a vulnerable signed version','Update metadata swapped between channels','Staged package replaced after verification','Signing-key compromise with no rotation/revocation path','Expired metadata replayed as current','Provenance that cannot be tied to a source revision','Elevation path smuggled into install or update','Supply-chain or lockfile tamper reaching a release','Secret or real identifier entering a package, SBOM, or repository','Uninstall leaving bridge trust or a certificate behind','Publisher-governance failure blocking the shared registration path') 'P0-DIST-CROSS-CONTRACT-001'
$ids=[regex]::Matches($trace,'(?m)^\| (P0-DIST-[A-Z-]+-001) \|')|ForEach-Object{$_.Groups[1].Value};Exact @($ids) $checks 'P0-DIST-INVENTORY-001';foreach($id in $checks){if(([regex]::Matches($trace,'\b'+[regex]::Escape($id)+'\b')).Count-ne 1){Fail 'P0-DIST-INVENTORY-001'}}
$fresh=[regex]::Match($trace,'(?m)^\| P0-DIST-FRESH-CHECKER-001 \|.*$').Value
Has $fresh @('Fresh-context security, registration-governance, governance, and adversarial judges','no prompt, transcript, or model output stored','Passed at P0-WI-15 closure') 'P0-DIST-FRESH-CHECKER-001'
if($fresh-match'Passed without independent review'){Fail 'P0-DIST-FRESH-CHECKER-001'}

if($script:fail.Count-gt 0){[Console]::Error.WriteLine(('FAIL: distribution and registration boundary: '+(($script:fail|Sort-Object)-join', ')));exit 1};if(-not$Quiet){Write-Output ('PASS: distribution and registration boundary ({0} checks; registration, signing, packaging, and claims disabled)' -f $checks.Count)}
