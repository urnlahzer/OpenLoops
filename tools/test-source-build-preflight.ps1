$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$sourceBuild = Join-Path $PSScriptRoot 'source-build.ps1'
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('openloops-preflight-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $tempRoot)
$originalPath = $env:Path
$passed = 0
$suiteMutex = [Threading.Mutex]::new($false, 'Local\OpenLoops-P0-WI04-SourceBuildPreflight')
$suiteMutexTaken = $false

function Write-Tool([string]$Name, [string]$Body) {
    [IO.File]::WriteAllText((Join-Path $tempRoot ($Name + '.cmd')), "@echo off`r`n$Body`r`n")
}
function Configure-ExactTools {
    Write-Tool 'node' 'echo v24.18.0'
    Write-Tool 'npm' 'echo 11.16.0'
    Write-Tool 'cargo' 'echo cargo 1.97.1 ^(c980f4866 2026-06-30^)'
    Write-Tool 'rustc' "if `%1==`-vV goto verbose`r`necho rustc 1.97.1 ^(8bab26f4f 2026-07-14^)`r`nexit /b 0`r`n:verbose`r`necho host: x86_64-pc-windows-msvc"
}
function Expect-PreflightRejected([string]$Name, [scriptblock]$Mutation, [string]$Expected) {
    Configure-ExactTools
    & $Mutation
    $env:Path = "$tempRoot;$originalPath"
    $output = (& pwsh -NoProfile -File $sourceBuild 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0 -or $output -notmatch [regex]::Escape($Expected)) { throw "Source-build preflight did not reject $Name exactly." }
    $script:passed++
}

try {
    try {
        $suiteMutexTaken = $suiteMutex.WaitOne([TimeSpan]::FromMinutes(5))
    } catch [Threading.AbandonedMutexException] {
        $suiteMutexTaken = $true
    }
    if (-not $suiteMutexTaken) { throw 'Timed out waiting for the source-build preflight suite lock.' }

    Expect-PreflightRejected 'node-suffix' { Write-Tool 'node' 'echo v24.18.0-extra' } 'node does not match'
    Expect-PreflightRejected 'npm-suffix' { Write-Tool 'npm' 'echo 11.16.0-extra' } 'npm does not match'
    Expect-PreflightRejected 'rustc-suffix' { Write-Tool 'rustc' 'echo rustc 1.97.1 ^(8bab26f4f 2026-07-14^)-extra' } 'rustc does not match'
    Expect-PreflightRejected 'cargo-suffix' { Write-Tool 'cargo' 'echo cargo 1.97.1 ^(c980f4866 2026-06-30^)-extra' } 'cargo does not match'
    $env:NPM_CONFIG_REGISTRY = 'https://example.invalid/'
    Expect-PreflightRejected 'ambient-npm-config' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:NPM_CONFIG_REGISTRY
    $env:RUSTC_WRAPPER = 'synthetic-wrapper-placeholder'
    Expect-PreflightRejected 'rustc-wrapper' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:RUSTC_WRAPPER
    $env:RUSTC = 'synthetic-compiler-placeholder'
    Expect-PreflightRejected 'rustc-override' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:RUSTC
    $env:CARGO_REGISTRIES_CRATES_IO_TOKEN = 'synthetic-token-placeholder'
    Expect-PreflightRejected 'cargo-registry-token' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:CARGO_REGISTRIES_CRATES_IO_TOKEN
    $env:NODE_OPTIONS = '--require=synthetic-placeholder'
    Expect-PreflightRejected 'node-options' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:NODE_OPTIONS
    $env:NODE_TLS_REJECT_UNAUTHORIZED = '0'
    Expect-PreflightRejected 'node-tls-bypass' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:NODE_TLS_REJECT_UNAUTHORIZED
    $env:SSL_CERT_FILE = 'synthetic-ca-placeholder'
    Expect-PreflightRejected 'ssl-ca-override' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:SSL_CERT_FILE
    $env:NO_PROXY = '*'
    Expect-PreflightRejected 'proxy-bypass' {} 'ambient package/build configuration is prohibited'
    Remove-Item Env:NO_PROXY

    $projectNpmConfig = Join-Path $repoRoot '.npmrc'
    if (Test-Path -LiteralPath $projectNpmConfig) { throw 'Cannot run synthetic .npmrc case while a local .npmrc already exists.' }
    [IO.File]::WriteAllText($projectNpmConfig, "registry=https://example.invalid/`n")
    try { Expect-PreflightRejected 'project-npm-config' {} 'repository-local npm configuration is prohibited' }
    finally { [IO.File]::Delete($projectNpmConfig) }

    $cargoDirectory = Join-Path $repoRoot '.cargo'
    $cargoConfig = Join-Path $cargoDirectory 'config.toml'
    $createdCargoDirectory = -not (Test-Path -LiteralPath $cargoDirectory)
    if (Test-Path -LiteralPath $cargoConfig) { throw 'Cannot run synthetic Cargo-config case while a local config already exists.' }
    [void](New-Item -ItemType Directory -Path $cargoDirectory -Force)
    [IO.File]::WriteAllText($cargoConfig, "[source.crates-io]`nreplace-with='synthetic'`n")
    try { Expect-PreflightRejected 'cargo-source-replacement' {} 'ancestor Cargo configuration is prohibited' }
    finally {
        [IO.File]::Delete($cargoConfig)
        if ($createdCargoDirectory) { [IO.Directory]::Delete($cargoDirectory, $false) }
    }
    Write-Host "OpenLoops source-build preflight negative suite passed ($passed synthetic rejection cases)."
} finally {
    $env:Path = $originalPath
    Remove-Item Env:NPM_CONFIG_REGISTRY -ErrorAction SilentlyContinue
    Remove-Item Env:RUSTC_WRAPPER -ErrorAction SilentlyContinue
    Remove-Item Env:RUSTC -ErrorAction SilentlyContinue
    Remove-Item Env:CARGO_REGISTRIES_CRATES_IO_TOKEN -ErrorAction SilentlyContinue
    Remove-Item Env:NODE_OPTIONS -ErrorAction SilentlyContinue
    Remove-Item Env:NODE_TLS_REJECT_UNAUTHORIZED -ErrorAction SilentlyContinue
    Remove-Item Env:SSL_CERT_FILE -ErrorAction SilentlyContinue
    Remove-Item Env:NO_PROXY -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $tempRoot) { Remove-Item -LiteralPath $tempRoot -Recurse -Force }
    if ($suiteMutexTaken) { $suiteMutex.ReleaseMutex() }
    $suiteMutex.Dispose()
}
