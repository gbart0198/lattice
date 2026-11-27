# Notes for agents working on lattice

## Testing changes in a real lattice

Unit tests (`cargo test`) cover layouts, config parsing and event diffs. To see a change working
in the real window manager, run a dev build on the desktop and drive it with the scripting CLI
(README, "Scripting"): `lattice msg` runs commands, `lattice query` reports state as JSON,
`lattice subscribe` streams events. This needs the user's go-ahead: their lattice is quit for
the duration, and their windows move.

### Building from WSL

Run the Windows toolchain (`cargo.exe`) with a target folder on the Windows side. Building
into the WSL filesystem fails with "could not create session directory lock file".

```bash
export CARGO_TARGET_DIR='C:\Users\<you>\AppData\Local\Temp\lattice-dev-target'
export WSLENV=CARGO_TARGET_DIR/w:$WSLENV
cargo.exe build --release --bin lattice --bin lattice-console
```

Copy `release\lattice.exe` and `release\lattice-console.exe` (renamed to `lattice.com`) into
a test folder such as `%TEMP%\lattice-dev`, along with a copy of the user's
`%APPDATA%\lattice\config.toml`. Add any `[[hook]]` entries you want to test to that copy.

### Swapping it in, and back

Only one lattice runs at a time. Quit the installed one the clean way, by closing its
control window, which shows every window it hid. Then start the dev build with
`LATTICE_CONFIG` pointing at the test config. Save this as `swap.ps1` and run it with
`powershell.exe -NoProfile -ExecutionPolicy Bypass -File swap.ps1 -Exe <path>`; inline
`Add-Type` through WSL quoting breaks.

```powershell
param([string]$Exe, [string]$Config)
Add-Type -Namespace W -Name U -MemberDefinition '[DllImport("user32.dll")] public static extern IntPtr FindWindow(string c, string t); [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);'
$h = [W.U]::FindWindow("lattice-display-listener", "lattice-control")
if ($h -ne [IntPtr]::Zero) { [void][W.U]::PostMessage($h, 0x10, [IntPtr]0, [IntPtr]0) }
foreach ($p in @(Get-Process lattice -ErrorAction SilentlyContinue)) { [void]$p.WaitForExit(8000) }
if ($Config) { $env:LATTICE_CONFIG = $Config } else { Remove-Item Env:LATTICE_CONFIG -ErrorAction SilentlyContinue }
Start-Process $Exe -WorkingDirectory (Split-Path $Exe)
```

When you're done, always restart the user's copy:
`swap.ps1 -Exe "$env:LOCALAPPDATA\Programs\lattice\lattice.exe"` (no `-Config`).

### Driving and checking it

From WSL, run `lattice.com` from the test folder through `cmd.exe`. Its output and exit code
come back like any console program's:

```bash
cd /mnt/c/Users/<you>/AppData/Local/Temp/lattice-dev
cmd.exe /c 'lattice.com msg workspace 2'          # exit 0 ok, 1 failed, 2 bad arguments
cmd.exe /c 'lattice.com query focused'            # JSON: assert on this, not on screenshots
(cmd.exe /c 'lattice.com subscribe' > events.jsonl 2>&1 &)   # record events in the background
```

- Commands go through the same queue as keypresses, so a `query` right after a `msg` sees its
  result; no sleeps are needed between them. Events do need a moment: focus changes are
  delivered 100 ms after the focus settles.
- Check state with `query`: a window's workspace, monitor, rect, floating and focused flags.
  Check order and duplicates in the `subscribe` output.
- To test hooks, add a `[[hook]]` whose `run` appends to a file
  (`findstr . >> %TEMP%\lattice-dev\hook.log` records the JSON from stdin), then read the
  file. Config errors, bad hooks among them, go to `%APPDATA%\lattice\lattice.log`.
- Leave the desktop as you found it: switch back to the starting workspace and layout, and
  close any windows you opened (`msg exec notepad.exe` … `msg close`). Subscribers exit by
  themselves when lattice quits.

### What the CLI can't reach

- **Keybinds themselves.** `msg` skips key-chord parsing and Win-key handling, and the keyboard
  hook ignores injected keys (`SendInput`). To test that path, add a temporary, env-gated patch
  to `hooks.rs` and remove it before committing.
- **Anything visual.** Animations, the border, the bar and the launcher UI need screen
  captures. For clips that may be posted publicly, keep the user's desktop out of them: use
  the monitor without desktop icons, and neutral windows such as Edge `--app=` pages.
- **Mouse dragging** needs real input.
