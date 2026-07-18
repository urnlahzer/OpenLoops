[CmdletBinding()]
param(
    [ValidateSet('Staged', 'Tracked', 'History')]
    [string]$Mode = 'Staged',

    [ValidateRange(1, 100)]
    [int]$MaxFileMiB = 2
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (& git rev-parse --show-toplevel 2>$null).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($repoRoot)) {
    throw 'Run this script from inside a Git repository.'
}

Set-Location -LiteralPath $repoRoot
$maxBytes = $MaxFileMiB * 1MB
$findings = [System.Collections.Generic.List[object]]::new()
$seenFindings = @{}

function Invoke-GitLines {
    param([Parameter(Mandatory)][string[]]$Arguments)

    $output = @(& git @Arguments 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "Git command failed: git $($Arguments -join ' ')"
    }

    return $output | ForEach-Object { [string]$_ }
}

function Add-Finding {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Rule
    )

    $key = "$Path`0$Rule"
    if (-not $seenFindings.ContainsKey($key)) {
        $seenFindings[$key] = $true
        $findings.Add([pscustomobject]@{ Path = $Path; Rule = $Rule })
    }
}

function Test-PlaceholderLine {
    param([Parameter(Mandatory)][string]$Line)

    return $Line -match '(?i)(example|template|placeholder|change[-_ ]?me|replace[-_ ]?me|dummy|fake|sample|not[-_ ]?real|your[-_ ]|<[^>]+>|\$\{|process\.env|os\.environ|getenv|00000000-0000-0000-0000-000000000000)'
}

function Get-MatchLine {
    param(
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][int]$Index
    )

    $start = $Text.LastIndexOf("`n", [Math]::Max(0, $Index - 1))
    if ($start -lt 0) { $start = 0 } else { $start++ }
    $end = $Text.IndexOf("`n", $Index)
    if ($end -lt 0) { $end = $Text.Length }
    return $Text.Substring($start, $end - $start)
}

function Test-CurrentlyIgnored {
    param([Parameter(Mandatory)][string]$Path)

    & git check-ignore --no-index --quiet -- $Path 2>$null
    $code = $LASTEXITCODE
    if ($code -gt 1) {
        throw "Could not check ignore rules for $Path"
    }
    return $code -eq 0
}

$pathRules = @(
    [pscustomobject]@{
        Name = 'environment or dotenv file'
        Pattern = '(?i)(^|/)\.env(?:$|\.)'
        Allow = '(?i)(^|/)\.env(?:\.[^/]+)?\.(?:example|template)$|(^|/)\.env\.(?:example|template)$'
    },
    [pscustomobject]@{
        Name = 'Codex local state outside the approved project config'
        Pattern = '(?i)(^|/)\.codex/(?!config\.toml$)'
        Allow = $null
    },
    [pscustomobject]@{
        Name = 'credential or token file'
        Pattern = '(?i)(^|/)(?:auth|tokens?|credentials?|client[_-]?secret)(?:[-_.][^/]*)?\.(?:json|ya?ml|toml|txt|db)$'
        Allow = '(?i)(?:^|[-_.])(?:example|template)\.(?:json|ya?ml|toml|txt)$'
    },
    [pscustomobject]@{
        Name = 'private key or signing credential'
        Pattern = '(?i)(?:^|/)(?:id_(?:rsa|dsa|ecdsa|ed25519)[^/]*|[^/]+\.(?:pem|key|p12|pfx|jks|keystore|mobileprovision))$'
        Allow = $null
    },
    [pscustomobject]@{
        Name = 'source map, debug bundle, or release archive'
        Pattern = '(?i)\.(?:map(?:\.gz)?|tgz|tar|tar\.gz|tar\.bz2|tar\.xz|zip|7z|rar|nupkg|snupkg|whl|egg|pdb|dmp|dump|core)$'
        Allow = $null
    },
    [pscustomobject]@{
        Name = 'private integration data or personal capture path'
        Pattern = '(?i)(^|/)(?:\.slack|\.graph|\.m365|\.azure|\.msal[^/]*|slack-export[^/]*|graph-export[^/]*|exports?|downloads?|recordings?|screenshots?|transcripts?|conversations?|\.private|private|local-data)(/|$)'
        Allow = $null
    },
    [pscustomobject]@{
        Name = 'log, trace, network capture, database, or crash artifact'
        Pattern = '(?i)\.(?:log(?:\.[^/]*)?|har|trace(?:\.[^/]*)?|sqlite3?|sqlite-[^/]+|db|db-[^/]+|dmp|dump|core)$'
        Allow = $null
    }
)

$contentRules = @(
    [pscustomobject]@{
        Name = 'private key material'
        Pattern = '-----BEGIN(?: [A-Z0-9]+)? PRIVATE KEY-----'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'Slack token'
        Pattern = '\bxox(?:a|b|p|r|s|c)-[A-Za-z0-9-]{10,}\b|\bxapp-[A-Za-z0-9-]{10,}\b'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'Slack webhook URL'
        Pattern = 'https://hooks\.slack\.com/services/[A-Za-z0-9/_-]{20,}'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'GitHub access token'
        Pattern = '\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'OpenAI-compatible API key'
        Pattern = '\bsk-[A-Za-z0-9_-]{20,}\b'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'cloud access key or signed credential'
        Pattern = '\b(?:AKIA|ASIA)[A-Z0-9]{16}\b|(?i)\b(?:AccountKey|SharedAccessKey|SharedAccessSignature|ClientSecret)\s*=\s*[^;\s]{8,}'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'JSON web token'
        Pattern = '\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'literal credential assignment'
        Pattern = '(?i)\b(?:client[_-]?secret|refresh[_-]?token|access[_-]?token|api[_-]?key|password|passwd|authorization)\b["'']?\s*[:=]\s*["''][^"'']{8,}["'']'
        AllowPlaceholders = $true
    },
    [pscustomobject]@{
        Name = 'npm authentication token'
        Pattern = '(?i)//[^/\s]+/:_authToken\s*=\s*\S+'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'credential embedded in URL'
        Pattern = '(?i)https?://[^/\s:@]+:[^/\s@]+@'
        AllowPlaceholders = $false
    },
    [pscustomobject]@{
        Name = 'personal Windows home-directory path'
        Pattern = '(?i)[A-Z]:[\\/]+Users[\\/]+(?!Public(?:[\\/]|$)|Default(?:[\\/]|$))[^\\/\s"''<>]+[\\/]'
        AllowPlaceholders = $true
    },
    [pscustomobject]@{
        Name = 'personal Unix home-directory path'
        Pattern = '(?i)/(?:Users|home)/(?!Shared/|runner/|sandbox/)[^/\s"''<>]+/'
        AllowPlaceholders = $true
    },
    [pscustomobject]@{
        Name = 'literal tenant or workspace identifier'
        Pattern = '(?im)^\s*["'']?(?:tenant[_-]?id|workspace[_-]?id|team[_-]?id)["'']?\s*[:=]\s*["'']?[A-Za-z0-9-]{8,}'
        AllowPlaceholders = $true
    }
)

function Test-PathRules {
    param([Parameter(Mandatory)][string]$Path)

    $normalized = $Path -replace '\\', '/'
    if (Test-CurrentlyIgnored -Path $normalized) {
        Add-Finding -Path $normalized -Rule 'file is ignored by the public-repo policy but was forced into Git'
    }

    foreach ($rule in $pathRules) {
        if ($normalized -match $rule.Pattern) {
            if ($null -ne $rule.Allow -and $normalized -match $rule.Allow) {
                continue
            }
            Add-Finding -Path $normalized -Rule $rule.Name
        }
    }
}

function Test-Blob {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ObjectId
    )

    Test-PathRules -Path $Path

    $sizeText = (Invoke-GitLines -Arguments @('cat-file', '-s', $ObjectId) | Select-Object -First 1).Trim()
    [long]$size = 0
    if (-not [long]::TryParse($sizeText, [ref]$size)) {
        throw "Could not determine the size of $Path"
    }
    if ($size -gt $maxBytes) {
        Add-Finding -Path $Path -Rule "file exceeds the default $MaxFileMiB MiB public-repo limit"
        return
    }

    $raw = Invoke-GitLines -Arguments @('cat-file', '-p', $ObjectId)
    $text = [string]::Join("`n", @($raw))

    foreach ($rule in $contentRules) {
        $matches = [regex]::Matches($text, $rule.Pattern)
        foreach ($match in $matches) {
            if ($rule.AllowPlaceholders) {
                $line = Get-MatchLine -Text $text -Index $match.Index
                if (Test-PlaceholderLine -Line $line) {
                    continue
                }
            }
            Add-Finding -Path $Path -Rule $rule.Name
            break
        }
    }
}

$blobs = [System.Collections.Generic.List[object]]::new()
$seenBlobs = @{}

function Add-Blob {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ObjectId
    )

    $key = "$ObjectId`0$Path"
    if (-not $seenBlobs.ContainsKey($key)) {
        $seenBlobs[$key] = $true
        $blobs.Add([pscustomobject]@{ Path = $Path; ObjectId = $ObjectId })
    }
}

switch ($Mode) {
    'Staged' {
        $paths = Invoke-GitLines -Arguments @('diff', '--cached', '--name-only', '--diff-filter=ACMR')
        foreach ($path in $paths) {
            if ([string]::IsNullOrWhiteSpace($path)) { continue }
            $objectId = (Invoke-GitLines -Arguments @('rev-parse', ":$path") | Select-Object -First 1).Trim()
            Add-Blob -Path $path -ObjectId $objectId
        }
    }
    'Tracked' {
        $paths = Invoke-GitLines -Arguments @('ls-tree', '-r', '--name-only', 'HEAD')
        foreach ($path in $paths) {
            if ([string]::IsNullOrWhiteSpace($path)) { continue }
            $objectId = (Invoke-GitLines -Arguments @('rev-parse', "HEAD:$path") | Select-Object -First 1).Trim()
            Add-Blob -Path $path -ObjectId $objectId
        }
    }
    'History' {
        $objects = Invoke-GitLines -Arguments @('rev-list', '--objects', '--all')
        foreach ($line in $objects) {
            if ($line -notmatch '^([0-9a-fA-F]{40,64})(?:\s+(.*))?$') { continue }
            $objectId = $Matches[1]
            $path = $Matches[2]
            if ([string]::IsNullOrWhiteSpace($path)) { continue }
            $type = (Invoke-GitLines -Arguments @('cat-file', '-t', $objectId) | Select-Object -First 1).Trim()
            if ($type -eq 'blob') {
                Add-Blob -Path $path -ObjectId $objectId
            }
        }
    }
}

foreach ($blob in $blobs) {
    Test-Blob -Path $blob.Path -ObjectId $blob.ObjectId
}

if ($findings.Count -gt 0) {
    [Console]::Error.WriteLine("BLOCKED: OpenLoops public-repo gate found $($findings.Count) issue(s).")
    foreach ($finding in ($findings | Sort-Object Path, Rule)) {
        [Console]::Error.WriteLine(" - $($finding.Path): $($finding.Rule)")
    }
    [Console]::Error.WriteLine('No suspected secret values were printed. Fix or unstage the files; do not bypass the gate.')
    exit 1
}

Write-Host "OpenLoops public-repo gate passed ($Mode, $($blobs.Count) blob(s) checked)."
