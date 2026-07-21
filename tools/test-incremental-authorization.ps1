[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) { throw 'Run this script inside the repository.' }
$checker = Join-Path $repoRoot 'tools/check-incremental-authorization.ps1'
$baseline = Get-Content -Raw -LiteralPath (Join-Path $repoRoot 'contracts/identity/permission-boundary.json') | ConvertFrom-Json
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = [IO.Path]::GetFullPath((Join-Path $tempBase ('openloops-authz-' + [guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase,[StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe temp path.' }
[void](New-Item -ItemType Directory -Path $tempRoot); $caseNumber = 0
function Invoke-Case([string]$Name,[string]$Expected,[scriptblock]$Mutate) {
    $script:caseNumber++; $case = $baseline | ConvertTo-Json -Depth 100 | ConvertFrom-Json; & $Mutate $case
    $path = Join-Path $tempRoot ("case-$($script:caseNumber).json"); $case | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $path -Encoding utf8NoBOM
    $output = (& pwsh -NoProfile -File $checker -ManifestPath $path -Quiet 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0 -or $output -notmatch [regex]::Escape($Expected)) { throw "Synthetic authorization case failed to trigger $Expected`: $Name" }
}
function Invoke-DocumentCase([string]$Name,[string]$Expected,[string]$SourcePath,[string]$CheckerArgument,[scriptblock]$Mutate) {
    $script:caseNumber++; $text = Get-Content -Raw -LiteralPath (Join-Path $repoRoot $SourcePath); $text = & $Mutate $text
    $path = Join-Path $tempRoot ("case-$($script:caseNumber)-document" + [IO.Path]::GetExtension($SourcePath)); Set-Content -LiteralPath $path -Value $text -Encoding utf8NoBOM
    $output = (& pwsh -NoProfile -File $checker $CheckerArgument $path -Quiet 2>&1 | Out-String)
    if ($LASTEXITCODE -eq 0 -or $output -notmatch [regex]::Escape($Expected)) { throw "Synthetic authorization case failed to trigger $Expected`: $Name" }
}
try {
    & pwsh -NoProfile -File $checker -Quiet; if ($LASTEXITCODE) { throw 'Valid permission boundary failed.' }
    Invoke-Case 'unknown field' 'P0-AUTHZ-INVENTORY-001' { param($c) $c | Add-Member extra 'synthetic' }
    Invoke-Case 'wrong work item' 'P0-AUTHZ-INVENTORY-001' { param($c) $c.work_item='P0-WI-99' }
    Invoke-Case 'owner drift' 'P0-AUTHZ-INVENTORY-001' { param($c) $c.owner_decisions=@('OWN-01') }
    Invoke-Case 'referenced owner mapping drift' 'P0-AUTHZ-CROSS-CONTRACT-001' { param($c) $c.referenced_owner_decisions[0].governing_adrs=@('ADR-013') }
    Invoke-Case 'requirement omitted' 'P0-AUTHZ-INVENTORY-001' { param($c) $c.requirements=@($c.requirements|Select-Object -Skip 1) }
    Invoke-Case 'duplicate row' 'P0-AUTHZ-INVENTORY-001' { param($c) $c.permission_rows+=@($c.permission_rows[0]) }
    Invoke-Case 'row enabled' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $c.permission_rows[0].state='enabled' }
    Invoke-Case 'row advertised' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $c.permission_rows[1].advertised=$true }
    Invoke-Case 'sign in scope guessed' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $c.permission_rows[0].permission='openid' }
    Invoke-Case 'Mail.ReadBasic detection' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $c.permission_rows[2].permission='Mail.ReadBasic' }
    Invoke-Case 'mail mutation scope' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $c.permission_rows[2].permission='Mail.ReadWrite' }
    Invoke-Case 'validation-only mail mutation row removed' 'P0-AUTHZ-INVENTORY-001' { param($c) $c.permission_rows=@($c.permission_rows|Where-Object id -ne 'PERM-MAIL-MUTATION-DIAG-001') }
    Invoke-Case 'mail mutation diagnostic becomes product feature' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $r=@($c.permission_rows|Where-Object id -eq 'PERM-MAIL-MUTATION-DIAG-001')[0]; $r.kind='graph_delegated_feature' }
    Invoke-Case 'Tasks.Read production' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $r=@($c.permission_rows|Where-Object id -eq 'PERM-TODO-WRITE-001')[0]; $r.permission='Tasks.Read' }
    Invoke-Case 'calendar guessed' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $r=@($c.permission_rows|Where-Object id -eq 'PERM-CALENDAR-001')[0]; $r.permission='Calendars.ReadWrite' }
    Invoke-Case 'self mail bundled' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $c.permission_rows[0].permission='Mail.Send' }
    Invoke-Case 'missing row gate' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $r=@($c.permission_rows|Where-Object id -eq 'PERM-SELFMAIL-001')[0]; $r.gates=@() }
    Invoke-Case 'To Do diagnostic missing identity gate' 'P0-AUTHZ-FEATURE-SCOPE-001' { param($c) $r=@($c.permission_rows|Where-Object id -eq 'PERM-TODO-DIAG-001')[0]; $r.gates=@('G-TODO','G-PRIV') }
    Invoke-Case 'initial combined grant' 'P0-AUTHZ-INCREMENTAL-001' { param($c) $c.incremental_consent_policy.initial_combined_grant='allowed' }
    Invoke-Case 'grant enables feature' 'P0-AUTHZ-INCREMENTAL-001' { param($c) $c.incremental_consent_policy.returned_scope_superset='enable all grants' }
    Invoke-Case 'missing scope still calls API' 'P0-AUTHZ-INCREMENTAL-001' { param($c) $c.incremental_consent_policy.missing_scope='continue' }
    Invoke-Case 'denial broadens' 'P0-AUTHZ-INCREMENTAL-001' { param($c) $c.incremental_consent_policy.denial='request broader scope' }
    Invoke-Case 'folder defaults drift' 'P0-AUTHZ-FOLDERS-001' { param($c) $c.mail_folder_policy.default_folders=@('Inbox') }
    Invoke-Case 'all folders' 'P0-AUTHZ-FOLDERS-001' { param($c) $c.mail_folder_policy.automatic_expansion='all folders' }
    Invoke-Case 'shared folder' 'P0-AUTHZ-FOLDERS-001' { param($c) $c.mail_folder_policy.shared_or_delegated_folders='allowed' }
    Invoke-Case 'unbounded history' 'P0-AUTHZ-FOLDERS-001' { param($c) $c.mail_folder_policy.history_window_days.maximum=9999 }
    Invoke-Case 'folder OAuth misrepresentation' 'P0-AUTHZ-FOLDERS-001' { param($c) $c.mail_folder_policy.authorization_disclosure='Mail.Read is folder limited' }
    Invoke-Case 'folder checkpoint retained after removal' 'P0-AUTHZ-FOLDERS-001' { param($c) $c.mail_folder_policy.folder_removal='stop processing but retain checkpoint' }
    Invoke-Case 'Office permission guessed' 'P0-AUTHZ-OFFICE-MANIFEST-001' { param($c) $c.addin_manifest_boundary.production_permission_status='ReadItem approved' }
    Invoke-Case 'Office requirement guessed' 'P0-AUTHZ-OFFICE-MANIFEST-001' { param($c) $c.addin_manifest_boundary.production_requirement_set_status='Mailbox 1.13 approved' }
    Invoke-Case 'Office Graph equivalence' 'P0-AUTHZ-OFFICE-MANIFEST-001' { param($c) $c.addin_manifest_boundary.graph_scope_equivalence=$true }
    Invoke-Case 'add-in support claim' 'P0-AUTHZ-OFFICE-MANIFEST-001' { param($c) $c.addin_manifest_boundary.support_claim=$true }
    Invoke-Case 'permission request claim' 'P0-AUTHZ-CLAIMS-001' { param($c) $c.claims.permissions_requested=@('Mail.Read') }
    Invoke-Case 'network contact claim' 'P0-AUTHZ-CLAIMS-001' { param($c) $c.claims.network_contacts=@('synthetic-service') }
    Invoke-Case 'gate pass claim' 'P0-AUTHZ-CLAIMS-001' { param($c) $c.claims.gates_passed=@('G-ID') }
    Invoke-Case 'capability enabled claim' 'P0-AUTHZ-CLAIMS-001' { param($c) $c.claims.capabilities_enabled=@('work_school_core') }
    Invoke-Case 'support claim' 'P0-AUTHZ-CLAIMS-001' { param($c) $c.claims.supported_rows=@('synthetic-row') }
    Invoke-DocumentCase 'privacy checkpoint retention drift' 'P0-AUTHZ-CROSS-CONTRACT-001' 'contracts/privacy/persistence-boundary.json' '-PrivacyPath' { param($text) $text -replace '"reset": "scope_removal_or_disconnect"','"reset": "disconnect_only"' }
    Invoke-DocumentCase 'ADR check inventory drift' 'P0-AUTHZ-CROSS-CONTRACT-001' 'docs/adr/ADR-003-incremental-authorization.md' '-AdrPath' { param($text) $text -replace 'exactly these eight checks','an unspecified check set' }
    Invoke-DocumentCase 'ADR validation-only Mail.ReadWrite language removed' 'P0-AUTHZ-CROSS-CONTRACT-001' 'docs/adr/ADR-003-incremental-authorization.md' '-AdrPath' { param($text) $text -replace '`Mail.ReadWrite` is permitted only for the disposable-tenant','`Mail.ReadWrite` has an unspecified status for the disposable-tenant' }
    Invoke-DocumentCase 'calendar reminder owner mapping drift' 'P0-AUTHZ-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' { param($text) $text -replace '"owner_decisions": \[\s*"OWN-04"\s*\]','"owner_decisions": ["OWN-03"]' }
    Invoke-DocumentCase 'calendar invitation owner mapping drift' 'P0-AUTHZ-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' { param($text) $text -replace '"owner_decisions": \[\s*"OWN-05"\s*\]','"owner_decisions": ["OWN-03"]' }
    Invoke-DocumentCase 'self-email owner mapping drift' 'P0-AUTHZ-CROSS-CONTRACT-001' 'contracts/governance/capabilities.json' '-GovernancePath' { param($text) $text -replace '"owner_decisions": \[\s*"OWN-10"\s*\]','"owner_decisions": ["OWN-03"]' }
    Write-Host "OpenLoops incremental-authorization negative suite passed ($caseNumber synthetic rejection cases)."
} finally { if ([IO.Directory]::Exists($tempRoot)) { [IO.Directory]::Delete($tempRoot,$true) } }
