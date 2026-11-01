# Decides whether and as what version to release HEAD. Writes version, tag, skip and
# create_tag to $env:GITHUB_OUTPUT (or prints them when run locally).
#
# Merge to main (EVENT=workflow_run): next patch after the latest v* tag, or the Cargo.toml
# version if that's higher (bump it there for a minor/major release). Skipped when nothing but
# docs, markdown, LICENSE or .github changed since that tag, or when HEAD is already tagged.
# Tag push / manual run (EVENT=push|workflow_dispatch): release TAG as is; it must match Cargo.toml.

$ErrorActionPreference = 'Stop'

function Out-Plan([hashtable]$plan) {
    foreach ($k in $plan.Keys) {
        $line = "$k=$($plan[$k])"
        if ($env:GITHUB_OUTPUT) { $line >> $env:GITHUB_OUTPUT } else { $line }
    }
}

$cargo = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value

if ($env:EVENT -ne 'workflow_run') {
    $version = $env:TAG.TrimStart('v')
    if ($version -ne $cargo) {
        Write-Error "Tag $($env:TAG) does not match the workspace version $cargo"
    }
    Out-Plan @{ version = $version; tag = $env:TAG; skip = 'false'; create_tag = 'false' }
    exit 0
}

if (git tag --points-at HEAD --list 'v*') {
    Write-Host 'HEAD is already released.'
    Out-Plan @{ skip = 'true' }
    exit 0
}

$last = git describe --tags --abbrev=0 --match 'v[0-9]*' 2>$null
if ($LASTEXITCODE -ne 0) { $last = $null }
$next = [version]$cargo
if ($last) {
    $changed = @(git diff --name-only "$last..HEAD")
    $relevant = @($changed | Where-Object { $_ -notmatch '^(docs/|\.github/)|\.md$|^LICENSE$' })
    if ($relevant.Count -eq 0) {
        Write-Host "Only docs/CI changed since $last; no release."
        Out-Plan @{ skip = 'true' }
        exit 0
    }
    $prev = [version]$last.TrimStart('v')
    $bumped = [version]::new($prev.Major, $prev.Minor, $prev.Build + 1)
    if ($bumped -gt $next) { $next = $bumped }
}
$version = "$($next.Major).$($next.Minor).$($next.Build)"
Out-Plan @{ version = $version; tag = "v$version"; skip = 'false'; create_tag = 'true' }
