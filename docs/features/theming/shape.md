---
id: shape
kind: theming
title: Shape and motion
summary: Bar shape, screen corners, and how much the shell animates.
status: stable
compositor: any
config: [shape, animation, keynav, panels]
commands: [layout]
deps: []
see_also: [bars, panels, palettes, widgets]
---

# Shape and motion

## Shape modes

A bar's shape is its own: a `Bar` area's `shape` table in the layout — `mode`, `gap`, `spacing`, `radius` and `fillet`.
`mode` is `bar`, `sections` or `chips`: one solid strip, the three zones as separate plates, or a plate per
module. Every module and every surface works in all three, on all four edges; that is a standing rule of the
project rather than a coincidence.

`fillet` curves the free space into a bar drawn as one strip, at its inner corners; [Bars](../surfaces/bars.md#shapes)
says where it is drawn.

What a bar leaves unset follows the theme: `spacing` and `radius` are the palette's, `mode` is `bar` and `gap`
is 0 — an edge-to-edge bar. Floating is opt-in. The built-in layout's bar says `mode = "bar"` and `gap = 0`
itself, so a fresh install draws the strip it always has. See [`BarShape`](../../reference/layout.md) in the
layout reference.

`[shape]` keeps the one thing every bar shares: `frame` draws a ring around the screen out of every bar.

## A widget in a screen corner

There is no separate corner config any more. A widget pinned to a screen corner — the clock face, say — is a
`Grid` area whose `anchor` is one of the nine places, the same mechanism [Desktop widgets](../surfaces/widgets.md)
places the clock and the visualiser with.

## Motion

`[animation]` — `enabled`, `curve`, `easing`, `duration_scale`, `panel_duration_ms`.

`duration_scale` is the one to reach for: it scales every animation at once, and `0` switches motion off
without changing anything else.

## Keyboard navigation

`[keynav] vim` adds `hjkl` navigation to the surfaces that take keyboard focus.

**The keys themselves are fixed** — there is no keybind table for in-surface navigation, only this switch.

## What it needs

Nothing.

## Related

- [Bars](../surfaces/bars.md) — where the shape modes are visible.
- [Panels](../surfaces/panels.md) — a panel takes its gap from the bar's, and its opacity from `[theme]`.
