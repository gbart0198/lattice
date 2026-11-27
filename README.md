# lattice

A tiling window manager for Windows 11 with Hyprland-style workspaces, a glowing focus border
and a top bar, with all of its UI drawn by [Rune](https://github.com/anomalyco/rune).

[Watch the 30-second teaser](docs/lattice-teaser.mp4)

- **Auto-tiling**: new app windows snap into place as soon as they appear.
- **Layouts**: `dwindle` (Hyprland's spiral, the default), `master` (master + stack) and
  `monocle`, chosen per workspace.
- **Workspaces** (1–10) that span all your monitors and switch instantly. They don't use
  Windows virtual desktops, so there's no Windows animation (lattice's own is optional).
- **Keybinds** for every action, with the Windows key as the modifier.
- **Top bar**: workspaces with the icons of the apps on each, the focused window's title, a
  layout button, CPU, memory, battery and the clock, in any installed font.
- **Focus border**: a soft glowing frame around the focused window.
- **App launcher** (Win+Space): a Raycast-style search over every app in the Start menu,
  Store apps included. Type a few letters, press Enter. Type `f` and a space to fuzzy-search
  files and folders instead: Enter opens, Ctrl+Enter shows a file in its folder or copies a
  folder's path (both configurable). It uses [Everything](https://www.voidtools.com) when it's
  running, else lattice's own background index of your user folder.
- **Window search** (Win+J): fuzzy-search the titles and apps of every open window, on every
  workspace and monitor, most recently used first. Enter switches straight to it.
- **Command mode**: type `>` and a space in the launcher to run commands: layouts, workspaces,
  float, close, reload, and system ones (lock, sleep, restart, shut down, sign out, audio
  output, Wi-Fi).
- **Settings window** (Win+Alt+Comma) for everything above.
- **Scripting**: `lattice msg` runs any command from a terminal or script, `lattice query`
  prints workspaces, windows and monitors as JSON, and `[[hook]]` entries run your commands
  when you switch workspace, focus or open a window, and more. See [Scripting](#scripting).
- **Guided tour**: on the first start, a spotlight walks you over your real desktop (the bar,
  workspaces, layouts, the launcher and its file, command and window searches) and waits for you to try each key. Replay it with
  Win+Shift+/.

## Install

Download `lattice-setup-<version>.exe` from the
[latest release](https://github.com/gbart0198/lattice/releases/latest) and run it. It installs
for your account only (no admin rights) to `%LOCALAPPDATA%\Programs\lattice`, adds a Start
menu entry, puts the [`lattice` command](#command-line) on your PATH, and can start lattice
when you sign in. Run a newer setup to upgrade: it quits the
running lattice cleanly first. Uninstall from **Settings > Apps > Installed apps**.

The installer isn't code-signed yet, so SmartScreen may warn about it: choose
**More info > Run anyway**.

Silent install for scripts: `lattice-setup-<version>.exe /VERYSILENT /TASKS=autostart`.

### Updates

Once a day lattice asks GitHub whether a newer release is out. If one is, a small notice
appears at the bottom right of the screen, above your windows. **Ignore** hides it until the
next release; **Update & restart** downloads the new installer, checks it against the
SHA-256 digest GitHub publishes for it, and runs it; lattice restarts on the new
version a few seconds later. Nothing is downloaded until you click. Preview the notice with `lattice --update-preview`.
**Settings > General >
Updates** has **Check now** and turns the daily check off (`check_for_updates = false` under
`[general]`). Only the installed copy updates itself; a build run from source never does.

## Building from source


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
cargo run --release -- --tour --tour-preview   # the guided tour over a sample desktop
cargo run --release -- --launcher-bench         # time launcher opens and keystrokes (takes the focus)
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
so no admin rights are needed. It also adds a Start menu shortcut, an entry under
**Settings > Apps > Installed apps**, and that folder to your PATH for the
[`lattice` command](#command-line). Re-run it after pulling or changing the code to update
the installed copy: it stops the running lattice cleanly first.

If PowerShell refuses to run the script, allow local scripts for your account once:

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

To uninstall, use **Installed apps > lattice**, or run
`%LOCALAPPDATA%\Programs\lattice\uninstall.ps1`. Your config is kept unless you pass
`-RemoveConfig`.

### 5. Build the installer

```powershell
winget install JRSoftware.InnoSetup   # once
.\installer\build.ps1                 # builds, then writes dist\lattice-setup-<version>.exe
```

The setup's version comes from `Cargo.toml`. Pushing a `v*` tag runs
`.github/workflows/release.yml`, which builds the installer on GitHub and attaches it to that
release.

**Apps running as administrator**: Windows only lets lattice move them if lattice is elevated
too. To start it elevated at sign-in, use a scheduled task instead of `-Autostart`:

```powershell
schtasks /Create /TN lattice /TR "\"$env:LOCALAPPDATA\Programs\lattice\lattice.exe\"" /SC ONLOGON /RL HIGHEST
```

## Configuration

Press **Win+Alt+Comma** to open the settings window. It floats above everything and is never
tiled, with these tabs:

- **General**: layouts, gaps, workspaces, animations and behavior.
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

### Animations

Turn on **Animate windows** in the settings window (or `enabled = true` under `[animations]`)
and windows glide instead of jumping: when you move or swap them, go fullscreen and back, and
when the others close up around a window you minimize (or make room when it comes back).
Switching workspaces slides the old one off the top of every screen and the new one up from
below; going to a lower workspace runs the other way. Two styles:

- `style = "fancy"`: about a third of a second, settling with a slight overshoot, and
  workspaces cascade in one window after another.
- `style = "fast"`: about half that, with a plain ease-out.

Changes apply as soon as you save: no restart. The compositor does the animating, so every
animation runs at your display's refresh rate however heavy the app: while a window animates,
lattice makes it transparent (it keeps drawing) and Windows' own live preview of it glides
across the screen. A window changing tiles is moved once, near the end, never step by step.
A window lattice can't animate this way (one running as administrator, say) just moves at
once. Lattice opens no GPU device for any of this, and the previews show the surfaces apps
already have; while nothing moves nothing exists, and with animations off nothing runs.
`lattice --animation-preview swap` (or `move`, `workspace`, `fullscreen`, `minimize`; add
`fast` for that style) plays one over demo windows without touching your desktop.

### Guided tour

The first time lattice starts, it dims the screen and walks you through it with a spotlight on
the real thing: the bar, the workspace pills, the focused window, the layout button, the stats
and the launcher. With the launcher open, it shows each of its modes in turn: apps, file search
(`f` and a space), command mode (`>` and a space) and window search (Win+J). Steps that teach a key wait for you to press it (Win+2, Win+\\, Win+Space)
and tick off when you do, then wait for **Next** so you can read what happened (the launcher
step moves straight on, to point at the launcher); each also has **Next**, **Back** and **Skip tour**. **Escape ends it**
whatever has focus (except while the launcher is open, where Escape closes the launcher). The
overlay never takes the keyboard from your apps, and clicks outside its card go through to the
desktop.

- Replay it with **Win+Shift+/** (the `tour` command), or run `lattice --tour`. Running either
  while the tour shows ends it.
- It shows once, then writes `%APPDATA%\lattice\tour-seen`. That includes upgrades from a
  version before the tour, so existing users see it once too. Delete the file to see it again on
  the next start.
- Configs written before 0.6 don't have the Win+Shift+/ bind. Add
  `"super+shift+slash" = "tour"` under `[keybinds]`, or run `lattice --tour`.
- `lattice --tour --tour-preview` shows it in a window over a drawn sample desktop, without
  lattice running.

The tour runs as its own short-lived process: its translucent overlay needs a GPU device (tens
of MB), which goes away with it instead of staying in lattice. It asks the running lattice
where things are and what you did through lattice's hidden control window.

### Launcher

Win+Space opens a search box in the middle of the focused monitor. It lists every app in the
Start menu's "All apps" (desktop and Store apps), matched fuzzily as you type: `vsc` finds
Visual Studio Code, `term` finds Terminal. Apps you launch often rise to the top, and with an
empty search the list shows them first. ↑/↓ (or Ctrl+N / Ctrl+P) pick, Enter opens, Escape
clears the search and then closes; clicking elsewhere closes it too. It wears the bar's colours
and font; set its `width`, the number of `max_results`, or turn it off in `[launcher]`. The
launch counts live in `%APPDATA%\lattice\launcher-history.tsv`.

Win+J opens it searching windows instead: every window lattice manages, on any workspace or
monitor, floating and minimized ones included, matched on its title and its app's name. With
an empty search they're listed most recently used first, the one you're in last, so Win+J then
Enter flips back to the previous window, as Alt+Tab does. Enter switches to the window's
workspace and focuses it (restoring it if it's minimized). In the app search, `w` and a space
does the same (`window_prefix` in `[launcher]`); Backspace on an empty field goes back to apps.

Type `>` and a space for command mode (`command_prefix` in `[launcher]`). The list is fuzzy
matched like the app search, and arguments are part of each command's name, so `> move 3`
finds "Move window to workspace 3", `> layout mono` finds "Layout: Monocle" and `> ws 2` finds
"Go to workspace 2". Enter runs the highlighted one. The commands:

- lattice: Layout: Dwindle / Master / Monocle, Toggle floating, Toggle fullscreen, Close
  window, Go to workspace N, Move window to workspace N (and follow it), Retile, Reload
  config, Open settings, Open config file, Quit lattice. Window commands act on the window
  you were in before opening the launcher.
- System: Lock, Sleep, Hibernate (only when hibernation is enabled), Restart, Shut down, Sign
  out. **Restart, Shut down and Sign out take a second Enter**: the first only asks, and the
  row says what the next one does (Escape or moving the selection cancels). Apps still get to
  ask about unsaved work; nothing is forced.
- Audio output: one entry per playback device ("Audio output: Headphones"), which makes it the
  default for everything, as Windows' sound settings do.
- Wi-Fi: Turn Wi-Fi on / off, and "Connect to Wi-Fi: <name>" for each saved network (the ones
  in range when Windows lets lattice see them, which on Windows 11 needs location access for
  desktop apps; otherwise every saved network is listed).

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
| Win + J | Search open windows and jump to one |
| Win + \\ | Cycle layout (dwindle → master → monocle) |
| Win + Shift + . / , | Expand the window right / left |
| Win + Shift + ; / ' | Expand the window up / down |
| Win + Shift + Space | Toggle floating |
| Win + F | Toggle fullscreen (fills the tiling area) |
| Win + Q / Win + W | Close the window |
| Win + Enter | Open Windows Terminal (`exec wt.exe`) |
| Win + Alt + , | Open the settings window |
| Win + Shift + / | Replay the guided tour (Escape ends it) |
| Win + Ctrl + , | Open config.toml in your editor |
| Win + Shift + R / Win + Shift + E | Reload config / quit |

Your keybinds replace Windows' own shortcuts for the same keys, and the Start menu won't open
when you release Win after a lattice shortcut. **Win+L always locks the PC**, because Windows
doesn't let any program override it. For the same reason the defaults use arrow keys instead
of h/j/k/l.

Mouse: drag a tiled window onto another to swap them, or onto another monitor to move it there.

## Scripting

You can drive lattice from outside in two ways. The `lattice` command sends commands to the
running window manager, asks it what's where, and streams events as they happen. `[[hook]]`
entries in `config.toml` run your own commands when something changes. Between them you can
build a status line, rearrange windows from a script, or react to a workspace switch.

### Command line

The installer puts lattice's folder on your PATH, so a new terminal has `lattice` (it runs
`lattice.com`, a small console front end that waits for `lattice.exe` and passes back its
output and exit code). These commands talk to the copy that's already running:

| Command | What it does |
|---|---|
| `lattice msg <command>` | Runs one command, as if you'd pressed its keybind |
| `lattice query <what>` | Prints JSON: `workspaces`, `windows`, `monitors`, `focused`, or `state` for all four |
| `lattice subscribe [event...]` | Prints one JSON line per event until lattice exits or you press Ctrl+C. Name events to get only those |

```powershell
lattice msg workspace 3
lattice msg move-to-workspace-silent 2
lattice msg exec wt.exe -d C:\src
lattice msg exec notepad "C:\notes\to do.txt"
lattice query focused
(lattice query windows | ConvertFrom-Json) | Where-Object exe -eq 'Code.exe'
lattice subscribe workspace-changed focus-changed
```

`lattice msg` takes the same commands as keybinds. Its words are joined back into one command
line, and a word with spaces in it keeps its quotes, as with the file path above. You can also
pass the whole line as one quoted argument (`lattice msg "workspace 3"`), which is used as
written.

| Command | Effect |
|---|---|
| `focus left\|right\|up\|down` | Focus the window that way (crosses monitors) |
| `move-window left\|right\|up\|down` | Swap the window that way, or move it to the next monitor |
| `workspace N\|next\|prev` | Switch workspace |
| `move-to-workspace N\|next\|prev` | Move the focused window there and follow it |
| `move-to-workspace-silent N\|next\|prev` | Move the focused window there and stay |
| `focus-monitor next\|prev` / `move-to-monitor next\|prev` | Focus the next monitor / move the window there |
| `layout dwindle\|master\|monocle\|next` | Set or cycle the active workspace's layout |
| `resize left\|right\|up\|down [amount]` | Expand the window that way (`amount`: share of the split, default 0.05) |
| `toggle-float` / `toggle-fullscreen` / `close` | Act on the focused window |
| `retile` / `reload` / `quit` | Re-tile everything / reload the config / quit lattice |
| `exec <command line>` | Start a program, as the Run dialog would |
| `launcher` / `window-search` / `settings` / `open-config` / `tour` | Open those windows |

Commands go through the same queue as your keypresses, in order, so a `query` straight after a
`msg` sees its result. `exec` starts the program from lattice, with lattice's environment and
working folder, not your terminal's.

**What queries return.** Workspaces are numbered from 1, as `workspace N` takes them. Monitors
are numbered from 0, left to right. Window handles (`hwnd`) are decimal numbers; `lattice
--list` prints the same handles in hex. Rectangles are
`{"x", "y", "width", "height"}` in physical pixels.

- `workspaces`: one object per workspace: `index`, `active`, `layout`, `windows` (how many),
  `monitors` (which monitors hold its windows), `focused` (the window last focused there) and
  `fullscreen` (a handle, or `null`).
- `windows`: every window lattice manages, by workspace and then tiling order: `hwnd`, `title`,
  `exe`, `class`, `workspace`, `monitor`, `floating`, `minimized`, `fullscreen`, `focused` and
  `rect`.
- `monitors`: `index`, `primary`, `focused`, `rect`, `work_area` (without the taskbar) and
  `scale` (1.5 means 150%).
- `focused`: `workspace`, `monitor`, `layout` and `window` (the focused window's object as in
  `windows`, or `null`).
- `state`: `{"workspaces", "windows", "monitors", "focused"}` in one go.

**Exit codes.** 0 on success, 1 if lattice isn't running or refused the request (the reason goes
to stderr), and 2 for a mistake in the arguments, such as an unknown command or query. A
lattice running as administrator only takes requests from an elevated terminal.

### Hook events

A `[[hook]]` runs a command line every time one of its events happens:

```toml
[[hook]]
on = "workspace-changed"
run = "powershell -NoProfile -File %USERPROFILE%\\scripts\\on-workspace.ps1"

[[hook]]
on = ["window-opened", "window-closed"]
run = "echo %LATTICE_EVENT% %LATTICE_EXE% >> %TEMP%\\lattice-windows.log"
```

`on` is one event name, a list of them, or `"*"` for every event. `run` goes through
`cmd /C`, so `%VARIABLES%`, `&&` and `>>` work. Nothing flashes up: hooks run without a
console window. They run in the background, each on its own, so a slow script never holds up
tiling; lattice doesn't wait for one to finish before starting the next. A hook gets its event
two ways:

- as environment variables: `LATTICE_EVENT` with the event name, and `LATTICE_<FIELD>` for each
  field of that event in the table below (`LATTICE_WORKSPACE`, `LATTICE_EXE`, ...), and no
  others. `true`/`false` stay as words. In `cmd`, a variable that isn't set prints as its own
  name (`%LATTICE_LAYOUT%`), so a hook on several events should read only the fields they share.
- as one line of JSON on stdin, the same object `lattice subscribe` prints:
  `{"event": "workspace-changed", "workspace": 2, "previous_workspace": 1}`.

Hooks are checked when the config loads: a misspelled event is reported in the log and only
that hook is skipped. Reloading the config (Win+Shift+R) picks up changed hooks. When a hook
exits with an error, the log says so. A hook that runs `lattice msg` triggers events of its
own, so take care not to set off the same hook in a loop.

| Event | When | Fields |
|---|---|---|
| `startup` | lattice has started and tiled your windows | `workspace`, `version` |
| `config-reloaded` | The config was reloaded (Win+Shift+R, or saving in settings) | none |
| `workspace-changed` | You switched workspace | `workspace`, `previous_workspace` |
| `layout-changed` | The layout of the workspace on screen changed | `workspace`, `layout`, `previous_layout` |
| `focus-changed` | Another managed window got the focus | the window's fields |
| `window-opened` | lattice started managing a window | the window's fields |
| `window-closed` | A window closed, or lattice stopped managing it | the window's fields, as they last were |
| `window-moved` | A window went to another workspace or monitor | the window's fields, `previous_workspace`, `previous_monitor` |
| `monitor-changed` | The focus moved to another monitor | `monitor`, `previous_monitor`, `workspace` |
| `display-changed` | Monitors were added, removed, moved or resized | `monitors` (how many there are now) |

A window's fields are `hwnd`, `title`, `exe`, `class`, `workspace`, `monitor` and `floating`,
numbered as in queries.

A few things to know about timing:

- lattice compares its state before and after each burst of activity, so one action reports
  each change once. Switching workspace with a keybind gives `workspace-changed`, then
  `focus-changed` for the window you land on.
- Focus changes wait 100 ms for the focus to settle: Alt+Tabbing past five windows reports
  only the one you stop on. Focus passing briefly through the bar or the launcher and back
  isn't reported.
- Within one burst, windows opening, closing and moving come first, then workspace, layout,
  display and monitor changes, then focus.
- Changing a window's title doesn't count as an event; query `windows` when you need the
  current titles.

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
| `src/ipc.rs` | `lattice msg`, `query` and `subscribe`: the `WM_COPYDATA` protocol, both ends, and the JSON state queries return |
| `src/events.rs` | Script events: diffs WM snapshots into events, runs `[[hook]]` commands and feeds subscribers from its own thread |
| `src/bin/lattice-console.rs` | `lattice.com`, the console front end that lets terminals wait for `lattice.exe` |
| `src/hooks.rs` | Keyboard hook (on its own thread), WinEvent hooks, the WM thread's message loop and control window |
| `src/wm.rs` | State machine: windows, workspaces, monitors, commands, events |
| `src/layout.rs` | Pure layout math (unit-tested) |
| `src/win.rs` | Win32 wrappers: window filters, DWM-aware placement, focus, monitors |
| `src/shared.rs` | State shared between the WM thread and the Rune UI, hidden-window bookkeeping |
| `src/bar.rs` | Rune top bar |
| `src/border.rs` | Rune focus-border overlay |
| `src/anim/` | Window animations: easing curves, the animator and its vsync frame clock (`mod.rs`), the live-preview overlay (`dwm.rs`), and the `--animation-preview` demo (`preview.rs`) |
| `src/settings.rs` | Rune settings window; edits config.toml with `toml_edit` |
| `src/tour/` | The guided tour (`--tour`, its own process): steps and lattice queries (`mod.rs`), spotlight and card (`view.rs`), sample-desktop preview and headless scenes (`preview.rs`) |
| `src/icon.rs` | Extracts app icons for the bar |
| `src/launcher/` | The app launcher, self-contained (imports nothing from the rest of lattice): fuzzy ranking and history (`rank.rs`), Start menu catalog, icons and launching (`system.rs`), window search ranking (`windows.rs`), command mode's catalog (`commands.rs`) and its power, audio and Wi-Fi actions (`control.rs`), Rune UI (`ui.rs`) |
| `src/sysinfo.rs` | CPU, memory, battery and clock |
| `src/config.rs` | Config file, keybind, command and hook parsing |
| `build.rs` | Draws the app icon and embeds it in the exe |
| `src/bin/teaser.rs` | The teaser video: Rune animation plus synthesized soundtrack |

## Known limitations

- The bar appears only on the primary monitor. The Windows taskbar stays.
- A window that runs as administrator can be moved only if lattice is also elevated.
- Resizing tiled windows with the mouse isn't supported yet: they snap back. Use Win+= and
  Win+− instead.
- Apps with a large minimum size can't shrink to small tiles and overflow them. Spread windows
  across monitors or workspaces, or use the `master` layout.
