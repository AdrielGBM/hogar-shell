---
id: first-run
kind: guide
title: First run
summary: What the first start writes, and the first three things worth changing.
status: stable
compositor: any
config: [general, bars, theme]
commands: [config, scheme]
see_also: [install, configuration]
---

# First run

The first start writes an annotated `~/.config/hogar-shell/config.toml` and puts a bar on screen. Nothing else is
created until something needs it.

## What is on screen

A top bar with a default set of modules, the wallpaper layer, and nothing else. Panels, the launcher, the OSD
and the toast stack are surfaces that exist only while they are open — an idle session carries no overlay.

## The first three changes

**1. Put the modules you want on the bar.** `[bars.top]` has three zones — `start`, `center`, `end` — each a
list of module ids:

```toml
[bars.top]
start  = ["workspaces", "activewindow"]
center = ["clock"]
end    = ["statusicons", "tray", "battery", "session"]
```

Every id is a page under [features/modules](../features/modules/). An id the build does not know is drawn as a
placeholder where you wrote it rather than failing the bar, and `hogar-shell config check` says which one.

**2. Pick a shape.** `[shape] mode` is `bar`, `sections` or `chips` — one solid bar, grouped zones, or a chip
per module. Every module works in all three; see [Bars](../features/surfaces/bars.md).

**3. Pick a palette.**

```sh
hogar-shell scheme list           # what `scheme set` accepts
hogar-shell scheme set dynamic    # derive one from the current wallpaper
```

See [Palettes](../features/theming/palettes.md) and [Dynamic scheme](../features/theming/dynamic-scheme.md).

## Getting a file with every key in it

The annotated starter is deliberately short. To edit down from the full set instead:

```sh
hogar-shell config schema > ~/.config/hogar-shell/config.toml
```

`config schema` prints a complete, valid config with every key, its default and its explanation — generated
from the source, so it is never out of date with the build you are running.

## Or use the settings application

```sh
hogar-shell panel toggle settings
```

Twelve pages, a nav pane and a search box over every key — including the ones no form displays. It writes back
to `config.toml` non-destructively, preserving your comments and ordering.
