---
id: per-monitor
kind: guide
title: Per-monitor setup
summary: Different bars, wallpapers and workspaces on different screens.
status: stable
compositor: any
config: [bars, background, workspaces]
commands: [shell, wallpaper, brightness]
deps: []
see_also: [bars, wallpaper, configuration]
---

# Per-monitor setup

## Finding your connector names

```sh
hogar-shell shell outputs     # names
hogar-shell shell screens     # with mode, scale and make
```

Everything below matches on the connector name — `DP-2`, `HDMI-A-1`, `eDP-1`.

## A different bar per screen

Per-monitor overrides live in a file of their own, with **the same shape as the global config**:

```
~/.config/hogar-shell/monitors/DP-2/config.toml
```

```toml
# monitors/DP-2/config.toml
[bars.top]
start  = ["workspaces"]
center = []
end    = ["clock"]
```

It is merged over the global file, so you write only the differences. There is nothing new to learn: if you
know where a key goes in `config.toml`, you know where it goes here.

A few sections are global-only — the ones that describe the shell rather than a screen. `config schema` is the
place to check when in doubt.

## No bars at all on a screen

```toml
[bars]
excluded_screens = ["HDMI-*"]
```

Matched as `*` patterns, so `HDMI-*` covers a port whose index moves between reboots. An output the compositor
gave no name to is never excluded — there would be nothing to match it by, and dropping the bars off an
unnameable screen would look like a bug.

## Wallpapers

```toml
[background.monitors]
DP-2  = "~/pictures/wide.jpg"
eDP-1 = "~/pictures/laptop.jpg"
```

Or at runtime, which writes to `state.json` rather than to your config:

```sh
hogar-shell wallpaper set ~/pictures/x.jpg DP-2
hogar-shell wallpaper random DP-2
hogar-shell wallpaper clear DP-2      # back to what `[background.monitors]` says
```

## Workspaces

`[workspaces] per_monitor` decides whether a bar shows every workspace or only the ones on its own screen. On a
multi-monitor desk it is usually the first thing to turn on.

## Brightness

```sh
hogar-shell brightness list           # every controllable display
hogar-shell brightness up DP-2
hogar-shell brightness up all
hogar-shell brightness refresh        # after plugging one in
```

An unnamed target means the **primary panel**, not every screen — see
[brightness](../features/modules/brightness.md).

## Hotplug

Surfaces are created for outputs as they appear and released when they go. A per-monitor config for a screen
that is not connected simply does not apply.

## Known limit

hogar-shell **reads** outputs and never writes them: resolution, refresh rate, scale and arrangement are your
compositor's business. `zwlr-output-management` is unbound.

## Related

- [Bars](../features/surfaces/bars.md), [Wallpaper layer](../features/surfaces/wallpaper.md).
