<#
.SYNOPSIS
    Works out what the next release is: whether the app changed since the last one, and the
    new version number.

.DESCRIPTION
    The Build and release workflow runs this on every build; you never need to run it yourself.
    Run locally, it shows what merging the current branch would release (and changes nothing
    unless -Apply is given).

    - Last release: the highest vX.Y.Z tag.
    - Something to release: any change since then under src/, assets/ or installer/, or to
      build.rs, Cargo.toml or Cargo.lock. Changes only to docs, the workflow or scripts are
      not released.
    - New version: the version in Cargo.toml when it is higher than the last release (set it
      in a pull request for a bigger jump, such as 1.0.0). Otherwise the next minor version
      when CHANGELOG.md has an "### Added" heading under Unreleased, and the next patch
      version when it doesn't.

    With -Apply, and something to release, it also:
      - sets that version in Cargo.toml and Cargo.lock;
      - moves the Unreleased notes in CHANGELOG.md under a dated heading for it and updates the
        comparison links (when there are no notes, it lists the merged pull requests instead);
      - writes the notes to release-notes.md for the GitHub release.

.OUTPUTS
    An object with Changed (bool), Current (the version in Cargo.toml), Next and Last.
#>
param(
    # Make the changes for the release (the workflow does this on main).
    [switch] $Apply
)

$ErrorActionPreference = 'Stop'
$repoUrl = 'https://github.com/joreeves/rust-dirstat'
$appPaths = @('src', 'assets', 'installer', 'build.rs', 'Cargo.toml', 'Cargo.lock')
$utf8 = New-Object System.Text.UTF8Encoding $false
$root = Split-Path -Parent $PSScriptRoot

function Assert-Ok([string] $what) {
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit code $LASTEXITCODE)" }
}

# The version in Cargo.toml (the [package] one comes first).
$cargoPath = Join-Path $root 'Cargo.toml'
$cargo = [IO.File]::ReadAllText($cargoPath)
$m = [regex]::Match($cargo, '(?m)^version\s*=\s*"(\d+\.\d+\.\d+)"')
if (-not $m.Success) { throw 'Cargo.toml has no version = "X.Y.Z" line.' }
$current = [version]$m.Groups[1].Value

# The last release.
$tags = @(git -C $root tag --list 'v*'); Assert-Ok 'git tag'
$last = $tags | Where-Object { $_ -match '^v\d+\.\d+\.\d+$' } | ForEach-Object { [version]$_.Substring(1) } |
    Sort-Object -Descending | Select-Object -First 1

# App changes since then. Three dots compare with where this branch left the release, so a
# commit that the release already contains has nothing new.
if ($last) {
    $changedFiles = @(git -C $root diff --name-only "v$last...HEAD" -- $appPaths); Assert-Ok 'git diff'
} else {
    $changedFiles = @('(first release)')
}
$changed = $changedFiles.Count -gt 0

# The Unreleased notes in CHANGELOG.md.
$logPath = Join-Path $root 'CHANGELOG.md'
$log = [IO.File]::ReadAllText($logPath)
$nl = if ($log.Contains("`r`n")) { "`r`n" } else { "`n" }
$head = [regex]::Match($log, '(?m)^## \[Unreleased\][ \t]*(?=\r?$)')
if (-not $head.Success) { throw 'CHANGELOG.md has no "## [Unreleased]" heading.' }
$headEnd = $head.Index + $head.Length
$after = $log.Substring($headEnd)
$end = [regex]::Match($after, '(?m)^(## \[|\[[^\]]+\]: )')
$notes = $(if ($end.Success) { $after.Substring(0, $end.Index) } else { $after }).Trim()
$rest = if ($end.Success) { $after.Substring($end.Index) } else { '' }

# The new version.
if ($last -and $current -le $last) {
    $next = if ($notes -match '(?m)^### Added') {
        [version]::new($last.Major, $last.Minor + 1, 0)
    } else {
        [version]::new($last.Major, $last.Minor, $last.Build + 1)
    }
} else {
    $next = $current
}

$lastText = if ($last) { "v$last" } else { 'none' }
if ($changed) {
    Write-Host "Next release: v$next (last release: $lastText; $($changedFiles.Count) app files changed since)"
} else {
    Write-Host "Nothing to release: the app hasn't changed since $lastText."
}

if ($Apply -and $changed) {
    if ($log -match "(?m)^## \[$([regex]::Escape("$next"))\]") { throw "CHANGELOG.md already has a section for $next." }
    if (-not $notes) {
        # Nobody wrote notes: list what was merged.
        $entries = foreach ($sha in @(git -C $root rev-list --first-parent "v$last..HEAD")) {
            $subject = git -C $root log -1 --format=%s $sha
            if ($subject -match '^Merge pull request #(\d+)') {
                $number = $Matches[1]
                $title = @(git -C $root log -1 --format=%b $sha) | Where-Object { $_.Trim() } | Select-Object -First 1
                if ($title) { "- $title (#$number)" }
            } elseif ($subject -notmatch '^Release v\d') {
                "- $subject"
            }
        }
        $notes = "### Changed$nl$nl" + (@($entries) -join $nl)
    }

    # CHANGELOG.md: the notes move under the new version, and Unreleased starts empty.
    $date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
    $log = $log.Substring(0, $headEnd) + $nl + $nl + "## [$next] - $date" + $nl + $nl + $notes + $nl + $nl + $rest
    $link = [regex]::Match($log, '(?m)^\[Unreleased\]: [^\r\n]*')
    if (-not $link.Success) { throw 'CHANGELOG.md has no [Unreleased] link at the bottom.' }
    $compare = if ($last) { "$repoUrl/compare/v$last...v$next" } else { "$repoUrl/releases/tag/v$next" }
    $links = "[Unreleased]: $repoUrl/compare/v$next...HEAD$nl[$next]: $compare"
    $log = $log.Remove($link.Index, $link.Length).Insert($link.Index, $links)
    [IO.File]::WriteAllText($logPath, $log, $utf8)

    # Cargo.toml, then Cargo.lock (cargo updates only this package's entry).
    $cargo = $cargo.Remove($m.Index, $m.Length).Insert($m.Index, "version = `"$next`"")
    [IO.File]::WriteAllText($cargoPath, $cargo, $utf8)
    cargo update --workspace --quiet --manifest-path $cargoPath; Assert-Ok 'cargo update'

    [IO.File]::WriteAllText((Join-Path $root 'release-notes.md'), $notes + $nl, $utf8)
    Write-Host "Prepared v${next}: Cargo.toml, Cargo.lock, CHANGELOG.md and release-notes.md."
}

[pscustomobject]@{ Changed = $changed; Current = "$current"; Next = "$next"; Last = $(if ($last) { "$last" } else { '' }) }
