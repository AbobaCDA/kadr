<#
.SYNOPSIS
  One-shot setup + build for Kadr on Windows.

.DESCRIPTION
  Checks the four things a Tauri build needs, installs the missing ones with
  winget, then builds. Run it from anywhere inside the repo:

      powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1

      powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1 -Dev
          # launches the app with hot reload instead of building an installer
      powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1 -Slim
          # ~7 MB installer instead of ~9 MB (drops PP-OCR/ONNX)
      powershell -ExecutionPolicy Bypass -File tools\setup-windows.ps1 -SkipPrereqs
          # only build, assume the toolchain is already there

.NOTES
  Installing Visual Studio Build Tools needs administrator rights. If you run
  this unelevated, the script stops and tells you to reopen PowerShell as
  administrator -- it does not try to escalate silently.
#>

[CmdletBinding()]
param(
    [switch]$Dev,
    [switch]$Slim,
    [switch]$SkipPrereqs
)

$ErrorActionPreference = 'Stop'

function Write-Step([string]$Text) {
    Write-Host ""
    Write-Host "==> $Text" -ForegroundColor Cyan
}

function Write-Ok([string]$Text) {
    Write-Host "    OK  $Text" -ForegroundColor Green
}

function Write-Warn2([string]$Text) {
    Write-Host "    !   $Text" -ForegroundColor Yellow
}

function Test-Admin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Update-SessionPath {
    # Installs done in this same window do not touch the current process PATH.
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = "$machine;$user"
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if ((Test-Path $cargoBin) -and ($env:Path -notlike "*$cargoBin*")) {
        $env:Path = "$cargoBin;$env:Path"
    }
}

function Get-CargoVersion {
    Update-SessionPath
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cargo) { return (& $cargo.Source --version) }
    return $null
}

function Get-NodeVersion {
    Update-SessionPath
    $node = Get-Command node -ErrorAction SilentlyContinue
    if ($node) { return (& $node.Source --version) }
    return $null
}

function Test-MsvcToolchain {
    # rustc on Windows needs the MSVC linker; Build Tools provide it.
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return $false }
    $found = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath
    return [bool]$found
}

function Test-WebView2 {
    $keys = @(
        'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
        'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
    )
    foreach ($key in $keys) {
        if (Test-Path $key) {
            $version = (Get-ItemProperty $key -ErrorAction SilentlyContinue).pv
            if ($version -and $version -ne '0.0.0.0') { return $true }
        }
    }
    return $false
}

function Install-WingetPackage([string]$Id, [string]$Label, [string]$Extra = '') {
    Write-Warn2 "$Label not found -- installing with winget (this can take a few minutes)"
    $base = @('install', '--id', $Id, '-e', '--accept-package-agreements', '--accept-source-agreements', '--silent')
    if ($Extra) { $base += @('--override', $Extra) }
    & winget @base
    if ($LASTEXITCODE -ne 0) {
        throw "winget failed to install $Label (exit $LASTEXITCODE). Install it manually and re-run with -SkipPrereqs."
    }
    Update-SessionPath
}

# ---------------------------------------------------------------- preflight

Write-Host "Kadr -- Windows setup" -ForegroundColor White
Write-Host "Build a local installer from source. Nothing is downloaded from any repo but the toolchains."

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $repoRoot
if (-not (Test-Path 'src-tauri\tauri.conf.json')) {
    throw "Run this from inside the Kadr source tree (src-tauri\tauri.conf.json not found at $repoRoot)."
}
Write-Ok "source tree: $repoRoot"

$winget = Get-Command winget -ErrorAction SilentlyContinue
if (-not $SkipPrereqs) {
    if (-not $winget) {
        throw "winget is missing. Install 'App Installer' from the Microsoft Store, or install the toolchain manually (see docs\BUILD.md) and re-run with -SkipPrereqs."
    }
    $isAdmin = Test-Admin
    if (-not $isAdmin) {
        Write-Warn2 "not running as administrator: Visual Studio Build Tools cannot be installed from here."
        Write-Warn2 "If Build Tools are already installed, this is fine -- continuing."
    }
}

# -------------------------------------------------------------------- Rust

Write-Step "1/5  Rust toolchain"
$cargoVersion = Get-CargoVersion
if ($cargoVersion) {
    Write-Ok $cargoVersion
} elseif (-not $SkipPrereqs) {
    Install-WingetPackage 'Rustlang.Rustup' 'Rust (rustup)'
    $cargoVersion = Get-CargoVersion
    if (-not $cargoVersion) { throw "Rust installed but 'cargo' is still not on PATH. Close this window, open a new PowerShell, and re-run with -SkipPrereqs." }
    Write-Ok $cargoVersion
} else {
    throw "Rust is not installed. See docs\BUILD.md."
}

if (-not $SkipPrereqs) {
    # Make sure the MSVC host toolchain is the active default (not GNU).
    & rustup default stable-msvc 2>$null | Out-Null
}

# -------------------------------------------------------- MSVC build tools

Write-Step "2/5  Visual Studio Build Tools (MSVC linker)"
if (Test-MsvcToolchain) {
    Write-Ok "MSVC C++ toolchain present"
} elseif (-not $SkipPrereqs) {
    if (-not (Test-Admin)) {
        throw @"
Visual Studio Build Tools (workload 'Desktop development with C++') are required.
They need administrator rights. Either:
  1. Reopen PowerShell as administrator and run this script again, or
  2. Install them from https://visualstudio.microsoft.com/visual-cpp-build-tools/
     (select 'Desktop development with C++'), then re-run with -SkipPrereqs
"@
    }
    Install-WingetPackage 'Microsoft.VisualStudio.2022.BuildTools' 'Visual Studio Build Tools' `
        '--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
    if (-not (Test-MsvcToolchain)) {
        throw "Build Tools installed but the C++ workload was not found. Open the Visual Studio Installer and add 'Desktop development with C++'."
    }
    Write-Ok "MSVC C++ toolchain installed"
} else {
    throw "MSVC toolchain not found and -SkipPrereqs was given. See docs\BUILD.md."
}

# -------------------------------------------------------------------- Node

Write-Step "3/5  Node.js"
$nodeVersion = Get-NodeVersion
if ($nodeVersion) {
    Write-Ok "node $nodeVersion"
} elseif (-not $SkipPrereqs) {
    Install-WingetPackage 'OpenJS.NodeJS.LTS' 'Node.js LTS'
    $nodeVersion = Get-NodeVersion
    if (-not $nodeVersion) { throw "Node installed but not on PATH. Open a new PowerShell and re-run with -SkipPrereqs." }
    Write-Ok "node $nodeVersion"
} else {
    throw "Node.js is not installed. See docs\BUILD.md."
}

# ---------------------------------------------------------------- WebView2

Write-Step "4/5  WebView2 runtime"
if (Test-WebView2) {
    Write-Ok "WebView2 present (system component on Windows 10 2021+ / 11)"
} else {
    Write-Warn2 "WebView2 not detected. The app will not start without it:"
    Write-Warn2 "  https://developer.microsoft.com/microsoft-edge/webview2/  (Evergreen Runtime)"
}

# ------------------------------------------------------------------- build

Write-Step "5/5  Building"
Write-Host "    installing frontend dependencies (first run downloads ~100 MB)" -ForegroundColor DarkGray
& npm ci --no-audit --no-fund
if ($LASTEXITCODE -ne 0) { throw "npm ci failed. Is Node.js on PATH? Reopen PowerShell and re-run with -SkipPrereqs." }

if ($Slim) {
    Write-Host "    slim profile: no PP-OCR/ONNX (~7 MB installer)" -ForegroundColor DarkGray
    & npm run dist:slim
} elseif ($Dev) {
    Write-Host "    dev mode: the window opens when the frontend is ready (first compile takes 10-20 min)" -ForegroundColor DarkGray
    & npm run tauri dev
    exit $LASTEXITCODE
} else {
    & npm run dist
}
if ($LASTEXITCODE -ne 0) { throw "Build failed (exit $LASTEXITCODE). Scroll up for the first error." }

$bundleDir = Join-Path $repoRoot 'src-tauri\target\release\bundle\nsis'
if (Test-Path $bundleDir) {
    $installer = Get-ChildItem $bundleDir -Filter *.exe | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($installer) {
        $sizeMb = [math]::Round($installer.Length / 1MB, 1)
        Write-Host ""
        Write-Host "Done: $($installer.FullName) ($sizeMb MB)" -ForegroundColor Green
        Write-Host "Double-click it to install, or run the portable build:"
        Write-Host "  src-tauri\target\release\kadr.exe"
        Write-Host ""
        $open = Read-Host "Open the folder containing the installer? [y/N]"
        if ($open -match '^[Yy]') { Start-Process explorer.exe $bundleDir }
    }
} else {
    Write-Host ""
    Write-Host "Done. Portable exe: src-tauri\target\release\kadr.exe" -ForegroundColor Green
}
