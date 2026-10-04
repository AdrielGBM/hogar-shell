---
id: per-monitor
kind: guide
title: Per-monitor setup
summary: Different bars, wallpapers and workspaces on different screens.
status: stable
compositor: any
config: [background, workspaces]
commands: [shell, layout, wallpaper, brightness]
deps: []
see_also: [bars, wallpaper, configuration, compositor-rules]
---

# Per-monitor setup

## Finding your connector names

```sh
hogar-shell shell outputs     # names
hogar-shell shell screens     # with mode, scale and make
```

Everything below matches on the connector name — `DP-2`, `HDMI-A-1`, `eDP-1`.

## A different arrangement per screen

*Which* bars a screen has, and what is on them, is an `OutputRule` in the active layout, not a per-monitor
config file:

```toml
# ~/.config/hogar-shell/layouts/custom.toml, or whichever layout `hogar-shell layout list` marks as drawing
[[outputs]]
match = "*"
# the base every output starts from

[[outputs]]
match = "DP-2"
# refines it for this connector only

[outputs.layers.top]
areas = [
  { id = "top-bar", kind = "bar", edge = "top", groups = [
      { id = "start", place = "zone", zone = "start", children = [
          { id = "workspaces", module = "workspaces" },
      ] },
  ] },
]
```

Output rules apply in file order, most-specific glob last, so a `*` rule is the base and a named monitor
refines it — merged **by area id**, so a monitor rule says only what it changes rather than repeating the
whole bar. `hogar-shell layout show` prints a layout as it is stored, and `hogar-shell layout check` reports
what is wrong with one before it is drawn. See the [Layout reference](../reference/layout.md) for `OutputRule`
and every area kind, and [Bars](../features/surfaces/bars.md) for the bar area itself.

## No bars at all on a screen

An output rule that places no bar area gives that screen none. There is no `excluded_screens`-style switch
any more — an empty `OutputRule.layers.top` (or one that never matches the output) is what an excluded screen
looks like.

## Monitor override files

`~/.config/hogar-shell/monitors/<output>/config.toml` still exists, with the same shape as the global
`config.toml`, but it is narrower than it used to be: what used to place a screen's bars and widgets is the
layout now, so a monitor override is for **behaviour and theme**, not placement — a lower `[theme] radius`, a
different `[theme]` accent, or a wallpaper set below.

```toml
# monitors/DP-2/config.toml
[theme]
accent = "red"
```

A few sections are global-only — the ones that describe the shell rather than a screen. `config schema` is the
place to check when in doubt. Writing a layout key here — the old `bars`, `corners` or `widgets` sections and
the rest — is a validation error naming the layout file to write it in instead; see `hogar-shell config check`.

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

Layer windows are created for outputs as they appear and released when they go. An `OutputRule` for a screen
that is not connected simply does not apply, the same as a monitor override for one.

## Known limit

hogar-shell **reads** outputs and never writes them: resolution, refresh rate, scale and arrangement are your
compositor's business. `zwlr-output-management` is unbound.

## Related

- [Bars](../features/surfaces/bars.md), [Wallpaper layer](../features/surfaces/wallpaper.md).
- [Layout reference](../reference/layout.md) — `OutputRule`, `WorkspaceRule` and every area kind.
- [Compositor rules](compositor-rules.md) — what a `layer_rule` reaches on a layout with several outputs.
