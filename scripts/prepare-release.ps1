<#
.SYNOPSIS
    Opens the pull request for a new release. Merging that PR publishes the release.

.DESCRIPTION
    Starting from an up-to-date main, this:
      1. works out the next version (patch, minor, major, or an exact X.Y.Z);
      2. sets it in Cargo.toml and Cargo.lock;
      3. moves the [Unreleased] notes in CHANGELOG.md under a dated heading for that version
         and updates the comparison links at the bottom;
      4. commits on a release/vX.Y.Z branch, pushes it and opens a "Release vX.Y.Z" PR
         (needs the GitHub CLI, gh, signed in).

    When the PR is merged, the Build and release workflow sees a version on main that has no
    release yet. It builds, tests and checks the installer, then tags the merge commit and
    publishes the GitHub release with the installer, the zip and checksums.

.EXAMPLE
    ./scripts/prepare-release.ps1 minor     # e.g. 0.1.0 -> 0.2.0

.EXAMPLE
    ./scripts/prepare-release.ps1 1.0.0 -NoPush   # prepare the commit locally only
#>
param(
    # patch, minor, major, or an exact version such as 1.2.3.
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $Bump,
    # Make the commit on the release branch, but don't push it or open the PR.
    [switch] $NoPush
)

$ErrorActionPreference = 'Stop'
$repoUrl = 'https://github.com/joreeves/rust-dirstat'
$utf8 = New-Object System.Text.UTF8Encoding $false
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Assert-Ok([string] $what) {
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)" }
}

# Start from an up-to-date main.
if (git status --porcelain) { throw 'The working tree has uncommitted changes. Commit or stash them first.' }
git fetch origin --tags --quiet; Assert-Ok 'git fetch'
git switch main --quiet; Assert-Ok 'git switch main'
git merge --ff-only origin/main --quiet; Assert-Ok 'Updating main from origin'

# Current and next version.
$cargoPath = Join-Path $root 'Cargo.toml'
$cargo = [IO.File]::ReadAllText($cargoPath)
$m = [regex]::Match($cargo, '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"')
if (-not $m.Success) { throw 'Cargo.toml has no version = "X.Y.Z" line.' }
$major = [int]$m.Groups[1].Value; $minor = [int]$m.Groups[2].Value; $patch = [int]$m.Groups[3].Value
$current = "$major.$minor.$patch"
switch ($Bump) {
    'major' { $next = "$($major + 1).0.0" }
    'minor' { $next = "$major.$($minor + 1).0" }
    'patch' { $next = "$major.$minor.$($patch + 1)" }
    default {
        if ($Bump -notmatch '^\d+\.\d+\.\d+$') { throw "Use patch, minor, major or a version like 1.2.3 (got '$Bump')." }
        $next = $Bump
    }
}
if ([version]$next -le [version]$current) { throw "The new version ($next) must be higher than the current one ($current)." }
if (git tag --list "v$next") { throw "Tag v$next already exists." }

# CHANGELOG.md: date the Unreleased notes as the new version.
$logPath = Join-Path $root 'CHANGELOG.md'
$log = [IO.File]::ReadAllText($logPath)
$nl = if ($log.Contains("`r`n")) { "`r`n" } else { "`n" }
$unreleased = [regex]::Match($log, '(?m)^## \[Unreleased\][ \t]*(?=\r?$)')
if (-not $unreleased.Success) { throw 'CHANGELOG.md has no "## [Unreleased]" heading.' }
$headEnd = $unreleased.Index + $unreleased.Length
$after = $log.Substring($headEnd)
$end = [regex]::Match($after, '(?m)^(## \[|\[[^\]]+\]: )')
$notes = if ($end.Success) { $after.Substring(0, $end.Index) } else { $after }
if (-not $notes.Trim()) { throw 'CHANGELOG.md has nothing under [Unreleased]. Add the release notes there first.' }
$date = Get-Date -Format 'yyyy-MM-dd'
$log = $log.Substring(0, $headEnd) + $nl + $nl + "## [$next] - $date" + $after
$link = [regex]::Match($log, '(?m)^\[Unreleased\]: [^\r\n]*')
if (-not $link.Success) { throw 'CHANGELOG.md has no [Unreleased] link at the bottom.' }
$links = "[Unreleased]: $repoUrl/compare/v$next...HEAD$nl[$next]: $repoUrl/compare/v$current...v$next"
$log = $log.Remove($link.Index, $link.Length).Insert($link.Index, $links)

# Write the files, then let cargo update Cargo.lock.
$cargo = $cargo.Remove($m.Index, $m.Length).Insert($m.Index, "version = `"$next`"")
[IO.File]::WriteAllText($cargoPath, $cargo, $utf8)
[IO.File]::WriteAllText($logPath, $log, $utf8)
cargo update --workspace --quiet; Assert-Ok 'cargo update'

# Commit, push and open the PR.
$branch = "release/v$next"
git switch -c $branch --quiet; Assert-Ok "Creating branch $branch"
git add Cargo.toml Cargo.lock CHANGELOG.md; Assert-Ok 'git add'
git commit --quiet -m "Release v$next"; Assert-Ok 'git commit'
if ($NoPush) {
    "Prepared v$next on branch $branch. Push it and open a PR; merging it publishes the release."
    return
}
git push --quiet -u origin $branch; Assert-Ok 'git push'
$body = @(
    "Releases **v$next** (from $current). Merging this PR publishes it: the Build and release workflow tags the merge commit and creates the GitHub release with the installer, the zip and checksums."
    ''
    '## Release notes'
    $notes.Trim()
) -join "`n"
$bodyFile = [IO.Path]::GetTempFileName()
try {
    [IO.File]::WriteAllText($bodyFile, $body, $utf8)
    gh pr create --base main --head $branch --title "Release v$next" --body-file $bodyFile; Assert-Ok 'gh pr create'
} finally {
    Remove-Item $bodyFile -ErrorAction SilentlyContinue
}
