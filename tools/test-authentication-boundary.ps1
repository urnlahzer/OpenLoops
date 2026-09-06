[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) { throw 'Run this script inside the repository.' }

$checker = Join-Path $repoRoot 'tools/check-authentication-boundary.ps1'
$manifestPath = Join-Path $repoRoot 'contracts/identity/authentication-boundary.json'
$baseline = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = [IO.Path]::GetFullPath((Join-Path $tempBase ('openloops-auth-' + [guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe synthetic temp path.' }
[void](New-Item -ItemType Directory -Path $tempRoot)
$caseNumber = 0

function Invoke-Case([string]$Name, [string]$ExpectedId, [scriptblock]$Mutate) {
    $script:caseNumber++
    $case = $baseline | ConvertTo-Json -Depth 100 | ConvertFrom-Json
    & $Mutate $case
    $casePath = Join-Path $tempRoot ("case-$($script:caseNumber).json")
    $case | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $casePath -Encoding utf8NoBOM
    $output = (& pwsh -NoProfile -File $checker -ManifestPath $casePath -Quiet 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0 -or $output -notmatch [regex]::Escape($ExpectedId)) {
        throw "Synthetic authentication case did not fail closed at $ExpectedId`: $Name"
    }
}

try {
    & pwsh -NoProfile -File $checker -Quiet
    if ($LASTEXITCODE -ne 0) { throw 'Valid authentication boundary failed.' }

    Invoke-Case 'unknown top-level field' 'P0-AUTH-INVENTORY-001' { param($c) $c | Add-Member note 'synthetic' }
    Invoke-Case 'wrong work item' 'P0-AUTH-INVENTORY-001' { param($c) $c.work_item = 'P0-WI-99' }
    Invoke-Case 'removed owner' 'P0-AUTH-INVENTORY-001' { param($c) $c.owner_decisions = @('OWN-00') }
    Invoke-Case 'embedded browser' 'P0-AUTH-FLOW-001' { param($c) $c.flow.authorization_surface = 'embedded_webview' }
    Invoke-Case 'plain PKCE' 'P0-AUTH-FLOW-001' { param($c) $c.flow.pkce_method = 'plain' }
    Invoke-Case 'confidential client material' 'P0-AUTH-FLOW-001' { param($c) $c.flow.confidential_client_material = 'accepted' }
    Invoke-Case 'device code fallback removed from prohibition' 'P0-AUTH-FLOW-001' { param($c) $c.prohibited_flows = @($c.prohibited_flows | Where-Object { $_ -ne 'device_code_fallback' }) }
    Invoke-Case 'fixed unvalidated redirect host' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.host_selection = 'localhost' }
    Invoke-Case 'LAN callback' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.lan_binding = 'allowed' }
    Invoke-Case 'wildcard redirect' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.wildcard_redirect = 'allowed' }
    Invoke-Case 'fragment response enabled' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.fragment_response = 'allowed' }
    Invoke-Case 'form post enabled' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.form_post = 'allowed' }
    Invoke-Case 'duplicate callback parameter accepted' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.duplicate_parameters = 'accept' }
    Invoke-Case 'mixed callback result accepted' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.mixed_success_error = 'accept' }
    Invoke-Case 'unknown callback parameter exposed' 'P0-AUTH-REDIRECT-001' { param($c) $c.redirect_contract.unexpected_parameters = 'return_to_caller' }
    Invoke-Case 'multiple pending transactions' 'P0-AUTH-CONCURRENCY-001' { param($c) $c.transaction_contract.pending_transactions = 2 }
    Invoke-Case 'replay accepted' 'P0-AUTH-CONCURRENCY-001' { param($c) $c.transaction_contract.replayed_callback = 'accept' }
    Invoke-Case 'same-process transaction replaces incumbent' 'P0-AUTH-CONCURRENCY-001' { param($c) $c.transaction_contract.second_same_process_attempt = 'replace' }
    Invoke-Case 'personal account enabled' 'P0-AUTH-ACCOUNT-001' { param($c) $c.account_contract.personal_accounts = 'enabled' }
    Invoke-Case 'multiple accounts' 'P0-AUTH-ACCOUNT-001' { param($c) $c.account_contract.accounts_per_os_user = 2 }
    Invoke-Case 'plaintext token persistence' 'P0-AUTH-TOKEN-001' { param($c) $c.token_boundary.persistence_status = 'plaintext_file' }
    Invoke-Case 'diagnostic token event' 'P0-AUTH-TOKEN-001' { param($c) $c.token_boundary.diagnostic_allowlist = @('synthetic_token_event') }
    Invoke-Case 'protocol URL permits return data' 'P0-AUTH-TOKEN-001' { param($c) $c.token_boundary.protocol_url_policy = 'allow arbitrary return URL' }
    Invoke-Case 'global logout claim' 'P0-AUTH-DISCONNECT-001' { param($c) $c.disconnect_contract.global_logout_claim = $true }
    Invoke-Case 'cache cleanup failure permits reconnect' 'P0-AUTH-DISCONNECT-001' { param($c) $c.disconnect_contract.cache_delete_failure = 'continue' }
    Invoke-Case 'floating dependency version' 'P0-AUTH-DEPENDENCIES-001' { param($c) $c.dependency_decisions[0].version = '*' }
    Invoke-Case 'oauth2 reqwest integration removed' 'P0-AUTH-DEPENDENCIES-001' { param($c) $c.dependency_decisions[0].features = @('timing-resistant-secret-traits') }
    Invoke-Case 'dependency default features enabled' 'P0-AUTH-DEPENDENCIES-001' { param($c) $c.dependency_decisions[1].default_features = $true }
    Invoke-Case 'dependency license drift' 'P0-AUTH-DEPENDENCIES-001' { param($c) $c.dependency_decisions[2].license = 'unknown' }
    Invoke-Case 'dependency feature drift' 'P0-AUTH-DEPENDENCIES-001' { param($c) $c.dependency_decisions[3].features = @('full') }
    Invoke-Case 'dependency activated early' 'P0-AUTH-DEPENDENCIES-001' { param($c) $c.dependency_policy.activation = 'active' }
    Invoke-Case 'scope requirement absorbed from ADR-003' 'P0-AUTH-INVENTORY-001' { param($c) $c.requirements += 'OL-AUTH-003' }
    Invoke-Case 'permission requested' 'P0-AUTH-CLAIMS-001' { param($c) $c.claims.permissions_requested = @('User.Read') }
    Invoke-Case 'gate claimed passed' 'P0-AUTH-CLAIMS-001' { param($c) $c.claims.gates_passed = @('G-ID') }
    Invoke-Case 'capability advertised' 'P0-AUTH-CLAIMS-001' { param($c) $c.claims.capabilities_advertised = @('work_school_core') }

    Write-Host "OpenLoops authentication-boundary negative suite passed ($caseNumber synthetic rejection cases)."
} finally {
    if ([IO.Directory]::Exists($tempRoot)) { [IO.Directory]::Delete($tempRoot, $true) }
}
