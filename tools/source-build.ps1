[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

function Require-ExactOutput([string]$Command, [string[]]$Arguments, [string]$Expected) {
    $resolved = Get-Command $Command -ErrorAction SilentlyContinue
    if (-not $resolved) { throw "BLOCKED: required public build prerequisite is unavailable: $Command" }
    $actual = [string](& $resolved.Source @Arguments 2>$null | Select-Object -First 1)
    if ($actual.Trim() -cne $Expected) {
        throw "BLOCKED: $Command does not match the repository-pinned version."
    }
}

Require-ExactOutput 'node' @('--version') 'v24.18.0'
Require-ExactOutput 'npm' @('--version') '11.16.0'
Require-ExactOutput 'rustc' @('--version') 'rustc 1.97.1 (8bab26f4f 2026-07-14)'
Require-ExactOutput 'cargo' @('--version') 'cargo 1.97.1 (c980f4866 2026-06-30)'
$rustHost = (& rustc -vV 2>$null | Where-Object { $_ -like 'host:*' } | Select-Object -First 1)
if ([string]$rustHost -cne 'host: x86_64-pc-windows-msvc') { throw 'BLOCKED: rustc host does not match the pinned Windows MSVC target.' }

$ambientConfigNames = @([Environment]::GetEnvironmentVariables().Keys | ForEach-Object { [string]$_ } | Where-Object {
    $_ -match '^(?i:NPM_CONFIG_|CARGO_|PKG_CONFIG|OPENSSL|DEP_)' -or
    $_ -match '^(?i:HTTP_PROXY|HTTPS_PROXY|ALL_PROXY|NO_PROXY|SSL_CERT_FILE|SSL_CERT_DIR|NODE_OPTIONS|NODE_PATH|NODE_EXTRA_CA_CERTS|NODE_TLS_REJECT_UNAUTHORIZED|RUSTC|RUSTDOC|RUSTFLAGS|RUSTDOCFLAGS|RUSTC_WRAPPER|RUSTC_WORKSPACE_WRAPPER|RUSTUP_TOOLCHAIN|CC|CXX|AR|CL|_CL_|LINK|_LINK_|CFLAGS|CXXFLAGS|LDFLAGS|INCLUDE|LIB|LIBPATH|WINDOWSSDKDIR|VCTOOLSINSTALLDIR|VSCMD_.+)$'
})
if ($ambientConfigNames.Count) {
    throw ('BLOCKED: ambient package/build configuration is prohibited: ' + (($ambientConfigNames | Sort-Object) -join ', '))
}
if (Test-Path -LiteralPath (Join-Path $repoRoot '.npmrc')) { throw 'BLOCKED: repository-local npm configuration is prohibited.' }
$ancestor = [IO.DirectoryInfo]::new($repoRoot)
while ($ancestor) {
    foreach ($relative in '.cargo\config','.cargo\config.toml','.cargo\credentials','.cargo\credentials.toml') {
        if (Test-Path -LiteralPath (Join-Path $ancestor.FullName $relative)) { throw 'BLOCKED: repository or ancestor Cargo configuration is prohibited.' }
    }
    $ancestor = $ancestor.Parent
}

$isolationRoot = Join-Path ([IO.Path]::GetTempPath()) ('openloops-build-' + [guid]::NewGuid().ToString('N'))
$npmConfigRoot = Join-Path $isolationRoot 'npm'
$cargoHome = Join-Path $isolationRoot 'cargo-home'
[void](New-Item -ItemType Directory -Path $npmConfigRoot -Force)
[void](New-Item -ItemType Directory -Path $cargoHome -Force)
$userConfig = Join-Path $npmConfigRoot 'user.npmrc'
$globalConfig = Join-Path $npmConfigRoot 'global.npmrc'
[IO.File]::WriteAllText($userConfig, "registry=https://registry.npmjs.org/`n")
[IO.File]::WriteAllText($globalConfig, "")
$env:NPM_CONFIG_USERCONFIG = $userConfig
$env:NPM_CONFIG_GLOBALCONFIG = $globalConfig
$env:NPM_CONFIG_REGISTRY = 'https://registry.npmjs.org/'
$env:CARGO_HOME = $cargoHome

$locationPushed = $false
try {
    $vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
    $vsInstallPath = if (Test-Path -LiteralPath $vswhere) {
        (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
    }
    if ([string]::IsNullOrWhiteSpace($vsInstallPath)) {
        throw 'BLOCKED: Microsoft C++ Build Tools are required for the pinned Windows MSVC target.'
    }
    $devShellModule = Join-Path $vsInstallPath 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll'
    if (-not (Test-Path -LiteralPath $devShellModule)) { throw 'BLOCKED: Visual Studio developer shell module is unavailable.' }
    Import-Module $devShellModule
    Enter-VsDevShell -VsInstallPath $vsInstallPath -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64' | Out-Null

    Push-Location $repoRoot
    $locationPushed = $true
    npm ci --ignore-scripts --strict-peer-deps --no-audit --no-fund
    if ($LASTEXITCODE) { throw 'npm ci failed.' }
    npm run test:addin
    if ($LASTEXITCODE) { throw 'TypeScript synthetic tests failed.' }
    npm run format:check
    if ($LASTEXITCODE) { throw 'TypeScript formatting failed.' }
    npm run lint
    if ($LASTEXITCODE) { throw 'TypeScript lint failed.' }
    cargo fmt --all -- --check
    if ($LASTEXITCODE) { throw 'Rust formatting failed.' }
    cargo clippy --workspace --all-targets --locked -- -D warnings
    if ($LASTEXITCODE) { throw 'Rust lint failed.' }
    cargo test --workspace --locked --jobs 1
    if ($LASTEXITCODE) { throw 'Rust tests failed.' }
    $smoke = cargo run --quiet --locked -p openloops-desktop
    if ($LASTEXITCODE -or $smoke -ne 'OPENLOOPS_SYNTHETIC_SMOKE_OK') { throw 'Synthetic desktop smoke failed.' }
    pwsh -NoProfile -File ./tools/check-package-allowlist.ps1
    if ($LASTEXITCODE) { throw 'Package allowlist failed.' }
    Write-Host 'OpenLoops source-build skeleton passed (synthetic, disabled, credential-free).'
} finally {
    if ($locationPushed) { Pop-Location }
    Remove-Item Env:NPM_CONFIG_USERCONFIG -ErrorAction SilentlyContinue
    Remove-Item Env:NPM_CONFIG_GLOBALCONFIG -ErrorAction SilentlyContinue
    Remove-Item Env:NPM_CONFIG_REGISTRY -ErrorAction SilentlyContinue
    Remove-Item Env:CARGO_HOME -ErrorAction SilentlyContinue
    if (Test-Path -LiteralPath $isolationRoot) { [IO.Directory]::Delete($isolationRoot, $true) }
}
