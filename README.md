# lattice

A tiling window manager for Windows 11 with Hyprland-style workspaces, a glowing focus border
and a top bar, with all of its UI drawn by [Rune](https://github.com/anomalyco/rune).

[Watch the 30-second teaser](docs/lattice-teaser.mp4)

- **Auto-tiling**: new app windows snap into place as soon as they appear.
- **Layouts**: `dwindle` (Hyprland's spiral, the default), `master` (master + stack) and
  `monocle`, chosen per workspace.
- **Workspaces** (1–10) that span all your monitors and switch instantly. They don't use
  Windows virtual desktops, so there's no animation.
- **Keybinds** for every action, with the Windows key as the modifier.
- **Top bar**: workspaces with the icons of the apps on each, the focused window's title, a
  layout button, CPU, memory, battery and the clock, in any installed font.
- **Focus border**: a soft glowing frame around the focused window.
- **App launcher** (Win+Space): a Raycast-style search over every app in the Start menu,
  Store apps included. Type a few letters, press Enter.
- **Settings window** (Win+Alt+Comma) for everything above.

## Getting started

### 1. Prerequisites

- **Windows 11** (Windows 10 mostly works, but the border and rounded corners assume 11).
- **Git**: <https://git-scm.com/download/win>
- **Rust** via rustup: <https://rustup.rs>. Use the default `x86_64-pc-windows-msvc` toolchain.
  The repo's `rust-toolchain.toml` pins the exact Rust version, and rustup installs it
  automatically on the first build.
- **Visual Studio Build Tools** with the **Desktop development with C++** workload:
  <https://visualstudio.microsoft.com/visual-cpp-build-tools/>. It provides the MSVC linker and
  the Windows SDK, whose resource compiler embeds the app icon.

### 2. Get the code

```powershell
git clone https://github.com/gbart0198/lattice.git
cd lattice
```

### 3. Run it from source

```powershell
cargo run --release
```

The first build downloads and compiles Rune and its dependencies, which takes a few minutes.
Later builds take seconds. lattice starts tiling your windows straight away, and
**Win+Shift+E** quits it and gives every window back.

To look around before letting it manage anything:

```powershell
cargo run --release -- --list          # monitors + windows it would tile/float/ignore; changes nothing
cargo run --release -- --bar-preview   # just the bar, with sample data
cargo run --release -- --settings      # just the settings window
```

If linking fails with `LNK1104: cannot open file`, the folder path is too long for the MSVC
linker. Build into a shorter directory:

```powershell
$env:CARGO_TARGET_DIR = "C:\lt"; cargo run --release
```

### 4. Install it as an app

```powershell
.\install.ps1              # build, install, add a Start menu entry, start it
.\install.ps1 -Autostart   # same, and start lattice when you sign in
```

The installer builds the release version and copies it to `%LOCALAPPDATA%\Programs\lattice`,
so no admin rights are needed. It also adds a Start menu shortcut and an entry under
**Settings > Apps > Installed apps**. Re-run it after pulling or changing the code to update
the installed copy: it stops the running lattice cleanly first.

If PowerShell refuses to run the script, allow local scripts for your account once:

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

To uninstall, use **Installed apps > lattice**, or run
`%LOCALAPPDATA%\Programs\lattice\uninstall.ps1`. Your config is kept unless you pass
`-RemoveConfig`.

**Apps running as administrator**: Windows only lets lattice move them if lattice is elevated
too. To start it elevated at sign-in, use a scheduled task instead of `-Autostart`:

```powershell
schtasks /Create /TN lattice /TR "\"$env:LOCALAPPDATA\Programs\lattice\lattice.exe\"" /SC ONLOGON /RL HIGHEST
```

## Configuration

Press **Win+Alt+Comma** to open the settings window. It floats above everything and is never
tiled, with these tabs:

- **General**: layouts, gaps, workspaces and behavior.
- **Bar**: what it shows, size, colors, and a font picker that lists every font installed on
  your PC, with a live preview.
- **Border**: the focus glow, with a live preview.
- **Keybinds**: an editable list, checked as you type.

Saving edits `config.toml` in place, keeping your comments, and applies immediately. Bar and
border changes offer a **Restart lattice** button.

Everything lives in `%APPDATA%\lattice\config.toml`, which is written on first run. Every
option is documented in that file, and [src/default_config.toml](src/default_config.toml) is
the same file. **Win+Ctrl+Comma** opens it in your editor, and **Win+Shift+R** reloads it.

The log is at `%APPDATA%\lattice\lattice.log`. Set `LATTICE_LOG=debug` for more detail.

### Tiling styles

| Layout | Behaviour |
|---|---|
| `dwindle` | Each new window takes half of the remaining space. The split runs along the longer side, so windows spiral inward. With `new_window = "after_focused"` a new window splits the focused one, as in Hyprland. `split_ratio` sets the size of the first half. |
| `master` | One master window on the left (`master_ratio` of the width). All other windows stack evenly on the right. |
| `monocle` | Every window fills the area. Only the focused one is visible, and focus left/right cycles through them. |

`resize left|right|up|down` (Win+Shift+`,` `.` `;` `'`) expands the focused window that way
by moving the split on that side. A window already at the screen edge on that side shrinks
instead, as its opposite edge moves the same way. In `master`, left/right move the master
column's edge and up/down do nothing; `monocle` has nothing to resize. Each dwindle split
remembers its own size until the window count changes it or you reload.

Settings shared by all layouts:

- `gaps_inner`: gap between windows.
- `gaps_outer`: gap at the screen edges.
- `float_unresizable`: dialogs and windows that can't be resized float instead of tiling.
- `[[rules]]`: float, ignore or pin windows to a workspace, matched by exe, window class or
  title. Run `lattice --list` to find these values. `repaint = true` fixes apps that come back
  black after a workspace switch; Steam has it by default.

### App workspaces

Send an app to a workspace every time it opens, e.g. Discord to workspace 3:

```toml
[[rules]]
exe = "Discord.exe"
workspace = 3
```

Or add it on the **Apps** tab of the settings window (Win+Shift+Comma), which edits these rules
for you (the program's file name, optionally text in its title, and a workspace) and leaves
your other rules alone. The window opens on that workspace without switching you to it. Rules
apply to new windows and to windows already open when lattice starts; when you save or reload,
open windows move only if their app's rule is new or changed, so a window you moved by hand
stays put.

### The bar

The bar shows your workspaces with the icons of the apps on each. Set `app_display` to
`"names"` or `"both"` to show names instead of or as well as icons. Click a workspace to switch
to it, and click the layout icon next to the CPU reading to cycle the layout. Choose its font
with `font` (any installed family; empty means the built-in Inter) and its size with `font_size`.

### Focus border

The focused window gets a glowing rounded frame, drawn by Rune on a transparent, click-through
overlay that lattice keeps wrapped around the window. Set it up in `[border]`: `color`,
`width`, `radius` (Windows 11 windows use 8), `glow` (blur size in pixels, 0 for a flat line)
and `glow_opacity`.

### Launcher

Win+Space opens a search box in the middle of the focused monitor. It lists every app in the
Start menu's "All apps" (desktop and Store apps), matched fuzzily as you type: `vsc` finds
Visual Studio Code, `term` finds Terminal. Apps you launch often rise to the top, and with an
empty search the list shows them first. ↑/↓ (or Ctrl+N / Ctrl+P) pick, Enter opens, Escape
clears the search and then closes; clicking elsewhere closes it too. It wears the bar's colours
and font; set its `width`, the number of `max_results`, or turn it off in `[launcher]`. The
launch counts live in `%APPDATA%\lattice\launcher-history.tsv`.

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
| Win + Space | Open / close the app launcher |
| Win + \\ | Cycle layout (dwindle → master → monocle) |
| Win + Shift + . / , | Expand the window right / left |
| Win + Shift + ; / ' | Expand the window up / down |
| Win + Shift + Space | Toggle floating |
| Win + F | Toggle fullscreen (fills the tiling area) |
| Win + Q / Win + W | Close the window |
| Win + Enter | Open Windows Terminal (`exec wt.exe`) |
| Win + Alt + , | Open the settings window |
| Win + Ctrl + , | Open config.toml in your editor |
| Win + Shift + R / Win + Shift + E | Reload config / quit |

Your keybinds replace Windows' own shortcuts for the same keys, and the Start menu won't open
when you release Win after a lattice shortcut. **Win+L always locks the PC**, because Windows
doesn't let any program override it. For the same reason the defaults use arrow keys instead
of h/j/k/l.

Mouse: drag a tiled window onto another to swap them, or onto another monitor to move it there.

## How workspaces work

A workspace spans all your monitors. Each window belongs to one workspace and to one monitor
within it. Switching workspaces changes what's on every monitor at once: the old workspace's
windows are hidden with `ShowWindow(SW_HIDE)` and the new one's are shown. Each monitor is
tiled separately with the workspace's layout. Moving a window to another workspace keeps it
on the same monitor, and Win+Shift+O or Win+Shift+arrows move it between monitors.

Hidden windows drop off the taskbar until you switch back. If an app activates a window on a
hidden workspace, for example when you click its notification, lattice switches to that
workspace. Set `follow_activation = false` to turn this off.

lattice never strands windows. It saves the list of windows it has hidden to
`%APPDATA%\lattice\hidden.txt`, shows them all when it quits, and if it was killed or crashed,
shows them again the next time it starts. Monitor changes (rotation, resolution, plugging
monitors in or out) are picked up automatically.

## Development

```powershell
cargo test                 # unit tests (layouts, config, keybinds, settings save round-trip)
cargo clippy --release
cargo run --bin teaser -- --out frames/ --duration 30 --fps 30 --size 1920x1080 --scale 1
```

The last command renders the teaser frames with Rune; `--music music.m4a` writes its
soundtrack.

| File | Role |
|---|---|
| `src/main.rs` | Startup, single-instance check, crash safety, CLI modes |
| `src/hooks.rs` | Keyboard hook (on its own thread), WinEvent hooks, the WM thread's message loop and control window |
| `src/wm.rs` | State machine: windows, workspaces, monitors, commands, events |
| `src/layout.rs` | Pure layout math (unit-tested) |
| `src/win.rs` | Win32 wrappers: window filters, DWM-aware placement, focus, monitors |
| `src/shared.rs` | State shared between the WM thread and the Rune UI, hidden-window bookkeeping |
| `src/bar.rs` | Rune top bar |
| `src/border.rs` | Rune focus-border overlay |
| `src/settings.rs` | Rune settings window; edits config.toml with `toml_edit` |
| `src/icon.rs` | Extracts app icons for the bar |
| `src/launcher/` | The app launcher, self-contained (imports nothing from the rest of lattice): fuzzy ranking and history (`rank.rs`), Start menu catalog, icons and launching (`system.rs`), Rune UI (`ui.rs`) |
| `src/sysinfo.rs` | CPU, memory, battery and clock |
| `src/config.rs` | Config file, keybind and command parsing |
| `build.rs` | Draws the app icon and embeds it in the exe |
| `src/bin/teaser.rs` | The teaser video: Rune animation plus synthesized soundtrack |

## Known limitations

- The bar appears only on the primary monitor. The Windows taskbar stays.
- A window that runs as administrator can be moved only if lattice is also elevated.
- Resizing tiled windows with the mouse isn't supported yet: they snap back. Use Win+= and
  Win+− instead.
- Apps with a large minimum size can't shrink to small tiles and overflow them. Spread windows
  across monitors or workspaces, or use the `master` layout.
