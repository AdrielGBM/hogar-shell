---
id: troubleshooting
kind: guide
title: Troubleshooting
summary: A chip is missing, a panel is empty, a command is refused.
status: stable
compositor: any
commands: [deps, shell, config]
deps: []
see_also: [dependencies, configuration]
---

# Troubleshooting

## Start here

```sh
hogar-shell deps missing     # what is absent, and what each absence costs
hogar-shell shell ping       # is the shell even up
hogar-shell config path      # which config is being read
hogar-shell config check     # what it names that the shell does not have, by file and line
RUST_LOG=debug hogar-shell   # the warnings
```

`deps missing` answers most of this page in one line each, and it is answered by the binary — so it works when
nothing started.

## A chip is missing entirely

Three causes, in order of likelihood:

1. **It is not in a zone.** `[bars.<edge>] start|center|end` — an id not listed is not shown.
2. **Its id is misspelt.** Then something else is there instead: an id no module answers to is drawn where it
   was declared as a placeholder in the error colour — the id written on it along a horizontal bar, a warning
   glyph alone down a vertical one — and pressing it opens the settings window. `hogar-shell config check` names
   it with its file and line; check the spelling against [features/modules](../features/modules/).
3. **Its dependency is absent, and its rule is "hide".** Bluetooth with no BlueZ, battery with no battery,
   media with no player, and the four Hyprland-bound modules on another compositor are all hidden by design.

## A chip is there but empty, or reads unknown

Its source is missing, and the module's rule is "degrade" rather than "hide". `hogar-shell deps missing` names
which. The common ones:

| Empty | Install |
| --- | --- |
| volume, mic, mixer | PipeWire (`pw-dump`, `wpctl`) |
| the visualiser | PipeWire (`libpipewire`) |
| network panel (chip still works) | NetworkManager |
| GPU fields | nothing on AMD/Intel — check `/sys/class/drm`; NVML on NVIDIA |
| external monitor brightness | `ddcutil` |

A reading that cannot be taken reports **unknown**, never zero — so an unknown GPU is not a GPU at 0 %.

## The shell will not start

```sh
hogar-shell deps check
```

Exactly one thing can cause it: no `wlr-layer-shell`. Everything else degrades.

## A command is refused

The reply begins with `err` and says why. Two common ones:

- **`unknown target` / `unknown command`** — `hogar-shell --list` is the authoritative menu.
- **`the session cannot be locked`** — either `ext-session-lock` is missing or libpam could not be loaded. That
  is checked *before* the screen is covered, on purpose. `hogar-shell lock status` gives the same answer without
  locking.

## A keybind does nothing

Check the command by hand first — `hogar-shell launcher toggle` in a terminal. If that works, the bind is the
problem, not the shell. If you bound it through the portal, remember the name is `:launcher`, not
`hogar-shell:launcher`, on a non-sandboxed install.

## The config changed and nothing happened

A save that changes `config.toml`, `tokens.toml` or a monitor override reloads it; a save that leaves the files
holding what they held reloads nothing. If a change did not show:

- **Check the problems notice** — a file that does not parse is not applied, and the notice says so until it
  loads again. `hogar-shell config check` gives the line.
- **Check the file being read** — `hogar-shell config path`. A per-monitor file at
  `monitors/<output>/config.toml` overrides the global one for that screen.
- **Check the key exists** — `hogar-shell config schema <section>` prints every real key. An unknown key is
  ignored silently.
- **Force it** — `hogar-shell shell reload` rebuilds everything whatever the files hold, which is also how a
  font or an icon theme installed since reaches the shell.

`hogar-shell config check` reports the *ids* and names the shell does not have — modules, dashboard pages,
utilities toggles, status icons, themes, accents and colour tokens — a corner module no bar is there to draw, and
a section a monitor override may not set, but not an unknown *key* yet: a misspelt key is still ignored silently.

## An external monitor's brightness is out of step

Levels are read once at detection and tracked optimistically, because a `getvcp` costs tens to hundreds of
milliseconds per monitor. A change made with the monitor's own buttons is not noticed.

```sh
hogar-shell brightness refresh
```

## The tray is empty

hogar-shell is the tray host. An empty tray usually means no application has published an item yet — or that
another shell already owns `org.kde.StatusNotifierWatcher`, in which case hogar-shell reads that watcher's list
instead and the two are consistent.

## Bluetooth will not pair

The shell registers no `org.bluez.Agent1`, so a device asking for a PIN or a passkey confirmation cannot
complete pairing here. Pair it once with `bluetoothctl`; everything afterwards works from the panel.

## Something is slow, or the shell is warm at idle

```sh
TELAR_PERF=1 hogar-shell     # per-phase frame timing
```

An idle shell should sit at 0 % CPU. The one service that publishes at a frame rate is the visualiser, and it
only runs while something is subscribed to it.

## Related

- [Dependencies](../getting-started/dependencies.md) — the contract, and how probes work.
- [Configuration](../getting-started/configuration.md).
