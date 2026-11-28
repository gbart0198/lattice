# Notes for agents working on lattice

## Code map

Where to start reading. Grep for the names below and read around them; `wm.rs` is large, so
don't read it whole. Every module opens with a `//!` comment saying what it owns.

**Threads.** The main thread runs Rune (bar, launcher, toast). The WM thread owns all window
state and the WinEvent hooks. A third thread runs only the low-level keyboard hook.
`shared.rs` carries state and commands between them.

**Platforms.** Everything OS-specific is in `platform/` (and `launcher/platform/`, kept apart
so the launcher imports nothing from the rest of lattice): one back end per OS with the same
modules and functions, picked at compile time. `platform/windows/` is the real one;
`platform/macos/` is stubs that log what isn't written yet. Win32 (`windows::`,
`std::os::windows`) only goes under a `platform/windows/` folder: `scripts/check-platforms.sh`
fails otherwise. A new OS call needs a function in both back ends.

| To change... | Look at |
|---|---|
| A command (keybind, `lattice msg`, launcher) | parsed in `config.rs` `parse_command`, run in `wm.rs` `Wm::command` |
| Keybinds and chords | `config.rs` `build_keymap`, `parse_chord`; lookup in `hooks.rs` `key_down`; the hook itself in `platform/windows/hooks.rs` |
| What happens when a window opens, closes, hides or gains focus | `wm.rs` `Wm::event`, `try_manage`, `unmanage`; the events come from `platform/windows/hooks.rs` |
| Startup and windows already open | `main.rs` `main` → `hooks.rs` `run` → `Wm::new` → `restore_saved_state` (when `state.json` exists) or `adopt_existing` |
| Tiling | `layout.rs` (pure math: `tile`, `resize_target`); `wm.rs` `retile`, `layout_monitor`, `place_tile` |
| Workspace switching, focus | `wm.rs` `show_workspace`, `switch_to_workspace`, `focus`, `focus_dir`, `neighbor` |
| Animations | `anim/mod.rs` timing and glides; `platform/windows/anim/` (`dwm.rs` the DWM-thumbnail overlay that does the drawing, `preview.rs` the `--animation-preview` demo) |
| `lattice msg/query/subscribe` | `ipc.rs` (wire format, CLI side, `State` JSON); the transport in `platform/windows/ipc.rs`; requests are queued and answered in `hooks.rs` `run` |
| Query output | `wm.rs` `Wm::state` builds `ipc::State`; `ipc.rs` `query_json` |
| `[[hook]]` and subscribe events | `events.rs` (`Tracker::update` diffs state once per batch; `emit`) |
| Setups (`[[setup]]`, `restore-setup`) | `setups.rs`; placement as windows appear in `wm.rs` (`Restore`, `try_manage`, `claim_slot`); when a restore is done, `check_restore` (`setup-restored`, `events.rs` `Event::setup_restored`); `msg restore-setup --wait` in `ipc.rs` (`take_wait`, `wait_restored`), matched by the id `hooks.rs` replies with |
| Placement kept across restarts (`state.json`) | `persist.rs` (file format, `load`/`save`, `assign` matches live windows to saved ones, `remap_monitors`); `wm.rs` `restore_saved_state`, `snapshot`, `save_state` (called once per batch in `hooks.rs` `run`, and in `shutdown`) |
| Config fields and defaults | `config.rs` structs; `default_config.toml` (the commented file users get) |
| OS calls | `platform/mod.rs` lists the modules; windows and monitors in `platform/windows/win.rs` (`WindowId`, `Hwnd` there, is an `isize`); hidden-window recovery in `shared.rs` (`hidden.txt`) |
| The bar | `bar.rs`, fed by `Wm::publish` |
| Launcher | `launcher/`: `mod.rs` entry, `ui.rs` view, `platform/windows/system.rs` app catalog and launching, `rank.rs` scoring, `files.rs` + `everything.rs` + `index.rs` file search, `window_search.rs` window search, `commands.rs` + `platform/windows/control.rs` command mode |
| Settings window | `settings/` (separate process, `--settings`; edits config.toml with `toml_edit`); the Setups tab in `settings/setups/` (`model.rs` state, `canvas.rs` canvas and drags, `view.rs` pages, `catalog.rs` the add picker's programs, `io.rs` save/capture/restore, `samples.rs` `--scene setups*` data) |
| Tour, focus border, updates | `tour/` (asks lattice through `platform/windows/control.rs`), `border.rs` + `platform/windows/border.rs`, `update.rs` + `update_toast.rs` |
| CLI entry for terminals | `bin/lattice-console.rs` (builds `lattice.com`) |
| User docs | `README.md`: command table under "Scripting", hook events table, "How workspaces work" |

Unit tests sit in each module's `mod tests`; `ipc.rs` `tests::sample()` is a ready-made
`State` for event and query tests. `bash scripts/check-platforms.sh` runs them on the host and
also `cargo check`s Windows from macOS or Linux (no MSVC needed: `scripts/msvc-stub.sh` stands
in for the C tools, nothing links); CI runs it on Windows and macOS.

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

A running exe can't be overwritten, so copy each new build to a new name (`lattice-2.exe`)
and swap to that. Also refresh the `lattice.exe` beside `lattice.com` every time: that copy
parses `msg` commands before sending them, so a stale one rejects new commands.

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
foreach ($p in @(Get-Process lattice* -ErrorAction SilentlyContinue)) { [void]$p.WaitForExit(8000) }
if ($Config) { $env:LATTICE_CONFIG = $Config } else { Remove-Item Env:LATTICE_CONFIG -ErrorAction SilentlyContinue }
Start-Process $Exe -WorkingDirectory (Split-Path $Exe)
```

`lattice*` matters: a dev copy named `lattice-2.exe` runs as process `lattice-2`, and the
script must wait for it too. To simulate a crash, `Stop-Process -Force` that process by its
real name.

Close your test windows *before* restarting the user's copy, and check which workspace the
user started on first (`query state`). A lattice that didn't save state when it quit (before
#24) adopts every open window onto workspace 1, mixing test windows in with the user's. Put
the user's windows back afterwards if that happens.

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
  close any windows you opened. Without a window argument `msg close` closes the *focused*
  window, so check `query focused` (exe and workspace) right before each one. Subscribers
  exit by themselves when lattice quits.
- Test windows: use Edge `--app=file:///...` pages or mspaint. **Never Notepad**: Windows 11
  Notepad reopens the user's previous tabs, which shows their document names.
- Start GUI programs from WSL with `powershell.exe Start-Process` (or `msg exec`), not
  `cmd.exe /c start`, which can block the shell.

### What the CLI can't reach

- **Keybinds themselves.** `msg` skips key-chord parsing and Win-key handling, and the keyboard
  hook ignores injected keys (`SendInput`). To test that path, add a temporary, env-gated patch
  to `hooks.rs` and remove it before committing.
- **Anything visual.** Animations, the border, the bar and the launcher UI need screen
  captures. For clips that may be posted publicly, keep the user's desktop out of them: use
  the monitor without desktop icons, and neutral windows such as Edge `--app=` pages.
- **Mouse dragging** needs real input.
