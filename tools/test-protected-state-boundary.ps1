[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$checker = Join-Path $PSScriptRoot 'check-protected-state-boundary.ps1'
$manifestPath = Join-Path $repoRoot 'contracts\persistence\protected-state-boundary.json'
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ("openloops-p0-state-" + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tempRoot)
$passed = 0
$failed = [System.Collections.Generic.List[string]]::new()

function Invoke-Checker([hashtable]$Overrides) {
    $args = @('-NoProfile','-File',$checker,'-Quiet')
    foreach ($key in $Overrides.Keys) { $args += @("-$key", [string]$Overrides[$key]) }
    $nativePreference = Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue
    $priorNativePreference = if ($nativePreference) { $nativePreference.Value } else { $null }
    $priorErrorPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $PSNativeCommandUseErrorActionPreference = $false
        $output = & pwsh @args 2>&1 | Out-String
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $priorErrorPreference
        if ($nativePreference) {
            $PSNativeCommandUseErrorActionPreference = $priorNativePreference
        } else {
            Remove-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue
        }
    }
    return @{ Code = $code; Output = $output }
}

function Record-Result([string]$Name, [string]$Expected, [hashtable]$Result) {
    if ($Result.Code -ne 0 -and $Result.Output -match [regex]::Escape($Expected)) {
        $script:passed++
    } else {
        $script:failed.Add("$Name expected $Expected")
    }
}

function Record-AcceptedResult([string]$Name, [hashtable]$Result) {
    if ($Result.Code -eq 0) {
        $script:passed++
    } else {
        $script:failed.Add("$Name expected acceptance")
    }
}

function Invoke-ManifestCase([string]$Name, [string]$Expected, [scriptblock]$Mutate) {
    $copy = (Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json)
    & $Mutate $copy
    $path = Join-Path $tempRoot (([guid]::NewGuid().ToString('N')) + '.json')
    $copy | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $path -Encoding utf8NoBOM
    Record-Result $Name $Expected (Invoke-Checker @{ ManifestPath = $path })
}

function Invoke-RawManifestCase([string]$Name, [string]$Expected, [scriptblock]$Mutate) {
    $raw = Get-Content -Raw -LiteralPath $manifestPath
    $changed = & $Mutate $raw
    $path = Join-Path $tempRoot (([guid]::NewGuid().ToString('N')) + '.json')
    Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM -NoNewline
    Record-Result $Name $Expected (Invoke-Checker @{ ManifestPath = $path })
}

function Invoke-ContractCase([string]$Name, [string]$Expected, [string]$RelativePath, [string]$Parameter, [scriptblock]$Mutate) {
    $source = Join-Path $repoRoot $RelativePath
    $raw = Get-Content -Raw -LiteralPath $source
    $changed = & $Mutate $raw
    $path = Join-Path $tempRoot (([guid]::NewGuid().ToString('N')) + [IO.Path]::GetExtension($source))
    Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM -NoNewline
    Record-Result $Name $Expected (Invoke-Checker @{ $Parameter = $path })
}

function Invoke-ExactWorkspaceCase([string]$Name, [string]$Expected, [scriptblock]$Mutate) {
    $root = Join-Path $tempRoot ([guid]::NewGuid().ToString('N'))
    $inputs = @('Cargo.lock','Cargo.toml','crates/openloops-application/Cargo.toml','crates/openloops-application/src/lib.rs','crates/openloops-contracts/Cargo.toml','crates/openloops-contracts/src/lib.rs','crates/openloops-desktop/Cargo.toml','crates/openloops-desktop/src/main.rs','crates/openloops-domain/Cargo.toml','crates/openloops-domain/src/command.rs','crates/openloops-domain/src/deadline.rs','crates/openloops-domain/src/display.rs','crates/openloops-domain/src/establishment.rs','crates/openloops-domain/src/facets.rs','crates/openloops-domain/src/hypothesis.rs','crates/openloops-domain/src/ids.rs','crates/openloops-domain/src/legality.rs','crates/openloops-domain/src/lib.rs','crates/openloops-domain/src/record.rs','crates/openloops-domain/src/transition.rs','crates/openloops-graph/Cargo.toml','crates/openloops-graph/src/lib.rs','crates/openloops-inference/Cargo.toml','crates/openloops-inference/src/blocks.rs','crates/openloops-inference/src/canonical.rs','crates/openloops-inference/src/error.rs','crates/openloops-inference/src/lib.rs','crates/openloops-inference/src/message.rs','crates/openloops-inference/src/walker.rs','crates/openloops-persistence/Cargo.toml','crates/openloops-persistence/src/aad.rs','crates/openloops-persistence/src/anchor.rs','crates/openloops-persistence/src/digest.rs','crates/openloops-persistence/src/dpapi_ffi.rs','crates/openloops-persistence/src/dpapi.rs','crates/openloops-persistence/src/envelope.rs','crates/openloops-persistence/src/error.rs','crates/openloops-persistence/src/ids.rs','crates/openloops-persistence/src/lib.rs','crates/openloops-persistence/src/protected_file.rs','crates/openloops-persistence/src/reservation.rs','crates/openloops-persistence/src/state_root.rs','crates/openloops-persistence/src/store.rs','rust-toolchain.toml')
    foreach ($relative in $inputs) {
        $target = Join-Path $root $relative
        [void](New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force)
        Copy-Item -LiteralPath (Join-Path $repoRoot $relative) -Destination $target
    }
    & $Mutate $root
    Record-Result $Name $Expected (Invoke-Checker @{ WorkspaceScanRoot = $root })
}

function Invoke-ExactWorkspaceAcceptedCase([string]$Name, [scriptblock]$Mutate) {
    $root = Join-Path $tempRoot ([guid]::NewGuid().ToString('N'))
    $inputs = @('Cargo.lock','Cargo.toml','crates/openloops-application/Cargo.toml','crates/openloops-application/src/lib.rs','crates/openloops-contracts/Cargo.toml','crates/openloops-contracts/src/lib.rs','crates/openloops-desktop/Cargo.toml','crates/openloops-desktop/src/main.rs','crates/openloops-domain/Cargo.toml','crates/openloops-domain/src/command.rs','crates/openloops-domain/src/deadline.rs','crates/openloops-domain/src/display.rs','crates/openloops-domain/src/establishment.rs','crates/openloops-domain/src/facets.rs','crates/openloops-domain/src/hypothesis.rs','crates/openloops-domain/src/ids.rs','crates/openloops-domain/src/legality.rs','crates/openloops-domain/src/lib.rs','crates/openloops-domain/src/record.rs','crates/openloops-domain/src/transition.rs','crates/openloops-graph/Cargo.toml','crates/openloops-graph/src/lib.rs','crates/openloops-inference/Cargo.toml','crates/openloops-inference/src/blocks.rs','crates/openloops-inference/src/canonical.rs','crates/openloops-inference/src/error.rs','crates/openloops-inference/src/lib.rs','crates/openloops-inference/src/message.rs','crates/openloops-inference/src/walker.rs','crates/openloops-persistence/Cargo.toml','crates/openloops-persistence/src/aad.rs','crates/openloops-persistence/src/anchor.rs','crates/openloops-persistence/src/digest.rs','crates/openloops-persistence/src/dpapi_ffi.rs','crates/openloops-persistence/src/dpapi.rs','crates/openloops-persistence/src/envelope.rs','crates/openloops-persistence/src/error.rs','crates/openloops-persistence/src/ids.rs','crates/openloops-persistence/src/lib.rs','crates/openloops-persistence/src/protected_file.rs','crates/openloops-persistence/src/reservation.rs','crates/openloops-persistence/src/state_root.rs','crates/openloops-persistence/src/store.rs','rust-toolchain.toml')
    foreach ($relative in $inputs) {
        $target = Join-Path $root $relative
        [void](New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force)
        Copy-Item -LiteralPath (Join-Path $repoRoot $relative) -Destination $target
    }
    & $Mutate $root
    Record-AcceptedResult $Name (Invoke-Checker @{ WorkspaceScanRoot = $root })
}

try {
    $baseline = Invoke-Checker @{}
    if ($baseline.Code -ne 0) { throw "Baseline protected-state checker failed." }

    Invoke-ManifestCase 'work item drift' 'P0-STATE-INVENTORY-001' { param($m) $m.work_item = 'P0-WI-X' }
    Invoke-ManifestCase 'owner omitted' 'P0-STATE-INVENTORY-001' { param($m) $m.owner_decisions = @('OWN-06') }
    Invoke-ManifestCase 'gate omitted' 'P0-STATE-INVENTORY-001' { param($m) $m.blocking_gates = @('G-STATE','G-PRIV') }
    Invoke-ManifestCase 'source omitted' 'P0-STATE-INVENTORY-001' { param($m) $m.sources = @($m.sources | Select-Object -Skip 1) }
    Invoke-ManifestCase 'secret class omitted' 'P0-STATE-INVENTORY-001' { param($m) $m.secret_inventory = @($m.secret_inventory | Where-Object id -ne 'rollback_commit_anchor') }
    Invoke-ManifestCase 'dependency added' 'P0-STATE-INVENTORY-001' { param($m) $m.dependency_decisions += [pscustomobject]@{crate='unknown';version='1.0.0';default_features=$false;features=@();license='MIT';purpose='x';risk_note='x'} }
    Invoke-ManifestCase 'Windows file feature omitted' 'P0-STATE-INVENTORY-001' { param($m) $dep=($m.dependency_decisions | Where-Object crate -eq 'windows-sys');$dep.features=@($dep.features | Where-Object { $_ -ne 'Win32_Storage_FileSystem' }) }
    Invoke-ManifestCase 'deferred owner omitted' 'P0-STATE-INVENTORY-001' { param($m) $m.separate_decisions.PSObject.Properties.Remove('self_email_operation') }
    Invoke-RawManifestCase 'duplicate JSON key' 'P0-STATE-SCHEMA-001' { param($r) $r.Replace('"schema_version": 1,','"schema_version": 1, "schema_version": 1,') }

    Invoke-ManifestCase 'runtime schema prematurely approved' 'P0-STATE-SCHEMA-001' { param($m) $m.logical_schema_boundary.logical_runtime_schema_approved = $true }
    Invoke-ManifestCase 'open enum allowed' 'P0-STATE-SCHEMA-001' { param($m) $m.logical_schema_boundary.prohibited_shapes = @($m.logical_schema_boundary.prohibited_shapes | Where-Object { $_ -ne 'open_enum' }) }
    Invoke-ManifestCase 'pre-encryption validation removed' 'P0-STATE-SCHEMA-001' { param($m) $m.logical_schema_boundary.required_validation_order = @($m.logical_schema_boundary.required_validation_order | Where-Object { $_ -notmatch 'immediately before encryption' }) }
    Invoke-ManifestCase 'post-decrypt validation removed' 'P0-STATE-SCHEMA-001' { param($m) $m.logical_schema_boundary.required_validation_order = @($m.logical_schema_boundary.required_validation_order | Where-Object { $_ -notmatch 'before any consumer' }) }
    Invoke-ManifestCase 'unbounded collections' 'P0-STATE-SCHEMA-001' { param($m) $m.logical_schema_boundary.field_contract.collections = 'unbounded' }
    Invoke-ManifestCase 'generic locator' 'P0-STATE-SCHEMA-001' { param($m) $m.logical_schema_boundary.field_contract.locators = 'string' }
    Invoke-ManifestCase 'domain catalogs accepted' 'P0-STATE-SCHEMA-001' { param($m) $m.logical_schema_boundary.field_contract.domain_catalogs = 'accepted' }
    Invoke-ManifestCase 'validation order swapped' 'P0-STATE-SCHEMA-001' { param($m) $v=$m.logical_schema_boundary.required_validation_order[0];$m.logical_schema_boundary.required_validation_order[0]=$m.logical_schema_boundary.required_validation_order[1];$m.logical_schema_boundary.required_validation_order[1]=$v }

    Invoke-ManifestCase 'algorithm substitution' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.algorithm_id = 'AES-128-GCM' }
    Invoke-ManifestCase 'algorithm code drift' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.algorithm_code = 2 }
    Invoke-ManifestCase 'short key' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.key_bytes = 16 }
    Invoke-ManifestCase 'wrong nonce length' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.nonce_bytes = 16 }
    Invoke-ManifestCase 'truncated tag' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.tag_bytes = 12 }
    Invoke-ManifestCase 'nonce counter' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.nonce_generation = 'rollbackable counter' }
    Invoke-ManifestCase 'collision accepted' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.nonce_collision_defense = 'accept' }
    Invoke-ManifestCase 'GCM hard limit widened' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.per_key_invocation_hard_limit = 8589934592 }
    Invoke-ManifestCase 'rotation too late' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.application_rotation_limit = 4294967296 }
    Invoke-ManifestCase 'RNG fallback' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.rng_failure = 'use timestamp' }
    Invoke-ManifestCase 'partial plaintext release' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.partial_plaintext_release = 'allowed' }
    Invoke-ManifestCase 'AAD algorithm omitted' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.aad_fields = @($m.envelope_suite.aad_fields | Where-Object { $_ -ne 'algorithm_id' }) }
    Invoke-ManifestCase 'AAD record omitted' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.aad_fields = @($m.envelope_suite.aad_fields | Where-Object { $_ -ne 'record_id' }) }
    Invoke-ManifestCase 'AAD fields reordered' 'P0-STATE-ENVELOPE-001' { param($m) $v=$m.envelope_suite.aad_fields[0];$m.envelope_suite.aad_fields[0]=$m.envelope_suite.aad_fields[1];$m.envelope_suite.aad_fields[1]=$v }
    Invoke-ManifestCase 'AAD rows reordered' 'P0-STATE-ENVELOPE-001' { param($m) $v=$m.envelope_suite.aad_encoding_rows[0];$m.envelope_suite.aad_encoding_rows[0]=$m.envelope_suite.aad_encoding_rows[1];$m.envelope_suite.aad_encoding_rows[1]=$v }
    Invoke-ManifestCase 'AAD row encoding drift' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.aad_encoding_rows[0].encoding = 'variable text' }
    Invoke-ManifestCase 'extra clear envelope field' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.minimum_clear_envelope_fields += 'debug_text' }
    Invoke-ManifestCase 'short account binding' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.identifier_layout.account_binding_aad_bytes = 8 }
    Invoke-ManifestCase 'time-derived record identifier' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.identifier_layout.identifier_generation = 'timestamp UUID' }
    Invoke-ManifestCase 'record type code collision' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.record_type_codes[1].code = 1 }
    Invoke-ManifestCase 'record type order swapped' 'P0-STATE-ENVELOPE-001' { param($m) $v=$m.envelope_suite.record_type_codes[0];$m.envelope_suite.record_type_codes[0]=$m.envelope_suite.record_type_codes[1];$m.envelope_suite.record_type_codes[1]=$v }
    Invoke-ManifestCase 'record type omitted' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.record_type_codes = @($m.envelope_suite.record_type_codes | Where-Object id -ne 'product_counter') }
    Invoke-ManifestCase 'envelope runtime enabled' 'P0-STATE-ENVELOPE-001' { param($m) $m.envelope_suite.implementation_status = 'available' }

    Invoke-ManifestCase 'AAD account binding omitted' 'P0-STATE-BINDING-001' { param($m) $m.envelope_suite.aad_fields = @($m.envelope_suite.aad_fields | Where-Object { $_ -ne 'account_binding_aad' }) }
    Invoke-ManifestCase 'account partition shared' 'P0-STATE-BINDING-001' { param($m) $m.database_contract.account_partition = 'shared database' }
    Invoke-ManifestCase 'broad database ACL' 'P0-STATE-BINDING-001' { param($m) $m.database_contract.file_acl = 'all users' }
    Invoke-ManifestCase 'cross-user accepted' 'P0-STATE-BINDING-001' { param($m) $m.rollback_recovery.cross_user = 'accept' }
    Invoke-ManifestCase 'machine copy accepted' 'P0-STATE-BINDING-001' { param($m) $m.rollback_recovery.simple_machine_transfer = 'accept' }
    Invoke-ManifestCase 'key mismatch reset' 'P0-STATE-BINDING-001' { param($m) $m.rollback_recovery.database_key_mismatch = 'delete and continue' }

    Invoke-ManifestCase 'unkeyed digest' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.unkeyed_content_digest = 'allowed' }
    Invoke-ManifestCase 'digest truncation' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.output_bytes = 16 }
    Invoke-ManifestCase 'nonconstant comparison' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.comparison = 'ordinary equality' }
    Invoke-ManifestCase 'key reuse' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.purpose_separation = 'reuse AEAD key' }
    Invoke-ManifestCase 'digest purpose omitted' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.purpose_catalog = @($m.digest_suite.purpose_catalog | Select-Object -Skip 1) }
    Invoke-ManifestCase 'digest purpose tag duplicated' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.purpose_catalog[1].tag = $m.digest_suite.purpose_catalog[0].tag }
    Invoke-ManifestCase 'digest purpose tags swapped' 'P0-STATE-KEYS-001' { param($m) $v=$m.digest_suite.purpose_catalog[0].tag;$m.digest_suite.purpose_catalog[0].tag=$m.digest_suite.purpose_catalog[1].tag;$m.digest_suite.purpose_catalog[1].tag=$v }
    Invoke-ManifestCase 'closed digest layout drift' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.purpose_catalog[0].input_layout = 'utf8 query' }
    Invoke-ManifestCase 'mailbox binding digest layout drift' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.purpose_catalog[3].input_layout = 'utf8 tenant and user' }
    Invoke-ManifestCase 'deferred digest layout prematurely approved' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.purpose_catalog[7].input_layout = 'utf8 event' }
    Invoke-ManifestCase 'digest framing weakened' 'P0-STATE-KEYS-001' { param($m) $m.digest_suite.input_encoding = 'concatenate values' }
    Invoke-ManifestCase 'machine DPAPI scope' 'P0-STATE-KEYS-001' { param($m) $m.os_protection.scope = 'CRYPTPROTECT_LOCAL_MACHINE' }
    Invoke-ManifestCase 'DPAPI prompt enabled' 'P0-STATE-KEYS-001' { param($m) $m.os_protection.prompt = 'interactive' }
    Invoke-ManifestCase 'colocated entropy' 'P0-STATE-KEYS-001' { param($m) $m.os_protection.optional_entropy = 'constant in config' }
    Invoke-ManifestCase 'plaintext fallback' 'P0-STATE-KEYS-001' { param($m) $m.os_protection.plaintext_fallback = 'allowed' }
    Invoke-ManifestCase 'session mutation allowed' 'P0-STATE-KEYS-001' { param($m) $m.os_protection.secure_store_unavailable = 'allow Microsoft mutation' }
    Invoke-ManifestCase 'pairing secret in database' 'P0-STATE-KEYS-001' { param($m) ($m.secret_inventory | Where-Object id -eq 'pairing_root_secret').database_value = 'plaintext secret' }
    Invoke-ManifestCase 'rollback anchor key omitted' 'P0-STATE-KEYS-001' { param($m) ($m.secret_inventory | Where-Object id -eq 'rollback_commit_anchor').material = 'generation only' }
    Invoke-ManifestCase 'rollback anchor database value introduced' 'P0-STATE-KEYS-001' { param($m) ($m.secret_inventory | Where-Object id -eq 'rollback_commit_anchor').database_value = 'matching generation in SQLite' }
    Invoke-ManifestCase 'protected root environment fallback' 'P0-STATE-KEYS-001' { param($m) $m.protected_blob_store.root = 'LOCALAPPDATA environment variable' }
    Invoke-ManifestCase 'protected directory broad ACL' 'P0-STATE-KEYS-001' { param($m) $m.protected_blob_store.directory_security = 'inherited ACL' }
    Invoke-ManifestCase 'protected path follows links' 'P0-STATE-KEYS-001' { param($m) $m.protected_blob_store.path_safety = 'follow links' }
    Invoke-ManifestCase 'predictable protected temporary name' 'P0-STATE-KEYS-001' { param($m) $m.protected_blob_store.temporary_name = 'state.tmp' }
    Invoke-ManifestCase 'protected write order swapped' 'P0-STATE-KEYS-001' { param($m) $v=$m.protected_blob_store.write_steps[0];$m.protected_blob_store.write_steps[0]=$m.protected_blob_store.write_steps[1];$m.protected_blob_store.write_steps[1]=$v }
    Invoke-ManifestCase 'protected startup promotes temporary' 'P0-STATE-KEYS-001' { param($m) $m.protected_blob_store.startup_recovery[1] = 'promote temporary file' }
    Invoke-ManifestCase 'protected recursive uninstall' 'P0-STATE-KEYS-001' { param($m) $m.protected_blob_store.disconnect_uninstall = 'recursively delete account directory' }

    Invoke-ManifestCase 'WAL disabled' 'P0-STATE-TRANSACTION-001' { param($m) $m.database_contract.journal_mode = 'OFF' }
    Invoke-ManifestCase 'NORMAL synchronous' 'P0-STATE-TRANSACTION-001' { param($m) $m.database_contract.synchronous = 'NORMAL' }
    Invoke-ManifestCase 'extensions enabled' 'P0-STATE-TRANSACTION-001' { param($m) $m.database_contract.extension_loading = 'allowed' }
    Invoke-ManifestCase 'raw live copy' 'P0-STATE-TRANSACTION-001' { param($m) $m.database_contract.live_copy = 'allowed' }
    Invoke-ManifestCase 'early cursor advance' 'P0-STATE-TRANSACTION-001' { param($m) $m.transaction_contract.page_commit = 'checkpoint first' }
    Invoke-ManifestCase 'operation after request' 'P0-STATE-TRANSACTION-001' { param($m) $m.transaction_contract.operation_commit = 'request before pending key' }
    Invoke-ManifestCase 'blind ambiguous retry' 'P0-STATE-TRANSACTION-001' { param($m) $m.transaction_contract.operation_commit = 'retry ambiguous outcome' }
    Invoke-ManifestCase 'dangling quarantine' 'P0-STATE-TRANSACTION-001' { param($m) $m.transaction_contract.quarantine_commit = 'leave quarantine_ref' }
    Invoke-ManifestCase 'recovery mutation' 'P0-STATE-TRANSACTION-001' { param($m) $m.transaction_contract.recovery_mutation = 'allowed' }
    Invoke-ManifestCase 'transaction order swapped' 'P0-STATE-TRANSACTION-001' { param($m) $v=$m.transaction_contract.record_write_order[7];$m.transaction_contract.record_write_order[7]=$m.transaction_contract.record_write_order[8];$m.transaction_contract.record_write_order[8]=$v }
    Invoke-ManifestCase 'transaction anchor publication removed' 'P0-STATE-TRANSACTION-001' { param($m) $m.transaction_contract.record_write_order = @($m.transaction_contract.record_write_order | Where-Object { $_ -notmatch 'atomically replace state-root' }) }
    Invoke-ManifestCase 'multi-record reservation undercount' 'P0-STATE-ENVELOPE-001' { param($m) $m.transaction_contract.encryption_reservation_order[1] = 'reserve one invocation for the transaction' }
    Invoke-ManifestCase 'RNG reservation rollback allowed' 'P0-STATE-ENVELOPE-001' { param($m) $m.transaction_contract.encryption_reservation_order[2] = 'reserve and roll back usage after RNG failure' }
    Invoke-ManifestCase 'publication order bypassed' 'P0-STATE-TRANSACTION-001' { param($m) $m.transaction_contract.post_encryption_publication_order = @($m.transaction_contract.post_encryption_publication_order | Where-Object { $_ -notmatch 'atomically replace state-root' }) }

    Invoke-ManifestCase 'unknown schema writes' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.compatibility.unknown_or_newer_schema = 'write' }
    Invoke-ManifestCase 'downgrade write' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.compatibility.old_binary_newer_store = 'convert and write' }
    Invoke-ManifestCase 'mixed generations writable' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.compatibility.mixed_writable_generations = 'allowed' }
    Invoke-ManifestCase 'old validation removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps = @($m.migration_rotation.migration_steps | Where-Object { $_ -notmatch 'exact old schema' }) }
    Invoke-ManifestCase 'new validation removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps = @($m.migration_rotation.migration_steps | Where-Object { $_ -notmatch 'complete new logical record' }) }
    Invoke-ManifestCase 'migration reservation removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps = @($m.migration_rotation.migration_steps | Where-Object { $_ -notmatch 'exact number of migration encryption attempts' }) }
    Invoke-ManifestCase 'migration nonce collision check removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps = @($m.migration_rotation.migration_steps | Where-Object { $_ -notmatch 'retained-generation collision-check' }) }
    Invoke-ManifestCase 'active WAL checkpoint removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps = @($m.migration_rotation.migration_steps | Where-Object { $_ -notmatch 'checkpoint the active WAL' }) }
    Invoke-ManifestCase 'staging WAL checkpoint removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps = @($m.migration_rotation.migration_steps | Where-Object { $_ -notmatch 'commit staging with synchronous FULL' }) }
    Invoke-ManifestCase 'migration generic copy swap' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps[13] = 'copy staging over active database' }
    Invoke-ManifestCase 'migration steps reordered' 'P0-STATE-MIGRATION-001' { param($m) $v=$m.migration_rotation.migration_steps[10];$m.migration_rotation.migration_steps[10]=$m.migration_rotation.migration_steps[11];$m.migration_rotation.migration_steps[11]=$v }
    Invoke-ManifestCase 'proposed commitment before staging' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.migration_steps[1] = 'write old and proposed commitments before creating staging' }
    Invoke-ManifestCase 'migration phase cleared before rollback deletion' 'P0-STATE-MIGRATION-001' { param($m) $v=$m.migration_rotation.migration_steps[16];$m.migration_rotation.migration_steps[16]=$m.migration_rotation.migration_steps[17];$m.migration_rotation.migration_steps[17]=$v }
    Invoke-ManifestCase 'migration promotes unverified staging' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.interrupted_migration_recovery = 'promote staging file' }
    Invoke-ManifestCase 'rotation batch reservation removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.key_rotation_steps = @($m.migration_rotation.key_rotation_steps | Where-Object { $_ -notmatch 'exact batch attempt count' }) }
    Invoke-ManifestCase 'rotation repeats encryption protocol' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.key_rotation_steps[5] = 'run transaction_contract.record_write_order again' }
    Invoke-ManifestCase 'rotation recovery removed' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.interrupted_key_rotation_recovery = 'restart from first record and reuse reservations' }
    Invoke-ManifestCase 'partial migration writable' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.crash_or_disk_full = 'continue partial store' }
    Invoke-ManifestCase 'old key retired early' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.key_rotation_steps = @($m.migration_rotation.key_rotation_steps | Where-Object { $_ -notmatch 'retain the old key' }) }
    Invoke-ManifestCase 'silent algorithm swap' 'P0-STATE-MIGRATION-001' { param($m) $m.migration_rotation.algorithm_compromise = 'substitute automatically' }

    Invoke-ManifestCase 'anchor algorithm changed' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.anchor_format.algorithm_id = 'SHA-256' }
    Invoke-ManifestCase 'anchor excludes ciphertext rows' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.anchor_format.input_encoding = 'generation only' }
    Invoke-ManifestCase 'anchor key stored in database' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.anchor_format.storage = 'SQLite plaintext' }
    Invoke-ManifestCase 'anchor protocol reordered' 'P0-STATE-ROLLBACK-001' { param($m) $v=$m.rollback_recovery.anchor_protocol[1];$m.rollback_recovery.anchor_protocol[1]=$m.rollback_recovery.anchor_protocol[2];$m.rollback_recovery.anchor_protocol[2]=$v }
    Invoke-ManifestCase 'anchor write before DB' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.anchor_protocol[2] = 'replace the protected anchor before every database commit' }
    Invoke-ManifestCase 'missing SQLite sidecar treated as failure' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.anchor_protocol[3] = 'treat missing WAL or SHM as recovery' }
    Invoke-ManifestCase 'startup reconciliation removed' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.anchor_protocol = @($m.rollback_recovery.anchor_protocol | Where-Object { $_ -notmatch 'every process start' }) }
    Invoke-ManifestCase 'DB rollback accepted' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.db_only_rollback = 'continue' }
    Invoke-ManifestCase 'profile restore trusted' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.roaming_or_full_profile_restore = 'trusted' }
    Invoke-ManifestCase 'VM rollback impossible claim' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.vm_restore = 'cryptographically impossible' }
    Invoke-ManifestCase 'clock used as nonce' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.clock_rollback = 'use wall clock for nonce' }
    Invoke-ManifestCase 'tamper reset' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.tamper_or_corruption = 'delete silently' }
    Invoke-ManifestCase 'recovery mutations enabled' 'P0-STATE-ROLLBACK-001' { param($m) $m.rollback_recovery.unknown_state = 'continue mutations' }

    Invoke-ManifestCase 'Graph-mutating disconnect' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.graph_mutation_on_delete_reset_disconnect_uninstall = 'allowed' }
    Invoke-ManifestCase 'state export enabled' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.product_state_export = 'enabled' }
    Invoke-ManifestCase 'metric export enabled' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.metric_export = 'enabled' }
    Invoke-ManifestCase 'recovery key export' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.recovery_key_export = 'allowed' }
    Invoke-ManifestCase 'plaintext backup' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.plaintext_backup = 'allowed' }
    Invoke-ManifestCase 'quarantine health retained' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.quarantine_resolution = 'retain job_health' }
    Invoke-ManifestCase 'Microsoft artifact deletion' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.disconnect_steps = @($m.lifecycle.disconnect_steps | ForEach-Object { $_ -replace 'leave Microsoft artifacts unchanged','delete Microsoft artifacts' }) }
    Invoke-ManifestCase 'disconnect order swapped' 'P0-STATE-LIFECYCLE-001' { param($m) $v=$m.lifecycle.disconnect_steps[0];$m.lifecycle.disconnect_steps[0]=$m.lifecycle.disconnect_steps[1];$m.lifecycle.disconnect_steps[1]=$v }
    Invoke-ManifestCase 'physical erasure claim' 'P0-STATE-LIFECYCLE-001' { param($m) $m.lifecycle.physical_erasure = 'guaranteed' }

    Invoke-ManifestCase 'diagnostic event added' 'P0-STATE-PRIVACY-001' { param($m) $m.privacy_artifacts.diagnostic_allowlist = @('free_text') }
    Invoke-ManifestCase 'WAL scan omitted' 'P0-STATE-PRIVACY-001' { param($m) $m.privacy_artifacts.canary_scan_surfaces = @($m.privacy_artifacts.canary_scan_surfaces | Where-Object { $_ -ne 'WAL' }) }
    Invoke-ManifestCase 'prompt allowed' 'P0-STATE-PRIVACY-001' { param($m) $m.privacy_artifacts.forbidden_classes = @($m.privacy_artifacts.forbidden_classes | Where-Object { $_ -notmatch '^prompt ' }) }
    Invoke-ManifestCase 'support bundle enabled' 'P0-STATE-PRIVACY-001' { param($m) $m.privacy_artifacts.support_bundle = 'enabled' }
    Invoke-ManifestCase 'automatic upload enabled' 'P0-STATE-PRIVACY-001' { param($m) $m.privacy_artifacts.automatic_upload = 'allowed' }
    Invoke-ManifestCase 'canary printed' 'P0-STATE-PRIVACY-001' { param($m) $m.privacy_artifacts.canary_output = 'print suspected value' }
    Invoke-ManifestCase 'generated artifacts tracked' 'P0-STATE-PRIVACY-001' { param($m) $m.privacy_artifacts.tracked_generated_artifacts = 'allowed' }

    Invoke-ManifestCase 'database claim' 'P0-STATE-CLAIMS-001' { param($m) $m.claims.database_files_created = @('state.db') }
    Invoke-ManifestCase 'secret claim' 'P0-STATE-CLAIMS-001' { param($m) $m.claims.secrets_accepted_or_persisted = @('secret') }
    Invoke-ManifestCase 'crypto operation claim' 'P0-STATE-CLAIMS-001' { param($m) $m.claims.cryptographic_operations = @('encrypt') }
    Invoke-ManifestCase 'dependency activated' 'P0-STATE-CLAIMS-001' { param($m) $m.dependency_policy.activation = 'activated' }
    Invoke-ManifestCase 'gate passed claim' 'P0-STATE-CLAIMS-001' { param($m) $m.claims.gates_passed = @('G-STATE') }
    Invoke-ManifestCase 'capability enabled claim' 'P0-STATE-CLAIMS-001' { param($m) $m.claims.capabilities_enabled = @('encrypted_local_state') }
    Invoke-ManifestCase 'acceptance completed claim' 'P0-STATE-CLAIMS-001' { param($m) $m.claims.acceptance_criteria_completed = @('AC-23') }
    Invoke-ManifestCase 'support advertised claim' 'P0-STATE-CLAIMS-001' { param($m) $m.claims.supported_rows = @('win') }

    Invoke-ContractCase 'privacy schema approved' 'P0-STATE-SCHEMA-001' 'contracts\privacy\persistence-boundary.json' 'PrivacyPath' { param($r) $r.Replace('"logical_runtime_schema_approved": false','"logical_runtime_schema_approved": true') }
    Invoke-ContractCase 'privacy envelope field drift' 'P0-STATE-CROSS-CONTRACT-001' 'contracts\privacy\persistence-boundary.json' 'PrivacyPath' { param($r) $r.Replace('"ciphertext"','"ciphertext", "debug_text"') }
    Invoke-ContractCase 'privacy keyed digest inventory drift' 'P0-STATE-CROSS-CONTRACT-001' 'contracts\privacy\persistence-boundary.json' 'PrivacyPath' { param($r) $r.Replace('"query_fingerprint_hmac"','"query_fingerprint_hmac", "extra_digest_hmac"') }
    Invoke-ContractCase 'ADR-005 planned again' 'P0-STATE-CROSS-CONTRACT-001' 'contracts\governance\capabilities.json' 'GovernancePath' { param($r) $r.Replace('{ "id": "ADR-005", "status": "accepted" }','{ "id": "ADR-005", "status": "planned" }') }
    Invoke-ContractCase 'state capability enabled' 'P0-STATE-CLAIMS-001' 'contracts\governance\capabilities.json' 'GovernancePath' { param($r) $r -replace '("id": "encrypted_local_state",\s+"state": )"disabled"','$1"enabled"' }
    Invoke-ContractCase 'auth plaintext fallback' 'P0-STATE-CROSS-CONTRACT-001' 'contracts\identity\authentication-boundary.json' 'AuthenticationPath' { param($r) $r.Replace('session_only_or_fail_closed_no_plaintext_fallback','plaintext_fallback') }
    Invoke-ContractCase 'permission requested' 'P0-STATE-CROSS-CONTRACT-001' 'contracts\identity\permission-boundary.json' 'PermissionPath' { param($r) $r.Replace('"permissions_requested": []','"permissions_requested": ["Mail.Read"]') }
    Invoke-ContractCase 'sync durable state enabled' 'P0-STATE-CROSS-CONTRACT-001' 'contracts\synchronization\mail-sync-boundary.json' 'SynchronizationPath' { param($r) $r.Replace('"durable_state": false','"durable_state": true') }
    Invoke-ContractCase 'support row enabled' 'P0-STATE-CLAIMS-001' 'contracts\support\support-matrix.json' 'SupportPath' { param($r) $r.Replace('"supported_rows": []','"supported_rows": ["row"]') }
    Invoke-ContractCase 'build state enabled' 'P0-STATE-CLAIMS-001' 'contracts\build-skeleton\skeleton.json' 'BuildPath' { param($r) $r.Replace('"durable_state": false','"durable_state": true') }
    Invoke-ContractCase 'trace ID omitted' 'P0-STATE-INVENTORY-001' 'docs\prd-traceability.md' 'TraceabilityPath' { param($r) $r.Replace('P0-STATE-FRESH-CHECKER-001','P0-STATE-FRESH-CHECKER-X') }
    Invoke-ContractCase 'ADR runtime claim' 'P0-STATE-CROSS-CONTRACT-001' 'docs\adr\ADR-005-persistence-and-cryptography.md' 'AdrPath' { param($r) $r.Replace('No gate, capability, support row, or acceptance criterion advances.','G-STATE passes.') }
    Invoke-ContractCase 'threat row omitted' 'P0-STATE-CROSS-CONTRACT-001' 'docs\threat-model\protected-local-state.md' 'ThreatPath' { param($r) $r.Replace('TM-STATE-012','TM-STATE-X') }
    Invoke-ExactWorkspaceCase 'dependency alias outside the sanctioned persistence crate' 'P0-STATE-CLAIMS-001' { param($root) $path=Join-Path $root 'crates\openloops-graph\Cargo.toml';$raw=Get-Content -Raw $path;$raw=$raw.Replace('[dependencies]',"[dependencies]`ndatabase_adapter = { package = `"rusqlite`", version = `"0.0.0`" }");Set-Content -LiteralPath $path -Value $raw -Encoding utf8NoBOM -NoNewline }
    Invoke-ExactWorkspaceCase 'selected dependency locked at an unpinned version' 'P0-STATE-CLAIMS-001' { param($root) Add-Content -LiteralPath (Join-Path $root 'Cargo.lock') -Encoding utf8NoBOM -Value "`n[[package]]`nname = `"rusqlite`"`nversion = `"0.0.0`"" }
    Invoke-ExactWorkspaceCase 'grouped Rust filesystem alias' 'P0-STATE-CLAIMS-001' { param($root) Add-Content -LiteralPath (Join-Path $root 'crates\openloops-graph\src\lib.rs') -Encoding utf8NoBOM -Value 'use std::{fs as durable}; pub fn persist(){ let _ = durable::write("synthetic", b"x"); }' }
    Invoke-ExactWorkspaceCase 'included non-rs persistence source' 'P0-STATE-CLAIMS-001' { param($root) $source=Join-Path $root 'crates\openloops-graph\src\lib.rs';Add-Content -LiteralPath $source -Encoding utf8NoBOM -Value 'include!("payload.inc");';Set-Content -LiteralPath (Join-Path $root 'crates\openloops-graph\src\payload.inc') -Encoding utf8NoBOM -Value 'pub fn hidden(){ let _ = std::fs::write("synthetic", b"x"); }' }
    Invoke-ExactWorkspaceCase 'alternate cryptography dependency' 'P0-STATE-CLAIMS-001' { param($root) $path=Join-Path $root 'crates\openloops-persistence\Cargo.toml';$raw=Get-Content -Raw $path;$raw=$raw.Replace('[dependencies]',"[dependencies]`ncrypto_engine = { package = `"ring`", version = `"0.0.0`" }");Set-Content -LiteralPath $path -Value $raw -Encoding utf8NoBOM -NoNewline }
    Invoke-ExactWorkspaceCase 'workspace Cargo configuration added' 'P0-STATE-CLAIMS-001' { param($root) $dir=Join-Path $root '.cargo';[void](New-Item -ItemType Directory -Path $dir -Force);Set-Content -LiteralPath (Join-Path $dir 'config.toml') -Encoding utf8NoBOM -Value '[build]' }
    Invoke-ExactWorkspaceAcceptedCase 'CRLF-normalized workspace fingerprint' { param($root) $path=Join-Path $root 'crates\openloops-domain\src\lib.rs';$raw=Get-Content -Raw $path;$normalized=$raw.Replace("`r`n","`n").Replace("`r","`n");Set-Content -LiteralPath $path -Encoding utf8NoBOM -NoNewline -Value $normalized.Replace("`n","`r`n") }

    if ($failed.Count -gt 0) {
        [Console]::Error.WriteLine("BLOCKED: protected-state negative cases failed: $($failed -join '; ')")
        exit 1
    }
    Write-Host "OpenLoops protected-state negative tests passed ($passed adversarial mutations)."
} finally {
    $resolvedTemp = [IO.Path]::GetFullPath($tempRoot)
    $systemTemp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
    if ($resolvedTemp.StartsWith($systemTemp, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolvedTemp).StartsWith('openloops-p0-state-', [StringComparison]::Ordinal)) {
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force -ErrorAction SilentlyContinue
    }
}
