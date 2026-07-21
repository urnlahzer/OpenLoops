[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (Test-Path -LiteralPath (Join-Path $repoRoot '.npmrc')) { throw 'BLOCKED: repository-local npm configuration is prohibited.' }
$environmentNames = @([Environment]::GetEnvironmentVariables().Keys | ForEach-Object { [string]$_ })
$proxyNames = @($environmentNames | Where-Object { $_ -in @('HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','NODE_EXTRA_CA_CERTS') })
if ($proxyNames.Count) { throw 'BLOCKED: ambient proxy configuration is prohibited.' }
foreach ($name in @($environmentNames | Where-Object { $_ -match '^(?i:NPM_CONFIG_)' })) {
    [Environment]::SetEnvironmentVariable($name, $null, 'Process')
}
$npmConfigRoot = Join-Path ([IO.Path]::GetTempPath()) ('openloops-pack-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $npmConfigRoot)
$userConfig = Join-Path $npmConfigRoot 'user.npmrc'
$globalConfig = Join-Path $npmConfigRoot 'global.npmrc'
[IO.File]::WriteAllText($userConfig, "registry=https://registry.npmjs.org/`n")
[IO.File]::WriteAllText($globalConfig, "")
$env:NPM_CONFIG_USERCONFIG = $userConfig
$env:NPM_CONFIG_GLOBALCONFIG = $globalConfig
$env:NPM_CONFIG_REGISTRY = 'https://registry.npmjs.org/'
Push-Location $repoRoot
try {
    $result = npm pack --dry-run --json --ignore-scripts | ConvertFrom-Json
    if ($LASTEXITCODE -or @($result).Count -ne 1) { throw 'BLOCKED: npm dry-run inspection failed.' }
    $paths = @($result[0].files.path)
    $expected = @('LICENSE', 'package.json')
    if ($paths.Count -ne $expected.Count -or (Compare-Object ($paths | Sort-Object) ($expected | Sort-Object))) {
        throw 'BLOCKED: npm dry-run contents differ from the closed metadata-only allowlist.'
    }
    if (@($paths | Where-Object { $_ -match '\.(map|log|har|trace|db|sqlite|tgz|zip)$|(^|/)(src|test|contracts|fixtures|target|node_modules)(/|$)' }).Count) {
        throw 'BLOCKED: npm dry-run contains a prohibited artifact class.'
    }
    Write-Host 'OpenLoops npm package allowlist passed (metadata only; publishing disabled).'
} finally {
    Pop-Location
    Remove-Item Env:NPM_CONFIG_USERCONFIG -ErrorAction SilentlyContinue
    Remove-Item Env:NPM_CONFIG_GLOBALCONFIG -ErrorAction SilentlyContinue
    Remove-Item Env:NPM_CONFIG_REGISTRY -ErrorAction SilentlyContinue
    [IO.Directory]::Delete($npmConfigRoot, $true)
}
