# hogar-shell

A Wayland desktop shell in Rust — bars, panels, launcher, dashboard, lock screen, notifications, capture and
dynamic theming — built on [`telar`](https://github.com/AdrielGBM/telar) and `wlr-layer-shell`, configured in
TOML.

It targets Hyprland but prefers Wayland protocols to compositor IPC wherever both exist, so most of it works
anywhere `wlr-layer-shell`, `ext-session-lock` and `ext-idle-notify` do. Workspaces and the focused window are
read over `ext-workspace-v1` and `wlr-foreign-toplevel-management` rather than off Hyprland's socket. What
stays Hyprland-only is what no protocol carries: a window's geometry, its workspace and its process id, and
therefore the window-info panel, the window count behind a workspace pill and the scratchpads.

## What it does

**Bars.** One per screen edge, all four at once if you like, on every monitor. Three shapes — a single `bar`,
grouped `sections`, or individual `chips` — with per-monitor overrides.

**Modules.** `activewindow` `battery` `bluetooth` `brightness` `clock` `cpu` `dashboard` `gpu` `kblayout`
`launcher` `lockstatus` `logo` `media` `memory` `mic` `netspeed` `network` `notes` `notifications` `session`
`settings` `spacer` `statusicons` `temperature` `tray` `utilities` `volume` `windowinfo` `workspaces`.

**Surfaces.** Drawers and floats anchored to the chip that opened them, hover popouts, OSDs, a full-screen
launcher, a notification centre, a session-lock screen, a per-monitor wallpaper layer, and in-shell toasts.

**Services.** Audio (PipeWire), network (NetworkManager), Bluetooth (BlueZ), battery (UPower), notifications
(the freedesktop spec), tray (StatusNotifierItem), MPRIS, weather, GPU, brightness (sysfs and DDC/CI), idle and
lock. Each is one producer for the whole process, however many surfaces subscribe.

**Theming.** Built-in palettes, or a scheme derived from the current wallpaper in OkLCH with a WCAG-AA contrast
pass over the result.

## Install

```sh
git clone https://github.com/AdrielGBM/hogar-shell
cd hogar-shell
cargo build --release        # target/release/hogar-shell
```

One clone is enough. Building needs Rust 1.95 or newer and `libxkbcommon` — the only library the binary links
besides glibc, so its development files have to be present. Everything under [Dependencies](#dependencies) is
reached at runtime and missing gracefully; `hogar-shell deps` reports which of them this machine actually has.

Start it from your compositor:

```ini
# ~/.config/hypr/hyprland.conf
exec-once = hogar-shell
```

The first run writes an annotated `~/.config/hogar-shell/config.toml`.

## Configuring

Two commands answer everything, from the build you are running rather than from a document that can drift:

```sh
hogar-shell config schema          # every section and key, annotated from the source's own doc comments
hogar-shell config schema <name>   # just one section
hogar-shell --list                 # every command, target and argument
```

`config schema` prints a complete, valid `config.toml`, so `hogar-shell config schema > config.toml` is also how
you get a file with every key in it to edit down.

The same two tables are the manual: [`man/hogar-shell.1`](man/hogar-shell.1) is the command reference and
[`man/hogar-shell.5`](man/hogar-shell.5) the configuration one, both generated and checked in.

[`docs/`](docs/README.md) is the prose half — a page per feature saying what it is for, what it needs and how
it behaves without that. It never restates a default or an argument list; those come from the two commands
above.

```sh
man ./man/hogar-shell.5            # or `man 5 hogar-shell` once it is installed
```

Config lives in `~/.config/hogar-shell/`:

| File | What it is |
| --- | --- |
| `config.toml` | everything; hot-reloaded, non-destructively, on save |
| `tokens.toml` | design-token overrides. Deliberately unstable — `[theme]` is the supported surface |
| `monitors/<output>/config.toml` | per-monitor overrides, same shape as the global file |
| `state.json` | runtime state the shell owns (the current wallpaper), not settings |

Or press the gear: the settings module is a full settings application — a nav pane, twelve pages and a search
box over every key, including the ones no form displays.

## Controlling it

Every action is an IPC command, so a keybind is a shell command:

```sh
hogar-shell volume up
hogar-shell launcher toggle
hogar-shell screenshot region
hogar-shell brightness up DP-2
hogar-shell lock on
```

26 targets: `shell` `lock` `idle` `panel` `launcher` `audio` `dashboard` `apps` `notifs` `screenshot` `record`
`toast` `volume` `mic` `weather` `gamemode` `wifi` `vpn` `bluetooth` `media` `session` `keyboard` `brightness`
`wallpaper` `scheme` `config`. Run `hogar-shell --list` for the arguments.

The shell also registers its actions as XDG global shortcuts, so they can be bound from the portal instead —
`hyprctl globalshortcuts` prints the names. See [`docs/guides/keybinds.md`](docs/guides/keybinds.md).

## Dependencies

Nothing below is required to start the shell. Each one is missing gracefully: the module that needs it reports
unknown rather than zero, or does not appear at all.

### Required

| | |
| --- | --- |
| Wayland compositor | with `wlr-layer-shell` |
| `fontconfig` | font discovery |

### Optional, per feature

| Feature | Needs | Without it |
| --- | --- | --- |
| Audio, mic, per-app volume | PipeWire (`pw-dump`, `wpctl`) | the audio modules stay empty |
| Audio visualiser | PipeWire (`libpipewire`, opened at runtime) | the bars stay silent |
| Network | NetworkManager (D-Bus) | the network module reports no connection |
| VPN | NetworkManager | the VPN list is empty |
| Bluetooth | BlueZ (D-Bus) | the Bluetooth module is hidden |
| Battery | UPower (D-Bus), or `/sys/class/power_supply` | the battery chip is hidden |
| Backlight | `/sys/class/backlight` | internal brightness is unavailable |
| External monitor brightness | `ddcutil` | only internal panels are dimmable |
| Notifications | none — hogar-shell *is* the daemon | — |
| Tray | apps speaking StatusNotifierItem | the tray is empty |
| Media | any MPRIS player | the media chip is hidden |
| Screenshot | `ext-image-copy-capture`, or `wlr-screencopy` | capture is unavailable |
| Screen recording | `wf-recorder` or `gpu-screen-recorder` | recording is unavailable |
| Clipboard | `ext-data-control`, or `wlr-data-control` | `[screenshot] copy` does nothing |
| Session lock | `ext-session-lock` + PAM | `lock status` says the session cannot be locked |
| Face unlock | `howdy` | password only |
| Fingerprint unlock | `fprintd` (D-Bus) | password only |
| Idle actions | `ext-idle-notify` | idle timers do not arm |
| Global shortcuts | `xdg-desktop-portal` with GlobalShortcuts | bind the IPC commands directly instead |
| Game mode | `gamemoded` (D-Bus) | the toggle is greyed out |
| GPU readings | NVML (`libnvidia-ml.so`), or `/sys/class/drm` (AMD/Intel) | GPU fields read unknown |
| Launcher calculator | `qalc` | a built-in evaluator handles the common cases |
| Weather | network access | the weather card says so |
| Night light | `wlr-gamma-control` | `nightlight` says the screen cannot be tinted |
| Workspaces | `ext-workspace-v1` | the workspaces module is hidden |
| Active window | `wlr-foreign-toplevel-management` | the chip reads as no window |
| Workspace occupancy, window info, `shell clients` | Hyprland IPC | pills lose their window count and app icons; the window-info panel and the client list are unavailable |

**Two bus names are owned, not consumed:** `org.freedesktop.Notifications` and
`org.kde.StatusNotifierWatcher`. So hogar-shell replaces dunst, mako or swaync rather than running beside one,
and the same goes for any other tray watcher — whichever process claims the name first wins.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

hogar-shell and [`telar`](https://github.com/AdrielGBM/telar) are developed together, and a fix that is agnostic
to this shell belongs upstream rather than worked around here. The dependency is a published version so that a
clone builds on its own; point cargo at a local checkout while you work on both, and take it back out before
committing:

```toml
# Cargo.toml — never commit this block: it makes the build need a second checkout
[patch.crates-io]
telar = { path = "../telar/crates/telar" }
telar-platform-headless = { path = "../telar/crates/platform/platform-headless" }
```

The manual is generated from the command table and the config schema, and `cargo test` fails if the checked-in
copies no longer match this build:

```sh
UPDATE_MAN=1 cargo test -p hogar-shell --lib man    # rewrite man/hogar-shell.{1,5}
```

**`cargo fmt` needs the file list handed to it.** `rsx_modules!` generates the crate's `mod` declarations at
expansion time, and rustfmt walks the module tree from the crate root — so it reaches almost nothing here and
`cargo fmt --check` passes while formatting one file. This is the honest form:

```sh
cargo fmt --check -- $(find apps crates -name '*.rs' -not -path '*/target/*' -not -path '*/.telar/*')
```

(`.telar/build` is the transpiler's own output, not source.)

`cargo fmt -- <files>` also formats more than the files you name, because cargo hands rustfmt each target's
crate root as well. To format an exact set — one new module, without dragging a reformat of unrelated files
into the same commit — call `rustfmt --edition 2024 <files>` directly.

**The build writes into the source tree.** `rsx_modules!` transpiles into `.telar/build/` at macro-expansion
time, so the source directory has to be writable — a build pointed at a read-only path fails with `Failed to
create .telar/build/`. The directory is gitignored and regenerated from the `.rsx` files.

Anything with a look is a `[preview]`, so it renders on every run rather than when an environment variable asks
— by hand as a window or a PNG, and in CI as a measurement:

```sh
cargo telar preview                          # every preview, in a window
cargo telar test                             # every preview built and laid out once
cargo test -p hogar-shell --lib layout_sweep   # all 12 edge × shape combinations, measured
TELAR_LIVE_VISUALISER=1 cargo test -p hogar-shell-services --lib live_capture -- --nocapture
TELAR_PERF=1 hogar-shell                       # per-phase frame timing
```

`layout_sweep` is the regression net: it lays every preview out on all four edges in all three shape modes and
fails on anything that measures to no area — a collapsed viewport clips its contents away while every other
check still passes. It stores no baseline images deliberately, since a stored picture cannot be told apart from
a font that shaped differently on the machine running it. See `apps/hogar-shell/src/layout_sweep.rs` for the two
questions it does *not* ask, and why.

## Licence

MIT or Apache-2.0, at your option — [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
