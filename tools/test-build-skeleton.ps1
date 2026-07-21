$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$source = Join-Path $repoRoot 'contracts\build-skeleton\skeleton.json'
$checker = Join-Path $PSScriptRoot 'check-build-skeleton.ps1'
$tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('openloops-skeleton-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tempRoot)
$passed = 0

function Expected([string]$Name) {
    if ($Name -match 'toolchain|node-|typescript-|source-') { return 'P0-SKELETON-TOOLCHAIN-001' }
    if ($Name -match 'layer|dependency') { return 'P0-SKELETON-DIRECTION-001' }
    if ($Name -match 'contract|schema|generated') { return 'P0-SKELETON-CONTRACT-001' }
    if ($Name -match 'graph|oauth|provider|state|manifest|origin|secret|stdout') { return 'P0-SKELETON-PRIVACY-001' }
    if ($Name -match 'lock|map|debug|npm-|bootstrap') { return 'P0-SKELETON-PACKAGE-001' }
    if ($Name -match 'acceptance|gate-|capability|advertised|work-item') { return 'P0-SKELETON-CLAIMS-001' }
    throw "No expected requirement ID for $Name"
}
function Reject([string]$Name, [scriptblock]$Mutation) {
    $path = Join-Path $tempRoot ($Name + '.json')
    $manifest = Get-Content $source -Raw | ConvertFrom-Json
    & $Mutation $manifest
    $manifest | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $path -Encoding utf8NoBOM
    $output = (& pwsh -NoProfile -File $checker -ManifestPath $path 2>&1 | Out-String)
    $expected = Expected $Name
    if ($LASTEXITCODE -eq 0) { throw "Negative skeleton case was accepted: $Name" }
    if ($output -notmatch [regex]::Escape($expected)) { throw "Negative skeleton case failed for wrong reason: $Name (expected $expected)" }
    $script:passed++
}
function Reject-Schema([string]$Name, [scriptblock]$Mutation) {
    $path = Join-Path $tempRoot ($Name + '.schema.json')
    $schema = Get-Content (Join-Path $repoRoot 'contracts\ipc\skeleton-status.schema.json') -Raw | ConvertFrom-Json
    & $Mutation $schema
    $schema | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $path -Encoding utf8NoBOM
    $output = (& pwsh -NoProfile -File $checker -SchemaPathOverride $path 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0) { throw "Negative skeleton schema case was accepted: $Name" }
    if ($output -notmatch 'P0-SKELETON-CONTRACT-001') { throw "Negative skeleton schema case failed for wrong reason: $Name" }
    $script:passed++
}

try {
    & pwsh -NoProfile -File $checker
    if ($LASTEXITCODE) { throw 'Baseline build skeleton failed.' }
    Reject 'toolchain-rust-drift' { param($m) $m.toolchains.rust.channel = 'stable' }
    Reject 'node-version-drift' { param($m) $m.toolchains.node.version = '26.5.0' }
    Reject 'typescript-version-drift' { param($m) $m.toolchains.typescript.version = 'latest' }
    Reject 'source-inventory-removed' { param($m) $m.sources = @($m.sources | Select-Object -Skip 1) }
    Reject 'layer-removed' { param($m) $m.workspace.PSObject.Properties.Remove('domain') }
    Reject 'dependency-reversed' { param($m) $m.workspace.domain.dependencies = @('openloops-graph') }
    Reject 'contract-schema-swapped' { param($m) $m.contract_generation.schema = 'contracts/privacy/persistence-boundary.json' }
    Reject 'generated-output-tracked' { param($m) $m.contract_generation.generated_outputs_tracked = $true }
    Reject 'graph-enabled' { param($m) $m.runtime_boundary.graph_transport = $true }
    Reject 'oauth-enabled' { param($m) $m.runtime_boundary.oauth = $true }
    Reject 'provider-enabled' { param($m) $m.runtime_boundary.model_provider = $true }
    Reject 'state-enabled' { param($m) $m.runtime_boundary.durable_state = $true }
    Reject 'manifest-enabled' { param($m) $m.runtime_boundary.addin_manifest = $true }
    Reject 'origin-added' { param($m) $m.runtime_boundary.network_origins = @('https://example.invalid') }
    Reject 'secret-slot-added' { param($m) $m.runtime_boundary.accepted_secrets = @('token') }
    Reject 'stdout-content-added' { param($m) $m.runtime_boundary.synthetic_stdout = 'mailbox text' }
    Reject 'lock-disabled' { param($m) $m.build_policy.locked_dependencies = $false }
    Reject 'map-enabled' { param($m) $m.build_policy.source_maps = $true }
    Reject 'debug-enabled' { param($m) $m.build_policy.debug_symbols_in_release = $true }
    Reject 'npm-public' { param($m) $m.build_policy.npm_private = $false }
    Reject 'npm-files-added' { param($m) $m.build_policy.npm_files = @('dist') }
    Reject 'npm-metadata-allowlist-drift' { param($m) $m.build_policy.npm_dry_run_metadata_files = @('package.json') }
    Reject 'bootstrap-installs' { param($m) $m.build_policy.bootstrap_installs_software = $true }
    Reject 'acceptance-added' { param($m) $m.acceptance_criteria = @('AC-01') }
    Reject 'gate-passed' { param($m) $m.gates_passed = @('G-RELEASE') }
    Reject 'capability-enabled' { param($m) $m.capabilities_enabled = @('outlook_addin') }
    Reject 'advertised-added' { param($m) $m.capabilities_advertised = @('desktop') }
    Reject 'work-item-drift' { param($m) $m.work_item = 'P1-WI-01' }
    Reject-Schema 'schema-array-type-drift' { param($s) $s.properties.enabled_capabilities.type = 'string' }
    Reject-Schema 'schema-required-removed' { param($s) $s.required = @($s.required | Where-Object { $_ -ne 'gates_passed' }) }
    Reject-Schema 'schema-property-added' { param($s) $s.properties | Add-Member mailbox_text @{ type='string' } }
    Write-Host "OpenLoops build-skeleton negative suite passed ($passed synthetic rejection cases)."
} finally {
    if (Test-Path -LiteralPath $tempRoot) { Remove-Item -LiteralPath $tempRoot -Recurse -Force }
}
