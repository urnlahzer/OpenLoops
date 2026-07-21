$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$source = Join-Path $repoRoot 'contracts\privacy\persistence-boundary.json'
$checker = Join-Path $PSScriptRoot 'check-privacy-boundary.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('openloops-privacy-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tempRoot)
$passed = 0

function Invoke-Check([string]$Path) {
    & pwsh -NoProfile -File $checker -ManifestPath $Path *> $null
    return $LASTEXITCODE
}
function Expect-Rejected([string]$Name, [scriptblock]$Mutation) {
    $path = Join-Path $tempRoot ($Name + '.json')
    $manifest = Get-Content $source -Raw | ConvertFrom-Json
    & $Mutation $manifest
    $manifest | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $path -Encoding utf8NoBOM
    if ((Invoke-Check $path) -eq 0) { throw "Negative privacy case was accepted: $Name" }
    $script:passed++
}
function Expect-RawRejected([string]$Name, [scriptblock]$Mutation) {
    $path = Join-Path $tempRoot ($Name + '.json')
    $raw = Get-Content $source -Raw
    $changed = & $Mutation $raw
    Set-Content -LiteralPath $path -Value $changed -Encoding utf8NoBOM
    if ((Invoke-Check $path) -eq 0) { throw "Negative privacy case was accepted: $Name" }
    $script:passed++
}

try {
    if ((Invoke-Check $source) -ne 0) { throw 'Baseline privacy manifest failed.' }

    Expect-Rejected 'unknown-top-key' { param($m) $m | Add-Member extra_policy 'blocked' }
    Expect-Rejected 'unknown-claim-key' { param($m) $m.claim_state | Add-Member future_claim $false }
    Expect-Rejected 'unknown-record-key' { param($m) $m.record_types[0] | Add-Member extension @{} }
    Expect-Rejected 'missing-record' { param($m) $m.record_types = @($m.record_types | Select-Object -Skip 1) }
    Expect-Rejected 'unknown-record' { param($m) $m.record_types += [pscustomobject]@{ id='extension'; field_groups=@() } }
    Expect-Rejected 'duplicate-record' { param($m) $m.record_types += $m.record_types[0] }
    Expect-Rejected 'missing-field' { param($m) $m.record_types[7].field_groups[0].fields = @($m.record_types[7].field_groups[0].fields | Select-Object -Skip 1) }
    Expect-Rejected 'unknown-field' { param($m) $m.record_types[7].field_groups[0].fields += 'metadata' }
    Expect-Rejected 'duplicate-field' { param($m) $m.record_types[7].field_groups[0].fields += $m.record_types[7].field_groups[0].fields[0] }
    Expect-Rejected 'generic-string-type' { param($m) $m.record_types[7].field_groups[0].semantic_type = 'string' }
    Expect-Rejected 'generic-map-type' { param($m) $m.semantic_types += [pscustomobject]@{ id='map'; rule='arbitrary' } }
    Expect-Rejected 'missing-field-classification' { param($m) $m.record_types[7].field_groups[0].PSObject.Properties.Remove('classification') }
    Expect-Rejected 'classification-drift' { param($m) $m.record_types[7].field_groups[0].classification = 'public' }
    Expect-Rejected 'purpose-drift' { param($m) $m.record_types[7].field_groups[0].purpose = 'future_use' }
    Expect-Rejected 'missing-ciphertext' { param($m) $m.record_types[0].field_groups[-1].fields = @() }
    Expect-Rejected 'unapproved-retention' { param($m) $m.record_types[7].field_groups[0].retention = 'forever' }
    Expect-Rejected 'retention-boundary-drift' { param($m) ($m.retention_policies | Where-Object id -eq 'completed_operation_90').rule = 'completed operation entries for 91 days' }
    Expect-Rejected 'terminal-retention-unreachable' { param($m) ($m.lifecycle_transitions | Where-Object id -eq 'loop_lineage_retention').states = @($m.lifecycle_transitions[0].states | Select-Object -First 1) }
    Expect-Rejected 'completed-operation-retention-unreachable' { param($m) ($m.lifecycle_transitions | Where-Object id -eq 'operation_retention').states[1].retention = 'unresolved_operation' }
    Expect-Rejected 'quarantine-retention-missing' { param($m) $m.lifecycle_transitions = @($m.lifecycle_transitions | Where-Object id -ne 'quarantine_retention') }
    Expect-Rejected 'quarantine-retention-unbounded' { param($m) ($m.retention_policies | Where-Object id -eq 'unresolved_quarantine').rule = 'retain quarantine forever' }
    Expect-Rejected 'quarantine-resolved-not-returned' { param($m) ($m.lifecycle_transitions | Where-Object id -eq 'quarantine_retention').states[1].retention = 'unresolved_quarantine' }
    Expect-Rejected 'quarantine-disconnect-retained' { param($m) ($m.retention_policies | Where-Object id -eq 'unresolved_quarantine').rule = 'disconnect returns to observation retention' }
    Expect-Rejected 'quarantine-health-reference-dangles' { param($m) ($m.cross_record_invariants | Where-Object id -eq 'PRIV-XREC-013').rule = 'resolved health may retain quarantine_ref' }
    Expect-Rejected 'resolved-health-retained' { param($m) ($m.retention_policies | Where-Object id -eq 'job_health_until_resolved').rule = 'resolved health retained for 30 days' }
    Expect-Rejected 'quarantine-disconnect-graph-mutation' { param($m) ($m.cross_record_invariants | Where-Object id -eq 'PRIV-XREC-013').rule = 'disconnect deletes Graph message' }
    Expect-Rejected 'export-enabled' { param($m) $m.record_types[7].field_groups[0].export = 'export_allowed' }
    Expect-Rejected 'diagnostic-event' { param($m) $m.diagnostic_events += [pscustomobject]@{ id='error' } }
    Expect-Rejected 'forbidden-class-removed' { param($m) $m.forbidden_classes = @($m.forbidden_classes | Where-Object { $_ -ne 'prompt' }) }
    Expect-Rejected 'raw-id-field' { param($m) $m.record_types[7].field_groups[0].fields += 'raw_graph_id' }
    Expect-Rejected 'mail-text-field' { param($m) $m.record_types[4].field_groups[0].fields += 'mail_body' }
    Expect-Rejected 'unknown-source-variant' { param($m) $m.source_variants += [pscustomobject]@{ id='file'; record_type='file' } }
    Expect-Rejected 'source-component-drift' { param($m) $m.source_variants[0].component_codes += 'full_message' }
    Expect-Rejected 'calendar-gate-drift' { param($m) $m.source_variants[1].availability = 'enabled' }
    Expect-Rejected 'manual-ownership-drift' { param($m) $m.source_variants[2].ownership_code = 'system_authoritative' }
    Expect-Rejected 'manual-location-drift' { param($m) $m.source_variants[2].authoritative_readable_source_location = 'local_copy' }
    Expect-Rejected 'manual-field-drift' { param($m) $m.source_variants[2].authoritative_fields = @('title','body') }
    Expect-Rejected 'missing-cross-record-invariant' { param($m) $m.cross_record_invariants = @($m.cross_record_invariants | Select-Object -Skip 1) }
    Expect-Rejected 'capability-overclaim' { param($m) $m.claim_state.capability_enabled = $true }
    Expect-Rejected 'runtime-schema-overclaim' { param($m) $m.claim_state.logical_runtime_schema_approved = $true }
    Expect-Rejected 'gate-overclaim' { param($m) $m.claim_state.gates_passed = @('G-PRIV') }
    Expect-Rejected 'acceptance-overclaim' { param($m) $m.acceptance_criteria = @('AC-23') }
    Expect-Rejected 'owner-drift' { param($m) $m.owner_decisions = @('OWN-06') }
    Expect-Rejected 'adr-status-drift' { param($m) $m.adr.status = 'planned' }
    Expect-RawRejected 'duplicate-json-key' { param($raw) $raw -replace '"schema_version": 1,', '"schema_version": 1, "schema_version": 1,' }
    Expect-RawRejected 'malformed-json' { param($raw) $raw.Substring(0, $raw.Length - 2) }

    Write-Host "OpenLoops privacy-boundary negative suite passed ($passed synthetic rejection cases)."
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
}
