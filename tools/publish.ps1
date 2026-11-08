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

$ErrorActionPreference = 'Stop'

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
Write-Ok (& git --version)

if (-not (Test-Path '.git')) {
    & git init | Out-Null
    & git symbolic-ref HEAD refs/heads/master
    Write-Ok "initialised a fresh repository on branch master"
} else {
    Write-Ok "existing repository"
}

# A commit identity is required; use a local (repo-only) one if git has none.
$email = & git config user.email
if (-not $email) {
    & git config user.email "$User@users.noreply.github.com"
    & git config user.name $User
    Write-Ok "set a repository-local commit identity ($User)"
}

# --------------------------------------------------------------- commit

Write-Step "2/4  commit"
& git add -A
& git commit -q -m "Kadr: local search with full-result paging and the collection skill" 2>$null
if ($LASTEXITCODE -eq 0) {
    Write-Ok "committed"
} else {
    Write-Ok "nothing new to commit"
}

# ------------------------------------------------------------ publish

Write-Step "3/4  publish to GitHub"
$gh = Get-Command gh -ErrorAction SilentlyContinue
$pushed = $false

if ($gh) {
    $auth = & gh auth status 2>&1
    if ($LASTEXITCODE -eq 0) {
        $visibility = if ($Private) { '--private' } else { '--public' }
        & gh repo view "$User/$Repo" 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) {
            Write-Ok "repository $User/$Repo already exists; pushing to it"
            & git remote remove origin 2>$null | Out-Null
            & git remote add origin "https://github.com/$User/$Repo.git"
            & git push -u origin master
        } else {
            & gh repo create "$User/$Repo" $visibility --source . --remote origin --push
        }
        if ($LASTEXITCODE -eq 0) {
            $pushed = $true
            Write-Ok "pushed to https://github.com/$User/$Repo"
        } else {
            Write-Warn2 "gh could not create/push the repository; falling back to manual steps"
        }
    } else {
        Write-Warn2 "gh is installed but not logged in (run 'gh auth login' to automate this step)"
    }
}

if (-not $pushed) {
    & git remote remove origin 2>$null | Out-Null
    & git remote add origin "https://github.com/$User/$Repo.git"
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
    if ($gh -and (& gh auth status 2>&1 | Out-String) -notmatch 'not logged') {
        # The release step needs a token that may write. New repositories
        # default to read-only workflow permissions, and the release step then
        # fails with a 403 -- a confusing red X on the first run.
        & gh api -X PUT "repos/$User/$Repo/actions/permissions/workflow" `
            -f default_workflow_permissions=write -f can_approve_pull_request_reviews=false 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) {
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
