<#
.SYNOPSIS
  Creates the GitHub repository and pushes this fork, so the cloud build can
  produce the installer.

.DESCRIPTION
  Run from anywhere inside the repo:

      powershell -ExecutionPolicy Bypass -File tools\publish.ps1

      powershell -ExecutionPolicy Bypass -File tools\publish.ps1 -Repo kadr -Private
      powershell -ExecutionPolicy Bypass -File tools\publish.ps1 -User SomeoneElse

  What it does:
    1. checks git,
    2. commits whatever is not committed yet,
    3. creates <User>/<Repo> and pushes (via the GitHub CLI when it is installed,
       otherwise it prepares the remote and tells you the one command to run),
    4. gives the repository write permission for Actions, which the release step
       needs to attach the installer to a release,
    5. prints the URL where the build appears.

  Nothing here touches your machine's toolchain: the exe is built by GitHub.
#>

[CmdletBinding()]
param(
    [string]$User = 'AbobaCDA',
    [string]$Repo = 'kadr',
    [switch]$Private,
    [switch]$SkipPermissionFix
)

# 'Continue', not 'Stop'. Windows PowerShell 5.1 turns ANY line a native tool
# writes to stderr into a terminating error, even a harmless one such as
# "error: No such remote: 'origin'". git talks to stderr all the time, so the
# script checks exit codes itself and throws its own messages instead.
$ErrorActionPreference = 'Continue'

# ---------------------------------------------------------------- helpers
# Runs a native tool, merges its stderr into the output and reports the exit
# code. Never throws: callers decide what a non-zero code means.
function Invoke-Native {
    param(
        [Parameter(Mandatory = $true, Position = 0)][string]$Exe,
        [Parameter(Position = 1, ValueFromRemainingArguments = $true)][string[]]$Arguments
    )
    $out = @(& $Exe @Arguments 2>&1 | ForEach-Object { "$_" })
    [pscustomobject]@{ Output = $out; Code = $LASTEXITCODE }
}

function Invoke-Git {
    param([Parameter(Position = 0, ValueFromRemainingArguments = $true)][string[]]$Arguments)
    Invoke-Native -Exe 'git' -Arguments $Arguments
}

function Test-GitRemote([string]$Name) {
    $result = Invoke-Git 'remote'
    if ($result.Code -ne 0) { return $false }
    foreach ($line in $result.Output) {
        if ($line.Trim() -eq $Name) { return $true }
    }
    return $false
}

# Points 'origin' at the repository, whether or not it already existed.
# 'set-url' would fail on a missing remote and 'add' on an existing one, so the
# two cases are checked instead of blindly deleting what is not there.
function Set-GitOrigin([string]$Url) {
    if (Test-GitRemote 'origin') {
        $result = Invoke-Git 'remote' 'set-url' 'origin' $Url
        if ($result.Code -eq 0) { Write-Ok "remote origin -> $Url" ; return $true }
        return $false
    }
    $result = Invoke-Git 'remote' 'add' 'origin' $Url
    if ($result.Code -eq 0) { Write-Ok "remote origin -> $Url" ; return $true }
    Write-Warn2 "could not add the remote: $($result.Output -join ' ')"
    return $false
}

function Write-Step([string]$Text) {
    Write-Host ""
    Write-Host "==> $Text" -ForegroundColor Cyan
}
function Write-Ok([string]$Text) { Write-Host "    OK  $Text" -ForegroundColor Green }
function Write-Warn2([string]$Text) { Write-Host "    !   $Text" -ForegroundColor Yellow }

# Repo root = parent of tools\.
$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $repoRoot
if (-not (Test-Path 'src-tauri\tauri.conf.json')) {
    throw "Run this from inside the Kadr source tree (src-tauri\tauri.conf.json not found)."
}
if (-not (Test-Path '.github\workflows\windows-installer.yml')) {
    throw "The Windows build workflow is missing (.github\workflows\windows-installer.yml). Re-extract the archive."
}

# ------------------------------------------------------------------- git

Write-Step "1/4  git"
$git = Get-Command git -ErrorAction SilentlyContinue
if (-not $git) {
    throw @"
git is not installed. Install it and re-run:

    winget install --id Git.Git -e

Or install GitHub Desktop (which bundles git) and use "Add existing repository".
"@
}
$version = Invoke-Git '--version'
Write-Ok ($version.Output -join ' ')

if (-not (Test-Path '.git')) {
    $init = Invoke-Git 'init'
    if ($init.Code -ne 0) { throw "git init failed: $($init.Output -join ' ')" }
    [void](Invoke-Git 'symbolic-ref' 'HEAD' 'refs/heads/master')
    Write-Ok "initialised a fresh repository on branch master"
} else {
    Write-Ok "existing repository"
}

# A commit identity is required; use a local (repo-only) one if git has none.
$email = (Invoke-Git 'config' 'user.email').Output
if (-not ($email -join '').Trim()) {
    [void](Invoke-Git 'config' 'user.email' "$User@users.noreply.github.com")
    [void](Invoke-Git 'config' 'user.name' $User)
    Write-Ok "set a repository-local commit identity ($User)"
}

# --------------------------------------------------------------- commit

Write-Step "2/4  commit"
[void](Invoke-Git 'add' '-A')
$commit = Invoke-Git 'commit' '-q' '-m' 'Kadr: local search with full-result paging and the collection skill'
if ($commit.Code -eq 0) {
    Write-Ok "committed"
} else {
    Write-Ok "nothing new to commit"
}

# ------------------------------------------------------------ publish

Write-Step "3/4  publish to GitHub"
$gh = Get-Command gh -ErrorAction SilentlyContinue
$pushed = $false

if ($gh) {
    $auth = Invoke-Native 'gh' 'auth' 'status'
    if ($auth.Code -eq 0) {
        $visibility = if ($Private) { '--private' } else { '--public' }
        $view = Invoke-Native 'gh' 'repo' 'view' "$User/$Repo"
        if ($view.Code -eq 0) {
            Write-Ok "repository $User/$Repo already exists; pushing to it"
            if (Set-GitOrigin "https://github.com/$User/$Repo.git") {
                $push = Invoke-Git 'push' '-u' 'origin' 'master'
                if ($push.Code -eq 0) { $pushed = $true }
            }
        } else {
            $create = Invoke-Native 'gh' 'repo' 'create' "$User/$Repo" $visibility '--source' '.' '--remote' 'origin' '--push'
            if ($create.Code -eq 0) { $pushed = $true }
        }
        if ($pushed) {
            Write-Ok "pushed to https://github.com/$User/$Repo"
        } else {
            Write-Warn2 "gh could not create/push the repository; falling back to manual steps"
        }
    } else {
        Write-Warn2 "gh is installed but not logged in (run 'gh auth login' to automate this step)"
    }
}

if (-not $pushed) {
    [void](Set-GitOrigin "https://github.com/$User/$Repo.git")
    Write-Host ""
    Write-Host "Create the repository, then push:" -ForegroundColor White
    Write-Host ""
    Write-Host "  1. Open https://github.com/new"
    Write-Host "     owner: $User   name: $Repo   visibility: $(if ($Private) {'Private'} else {'Public'})"
    Write-Host "     Do NOT add a README, .gitignore or license (the code is already here)."
    Write-Host ""
    Write-Host "  2. Run:"
    Write-Host "     git push -u origin master" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  A browser window will ask you to sign in to GitHub the first time."
}

# --------------------------------------------------- actions permissions

if ($pushed -and -not $SkipPermissionFix) {
    Write-Step "4/4  Actions permissions"
    $ghOk = $gh -and ((Invoke-Native 'gh' 'auth' 'status').Code -eq 0)
    if ($ghOk) {
        # The release step needs a token that may write. New repositories
        # default to read-only workflow permissions, and the release step then
        # fails with a 403 -- a confusing red X on the first run.
        $perm = Invoke-Native 'gh' 'api' '-X' 'PUT' "repos/$User/$Repo/actions/permissions/workflow" `
            '-f' 'default_workflow_permissions=write' '-f' 'can_approve_pull_request_reviews=false'
        if ($perm.Code -eq 0) {
            Write-Ok "workflow token set to read/write"
        } else {
            Write-Warn2 "could not set it automatically. If the release step fails with 403:"
            Write-Warn2 "  Settings -> Actions -> General -> Workflow permissions -> Read and write permissions"
        }
    } else {
        Write-Warn2 "Set Settings -> Actions -> General -> Workflow permissions -> 'Read and write permissions'"
        Write-Warn2 "(needed only for the release step; the build itself works without it)"
    }
}

# ------------------------------------------------------------------ next

Write-Host ""
Write-Host "Next:" -ForegroundColor White
Write-Host "  1. Open https://github.com/$User/$Repo/actions/workflows/windows-installer.yml"
Write-Host "  2. Click 'Run workflow' -> 'Run workflow' (green button on the right)"
Write-Host "  3. Wait ~20 minutes, open the finished run, download the artifact"
Write-Host "     'kadr-windows-...' - it contains the installer and kadr-portable.exe."
Write-Host ""
Write-Host "  Pushing a tag also publishes a Release with the exe attached:" -ForegroundColor DarkGray
Write-Host "     git tag v1.0.16; git push --tags" -ForegroundColor DarkGray
