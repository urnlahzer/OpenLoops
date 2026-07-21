[CmdletBinding()]
param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot '..\contracts\persistence\protected-state-boundary.json'),
    [string]$PrivacyPath = (Join-Path $PSScriptRoot '..\contracts\privacy\persistence-boundary.json'),
    [string]$GovernancePath = (Join-Path $PSScriptRoot '..\contracts\governance\capabilities.json'),
    [string]$AuthenticationPath = (Join-Path $PSScriptRoot '..\contracts\identity\authentication-boundary.json'),
    [string]$PermissionPath = (Join-Path $PSScriptRoot '..\contracts\identity\permission-boundary.json'),
    [string]$SynchronizationPath = (Join-Path $PSScriptRoot '..\contracts\synchronization\mail-sync-boundary.json'),
    [string]$SupportPath = (Join-Path $PSScriptRoot '..\contracts\support\support-matrix.json'),
    [string]$BuildPath = (Join-Path $PSScriptRoot '..\contracts\build-skeleton\skeleton.json'),
    [string]$TraceabilityPath = (Join-Path $PSScriptRoot '..\docs\prd-traceability.md'),
    [string]$AdrPath = (Join-Path $PSScriptRoot '..\docs\adr\ADR-005-persistence-and-cryptography.md'),
    [string]$ThreatPath = (Join-Path $PSScriptRoot '..\docs\threat-model\protected-local-state.md'),
    [string]$WorkspaceScanRoot = (Join-Path $PSScriptRoot '..'),
    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
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
function ExactOrdered([object[]]$Actual, [object[]]$Expected, [string]$Id) {
    $a = @($Actual | ForEach-Object { [string]$_ })
    $e = @($Expected | ForEach-Object { [string]$_ })
    if ($a.Count -ne $e.Count) { Fail $Id; return }
    for ($index = 0; $index -lt $a.Count; $index++) {
        if ($a[$index] -cne $e[$index]) { Fail $Id; return }
    }
}
function Empty([object[]]$Actual, [string]$Id) { if (@($Actual).Count -ne 0) { Fail $Id } }
function Has([string]$Text, [string[]]$Phrases, [string]$Id) {
    foreach ($phrase in $Phrases) {
        if ($Text -notmatch [regex]::Escape($phrase)) { Fail $Id }
    }
}
function Resolve-Input([string]$Path) {
    if (-not [IO.Path]::IsPathRooted($Path)) { $Path = Join-Path $repoRoot $Path }
    return (Resolve-Path -LiteralPath $Path).Path
}
function Test-JsonObject([System.Text.Json.JsonElement]$Element) {
    if ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Object) {
        $names = @($Element.EnumerateObject() | ForEach-Object Name)
        if ($names.Count -ne (@($names | Sort-Object -Unique)).Count) { Fail 'P0-STATE-SCHEMA-001' }
        foreach ($property in $Element.EnumerateObject()) { Test-JsonObject $property.Value }
    } elseif ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Array) {
        foreach ($item in $Element.EnumerateArray()) { Test-JsonObject $item }
    }
}

try {
    $raw = Get-Content -Raw -LiteralPath (Resolve-Input $ManifestPath)
    $document = [System.Text.Json.JsonDocument]::Parse($raw)
    Test-JsonObject $document.RootElement
    $m = $raw | ConvertFrom-Json
} catch {
    [Console]::Error.WriteLine('BLOCKED: P0-STATE-SCHEMA-001 manifest parse failed.')
    exit 1
} finally {
    if ($document) { $document.Dispose() }
}

$canonical = $m | ConvertTo-Json -Depth 100 -Compress
$hash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($canonical))).ToLowerInvariant()
if ($hash -ne 'c5d97574093ad31ef2c0d55a85b125469246b82c5f63f7ad9deb759574533282') { Fail 'P0-STATE-INVENTORY-001' }

$topKeys = 'schema_version','work_item','adr','decision_status','snapshot_date','owner_decisions','requirements','blocking_gates','acceptance_criteria','claims','logical_schema_boundary','envelope_suite','digest_suite','secret_inventory','os_protection','protected_blob_store','database_contract','transaction_contract','rollback_recovery','migration_rotation','lifecycle','privacy_artifacts','dependency_decisions','dependency_policy','cross_contracts','separate_decisions','sources'
Exact @($m.PSObject.Properties.Name) $topKeys 'P0-STATE-INVENTORY-001'
if ($m.schema_version -ne 1 -or $m.work_item -ne 'P0-WI-08' -or $m.adr -ne 'ADR-005' -or
    $m.decision_status -ne 'accepted_contract_runtime_unimplemented' -or $m.snapshot_date -ne '2026-07-20') { Fail 'P0-STATE-INVENTORY-001' }
Exact @($m.owner_decisions) @('OWN-06','OWN-07') 'P0-STATE-INVENTORY-001'
Exact @($m.requirements) @('OL-GOV-001','OL-AUTH-004','OL-AUTH-005','OL-AUTH-006','OL-SYNC-001','OL-SYNC-007','OL-SYNC-012','OL-SYNC-013','OL-NFR-004','OL-NFR-005','OL-NFR-006','OL-NFR-007','OL-NFR-012') 'P0-STATE-INVENTORY-001'
Exact @($m.blocking_gates) @('G-STATE','G-PRIV','G-SEC-AUDIT') 'P0-STATE-INVENTORY-001'
Exact @($m.acceptance_criteria) @() 'P0-STATE-CLAIMS-001'
Exact @($m.secret_inventory.id) @('state_aead_keyring','state_hmac_keyring','rollback_commit_anchor','oauth_token_state','provider_credential','pairing_root_secret','session_secret') 'P0-STATE-INVENTORY-001'
Exact @($m.sources.id) @('SRC-MS-DPAPI-PROTECT','SRC-MS-DPAPI-UNPROTECT','SRC-MS-CREDENTIAL','SRC-NIST-800-38D','SRC-NIST-800-38D-R1-PREDRAFT','SRC-RFC-5116','SRC-MS-REPLACEFILE','SRC-MS-MOVEFILEEX','SRC-MS-FLUSHFILEBUFFERS','SRC-MS-REPARSE-POINTS','SRC-MS-KNOWN-FOLDER','SRC-WINDOWS-SYS-FEATURES','SRC-SQLITE-WAL','SRC-SQLITE-BACKUP','SRC-CRATES-IO','SRC-RUSTSEC') 'P0-STATE-INVENTORY-001'
Exact @($m.dependency_decisions.crate) @('aes-gcm','hmac','sha2','getrandom','rusqlite','windows-sys','zeroize') 'P0-STATE-INVENTORY-001'
Exact @($m.separate_decisions.PSObject.Properties.Name) @('evidence_locator_catalogs','model_provider_use_and_transport','loop_state_and_policy_catalogs','reminder_ownership_and_operation_catalogs','pairing_issue_rotation_and_bridge','automation_mode_and_evaluation','installer_updater_backup_and_release','self_email_operation') 'P0-STATE-INVENTORY-001'

$schema = $m.logical_schema_boundary
Exact @($schema.PSObject.Properties.Name) @('authoritative_field_allowlist','allowlist_relation','logical_runtime_schema_approved','envelope_schema_closed','domain_catalog_status','required_validation_order','prohibited_shapes','field_contract') 'P0-STATE-SCHEMA-001'
if ($schema.authoritative_field_allowlist -ne 'contracts/privacy/persistence-boundary.json' -or
    $schema.logical_runtime_schema_approved -ne $false -or $schema.envelope_schema_closed -ne $true) { Fail 'P0-STATE-SCHEMA-001' }
ExactOrdered @($schema.required_validation_order) @('construct one exact typed logical record','reject unknown duplicate missing nullable-shape-invalid unbounded or unowned values','validate the complete owning-ADR catalog immediately before encryption','serialize with the canonical binary codec','authenticate and encrypt the bounded record','on read authenticate the entire envelope before releasing any plaintext','decrypt into a bounded temporary buffer','decode and validate the exact versioned logical schema before any consumer sees it','on migration validate both the old schema and the total new schema before replacement') 'P0-STATE-SCHEMA-001'
Exact @($schema.prohibited_shapes) @('arbitrary_string','arbitrary_bytes','extension_map','flattened_unknown_fields','generic_json_value','open_enum','silent_field_drop','unbounded_array','unbounded_blob','unbounded_map','unbounded_text') 'P0-STATE-SCHEMA-001'
Exact @($schema.field_contract.PSObject.Properties.Name) @('coverage','types','strings','collections','versions','locators','domain_catalogs') 'P0-STATE-SCHEMA-001'
Has ($schema.field_contract | ConvertTo-Json -Depth 10 -Compress) @('every privacy-allowlisted field maps exactly once','fixed maximum count','unknown future duplicate or zero versions reject','strict tagged variants','ADR-006 ADR-008 ADR-009') 'P0-STATE-SCHEMA-001'

$e = $m.envelope_suite
if ($e.ciphertext_version -ne 1 -or $e.algorithm_id -ne 'AES-256-GCM' -or $e.key_bytes -ne 32 -or
    $e.nonce_bytes -ne 12 -or $e.tag_bytes -ne 16 -or $e.maximum_plaintext_bytes -ne 1048576 -or
    $e.maximum_ciphertext_bytes -ne 1048576 -or $e.algorithm_code -ne 1 -or $e.per_key_invocation_hard_limit -ne 4294967296 -or
    $e.application_rotation_limit -ne 1073741824 -or $e.implementation_status -ne 'unimplemented') { Fail 'P0-STATE-ENVELOPE-001' }
if ($e.partial_plaintext_release -ne 'prohibited' -or $e.tag_truncation -ne 'prohibited' -or
    $e.hazardous_api_use -ne 'prohibited') { Fail 'P0-STATE-ENVELOPE-001' }
if ($e.identifier_layout.account_binding_aad_bytes -ne 16 -or $e.identifier_layout.record_id_bytes -ne 16 -or
    $e.identifier_layout.key_id_bytes -ne 16 -or $e.identifier_layout.identifier_generation -notmatch 'independent random Windows-CSPRNG bytes' -or
    $e.identifier_layout.identifier_generation -notmatch 'all-zero rejected' -or
    $e.identifier_layout.schema_version_encoding -ne 'unsigned 32-bit big-endian nonzero closed catalog' -or
    $e.identifier_layout.ciphertext_version_encoding -ne 'unsigned 32-bit big-endian value 1' -or
    $e.identifier_layout.record_type_encoding -ne 'unsigned 16-bit big-endian closed code') { Fail 'P0-STATE-ENVELOPE-001' }
$expectedRecordTypes = @('account_binding','sync_checkpoint','message_observation','email_evidence_ref','calendar_invitation_ref','user_authored_artifact_ref','loop','deadline_evidence','transition','reminder_link','operation_ledger','job_health','user_setting','client_association','product_counter')
ExactOrdered @($e.record_type_codes.id) $expectedRecordTypes 'P0-STATE-ENVELOPE-001'
ExactOrdered @($e.record_type_codes.code) @(1..15) 'P0-STATE-ENVELOPE-001'
$expectedAadFields = @('openloops_state_v1_domain','algorithm_id','key_id','account_binding_aad','record_type','record_id','schema_version','ciphertext_version')
ExactOrdered @($e.aad_fields) $expectedAadFields 'P0-STATE-ENVELOPE-001'
ExactOrdered @($e.aad_encoding_rows.field) $expectedAadFields 'P0-STATE-ENVELOPE-001'
ExactOrdered @($e.aad_encoding_rows.encoding) @('18 fixed ASCII bytes openloops-state-v1','unsigned 16-bit big-endian code 1 for AES-256-GCM','16 bytes','16 bytes','unsigned 16-bit big-endian closed record_type_codes value','16 bytes','unsigned 32-bit big-endian nonzero closed value','unsigned 32-bit big-endian value 1') 'P0-STATE-ENVELOPE-001'
ExactOrdered @($e.minimum_clear_envelope_fields) @('record_id','account_binding_aad','record_type','schema_version','ciphertext_version','key_id','nonce','authentication_tag','ciphertext') 'P0-STATE-ENVELOPE-001'
if ($e.aad_total_bytes -ne 78 -or $e.aad_encoding -ne 'exact 78-byte concatenation with no length prefixes optional fields or alternative encoding') { Fail 'P0-STATE-ENVELOPE-001' }
Has ($e | ConvertTo-Json -Depth 10 -Compress) @('fresh 96-bit output from the Windows CSPRNG','never derived from time counter content identifier process state or a rollbackable value','reject a duplicate nonce','deleted-envelope nonces are not retained','previously durable protected usage reservation remains burned','release zero plaintext','tag_truncation":"prohibited','hazardous_api_use":"prohibited') 'P0-STATE-ENVELOPE-001'
if (@($e.aad_fields) -notcontains 'account_binding_aad' -or @($e.aad_fields) -notcontains 'record_id' -or
    $e.cleartext_policy -ne 'only the exact minimum-clear envelope fields; account binding and record identifiers are random application-local values') { Fail 'P0-STATE-BINDING-001' }

$d = $m.digest_suite
if ($d.algorithm_id -ne 'HMAC-SHA-256' -or $d.key_bytes -ne 32 -or $d.output_bytes -ne 32 -or
    $d.comparison -ne 'constant-time full-length comparison' -or $d.truncation -ne 'prohibited' -or
    $d.unkeyed_content_digest -ne 'prohibited' -or $d.reversible_encoding -ne 'prohibited' -or
    $d.implementation_status -ne 'unimplemented') { Fail 'P0-STATE-KEYS-001' }
if ($d.input_encoding -ne '17 fixed ASCII bytes openloops-hmac-v1, unsigned 16-bit big-endian purpose-tag byte length, exact ASCII purpose tag, 16-byte account binding, unsigned 32-bit big-endian nonzero owning-ADR schema version, unsigned 16-bit big-endian component count, then each owning-ADR-ordered component as unsigned 32-bit big-endian byte length plus exact bytes; no alternate normalization encoding or field order' -or
    $d.purpose_catalog_status -ne 'purpose tags are closed here; ADR-006 exact input-layout references are active decision contracts while remaining rows stay unavailable until every named owner closes them') { Fail 'P0-STATE-KEYS-001' }
Has $d.purpose_separation @('distinct random content-digest HMAC key per account','exact closed purpose tag','never reuse the AEAD rollback-anchor provider pairing token or session key') 'P0-STATE-KEYS-001'
$expectedPurposePairs = @('sync_checkpoint|query_fingerprint_hmac','message_observation|source_version_hmac','message_observation|observed_version_hmac','email_evidence_ref.message_locator|mailbox_binding_hmac','email_evidence_ref|content_digest_hmac','email_evidence_ref|prefix_digest_hmac','email_evidence_ref|suffix_digest_hmac','calendar_invitation_ref|event_version_hmac','user_authored_artifact_ref|title_digest_hmac','user_authored_artifact_ref|body_digest_hmac','user_authored_artifact_ref|due_digest_hmac','user_authored_artifact_ref|title_version_hmac','user_authored_artifact_ref|body_version_hmac','user_authored_artifact_ref|due_version_hmac','reminder_link|title_digest_hmac','reminder_link|body_digest_hmac','reminder_link|due_digest_hmac','reminder_link|artifact_version_hmac','reminder_link|observed_remote_version_hmac','reminder_link|remote_correlation_hmac','operation_ledger|operation_key_hmac','operation_ledger|expected_remote_version_hmac','operation_ledger|remote_correlation_hmac')
$purposePairs = @($d.purpose_catalog | ForEach-Object { "$($_.record)|$($_.field)" })
ExactOrdered $purposePairs $expectedPurposePairs 'P0-STATE-KEYS-001'
$expectedPurposeTags = @($expectedPurposePairs | ForEach-Object { $parts = $_ -split '\|'; "openloops-hmac-v1/$($parts[0])/$($parts[1] -replace '_hmac$','')" })
ExactOrdered @($d.purpose_catalog.tag) $expectedPurposeTags 'P0-STATE-KEYS-001'
$expectedClosedLayouts = @(
    'ADR-006 observation_digest_contract.query_fingerprint_v1',
    'ADR-006 observation_digest_contract.source_version_v1',
    'ADR-006 observation_digest_contract.observed_version_v1',
    'ADR-006 locator_contract.account_binding.mailbox_binding_hmac input_layout',
    'ADR-006 canonical_anchor_contract content_input and framing',
    'ADR-006 canonical_anchor_contract prefix_input and framing',
    'ADR-006 canonical_anchor_contract suffix_input and framing'
)
ExactOrdered @($d.purpose_catalog[0..6].input_layout) $expectedClosedLayouts 'P0-STATE-KEYS-001'
if (@($d.purpose_catalog.tag | Sort-Object -Unique).Count -ne 23 -or
    @($d.purpose_catalog | Where-Object { $_.owner -notmatch '^ADR-' }).Count -ne 0 -or
    @($d.purpose_catalog[7..22] | Where-Object { $_.input_layout -ne 'unavailable_pending_owner_ADR' }).Count -ne 0) { Fail 'P0-STATE-KEYS-001' }
foreach ($secret in $m.secret_inventory) {
    Exact @($secret.PSObject.Properties.Name) @('id','owner','material','location','database_value','rotation','deletion','failure') 'P0-STATE-KEYS-001'
}
$secretText = $m.secret_inventory | ConvertTo-Json -Depth 10 -Compress
Has $secretText @('state-root.dpapi inside the protected blob store','random 256-bit rollback-anchor HMAC key and 128-bit key ID','distinct key and purpose','oauth-token-state.dpapi inside the protected blob store','provider-credential.dpapi inside the protected blob store','pairing-root.dpapi inside the protected blob store','ADR-010 defines issuance rotation and revocation','locked process memory only') 'P0-STATE-KEYS-001'
$pairing = @($m.secret_inventory | Where-Object id -eq 'pairing_root_secret')
$session = @($m.secret_inventory | Where-Object id -eq 'session_secret')
$rollbackSecret = @($m.secret_inventory | Where-Object id -eq 'rollback_commit_anchor')
if ($rollbackSecret.Count -ne 1 -or $rollbackSecret[0].database_value -ne 'prohibited; SQLite contains no generation commitment rollback-anchor key or rollback-anchor reference' -or
    $pairing.Count -ne 1 -or $pairing[0].database_value -ne 'no secret value; a reference only if later privacy-approved' -or
    $session.Count -ne 1 -or $session[0].database_value -ne 'prohibited') { Fail 'P0-STATE-KEYS-001' }

$os = $m.os_protection
if ($os.platform -ne 'Windows' -or $os.primitive -ne 'CryptProtectData and CryptUnprotectData' -or
    $os.scope -ne 'current user without CRYPTPROTECT_LOCAL_MACHINE' -or $os.description -ne 'NULL' -or
    $os.prompt -ne 'NULL' -or $os.plaintext_fallback -ne 'prohibited') { Fail 'P0-STATE-KEYS-001' }
Exact @($os.flags) @('CRYPTPROTECT_UI_FORBIDDEN') 'P0-STATE-KEYS-001'
Has ($os | ConvertTo-Json -Depth 10 -Compress) @('no constant colocated or recoverable pseudo-secret','validate exact key-bundle magic version length unique key IDs algorithms statuses and bounded entries','enterprise persistence can roam','never claim absolute machine binding','ephemeral non-mutating status or test behavior only','every Microsoft mutation fail closed without creating a cache artifact') 'P0-STATE-KEYS-001'

$blob = $m.protected_blob_store
Exact @($blob.PSObject.Properties.Name) @('root','account_directory','filesystem_boundary','directory_security','path_safety','canonical_files','temporary_name','write_steps','startup_recovery','disconnect_uninstall','runtime_status') 'P0-STATE-KEYS-001'
ExactOrdered @($blob.canonical_files.name) @('state-root.dpapi','oauth-token-state.dpapi','provider-credential.dpapi','pairing-root.dpapi') 'P0-STATE-KEYS-001'
ExactOrdered @($blob.canonical_files.maximum_bytes) @(65536,1048576,16384,4096) 'P0-STATE-KEYS-001'
if ($blob.runtime_status -ne 'unimplemented') { Fail 'P0-STATE-CLAIMS-001' }
Has ($blob | ConvertTo-Json -Depth 20 -Compress) @('FOLDERID_LocalAppData','exact 16-byte random account_binding_aad','local fixed NTFS volume only','protected DACL granting that SID only','FILE_FLAG_OPEN_REPARSE_POINT','reject every reparse point hard link count other than one path escape alternate data stream','CREATE_NEW','same account directory','MoveFileExW without REPLACE_EXISTING','MOVEFILE_WRITE_THROUGH','ReplaceFileW with lpBackupFileName NULL and dwReplaceFlags zero','REPLACEFILE_WRITE_THROUGH is prohibited','reopen the canonical target by handle','temporary file is never automatically promoted','enumerate and delete only the exact canonical and temporary-name allowlist','without recursive traversal') 'P0-STATE-KEYS-001'
$expectedBlobWriteSteps = @(
    'hold the single-account writer lock and validate the directory and existing canonical target by handle',
    'serialize one closed bounded secret-bundle version and protect the complete bytes with current-user DPAPI',
    'create the same-directory random temporary file with CREATE_NEW FILE_FLAG_OPEN_REPARSE_POINT and FILE_FLAG_WRITE_THROUGH under the exact protected DACL',
    'write the complete bounded DPAPI blob check exact byte count call FlushFileBuffers close reopen without following reparse points and validate owner DACL link count volume file identity size DPAPI authentication and the strict inner schema',
    'when no canonical target exists use MoveFileExW without REPLACE_EXISTING and with MOVEFILE_WRITE_THROUGH; a concurrent target appearance fails closed',
    'when the canonical target exists use ReplaceFileW with lpBackupFileName NULL and dwReplaceFlags zero after the temporary file is flushed; REPLACEFILE_WRITE_THROUGH is prohibited because Microsoft documents it as unsupported',
    'reopen the canonical target by handle and repeat path ACL size DPAPI and strict inner-schema validation before publishing the new protected state',
    'on any create write flush replace reopen or validation error publish nothing preserve the last independently valid canonical target where Windows did so and enter typed non-mutating recovery for every ambiguous result')
ExactOrdered @($blob.write_steps) $expectedBlobWriteSteps 'P0-STATE-KEYS-001'
$expectedBlobStartup = @(
    'enumerate by handle only the four canonical names and exact temporary-name grammar without following links; any other entry or alternate stream blocks writes and requires local cleanup review',
    'a valid canonical target is the only active value; a temporary file is never automatically promoted and is deleted best effort only after the canonical target validates',
    'missing invalid duplicate linked oversized or ambiguous canonical state enters non-mutating recovery and is never silently recreated over an existing account database',
    'power-loss durability of file replacement is not assumed from API success alone and remains subject to G-STATE restart and fault-injection tests')
ExactOrdered @($blob.startup_recovery) $expectedBlobStartup 'P0-STATE-KEYS-001'

$db = $m.database_contract
if ($db.engine -ne 'SQLite' -or $db.journal_mode -ne 'WAL' -or $db.synchronous -ne 'FULL' -or
    $db.foreign_keys -ne 'ON' -or $db.trusted_schema -ne 'OFF' -or $db.temp_store -ne 'MEMORY' -or
    $db.extension_loading -ne 'prohibited' -or $db.trace_callbacks -ne 'prohibited' -or
    $db.live_copy -ne 'prohibited' -or $db.runtime_status -ne 'unimplemented') { Fail 'P0-STATE-TRANSACTION-001' }
Has ($db | ConvertTo-Json -Depth 10 -Compress) @('one database and one protected key namespace per opaque local account binding','read back every required setting and fail closed','only the minimum-clear envelope plus authenticated ciphertext','ATTACH cross-database atomicity is prohibited','SQLite Online Backup API only after ADR-012','current user and application execution context only') 'P0-STATE-TRANSACTION-001'
if ($db.single_database_atomicity -ne 'all account page outcomes checkpoints operation entries and quarantine records share one database; ATTACH cross-database atomicity is prohibited') { Fail 'P0-STATE-TRANSACTION-001'; Fail 'P0-STATE-CROSS-CONTRACT-001' }
Exact @($db.file_catalog.PSObject.Properties.Name) @('active','rollback','staging','sidecars') 'P0-STATE-TRANSACTION-001'
Has ($db.file_catalog | ConvertTo-Json -Compress) @('state.db','state.rollback.db; internal encrypted-record recovery generation only','never a product backup or export','state.staging. plus exactly 32 lowercase hexadecimal characters','state.db-wal and state.db-shm are permitted only while the active database is open in WAL mode','rollback journals and every staging or rollback sidecar are prohibited') 'P0-STATE-TRANSACTION-001'
if ($db.account_partition -ne 'one database and one protected key namespace per opaque local account binding' -or
    $db.file_acl -ne 'current user and application execution context only; unsafe inheritance or link traversal fails closed') { Fail 'P0-STATE-BINDING-001' }

$tx = $m.transaction_contract
Exact @($tx.PSObject.Properties.Name) @('writer_model','encryption_reservation_order','post_encryption_publication_order','record_write_order','page_commit','operation_commit','quarantine_commit','whole_page_failure','crash_before_commit','crash_after_commit_before_anchor','recovery_mutation') 'P0-STATE-TRANSACTION-001'
$expectedEncryptionOrder = @(
    'hold the single-account writer lock and validate the protected state-root active database and the complete canonically ordered set of logical records and relationships in the mutation',
    'calculate the exact number of encryption attempts per active key for every record in the set and reject zero overflow hard-limit or key-status error before reservation',
    'atomically reserve that exact per-key attempt count in state-root.dpapi; every reserved slot is durable before any nonce generation and is burned on every later failure',
    'for each record in canonical transaction order generate and retained-generation collision-check one fresh random nonce against its reserved key slot',
    'encrypt and authenticate each bounded record using exactly one distinct reserved attempt')
$expectedPublicationOrder = @(
    'begin one SQLite transaction for all related records',
    'write only complete envelopes and relationship updates',
    'verify no dangling or cross-account reference is introduced',
    'commit the SQLite transaction with synchronous FULL while publishing no result cursor or outward mutation',
    'reopen and read the committed logical rows and recompute the exact database-root commitment',
    'atomically replace state-root.dpapi with anchor generation n plus one the recomputed commitment key IDs and conservative usage floors',
    'reopen and verify state-root.dpapi and compare its commitment with the current logical database',
    'only after both durable values verify may the result checkpoint cursor or outward mutation be published')
$expectedWriteOrder = @($expectedEncryptionOrder) + @($expectedPublicationOrder)
ExactOrdered @($tx.encryption_reservation_order) $expectedEncryptionOrder 'P0-STATE-ENVELOPE-001'
ExactOrdered @($tx.post_encryption_publication_order) $expectedPublicationOrder 'P0-STATE-TRANSACTION-001'
ExactOrdered @($tx.record_write_order) $expectedWriteOrder 'P0-STATE-TRANSACTION-001'
Has ($tx | ConvertTo-Json -Depth 10 -Compress) @('single process single writer per account','durable safe item outcomes or complete recoverable quarantine','before checkpoint replacement','pending operation key commits before an external request','ambiguous outcome reconciles before retry','no quarantine_ref may dangle','checkpoint replacement is prohibited','source-version idempotency replays','anchor mismatch enters recovery','recovery_mutation":"prohibited') 'P0-STATE-TRANSACTION-001'

$r = $m.rollback_recovery
$anchor = $r.anchor_format
Exact @($anchor.PSObject.Properties.Name) @('version','algorithm_id','key_bytes','key_id_bytes','output_bytes','storage','row_order','input_encoding','commitment','database_generation_field') 'P0-STATE-ROLLBACK-001'
if ($anchor.version -ne 1 -or $anchor.algorithm_id -ne 'HMAC-SHA-256' -or $anchor.key_bytes -ne 32 -or
    $anchor.key_id_bytes -ne 16 -or $anchor.output_bytes -ne 32 -or $anchor.database_generation_field -ne 'prohibited; the protected anchor generation is the sole generation counter so no unapproved database field is introduced') { Fail 'P0-STATE-ROLLBACK-001' }
Has ($anchor | ConvertTo-Json -Depth 10 -Compress) @('distinct rollback-anchor key','never stored in SQLite or reused for content digests','ascending unsigned record_type code then lexicographic unsigned 16-byte record_id','duplicate sort keys reject','25 fixed ASCII bytes openloops-state-anchor-v1','unsigned 64-bit big-endian nonzero anchor generation','every current logical row','16-byte record_id','16-byte account_binding_aad','unsigned 16-bit big-endian record_type','unsigned 32-bit big-endian ciphertext length','exact ciphertext bytes','no SQLite page freelist WAL SHM file timestamp path or physical-layout input','full 32-byte HMAC output','no unknown row or clear metadata is ignored') 'P0-STATE-ROLLBACK-001'
$expectedAnchorProtocol = @(
    'under the single-account writer lock read and strictly validate state-root.dpapi and the active database before any write',
    'recompute the exact logical database-root HMAC and compare the full commitment key IDs and reserved usage floors with the protected anchor',
    'for every mutating SQLite transaction follow transaction_contract.record_write_order and replace the protected anchor only after the database commit',
    'treat commitment mismatch unexpected sidecar missing key unknown version reserved-usage regression clock reversal tamper corruption or an interrupted transaction as recovery; ordinary absence of WAL SHM and rollback-journal files while closed is valid',
    'on every process start reconcile mail checkpoints and reminder operations before enabling outward mutation because coherent profile or VM rollback may be undetectable',
    'after bounded reconciliation and explicit review of ambiguity create fresh active write keys and a new anchor before mutation')
ExactOrdered @($r.anchor_protocol) $expectedAnchorProtocol 'P0-STATE-ROLLBACK-001'
Has ($r | ConvertTo-Json -Depth 10 -Compress) @('anchor mismatch; enter recovery','enter recovery without automatic deletion or reset','DPAPI unprotect or ACL check fails closed','always treated as an untrusted restored snapshot and fully reconciled before mutation','never claimed cryptographically detectable or impossible','never affects nonce or key generation','release no plaintext and do not silently reset','all Graph and reminder mutation disabled','requires explicit user recovery choice') 'P0-STATE-ROLLBACK-001'
if ($r.database_key_mismatch -ne 'enter recovery without automatic deletion or reset' -or
    $r.cross_user -ne 'DPAPI unprotect or ACL check fails closed' -or
    $r.simple_machine_transfer -ne 'DPAPI unprotect normally fails; any unexpected success still requires account binding and recovery') { Fail 'P0-STATE-BINDING-001' }

$mr = $m.migration_rotation
Exact @($mr.compatibility.PSObject.Properties.Name) @('same_known_versions','known_older_versions','unknown_or_newer_schema','unknown_or_newer_ciphertext','old_binary_newer_store','mixed_writable_generations') 'P0-STATE-MIGRATION-001'
if ($mr.compatibility.unknown_or_newer_schema -ne 'refuse read-for-use and write; enter recovery or require newer binary' -or
    $mr.compatibility.unknown_or_newer_ciphertext -ne 'refuse decrypt-for-use and write' -or
    $mr.compatibility.old_binary_newer_store -ne 'read-only refusal with zero mutation and no downgrade conversion' -or
    $mr.compatibility.mixed_writable_generations -ne 'prohibited') { Fail 'P0-STATE-MIGRATION-001' }
$expectedMigrationSteps = @(
    'stop jobs outward mutations and cursor advancement under exclusive account lock',
    'verify the active logical database-root commitment then atomically write migration phase copying with the old commitment target schema and ciphertext versions and no proposed commitment into state-root.dpapi while preserving the verified old generation and keys',
    'checkpoint the active WAL with PRAGMA wal_checkpoint(TRUNCATE) requiring zero busy and zero uncheckpointed frames set journal_mode DELETE close every active handle and require the active WAL SHM and rollback journal to be absent',
    'create a current-user-only same-directory sibling staging database with the exact random staging filename set journal_mode WAL and synchronous FULL and read back both settings',
    'authenticate decrypt and validate each old record against its exact old schema',
    'apply one total typed migration with no unknown-field drop',
    'validate each complete new logical record and form the canonical ordered migration record set before any encryption',
    'calculate the exact number of migration encryption attempts per active key and atomically add those durable reservations to state-root.dpapi migration phase copying before any nonce generation; every reserved slot burns on later failure',
    'for each migration record in canonical order generate and retained-generation collision-check one fresh nonce then encrypt it with exactly one distinct reserved attempt',
    'copy all encrypted records and cross-record relations then verify counts identities references and authentication',
    'commit staging with synchronous FULL run PRAGMA wal_checkpoint(TRUNCATE) requiring zero busy and zero uncheckpointed frames set journal_mode DELETE close every staging handle require every staging WAL SHM and rollback journal to be absent and FlushFileBuffers the staging main file',
    'reopen staging read every logical row recompute and verify the exact proposed database-root commitment then close it',
    'atomically replace state-root.dpapi with migration phase ready_to_swap containing the verified old and proposed commitments while anchor generation n and the old active commitment remain authoritative then reopen and verify it',
    'with active staging and state.rollback.db on one validated volume call ReplaceFileW to replace state.db with staging and create exactly state.rollback.db using dwReplaceFlags zero; no generic backup export or copy is permitted',
    'reopen state.db validate every row and verify the proposed commitment while state.rollback.db remains read-only recovery state and jobs remain stopped',
    'atomically replace state-root.dpapi with anchor generation n plus one the verified active commitment and migration phase installed_pending_cleanup then reopen and verify both durable values',
    'delete state.rollback.db only after the new anchor and active database verify then reopen state.db in WAL mode and read back all required PRAGMAs while installed_pending_cleanup remains durable',
    'atomically clear migration state from state-root.dpapi only after rollback deletion WAL reopening and PRAGMA verification then reopen and verify the final anchor',
    'reconcile checkpoints and operations before jobs cursor advancement results or outward mutations resume; retire old keys only after the explicit recovery decision')
ExactOrdered @($mr.migration_steps) $expectedMigrationSteps 'P0-STATE-MIGRATION-001'
ExactOrdered @($mr.key_rotation_steps) @('stop jobs outward mutations and cursor advancement under the exclusive account lock and create a protected rotation phase','create a fresh random key and unique random key ID in a protected pending key bundle and make it active only after durable bundle replacement','for each bounded resumable canonical record batch authenticate decrypt validate and calculate the exact number of encryption attempts under the new key','atomically reserve that exact batch attempt count in state-root.dpapi before any nonce generation; every reserved slot burns on any later failure','for each batch record generate and retained-generation collision-check one fresh random nonce then re-encrypt with exactly one distinct reserved attempt','commit and verify each already encrypted complete batch using transaction_contract.post_encryption_publication_order without repeating reservation nonce generation or encryption','reopen and verify every rotated record relation protected reservation floor and current logical database-root commitment','retain the old key as decrypt-only while any active staging recovery or retained record needs it','retire the old key best effort only after complete verification and the explicit recovery decision') 'P0-STATE-MIGRATION-001'
Has ($mr | ConvertTo-Json -Depth 10 -Compress) @('retain one verified prior generation or enter non-mutating recovery','disable writes and require a reviewed new ciphertext version migration','jobs remain stopped or transactionally retry','older binary never writes a newer store') 'P0-STATE-MIGRATION-001'
Has $mr.interrupted_migration_recovery @('on every startup before ordinary anchor validation','copying requires state.db to match the old commitment','without promoting staging','ready_to_swap accepts only state.db old with rollback absent','state.db proposed with state.rollback.db old','installed_pending_cleanup requires state.db and anchor to match the proposed commitment','accepts the verified old rollback generation as present or already deleted','any migration artifact without a recognized phase','enters non-mutating recovery') 'P0-STATE-MIGRATION-001'
Has $mr.interrupted_key_rotation_recovery @('protected rotation phase records old and new key IDs','last fully anchored batch boundary','reservation floors','resumes only after the last fully anchored batch','never reuses burned reservations','old key decrypt-only','non-mutating recovery') 'P0-STATE-MIGRATION-001'

$life = $m.lifecycle
Has ($life | ConvertTo-Json -Depth 10 -Compress) @('contracts/privacy/persistence-boundary.json exact policies and transitions','reference-aware local record deletion','not promised for SSD history paging WAL journals temp files crash residue backups snapshots','delete only the affected local checkpoint','atomically delete related job_health','regardless of age with zero Graph mutation','stop jobs new identity work cursor advancement and outward mutations','delete account-bound database anchors keys and local state','leave Microsoft artifacts unchanged','identity reconnect and affected capabilities remain disabled','global logout or revocation','residual consent Microsoft artifacts backups and snapshots','graph_mutation_on_delete_reset_disconnect_uninstall":"prohibited','product_state_export":"disabled','metric_export":"disabled','recovery_key_export":"prohibited','plaintext_backup":"prohibited') 'P0-STATE-LIFECYCLE-001'
ExactOrdered @($life.disconnect_steps) @('stop jobs new identity work cursor advancement and outward mutations','commit or mark every in-flight and ambiguous operation for non-mutating recovery','clear ephemeral transaction and session state','remove application-owned protected token and provider state where supported','delete account-bound database anchors keys and local state','leave Microsoft artifacts unchanged for remain detach or individually reviewed user handling','disclose that browser sessions consent access tokens backups and Microsoft artifacts may remain') 'P0-STATE-LIFECYCLE-001'

$pa = $m.privacy_artifacts
Exact @($pa.forbidden_classes) @('human-readable mailbox source or generated text','task title body checklist or deadline phrase','participant attachment link filename or URL text','raw Microsoft tenant account workspace object or message identifiers','raw Graph cursor URL request response header body or error','credential token authorization code verifier state key or secret','prompt model request response output rationale embedding transcript or vector','real fixture sample label prediction correction or diagnostic content','log telemetry browser cache support bundle crash dump screenshot HAR source map package diagnostic or automatic upload') 'P0-STATE-PRIVACY-001'
Exact @($pa.canary_scan_surfaces) @('database','WAL','SHM','rollback journal','temp and spill files','migration staging','backup and recovery copies','logs telemetry and browser caches under application control','application-created crash and diagnostic artifacts','packages installers CI artifacts and repository history') 'P0-STATE-PRIVACY-001'
Empty @($pa.diagnostic_allowlist) 'P0-STATE-PRIVACY-001'
if ($pa.support_bundle -ne 'disabled' -or $pa.automatic_upload -ne 'prohibited' -or
    $pa.canary_output -ne 'fail without printing the suspected value' -or $pa.tracked_generated_artifacts -ne 'prohibited') { Fail 'P0-STATE-PRIVACY-001' }

foreach ($name in $m.claims.PSObject.Properties.Name) { Empty @($m.claims.$name) 'P0-STATE-CLAIMS-001' }
foreach ($dep in $m.dependency_decisions) {
    Exact @($dep.PSObject.Properties.Name) @('crate','version','default_features','features','license','purpose','risk_note') 'P0-STATE-INVENTORY-001'
    if ($dep.default_features -ne $false -or $dep.version -notmatch '^\d+\.\d+\.\d+$') { Fail 'P0-STATE-INVENTORY-001' }
}
$windowsDependency = @($m.dependency_decisions | Where-Object crate -eq 'windows-sys')
$expectedWindowsFeatures = @('Win32_Foundation','Win32_Security','Win32_Security_Authorization','Win32_Security_Cryptography','Win32_Storage_FileSystem','Win32_System_Com','Win32_System_Memory','Win32_System_Threading','Win32_UI_Shell')
if ($windowsDependency.Count -ne 1) { Fail 'P0-STATE-INVENTORY-001' } else {
    ExactOrdered @($windowsDependency[0].features) $expectedWindowsFeatures 'P0-STATE-INVENTORY-001'
    Has ($windowsDependency[0] | ConvertTo-Json -Compress) @('DPAPI CSPRNG known-folder file atomic-replacement handle identity ACL memory-release and interprocess-lock bindings','LocalFree CoTaskMemFree ACL handle path') 'P0-STATE-INVENTORY-001'
}
if ($m.dependency_policy.activation -ne 'selected_not_activated' -or $m.dependency_policy.git_dependencies -ne 'prohibited' -or
    $m.dependency_policy.wildcard_versions -ne 'prohibited' -or $m.dependency_policy.ad_hoc_cryptography -ne 'prohibited' -or
    $m.dependency_policy.rust_compatibility_target -ne '1.97.1') { Fail 'P0-STATE-CLAIMS-001' }

# Cross-contract checks intentionally compare exact identities and the inactive runtime posture.
$privacy = Get-Content -Raw -LiteralPath (Resolve-Input $PrivacyPath) | ConvertFrom-Json
$privacyEnvelope = @($privacy.record_types | Where-Object id -eq 'persistence_envelope')
if ($privacyEnvelope.Count -ne 1 -or $privacy.claim_state.logical_runtime_schema_approved -ne $false) { Fail 'P0-STATE-CROSS-CONTRACT-001'; Fail 'P0-STATE-SCHEMA-001' }
$privacyEnvelopeFields = @($privacyEnvelope[0].field_groups | ForEach-Object { @($_.fields) })
Exact @($e.minimum_clear_envelope_fields) $privacyEnvelopeFields 'P0-STATE-CROSS-CONTRACT-001'
Exact @($privacy.diagnostic_events) @() 'P0-STATE-PRIVACY-001'
Exact @($privacy.source_variants.id) @('email_evidence','calendar_invitation','user_authored_microsoft_artifact') 'P0-STATE-CROSS-CONTRACT-001'
$privacyLogicalRecords = @($privacy.record_types.id | Where-Object { $_ -ne 'persistence_envelope' })
Exact @($e.record_type_codes.id) $privacyLogicalRecords 'P0-STATE-CROSS-CONTRACT-001'
$privacyPurposePairs = [System.Collections.Generic.List[string]]::new()
foreach ($record in $privacy.record_types) {
    foreach ($group in $record.field_groups) {
        if ($group.semantic_type -eq 'keyed_digest') {
            foreach ($field in $group.fields) { $privacyPurposePairs.Add("$($record.id)|$field") }
        }
    }
}
$privacyPurposePairs.Add('email_evidence_ref.message_locator|mailbox_binding_hmac')
Exact @($purposePairs) @($privacyPurposePairs) 'P0-STATE-CROSS-CONTRACT-001'

$gov = Get-Content -Raw -LiteralPath (Resolve-Input $GovernancePath) | ConvertFrom-Json
$a5 = @($gov.adrs | Where-Object id -eq 'ADR-005')
if ($a5.Count -ne 1 -or $a5[0].status -ne 'accepted' -or
    @($gov.adrs | Where-Object { $_.id -notin @('ADR-001','ADR-002','ADR-003','ADR-004','ADR-005','ADR-006','ADR-007','ADR-008','ADR-009','ADR-010','ADR-PRIV-001') -and $_.status -ne 'planned' }).Count -ne 0 -or
    @($gov.gates | Where-Object status -ne 'unrun').Count -ne 0 -or
    @($gov.capabilities | Where-Object { $_.state -ne 'disabled' -or $_.advertised -ne $false }).Count -ne 0 -or
    @($gov.product_acceptance_criteria_completed).Count -ne 0) { Fail 'P0-STATE-CROSS-CONTRACT-001'; Fail 'P0-STATE-CLAIMS-001' }
$stateCaps = @($gov.capabilities | Where-Object id -in @('encrypted_local_state','microsoft_hosted_state_adapter'))
if ($stateCaps.Count -ne 2 -or @($stateCaps | Where-Object { $_.state -ne 'disabled' -or $_.advertised -ne $false }).Count -ne 0) { Fail 'P0-STATE-CLAIMS-001' }
Exact @($m.cross_contracts.threat_routes) @('oauth_token_state','local_state','disconnect_uninstall') 'P0-STATE-CROSS-CONTRACT-001'
foreach ($route in $m.cross_contracts.threat_routes) {
    if (@($gov.threat_flows | Where-Object id -eq $route).Count -ne 1) { Fail 'P0-STATE-CROSS-CONTRACT-001' }
}

$auth = Get-Content -Raw -LiteralPath (Resolve-Input $AuthenticationPath) | ConvertFrom-Json
if ($auth.flow.runtime_status -ne 'unimplemented' -or $auth.token_boundary.secure_store_failure -ne 'session_only_or_fail_closed_no_plaintext_fallback' -or
    $auth.disconnect_contract.'delete_account_bound_application_state_after_ADR-005' -ne $true -or @($auth.claims.network_contacts).Count -ne 0) { Fail 'P0-STATE-CROSS-CONTRACT-001' }
$perm = Get-Content -Raw -LiteralPath (Resolve-Input $PermissionPath) | ConvertFrom-Json
if (@($perm.claims.permissions_requested).Count -ne 0 -or $perm.mail_folder_policy.automatic_expansion -ne 'prohibited') { Fail 'P0-STATE-CROSS-CONTRACT-001' }
$sync = Get-Content -Raw -LiteralPath (Resolve-Input $SynchronizationPath) | ConvertFrom-Json
if ($sync.runtime_boundary.durable_state -ne $false -or $sync.page_transaction.cursor_advance_rule -ne 'prohibited until every page item is durable and safe' -or
    $sync.quarantine_policy.resolution_cleanup -notmatch 'no quarantine_ref may dangle' -or $sync.quarantine_policy.disconnect_cleanup -notmatch 'zero Graph mutation') { Fail 'P0-STATE-CROSS-CONTRACT-001'; Fail 'P0-STATE-TRANSACTION-001' }
$support = Get-Content -Raw -LiteralPath (Resolve-Input $SupportPath) | ConvertFrom-Json
if (@($support.claim_state.supported_rows).Count -ne 0) { Fail 'P0-STATE-CLAIMS-001' }
$build = Get-Content -Raw -LiteralPath (Resolve-Input $BuildPath) | ConvertFrom-Json
if ($build.runtime_boundary.durable_state -ne $false -or @($build.runtime_boundary.accepted_secrets).Count -ne 0 -or @($build.runtime_boundary.network_origins).Count -ne 0) { Fail 'P0-STATE-CLAIMS-001' }

$persistenceRoot = Join-Path $repoRoot 'crates\openloops-persistence'
$persistenceFiles = @(Get-ChildItem -LiteralPath $persistenceRoot -Recurse -File | ForEach-Object {
    [IO.Path]::GetRelativePath($persistenceRoot, $_.FullName).Replace('\','/')
})
Exact @($persistenceFiles) @('Cargo.toml','src/lib.rs') 'P0-STATE-CLAIMS-001'
$persistenceSource = Get-Content -Raw -LiteralPath (Join-Path $persistenceRoot 'src\lib.rs')
if ($persistenceSource -notmatch 'const fn is_available\(\) -> bool\s*\{\s*false\s*\}') { Fail 'P0-STATE-CLAIMS-001' }

function Get-WorkspaceBuildInputs([string]$Root) {
    $results = [System.Collections.Generic.List[string]]::new()
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $pending.Push((Resolve-Path -LiteralPath $Root).Path)
    $excluded = @('.git','target','node_modules','.generated','dist','coverage')
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        foreach ($entry in Get-ChildItem -LiteralPath $directory -Force) {
            if ($entry.PSIsContainer) {
                if ($entry.Name -notin $excluded -and -not ($entry.Attributes -band [IO.FileAttributes]::ReparsePoint)) { $pending.Push($entry.FullName) }
            } elseif ($entry.Name -in @('Cargo.toml','Cargo.lock','rust-toolchain.toml') -or $entry.Extension -eq '.rs' -or
                ($entry.Directory.Name -eq '.cargo' -and $entry.Name -in @('config','config.toml'))) {
                $results.Add($entry.FullName)
            }
        }
    }
    return @($results)
}
$workspaceRoot = Resolve-Input $WorkspaceScanRoot
$workspaceInputs = @(Get-WorkspaceBuildInputs $workspaceRoot | Sort-Object)
$workspaceRelative = @($workspaceInputs | ForEach-Object { [IO.Path]::GetRelativePath($workspaceRoot, $_).Replace('\','/') })
$expectedWorkspaceInputs = @('Cargo.lock','Cargo.toml','crates/openloops-application/Cargo.toml','crates/openloops-application/src/lib.rs','crates/openloops-contracts/Cargo.toml','crates/openloops-contracts/src/lib.rs','crates/openloops-desktop/Cargo.toml','crates/openloops-desktop/src/main.rs','crates/openloops-domain/Cargo.toml','crates/openloops-domain/src/lib.rs','crates/openloops-graph/Cargo.toml','crates/openloops-graph/src/lib.rs','crates/openloops-inference/Cargo.toml','crates/openloops-inference/src/lib.rs','crates/openloops-persistence/Cargo.toml','crates/openloops-persistence/src/lib.rs','rust-toolchain.toml')
ExactOrdered $workspaceRelative $expectedWorkspaceInputs 'P0-STATE-CLAIMS-001'
$workspaceFingerprintRows = @()
$strictUtf8 = [Text.UTF8Encoding]::new($false, $true)
foreach ($sourcePath in $workspaceInputs) {
    try {
        $sourceText = $strictUtf8.GetString([IO.File]::ReadAllBytes($sourcePath))
    } catch {
        Fail 'P0-STATE-CLAIMS-001'
        continue
    }
    if ([IO.Path]::GetFileName($sourcePath) -eq 'Cargo.toml') {
        if ($sourceText -match '(?i)\b(aes-gcm|hmac|sha2|getrandom|rusqlite|windows-sys|zeroize)\b') { Fail 'P0-STATE-CLAIMS-001' }
    } elseif ([IO.Path]::GetFileName($sourcePath) -eq 'Cargo.lock') {
        if ($sourceText -match '(?ms)\[\[package\]\]\s+name\s*=\s*"(aes-gcm|hmac|sha2|getrandom|rusqlite|windows-sys|zeroize)"') { Fail 'P0-STATE-CLAIMS-001' }
    } elseif ($sourceText -match '(?i)CryptProtectData|CryptUnprotectData|\brusqlite\b|\baes_gcm\b|Aes256Gcm|\bhmac::|\bsha2::|\bgetrandom::|\bwindows_sys\b|\bzeroize\b|\bstd::fs\b|\btokio::fs\b|File::create|OpenOptions|Connection::open') {
        Fail 'P0-STATE-CLAIMS-001'
    }
    $relative = [IO.Path]::GetRelativePath($workspaceRoot, $sourcePath).Replace('\','/')
    $normalizedSourceText = $sourceText.Replace("`r`n", "`n").Replace("`r", "`n")
    $fileHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($strictUtf8.GetBytes($normalizedSourceText))).ToLowerInvariant()
    $workspaceFingerprintRows += "$relative`0$fileHash"
}
$workspaceFingerprintMaterial = $workspaceFingerprintRows -join "`n"
$workspaceFingerprint = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($workspaceFingerprintMaterial))).ToLowerInvariant()
if ($workspaceFingerprint -ne '860f64b95f94648163670c860ec611efb21c101b714c36b04c9485019d30a235') { Fail 'P0-STATE-CLAIMS-001' }

$trace = Get-Content -Raw -LiteralPath (Resolve-Input $TraceabilityPath)
$expectedChecks = @('P0-STATE-INVENTORY-001','P0-STATE-SCHEMA-001','P0-STATE-ENVELOPE-001','P0-STATE-KEYS-001','P0-STATE-BINDING-001','P0-STATE-TRANSACTION-001','P0-STATE-MIGRATION-001','P0-STATE-ROLLBACK-001','P0-STATE-LIFECYCLE-001','P0-STATE-PRIVACY-001','P0-STATE-CROSS-CONTRACT-001','P0-STATE-CLAIMS-001','P0-STATE-FRESH-CHECKER-001')
$traceIds = @([regex]::Matches($trace, '(?m)^\| (P0-STATE-[A-Z-]+-001) \|') | ForEach-Object { $_.Groups[1].Value })
Exact $traceIds $expectedChecks 'P0-STATE-INVENTORY-001'
$adr = (Get-Content -Raw -LiteralPath (Resolve-Input $AdrPath)) -replace '\s+',' '
Has $adr @('Status:** Accepted','P0-WI-08','Owner decisions:** OWN-06, OWN-07','exactly twelve executable P0-STATE assertions','AES-256-GCM','exact 78-byte fixed-width encoding','closed unique purpose-tag catalog','exact number of encryption attempts per active AES key','Every record consumes one distinct reservation','2^30','2^32','known-folder-resolved per-user local application-data root','Temporary files are never promoted','No rollback generation, commitment, key, or anchor reference is stored in SQLite','Every mutating SQLite transaction commits first','Nothing publishes a result, cursor, or outward mutation before that sequence completes','proposed commitment is added only after','Staging explicitly starts in WAL mode','installed_pending_cleanup','PRAGMA wal_checkpoint(TRUNCATE)','state.rollback.db','every process start reconciles mail checkpoints and reminder operations before outward mutation','Secure-store absence permits only ephemeral, non-mutating','No gate, capability, support row, or acceptance criterion advances') 'P0-STATE-CROSS-CONTRACT-001'
$threat = Get-Content -Raw -LiteralPath (Resolve-Input $ThreatPath)
foreach ($id in 1..12) { if ($threat -notmatch ('TM-STATE-{0:d3}' -f $id)) { Fail 'P0-STATE-CROSS-CONTRACT-001' } }

if ($failures.Count -gt 0) {
    [Console]::Error.WriteLine("BLOCKED: protected-state checks failed: $($failures -join ', ')")
    exit 1
}
if (-not $Quiet) { Write-Host 'OpenLoops protected-state checks passed (12 P0-WI-08 assertions; runtime and all gates remain inactive).' }
