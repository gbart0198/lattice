# Scripting ideas

Things to build with `lattice msg` / `query` (pull) and `[[hook]]` / `lattice subscribe`
(push). Static placement ("Discord always on workspace 3") is already `[[rules]]`; these are
for behaviour that depends on state, time or conditions. Untested sketches.

## 1. Scratchpad terminal

One key pulls the terminal to the current workspace, or sends it away.

```powershell
# scratch.ps1 - bind: "super+grave" = "exec powershell -NoProfile -File C:\\scripts\\scratch.ps1"
$s = lattice query state | ConvertFrom-Json
$w = $s.windows | Where-Object exe -eq 'WindowsTerminal.exe' | Select-Object -First 1
if (-not $w)                               { lattice msg exec wt.exe; return }
if ($w.workspace -eq $s.focused.workspace) { lattice msg move-to-workspace-silent 9 $w.hwnd }
else { lattice msg move-to-workspace-silent $s.focused.workspace $w.hwnd
       lattice msg focus-window $w.hwnd }
```

## 2. Layout by window count

1 window → monocle, 2-3 → dwindle, 4+ → master.

```toml
[[hook]]
on = ["window-opened", "window-closed", "window-moved"]
run = "powershell -NoProfile -File %USERPROFILE%\\scripts\\auto-layout.ps1"
```

```powershell
$f  = lattice query focused | ConvertFrom-Json
$ws = lattice query workspaces | ConvertFrom-Json | Where-Object index -eq $f.workspace
$want = if ($ws.windows -le 1) {'monocle'} elseif ($ws.windows -ge 4) {'master'} else {'dwindle'}
if ($ws.layout -ne $want) { lattice msg layout $want }
```

No loop: `layout` emits `layout-changed`, which this hook doesn't listen to.

## 3. Dock / undock

```toml
[[hook]]
on = "display-changed"
run = "if %LATTICE_MONITORS%==1 (lattice msg restore-setup Laptop) else (lattice msg restore-setup Desk)"
```

## 4. Start-of-day script

```powershell
lattice msg restore-setup --wait Dev
if ($LASTEXITCODE -eq 0) {
  lattice msg exec wt.exe -d C:\src\app
  lattice msg workspace 1
}
```

Or from config with a `setup-restored` hook gated on `%LATTICE_COMPLETE%==true` (README).

## 5. Time tracking

```toml
[[hook]]
on = "focus-changed"
run = "echo %DATE% %TIME%,%LATTICE_WORKSPACE%,%LATTICE_EXE% >> %USERPROFILE%\\focus-log.csv"
```

Diff timestamps between rows for time per app per day. The 100 ms focus settle keeps
Alt+Tab bursts out of the log.

## 6. Meeting mode

Zoom/Teams opens → own workspace; closes → back to where you were.

```toml
[[hook]]
on = ["window-opened", "window-closed"]
run = "powershell -NoProfile -File %USERPROFILE%\\scripts\\meeting.ps1"
```

```powershell
$e = [Console]::In.ReadLine() | ConvertFrom-Json      # event JSON on stdin
if ($e.exe -notin 'Zoom.exe','ms-teams.exe') { return }
$state = "$env:TEMP\meeting-prev.txt"
if ($e.event -eq 'window-opened') {
  (lattice query focused | ConvertFrom-Json).workspace | Set-Content $state
  lattice msg move-to-workspace 8 $e.hwnd
} elseif (Test-Path $state) {
  lattice msg workspace (Get-Content $state); Remove-Item $state
}
```

Could also toggle Do Not Disturb, pause music, or light a "busy" lamp.

## 7. External displays (Rainmeter, Stream Deck, OBS)

```powershell
lattice subscribe workspace-changed focus-changed | ForEach-Object {
  $e = $_ | ConvertFrom-Json
  "WS $($e.workspace) · $($e.exe)" | Set-Content $env:TEMP\lattice-now.txt   # OBS text source
}
```

## 8. Bulk actions

- **Clean slate:** close everything on the current workspace (README's "close every Edge
  window" pipeline, filtered by `query focused`'s workspace).
- **Gather:** `move-to-workspace-silent <current> <hwnd>` for every window of one app.
- **Per-workspace wallpaper/accent:** `workspace-changed` hook keyed on `%LATTICE_WORKSPACE%`.

## Notes

- **Hooks vs subscribe:** every hook run spawns `cmd` (and maybe `powershell`), ~100-300 ms.
  Fine for rare events; for frequent ones like `focus-changed`, a single `subscribe` loop is
  cheaper and can keep state in memory instead of temp files.
- **Handles are safe:** `[hwnd]` commands exit 1 on a window lattice doesn't manage, so a
  stale handle can't hit the wrong window.
- **Ordering:** `msg` shares the keypress queue (no sleeps before `query`); events within a
  burst come windows → workspace/layout/display/monitor → focus.

## Gaps

- Title changes fire no event; poll `query windows` for title-based triggers.
- No loop protection: a hook that emits its own event re-triggers itself.
