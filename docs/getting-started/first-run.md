---
id: first-run
kind: guide
title: First run
summary: What the first start writes, and the first three things worth changing.
status: stable
compositor: any
config: [general, theme]
commands: [config, scheme, layout]
see_also: [install, configuration, per-monitor]
---

# First run

The first start writes an annotated `~/.config/hogar-shell/config.toml` and draws the built-in layout, which
puts a bar and a clock face on screen. Nothing else is created until something needs it.

## What is on screen

A top bar with a default set of modules, a clock face in the middle of the desktop, the wallpaper layer, and
nothing else. Panels, the launcher, the OSD
and the toast stack are nodes that exist only while they are open — an idle session carries no overlay.

## The first three changes

**1. Put the modules you want on the bar.** That is a layout edit, not a config key. The quickest way in is the
IPC verbs:

```sh
hogar-shell layout add activewindow top-bar start
hogar-shell layout add clock top-bar center
hogar-shell layout remove statusicons
```

The built-in layout, `default`, is read-only so there is always one that works: the first of those edits forks
it into `~/.config/hogar-shell/layouts/custom.toml` and draws that from then on, and that is the file to
hand-edit. To start a file by hand instead, `hogar-shell layout show > ~/.config/hogar-shell/layouts/mine.toml`
copies the built-in one and `hogar-shell layout use mine` draws it. A file called `default.toml` is never read —
the built-in layout keeps that name — and `hogar-shell layout check` says so. `hogar-shell layout show <name>`
prints a layout as it is stored, and `hogar-shell layout check <name>` says what is wrong with one before it
applies. See [Bars](../features/surfaces/bars.md) and the [Layout reference](../reference/layout.md) for the
full shape.

Every module id is a page under [features/modules](../features/modules/). An id the build does not know is
drawn as a placeholder where you placed it rather than failing the bar, and `hogar-shell layout check` says
which one.

**2. Pick a shape.** A bar's `shape.mode` in the layout is `bar`, `sections` or `chips` — one solid bar,
grouped zones, or a chip per module (`hogar-shell layout show <name>` prints a layout as it is stored). Every module works
in all three; see [Bars](../features/surfaces/bars.md).

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
