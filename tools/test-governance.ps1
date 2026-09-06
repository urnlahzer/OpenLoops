[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) {
    throw 'Run this script from inside the OpenLoops Git repository.'
}

$checker = Join-Path $repoRoot 'tools/check-governance.ps1'
$registryPath = Join-Path $repoRoot 'contracts/governance/capabilities.json'
$researchDecisionPath = Join-Path $repoRoot 'research/microsoft-graph/product-decisions.md'
$threatIndexPath = Join-Path $repoRoot 'docs/threat-model/README.md'
$baseline = Get-Content -Raw -LiteralPath $registryPath | ConvertFrom-Json
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = [IO.Path]::GetFullPath((Join-Path $tempBase ('openloops-governance-' + [guid]::NewGuid().ToString('N'))))
if (-not $tempRoot.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Synthetic test directory did not resolve beneath the operating-system temporary directory.'
}

New-Item -ItemType Directory -Path $tempRoot | Out-Null
$caseNumber = 0

function Invoke-Case {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][scriptblock]$Mutate
    )

    $script:caseNumber++
    $case = $baseline | ConvertTo-Json -Depth 100 | ConvertFrom-Json
    & $Mutate $case
    $casePath = Join-Path $tempRoot ("case-$($script:caseNumber).json")
    $case | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $casePath -Encoding utf8NoBOM
    & pwsh -NoProfile -File $checker -RegistryPath $casePath -Quiet *> $null
    if ($LASTEXITCODE -eq 0) {
        throw "Synthetic governance case unexpectedly passed: $Name"
    }
}

function Invoke-DocumentCase {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$SourcePath,
        [Parameter(Mandatory)][ValidateSet('ResearchDecisionPath', 'ThreatIndexPath')][string]$CheckerParameter,
        [Parameter(Mandatory)][scriptblock]$MutateText
    )

    $script:caseNumber++
    $original = Get-Content -Raw -LiteralPath $SourcePath
    $mutated = & $MutateText $original
    if ($mutated -eq $original) { throw "Synthetic document mutation did not change input: $Name" }
    $casePath = Join-Path $tempRoot ("document-case-$($script:caseNumber).md")
    Set-Content -LiteralPath $casePath -Value $mutated -Encoding utf8NoBOM -NoNewline
    $arguments = @('-NoProfile', '-File', $checker, '-RegistryPath', $registryPath, "-$CheckerParameter", $casePath, '-Quiet')
    & pwsh @arguments *> $null
    if ($LASTEXITCODE -eq 0) {
        throw "Synthetic governance document case unexpectedly passed: $Name"
    }
}

try {
    & pwsh -NoProfile -File $checker -RegistryPath $registryPath -Quiet *> $null
    if ($LASTEXITCODE -ne 0) { throw 'The valid governance registry did not pass.' }

    Invoke-Case -Name 'missing owner decision' -Mutate {
        param($case)
        $case.owner_decisions = @($case.owner_decisions | Where-Object { $_.id -ne 'OWN-10' })
    }
    Invoke-Case -Name 'duplicate owner decision' -Mutate {
        param($case)
        $case.owner_decisions = @($case.owner_decisions) + @($case.owner_decisions[0])
    }
    Invoke-Case -Name 'lost security-conditioned owner disposition' -Mutate {
        param($case)
        $case.owner_decisions[0].status = 'accepted_with_security_condition'
        $case.owner_decisions[6].status = 'accepted'
    }
    Invoke-Case -Name 'missing required capability' -Mutate {
        param($case)
        $case.capabilities = @($case.capabilities | Where-Object { $_.id -ne 'personal_accounts' })
    }
    Invoke-Case -Name 'unknown capability' -Mutate {
        param($case)
        $case.capabilities[0].id = 'unknown_capability'
    }
    Invoke-Case -Name 'enabled unresolved capability' -Mutate {
        param($case)
        $case.capabilities[0].state = 'enabled'
        $case.capabilities[0].advertised = $true
    }
    Invoke-Case -Name 'unknown gate reference' -Mutate {
        param($case)
        $case.capabilities[0].gates = @('G-UNKNOWN')
    }
    Invoke-Case -Name 'duplicate capability gate' -Mutate {
        param($case)
        $case.capabilities[0].gates = @($case.capabilities[0].gates) + @($case.capabilities[0].gates[0])
    }
    Invoke-Case -Name 'missing failure fallback' -Mutate {
        param($case)
        $case.capabilities[0].fallback = ''
    }
    Invoke-Case -Name 'premature acceptance claim' -Mutate {
        param($case)
        $case.product_acceptance_criteria_completed = @('AC-23')
    }
    Invoke-Case -Name 'unearned gate pass' -Mutate {
        param($case)
        $case.gates[0].status = 'passed'
    }
    Invoke-Case -Name 'prohibited broad permission' -Mutate {
        param($case)
        $case.capabilities[0].permission_contracts = @('Mail.ReadWrite')
    }
    Invoke-Case -Name 'disguised unresolved broad permission' -Mutate {
        param($case)
        $case.capabilities[0].permission_contracts = @('unresolved_User.ReadWrite:G-ID')
    }
    Invoke-Case -Name 'owner crosswalk mapping drift' -Mutate {
        param($case)
        $case.owner_decisions[3].adrs = @('ADR-011')
    }
    Invoke-Case -Name 'unknown threat ADR' -Mutate {
        param($case)
        $case.threat_flows[0].adrs = @('ADR-UNKNOWN')
    }
    Invoke-Case -Name 'unknown threat gate' -Mutate {
        param($case)
        $case.threat_flows[0].gates = @('G-UNKNOWN')
    }
    Invoke-Case -Name 'known but wrong threat route' -Mutate {
        param($case)
        $case.threat_flows[0].adrs = @('ADR-013')
        $case.threat_flows[0].gates = @('G-SELFMAIL')
        $case.threat_flows[0].prohibited_behavior = 'placeholder'
        $case.threat_flows[0].fallback = 'continue anyway'
    }
    Invoke-Case -Name 'duplicate threat route' -Mutate {
        param($case)
        $case.threat_flows[0].adrs = @($case.threat_flows[0].adrs) + @($case.threat_flows[0].adrs[0])
    }
    Invoke-Case -Name 'missing threat owner' -Mutate {
        param($case)
        $case.threat_flows[0].owner_on_failure = ''
    }
    Invoke-Case -Name 'missing threat fallback' -Mutate {
        param($case)
        $case.threat_flows[0].fallback = ''
    }
    Invoke-DocumentCase -Name 'research crosswalk document drift' -SourcePath $researchDecisionPath -CheckerParameter ResearchDecisionPath -MutateText {
        param($text)
        $text.Replace('ADR-008, ADR-009, ADR-011 | G-AUTO, G-AUTO-FULL', 'ADR-011 | G-AUTO, G-AUTO-FULL')
    }
    Invoke-DocumentCase -Name 'threat routing document drift' -SourcePath $threatIndexPath -CheckerParameter ThreatIndexPath -MutateText {
        param($text)
        $text.Replace('ADR-002, ADR-003, ADR-005 | G-ID, G-PRIV', 'ADR-013 | G-SELFMAIL')
    }

    Write-Host "Synthetic governance tests passed ($caseNumber negative cases)."
} finally {
    if (Test-Path -LiteralPath $tempRoot) {
        $resolvedForDelete = [IO.Path]::GetFullPath($tempRoot)
        if ($resolvedForDelete.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
            [IO.Path]::GetFileName($resolvedForDelete).StartsWith('openloops-governance-', [StringComparison]::Ordinal)) {
            Remove-Item -LiteralPath $resolvedForDelete -Recurse -Force
        }
    }
}
