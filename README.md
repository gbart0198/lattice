# lattice

A tiling window manager for Windows 11 with Hyprland-style workspaces and a top bar drawn with
[Rune](https://github.com/anomalyco/rune).

- **Auto-tiling**: new app windows are tiled as soon as they appear.
- **Layouts**: `dwindle` (Hyprland's spiral, the default), `master` (master + stack), `monocle`.
  You can pick a layout for each workspace.
- **Virtual workspaces** (1–10) that switch instantly. They don't use Windows virtual desktops,
  so there's no animation (details below).
- **Configurable keybinds** for every action, with the Windows key as a modifier.
- **Top bar** showing workspace numbers with the apps on each, the focused window's title,
  the layout, CPU, memory, battery, and the clock. Click a workspace to switch to it.

## Install

From the project folder, in PowerShell:

```powershell
.\install.ps1            # build, install to %LOCALAPPDATA%\Programs\lattice, add a Start menu entry, start it
.\install.ps1 -Autostart # same, and start lattice when you sign in
```

Re-run it after changing the code to update the installed copy. It stops the running lattice
cleanly first. To remove lattice, use **Settings > Apps > Installed apps > lattice**, or
run `%LOCALAPPDATA%\Programs\lattice\uninstall.ps1`. Your config is kept unless you pass
`-RemoveConfig`.

## Build and run

Requires Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools. Rune is pinned to a
`dev` revision in `Cargo.toml`.

```bash
cargo build --release
```

```bash
target/release/lattice.exe
```

If linking fails with `LNK1104: cannot open file`, the project path is too long for the MSVC
linker. Build into a shorter target directory, for example `CARGO_TARGET_DIR=C:/lt cargo build --release`.

Other modes:

| Command | What it does |
|---|---|
| `lattice --list` | Prints monitors and the windows lattice would tile, float or ignore. Changes nothing. |
| `lattice --bar-preview` | Shows only the bar with sample data, without managing any windows. |
| `lattice --help` | Prints usage. |

Quit with **Win+Shift+E**. When lattice exits, even after a crash, it shows every window it
had hidden.

**Start at login:** a scheduled task with highest privileges also lets lattice manage
apps that run as administrator:

```bash
schtasks /Create /TN lattice /TR "\"C:\path\to\lattice.exe\"" /SC ONLOGON /RL HIGHEST
```

## Configuration

Press **Win+Shift+Comma** for the settings window: layouts, gaps, workspaces, the bar, the
focus border (with a live preview) and keybinds. Saving edits `config.toml` in place, keeping
your comments, and applies immediately. Bar and border changes offer a "Restart lattice" button.

On first run, lattice writes `%APPDATA%\lattice\config.toml`. Every option is documented
in that file, and [src/default_config.toml](src/default_config.toml) is the same file. Press
**Win+Shift+R** to reload it. Changes to the bar take effect after a restart.

The log is at `%APPDATA%\lattice\lattice.log`. Set `LATTICE_LOG=debug` for more detail.

### Tiling styles

| Layout | Behaviour |
|---|---|
| `dwindle` | Each new window takes half of the remaining space. The split runs along the longer side, so windows spiral inward. With `new_window = "after_focused"` a new window splits the focused one, as in Hyprland. `split_ratio` sets the size of the first half. |
| `master` | One master window on the left (`master_ratio` of the width). All other windows stack evenly on the right. |
| `monocle` | Every window fills the area. Only the focused one is visible, and focus left/right cycles through them. |

Settings shared by all layouts:

- `gaps_inner`: gap between windows.
- `gaps_outer`: gap at the screen edges.
- `float_unresizable`: dialogs and windows that can't be resized float instead of tiling.
- `[[rules]]`: float, ignore, or pin windows to a workspace, matched by exe, window class or title. Run `lattice --list` to find these values.

### Focus border

The focused window gets a glowing rounded frame, drawn by Rune on a transparent, click-through
overlay that lattice keeps wrapped around the window. Set it up in `[border]`:
`color`, `width`, `radius` (Windows 11 windows use 8), `glow` (blur size in pixels, 0 for a flat
line) and `glow_opacity`. Set `enabled = false` to turn it off. Changes take effect after a restart.

### Default keybinds

| Keys | Action |
|---|---|
| Win + ←/→/↑/↓ | Focus the window in that direction (crosses monitors) |
| Win + Shift + ←/→/↑/↓ | Swap the window that way, or move it to the next monitor |
| Win + 1…9 | Switch to workspace N |
| Win + Shift + 1…9 | Move the window to workspace N and follow it |
| Win + Ctrl + 1…9 | Move the window to workspace N and stay |
| Win + . / Win + , | Next / previous workspace |
| Win + O / Win + Shift + O | Focus the next monitor / move the window there |
| Win + Space | Cycle layout (dwindle → master → monocle) |
| Win + = / Win + − | Grow / shrink the master or first split |
| Win + Shift + Space | Toggle floating |
| Win + F | Toggle fullscreen (fills the tiling area) |
| Win + Q | Close the window |
| Win + Enter | Open Windows Terminal (`exec wt.exe`) |
| Win + Shift + R / Win + Shift + E | Reload config / quit |
| Win + Shift + , | Open the settings window (floats on top, never tiled) |
| Win + Ctrl + , | Open config.toml in your editor |

Your keybinds replace Windows' own shortcuts for the same keys, and the Start menu won't open
when you release Win after a lattice shortcut. **Win+L always locks the PC**, because Windows
doesn't let any program override it. For the same reason the defaults use arrow keys instead
of h/j/k/l.

Mouse: drag a tiled window onto another to swap them, or onto another monitor to move it there.

## How workspaces work

A workspace spans all your monitors. Each window belongs to one workspace and to one monitor
within it. Switching workspaces changes what's on every monitor at once: the old workspace's
windows are hidden with `ShowWindow(SW_HIDE)` and the new one's are shown. Each monitor is
tiled separately with the workspace's layout. Moving a window to another workspace keeps it on
the same monitor, and Win+Shift+O or Win+Shift+arrows move it between monitors. Windows virtual
desktops aren't involved, so there's no slide animation and switching takes as long as hiding
and showing a few windows.

Hidden windows also drop off the taskbar until you switch back. If an app activates a window on
a hidden workspace, for example when you click its notification, lattice switches to that
workspace. Set `follow_activation = false` to turn this off.

## Architecture

| File | Role |
|---|---|
| `src/main.rs` | Startup, single-instance check, crash safety, CLI modes |
| `src/hooks.rs` | Keyboard hook (on its own thread) and the WM thread's WinEvent hooks and message loop |
| `src/wm.rs` | State machine: windows, workspaces, monitors, commands, events |
| `src/layout.rs` | Pure layout math (unit-tested) |
| `src/win.rs` | Win32 wrappers: window filters, DWM-aware placement, focus, monitors |
| `src/bar.rs` | Rune top bar. It receives the state the WM publishes and sends click commands back. |
| `src/sysinfo.rs` | CPU, memory, battery and clock |
| `src/config.rs` | Config file, keybind and command parsing |

## Known limitations

- The bar appears only on the primary monitor. The Windows taskbar stays.
- A window that runs as administrator can be moved only if lattice is also elevated (see the start-at-login note).
- Resizing tiled windows with the mouse isn't supported yet: they snap back. Use Win+= and Win+− instead.
- Monitor changes (rotation, resolution, plugging monitors in or out) are picked up automatically.
