<#
.SYNOPSIS
  Builds lattice and installs it for the current user (no admin needed).

.DESCRIPTION
  - builds the release executable (skip with -NoBuild)
  - stops a running lattice cleanly (its hidden windows come back)
  - copies lattice.exe to %LOCALAPPDATA%\Programs\lattice
  - adds a Start menu shortcut and an "Apps > Installed apps" entry
  - with -Autostart, starts lattice when you sign in
  - starts the installed lattice

.EXAMPLE
  .\install.ps1 -Autostart
#>
param(
    [switch]$Autostart,
    [switch]$NoBuild,
    [switch]$NoStart
)
$ErrorActionPreference = 'Stop'

$src = $PSScriptRoot
$dest = Join-Path $env:LOCALAPPDATA 'Programs\lattice'
$exe = Join-Path $dest 'lattice.exe'
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\lattice.lnk'
$startup = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Startup\lattice.lnk'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\lattice'

# 1. Build.
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $src 'target' }
$built = Join-Path $target 'release\lattice.exe'
if (-not $NoBuild) {
    Write-Host 'Building lattice (release)...'
    Push-Location $src
    # cargo reports progress on stderr, which Windows PowerShell would treat as an error.
    $ErrorActionPreference = 'Continue'
    try { cmd /c "cargo build --release --bin lattice 2>&1" | Out-Host; if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' } }
    finally { Pop-Location; $ErrorActionPreference = 'Stop' }
}
if (-not (Test-Path $built)) { throw "No build found at $built" }

# 2. Stop a running lattice the clean way: closing its bar restores every hidden window.
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public class LatticeWin {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  // "lattice-control" quits like Win+Shift+E (restores hidden windows); older builds only have the bar.
  public static void CloseBar(uint pid) {
    IntPtr ctl = IntPtr.Zero, bar = IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); var t = new StringBuilder(64);
      GetWindowText(h, t, 64); if (p == pid && t.ToString() == "lattice-control") { ctl = h; } if (p == pid && t.ToString() == "lattice-bar") { bar = h; }
      return true; }, IntPtr.Zero);
    PostMessage(ctl != IntPtr.Zero ? ctl : bar, 0x10, IntPtr.Zero, IntPtr.Zero);
  }
}
"@
foreach ($p in @(Get-Process lattice -ErrorAction SilentlyContinue)) {
    Write-Host "Stopping running lattice (pid $($p.Id))..."
    [LatticeWin]::CloseBar([uint32]$p.Id)
    if (-not $p.WaitForExit(5000)) { Stop-Process -Id $p.Id -Force }   # the next start restores windows
}

# 3. Install files.
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item $built $exe -Force
Copy-Item (Join-Path $src 'uninstall.ps1') (Join-Path $dest 'uninstall.ps1') -Force
Copy-Item (Join-Path $src 'README.md') (Join-Path $dest 'README.md') -Force -ErrorAction SilentlyContinue

# 4. Shortcuts.
$shell = New-Object -ComObject WScript.Shell
function New-Shortcut($path) {
    $lnk = $shell.CreateShortcut($path)
    $lnk.TargetPath = $exe
    $lnk.WorkingDirectory = $dest
    $lnk.IconLocation = "$exe,0"
    $lnk.Description = 'lattice tiling window manager'
    $lnk.Save()
}
New-Shortcut $startMenu
if ($Autostart) { New-Shortcut $startup; Write-Host 'lattice will start when you sign in.' }
elseif (Test-Path $startup) { Remove-Item $startup }

# 5. Installed-apps entry (per user).
New-Item -Path $uninstallKey -Force | Out-Null
$version = (Select-String -Path (Join-Path $src 'Cargo.toml') -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
Set-ItemProperty $uninstallKey DisplayName 'lattice'
Set-ItemProperty $uninstallKey DisplayVersion $version
Set-ItemProperty $uninstallKey Publisher 'lattice'
Set-ItemProperty $uninstallKey DisplayIcon "$exe,0"
Set-ItemProperty $uninstallKey InstallLocation $dest
Set-ItemProperty $uninstallKey UninstallString "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$dest\uninstall.ps1`""
Set-ItemProperty $uninstallKey NoModify 1 -Type DWord
Set-ItemProperty $uninstallKey NoRepair 1 -Type DWord
Set-ItemProperty $uninstallKey EstimatedSize ([int]((Get-Item $exe).Length / 1KB)) -Type DWord

Write-Host "Installed lattice $version to $dest"

# 6. Start it.
if (-not $NoStart) {
    Start-Process $exe -WorkingDirectory $dest
    Write-Host 'lattice started. Quit with Win+Shift+E.'
}
