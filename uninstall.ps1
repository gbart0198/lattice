<#
.SYNOPSIS
  Removes lattice installed by install.ps1. Your config (%APPDATA%\lattice) is kept unless
  you pass -RemoveConfig.
#>
param([switch]$RemoveConfig)
$ErrorActionPreference = 'Continue'

$dest = Join-Path $env:LOCALAPPDATA 'Programs\lattice'

# Stop lattice cleanly so hidden windows come back.
Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices;
public class LatticeWinU {
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
    [LatticeWinU]::CloseBar([uint32]$p.Id)
    if (-not $p.WaitForExit(5000)) { Stop-Process -Id $p.Id -Force }
}

# Belt and braces: show any window a killed lattice left hidden (hidden.txt: "hwnd pid").
Add-Type @"
using System; using System.Runtime.InteropServices;
public class LatticeShow {
  [DllImport("user32.dll")] public static extern bool ShowWindowAsync(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
$hiddenList = Join-Path $env:APPDATA 'lattice\hidden.txt'
if (Test-Path $hiddenList) {
    foreach ($line in Get-Content $hiddenList) {
        $parts = $line -split '\s+'
        if ($parts.Count -lt 2) { continue }
        $h = [IntPtr][Convert]::ToInt64($parts[0], 16); $want = [uint32]$parts[1]; $pid2 = [uint32]0
        if ([LatticeShow]::IsWindow($h)) { [void][LatticeShow]::GetWindowThreadProcessId($h, [ref]$pid2); if ($pid2 -eq $want) { [void][LatticeShow]::ShowWindowAsync($h, 8) } }
    }
    Remove-Item $hiddenList -ErrorAction SilentlyContinue
}

Remove-Item (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\lattice.lnk') -ErrorAction SilentlyContinue
Remove-Item (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Startup\lattice.lnk') -ErrorAction SilentlyContinue
Remove-Item 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\lattice' -Recurse -ErrorAction SilentlyContinue
# This script may be running from the install folder: delete it after we exit.
Start-Process cmd.exe -ArgumentList "/c timeout /t 2 >nul & rmdir /s /q `"$dest`"" -WindowStyle Hidden
if ($RemoveConfig) { Remove-Item (Join-Path $env:APPDATA 'lattice') -Recurse -ErrorAction SilentlyContinue }
Write-Host 'lattice uninstalled.'
