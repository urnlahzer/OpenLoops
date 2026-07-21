[CmdletBinding()]
param(
    [string]$ManifestPath = (Join-Path $PSScriptRoot '..\contracts\build-skeleton\skeleton.json'),
    [string]$SchemaPathOverride
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$failures = [System.Collections.Generic.HashSet[string]]::new()
function Fail([string]$Id) { [void]$failures.Add($Id) }
function Exact([object[]]$Actual, [object[]]$Expected, [string]$Id) {
    $a = @($Actual | ForEach-Object { [string]$_ })
    $e = @($Expected | ForEach-Object { [string]$_ })
    if ($a.Count -eq 0 -and $e.Count -eq 0) { return }
    if ($a.Count -eq 0 -or $e.Count -eq 0 -or $a.Count -ne $e.Count -or
        $a.Count -ne (@($a | Sort-Object -Unique)).Count -or
        $e.Count -ne (@($e | Sort-Object -Unique)).Count -or
        (Compare-Object ($a | Sort-Object) ($e | Sort-Object))) { Fail $Id }
}
function Test-JsonObject([System.Text.Json.JsonElement]$Element, [string]$Id) {
    if ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Object) {
        $names = @($Element.EnumerateObject() | ForEach-Object Name)
        if ($names.Count -ne (@($names | Sort-Object -Unique)).Count) { Fail $Id }
        foreach ($property in $Element.EnumerateObject()) { Test-JsonObject $property.Value $Id }
    } elseif ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Array) {
        foreach ($item in $Element.EnumerateArray()) { Test-JsonObject $item $Id }
    }
}
function Read-StrictJson([string]$Path, [string]$Id) {
    try {
        $raw = Get-Content -LiteralPath $Path -Raw
        $document = [System.Text.Json.JsonDocument]::Parse($raw)
        Test-JsonObject $document.RootElement $Id
        return $raw | ConvertFrom-Json
    } catch {
        Fail $Id
        return $null
    } finally {
        if ($document) { $document.Dispose() }
    }
}
function Text([string]$Relative) { Get-Content -LiteralPath (Join-Path $repoRoot $Relative) -Raw }

$manifest = Read-StrictJson $ManifestPath 'P0-SKELETON-CONTRACT-001'
if (-not $manifest) {
    [Console]::Error.WriteLine('BLOCKED build-skeleton checks: P0-SKELETON-CONTRACT-001')
    exit 1
}

$topKeys = 'schema_version','work_item','snapshot_date','requirements','acceptance_criteria','gates_passed','capabilities_enabled','capabilities_advertised','toolchains','workspace','contract_generation','runtime_boundary','build_policy','sources'
Exact @($manifest.PSObject.Properties.Name) $topKeys 'P0-SKELETON-CONTRACT-001'
if ($manifest.schema_version -ne 1 -or $manifest.work_item -ne 'P0-WI-04' -or $manifest.snapshot_date -ne '2026-07-19') { Fail 'P0-SKELETON-CLAIMS-001' }
Exact @($manifest.requirements) @('OL-NFR-010','OL-NFR-011','OL-NFR-012') 'P0-SKELETON-CLAIMS-001'
foreach ($claim in 'acceptance_criteria','gates_passed','capabilities_enabled','capabilities_advertised') { Exact @($manifest.$claim) @() 'P0-SKELETON-CLAIMS-001' }

Exact @($manifest.toolchains.PSObject.Properties.Name) @('rust','node','typescript','rust_contract_tooling') 'P0-SKELETON-TOOLCHAIN-001'
Exact @($manifest.toolchains.rust.PSObject.Properties.Name) @('channel','host','profile','components') 'P0-SKELETON-TOOLCHAIN-001'
Exact @($manifest.toolchains.node.PSObject.Properties.Name) @('version','lifecycle','npm') 'P0-SKELETON-TOOLCHAIN-001'
Exact @($manifest.toolchains.typescript.PSObject.Properties.Name) @('version','office_types','node_types') 'P0-SKELETON-TOOLCHAIN-001'
Exact @($manifest.toolchains.rust_contract_tooling.PSObject.Properties.Name) @('typify','serde','serde_json') 'P0-SKELETON-TOOLCHAIN-001'
foreach ($tool in 'typify','serde','serde_json') { Exact @($manifest.toolchains.rust_contract_tooling.$tool.PSObject.Properties.Name) @('version','license') 'P0-SKELETON-TOOLCHAIN-001' }
if ($manifest.toolchains.rust.channel -ne '1.97.1' -or $manifest.toolchains.rust.host -ne 'x86_64-pc-windows-msvc' -or
    $manifest.toolchains.rust.profile -ne 'minimal') { Fail 'P0-SKELETON-TOOLCHAIN-001' }
Exact @($manifest.toolchains.rust.components) @('clippy','rustfmt') 'P0-SKELETON-TOOLCHAIN-001'
if ($manifest.toolchains.node.version -ne '24.18.0' -or $manifest.toolchains.node.lifecycle -ne 'LTS' -or
    $manifest.toolchains.node.npm -ne '11.16.0' -or $manifest.toolchains.typescript.version -ne '6.0.2' -or
    $manifest.toolchains.typescript.office_types -ne '1.0.600' -or $manifest.toolchains.typescript.node_types -ne '24.13.3') { Fail 'P0-SKELETON-TOOLCHAIN-001' }
$rustToolchain = Text 'rust-toolchain.toml'
$nodeVersion = (Text '.node-version').Trim()
if ($rustToolchain -notmatch 'channel = "1\.97\.1"' -or $rustToolchain -notmatch 'components = \["clippy", "rustfmt"\]' -or
    $rustToolchain -notmatch 'targets = \["x86_64-pc-windows-msvc"\]' -or $nodeVersion -ne '24.18.0') { Fail 'P0-SKELETON-TOOLCHAIN-001' }
if ($manifest.toolchains.rust_contract_tooling.typify.version -ne '0.7.0' -or
    $manifest.toolchains.rust_contract_tooling.serde.version -ne '1.0.229' -or
    $manifest.toolchains.rust_contract_tooling.serde_json.version -ne '1.0.150') { Fail 'P0-SKELETON-TOOLCHAIN-001' }

$expectedLayers = @{
    domain = @{ package='openloops-domain'; dependencies=@() }
    contracts = @{ package='openloops-contracts'; dependencies=@() }
    application = @{ package='openloops-application'; dependencies=@('openloops-contracts','openloops-domain') }
    graph = @{ package='openloops-graph'; dependencies=@('openloops-application','openloops-domain') }
    inference = @{ package='openloops-inference'; dependencies=@('openloops-application','openloops-domain') }
    persistence = @{ package='openloops-persistence'; dependencies=@('openloops-application','openloops-domain') }
    desktop = @{ package='openloops-desktop'; dependencies=@('openloops-application','openloops-contracts','openloops-domain','openloops-graph','openloops-inference','openloops-persistence') }
}
Exact @($manifest.workspace.PSObject.Properties.Name) @($expectedLayers.Keys) 'P0-SKELETON-DIRECTION-001'
foreach ($layer in $expectedLayers.Keys) {
    $actual = $manifest.workspace.$layer
    Exact @($actual.PSObject.Properties.Name) @('package','dependencies') 'P0-SKELETON-DIRECTION-001'
    if ($actual.package -ne $expectedLayers[$layer].package) { Fail 'P0-SKELETON-DIRECTION-001' }
    Exact @($actual.dependencies) @($expectedLayers[$layer].dependencies) 'P0-SKELETON-DIRECTION-001'
    $cargoPath = "crates\$($actual.package)\Cargo.toml"
    if (-not (Test-Path (Join-Path $repoRoot $cargoPath))) { Fail 'P0-SKELETON-DIRECTION-001'; continue }
    $cargoText = Text $cargoPath
    foreach ($dependency in $actual.dependencies) {
        if ($cargoText -notmatch "(?m)^$([regex]::Escape($dependency)) = \{ path =") { Fail 'P0-SKELETON-DIRECTION-001' }
    }
    foreach ($other in @($expectedLayers.Values.package | Where-Object { $_ -notin $actual.dependencies -and $_ -ne $actual.package })) {
        if ($cargoText -match "(?m)^$([regex]::Escape($other)) =") { Fail 'P0-SKELETON-DIRECTION-001' }
    }
}
$cargoRoot = Text 'Cargo.toml'
if ($cargoRoot -notmatch 'resolver = "3"' -or $cargoRoot -notmatch 'publish = false' -or
    $cargoRoot -notmatch 'unsafe_code = "forbid"' -or $cargoRoot -notmatch 'debug = 0') { Fail 'P0-SKELETON-PACKAGE-001' }
$cargoLock = Text 'Cargo.lock'
if (([regex]::Matches($cargoLock, '(?m)^name = "openloops-')).Count -ne 7 -or
    $cargoLock -match '(?m)^source = "(?!registry\+https://github\.com/rust-lang/crates\.io-index")' -or
    $cargoLock -notmatch '(?s)name = "typify"\s+version = "0\.7\.0"' -or
    $cargoLock -notmatch '(?s)name = "serde"\s+version = "1\.0\.229"' -or
    $cargoLock -notmatch '(?s)name = "serde_json"\s+version = "1\.0\.150"') { Fail 'P0-SKELETON-DIRECTION-001' }

Exact @($manifest.contract_generation.PSObject.Properties.Name) @('schema','rust_output','typescript_output','generated_outputs_tracked') 'P0-SKELETON-CONTRACT-001'
if ($manifest.contract_generation.schema -ne 'contracts/ipc/skeleton-status.schema.json' -or
    $manifest.contract_generation.generated_outputs_tracked -ne $false) { Fail 'P0-SKELETON-CONTRACT-001' }
$schemaPath = if ($SchemaPathOverride) { $SchemaPathOverride } else { Join-Path $repoRoot $manifest.contract_generation.schema }
$schema = Read-StrictJson $schemaPath 'P0-SKELETON-CONTRACT-001'
if (-not $schema -or $schema.additionalProperties -ne $false) { Fail 'P0-SKELETON-CONTRACT-001' }
Exact @($schema.PSObject.Properties.Name) @('$schema','$id','title','type','additionalProperties','required','properties') 'P0-SKELETON-CONTRACT-001'
Exact @($schema.required) @('contract_version','companion_state','enabled_capabilities','gates_passed') 'P0-SKELETON-CONTRACT-001'
Exact @($schema.properties.PSObject.Properties.Name) @('contract_version','companion_state','enabled_capabilities','gates_passed') 'P0-SKELETON-CONTRACT-001'
if ($schema.properties.contract_version.const -ne 1 -or @($schema.properties.companion_state.enum).Count -ne 1 -or
    $schema.properties.companion_state.enum[0] -ne 'skeleton_disabled' -or $schema.properties.enabled_capabilities.maxItems -ne 0 -or
    $schema.properties.gates_passed.maxItems -ne 0) { Fail 'P0-SKELETON-CONTRACT-001' }
if ($schema.type -ne 'object' -or $schema.properties.enabled_capabilities.type -ne 'array' -or
    $schema.properties.gates_passed.type -ne 'array') { Fail 'P0-SKELETON-CONTRACT-001' }
if ((Text 'crates\openloops-contracts\src\lib.rs') -notmatch 'typify::import_types!' -or
    (Text 'crates\openloops-contracts\Cargo.toml') -notmatch 'typify = "=0\.7\.0"' -or
    (Text 'tools\generate-contracts.mjs') -notmatch 'outlook-addin/\.generated/skeleton-status\.ts') { Fail 'P0-SKELETON-CONTRACT-001' }
git check-ignore -q outlook-addin/.generated/skeleton-status.ts
if ($LASTEXITCODE -ne 0) { Fail 'P0-SKELETON-CONTRACT-001' }

$boundary = $manifest.runtime_boundary
Exact @($boundary.PSObject.Properties.Name) @('graph_transport','oauth','model_provider','durable_state','addin_manifest','network_origins','accepted_secrets','synthetic_stdout') 'P0-SKELETON-PRIVACY-001'
foreach ($flag in 'graph_transport','oauth','model_provider','durable_state','addin_manifest') { if ($boundary.$flag -ne $false) { Fail 'P0-SKELETON-PRIVACY-001' } }
Exact @($boundary.network_origins) @() 'P0-SKELETON-PRIVACY-001'
Exact @($boundary.accepted_secrets) @() 'P0-SKELETON-PRIVACY-001'
if ($boundary.synthetic_stdout -ne 'OPENLOOPS_SYNTHETIC_SMOKE_OK') { Fail 'P0-SKELETON-PRIVACY-001' }
foreach ($relative in 'crates\openloops-graph\src\lib.rs','crates\openloops-inference\src\lib.rs','crates\openloops-persistence\src\lib.rs') {
    if ((Text $relative) -notmatch 'const fn is_available\(\) -> bool \{\s*false') { Fail 'P0-SKELETON-PRIVACY-001' }
}
if (Test-Path (Join-Path $repoRoot 'outlook-addin\manifest.xml')) { Fail 'P0-SKELETON-PRIVACY-001' }

$policy = $manifest.build_policy
Exact @($policy.PSObject.Properties.Name) @('locked_dependencies','source_maps','debug_symbols_in_release','npm_private','npm_files','npm_dry_run_metadata_files','generated_output_tracked','bootstrap_installs_software','bootstrap_accepts_secret_arguments') 'P0-SKELETON-PACKAGE-001'
foreach ($flag in 'locked_dependencies','npm_private') { if ($policy.$flag -ne $true) { Fail 'P0-SKELETON-PACKAGE-001' } }
foreach ($flag in 'source_maps','debug_symbols_in_release','generated_output_tracked','bootstrap_installs_software','bootstrap_accepts_secret_arguments') { if ($policy.$flag -ne $false) { Fail 'P0-SKELETON-PACKAGE-001' } }
Exact @($policy.npm_files) @() 'P0-SKELETON-PACKAGE-001'
Exact @($policy.npm_dry_run_metadata_files) @('LICENSE','package.json') 'P0-SKELETON-PACKAGE-001'
$package = Read-StrictJson (Join-Path $repoRoot 'package.json') 'P0-SKELETON-PACKAGE-001'
$packageLockRaw = Text 'package-lock.json'
try {
    $packageLockDocument = [System.Text.Json.JsonDocument]::Parse($packageLockRaw)
    Test-JsonObject $packageLockDocument.RootElement 'P0-SKELETON-TOOLCHAIN-001'
    $packageLock = $packageLockRaw | ConvertFrom-Json -AsHashtable
} catch {
    Fail 'P0-SKELETON-TOOLCHAIN-001'
} finally {
    if ($packageLockDocument) { $packageLockDocument.Dispose() }
}
if (-not $package.private -or @($package.files).Count -ne 0 -or $package.packageManager -ne 'npm@11.16.0' -or $package.engines.node -ne '24.18.0' -or
    $package.engines.npm -ne '11.16.0' -or $package.devDependencies.typescript -ne '6.0.2' -or
    $package.devDependencies.'@types/office-js' -ne '1.0.600' -or $package.devDependencies.'@types/node' -ne '24.13.3') { Fail 'P0-SKELETON-TOOLCHAIN-001' }
if ($packageLock['lockfileVersion'] -ne 3 -or $packageLock['packages']['node_modules/typescript']['version'] -ne '6.0.2' -or
    $packageLock['packages']['node_modules/@types/office-js']['version'] -ne '1.0.600' -or
    $packageLock['packages']['node_modules/@types/node']['version'] -ne '24.13.3') { Fail 'P0-SKELETON-TOOLCHAIN-001' }
$tsconfig = Read-StrictJson (Join-Path $repoRoot 'outlook-addin\tsconfig.json') 'P0-SKELETON-PACKAGE-001'
foreach ($flag in 'sourceMap','inlineSourceMap','declarationMap') { if ($tsconfig.compilerOptions.$flag -ne $false) { Fail 'P0-SKELETON-PACKAGE-001' } }

$bootstrap = Text 'tools\source-build.ps1'
foreach ($required in 'npm ci --ignore-scripts --strict-peer-deps --no-audit --no-fund','repository-local npm configuration is prohibited','CARGO_HOME','ancestor Cargo configuration is prohibited','NPM_CONFIG_USERCONFIG','https://registry.npmjs.org/','Enter-VsDevShell','host: x86_64-pc-windows-msvc','npm run format:check','npm run lint','cargo fmt','cargo clippy','cargo test','OPENLOOPS_SYNTHETIC_SMOKE_OK','check-package-allowlist.ps1') {
    if ($bootstrap -notmatch [regex]::Escape($required)) { Fail 'P0-SKELETON-BUILD-001' }
}
if ($bootstrap -match 'Read-Host|Get-Credential|client.secret|access.token|provider.key') { Fail 'P0-SKELETON-PRIVACY-001' }

$sourceIds = 'SRC-RUST-RELEASE','SRC-NODE-RELEASES','SRC-OFFICE-JS','SRC-SPEC-BUILD','SRC-PLAN-PHASE-0'
Exact @($manifest.sources.id) $sourceIds 'P0-SKELETON-TOOLCHAIN-001'
foreach ($source in $manifest.sources) {
    Exact @($source.PSObject.Properties.Name) @('id','location') 'P0-SKELETON-TOOLCHAIN-001'
    if ($source.location -match '^https://') { continue }
    $relative = ($source.location -split '#')[0]
    if (-not (Test-Path (Join-Path $repoRoot $relative))) { Fail 'P0-SKELETON-TOOLCHAIN-001' }
}

$governance = Read-StrictJson (Join-Path $repoRoot 'contracts\governance\capabilities.json') 'P0-SKELETON-CLAIMS-001'
if (@($governance.product_acceptance_criteria_completed).Count -ne 0 -or
    @($governance.gates | Where-Object status -ne 'unrun').Count -ne 0 -or
    @($governance.capabilities | Where-Object { $_.state -ne 'disabled' -or $_.advertised }).Count -ne 0) { Fail 'P0-SKELETON-CLAIMS-001' }
$trace = Text 'docs\prd-traceability.md'
foreach ($id in 'P0-SKELETON-TOOLCHAIN-001','P0-SKELETON-DIRECTION-001','P0-SKELETON-CONTRACT-001','P0-SKELETON-PRIVACY-001','P0-SKELETON-BUILD-001','P0-SKELETON-PACKAGE-001','P0-SKELETON-CLAIMS-001','P0-SKELETON-FRESH-CHECKER-001') {
    if (([regex]::Matches($trace, [regex]::Escape($id))).Count -ne 1) { Fail 'P0-SKELETON-CLAIMS-001' }
}
if ((Text 'docs\source-build.md') -notmatch 'no product capability or gate is enabled') { Fail 'P0-SKELETON-CLAIMS-001' }

if ($failures.Count) {
    [Console]::Error.WriteLine(('BLOCKED build-skeleton checks: ' + (($failures | Sort-Object) -join ', ')))
    exit 1
}
Write-Host 'OpenLoops build-skeleton checks passed (8 P0-WI-04 assertions; zero capability or gate advanced).'
