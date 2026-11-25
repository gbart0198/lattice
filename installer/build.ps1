<#
.SYNOPSIS
  Builds the release exe and packs it into dist\lattice-setup-<version>.exe.

.DESCRIPTION
  Needs Inno Setup 6 (winget install JRSoftware.InnoSetup) on top of the normal build tools.
  Pass -NoBuild to package an existing target\release\lattice.exe.
#>
param([switch]$NoBuild)
$ErrorActionPreference = 'Stop'

$root = Split-Path $PSScriptRoot -Parent
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$exe = Join-Path $target 'release\lattice.exe'
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value

if (-not $NoBuild) {
    Write-Host "Building lattice $version (release)..."
    # Run from the repo so rustup picks up rust-toolchain.toml. Called directly rather than via
    # cmd, which can't start in a UNC directory (\\wsl.localhost\...). cargo reports progress on
    # stderr, which Windows PowerShell would treat as an error.
    Push-Location $root
    $ErrorActionPreference = 'Continue'
    try { & cargo build --release --bin lattice 2>&1 | ForEach-Object { "$_" } | Out-Host; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } }
    finally { Pop-Location; $ErrorActionPreference = 'Stop' }
}
if (-not (Test-Path $exe)) { throw "No build found at $exe" }

$iscc = @(
    (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source,
    (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
    (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
    (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup' }

# build.rs draws the app icon into OUT_DIR; reuse it for the setup exe.
$icon = Get-ChildItem (Join-Path $target 'release\build\lattice-*\out\lattice.ico') -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1

$isccArgs = @("/DAppVersion=$version", "/DExePath=$exe", "/O$(Join-Path $root 'dist')")
if ($icon) { $isccArgs += "/DIconPath=$($icon.FullName)" }
& $iscc @isccArgs (Join-Path $PSScriptRoot 'lattice.iss')
if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }
Write-Host "Built dist\lattice-setup-$version.exe"
