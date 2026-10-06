---
id: bars
kind: surface
title: Bars
summary: One per screen edge, all four at once if you like, on every monitor.
status: stable
compositor: any
config: [shape, modules]
commands: [shell, layout]
deps: [wlr-layer-shell]
see_also: [panels, per-monitor, shape, compositor-rules]
---

# Bars

## What it is

A `Bar` area: a strip along one edge, carrying modules in three zones. There is one per edge — top, bottom,
left, right — and you can have all four at once, on every monitor. Every bar, whichever edge it hugs, is drawn
inside the shell's **Top** layer window (`hogar-shell-top`) — see [Compositor rules](../../guides/compositor-rules.md)
for what that means for a rule you write yourself.

Where a screen's bars are, and what is on them, is the **layout**, not a config key. An empty layout draws
nothing: you get only the bars a layout describes.

## Zones

A bar area is written in `~/.config/hogar-shell/layouts/<name>.toml`:

```toml
[[outputs]]
match = "*"

[outputs.layers.top]
areas = [
  { id = "top-bar", kind = "bar", edge = "top", thickness = 34, groups = [
      { id = "start",  place = "zone", zone = "start",  children = [
          { id = "workspaces", module = "workspaces" },
          { id = "spacer",     module = "spacer" },
          { id = "activewindow", module = "activewindow" },
      ] },
      { id = "center", place = "zone", zone = "center", children = [
          { id = "clock", module = "clock" },
      ] },
      { id = "end",    place = "zone", zone = "end",    children = [
          { id = "statusicons", module = "statusicons" },
          { id = "tray",        module = "tray" },
          { id = "battery",     module = "battery" },
      ] },
  ] },
]
```

Three anchor points inside the bar's groups: `start`, `center`, `end`. [spacer](../modules/spacer.md) is what
buys every arrangement in between.

`hogar-shell layout add <module> <area> [group]` is the same edit from a script or a keybind, and
`hogar-shell layout show` prints a layout as it is stored. `instance.options` on a placed module sets any of
that module's options for that copy alone — `accent = "red"`, a `drawer_width` for the panel it opens, a key of
its own section — over the module's `[<module>]` and `[modules.<id>]` defaults; a key the module does not have
is a `layout check` error. See the [Layout reference](../../reference/layout.md) for the full shape.

An id placed twice keeps its options independent — that is the whole reason an `Instance` has an id: two copies
of `clock` can be styled differently, and each is addressed on its own by IPC and by an edit.

## Shapes

A bar's own `shape` field decides what it looks like, and every module works in all three modes:

| Mode | What it is |
| --- | --- |
| `bar` | one solid strip, edge to edge |
| `sections` | the three zones as separate plates |
| `chips` | a plate per module |

The same table's `gap` floats the bar off the edge, and `spacing` and `radius` set the room between its
modules and how round it is — so a top bar can be one solid strip while a left bar is chips. What a bar leaves
unset follows the theme. See [`BarShape`](../../reference/layout.md) in the layout reference.

`[shape] frame` draws a ring around the screen out of every bar.

A bar that stays on screen owns the corners it shares with a vertical bar, so it runs the whole edge. A bar that
`autohide`s yields them instead: it starts after the reserving bar at its side that stays on screen, so it never
slides in underneath it. A vertical bar that hides itself leaves only its peek strip, which it does not yield to.

`shape.fillet` curves the free space into a bar drawn as one strip (`mode = "bar"`; it is not drawn in `sections` or
`chips`). On a bar that hides itself the curve sits at each of its inner ends and fades with it. On a bar that
reserves, it rounds the corners of the usable area where the bar runs over a reserving bar crossing its end —
including under `[shape] frame`, where it restores the rounded inner corners the ring leaves square. That corner piece
belongs to the bar that owns the corner, the horizontal one, and takes its colour; its radius is that bar's own
`shape.fillet`, or else the fillet of the vertical bar it meets there, so a fillet written only on the vertical bar
still rounds the corner. The curve only paints: it never takes input.

## Auto-hide

An `autohide` table on a bar area is what makes it hide itself. Its absence is what "always on screen" means —
there is no separate on/off key.

It is not hidden by drawing it somewhere else — it is **moved**. Its tree is translated toward its own anchored
edge, far enough off that only `peek` logical pixels remain, and reveals itself by animating that offset back.
Two things follow: the bar takes no input over the strip it is not occupying, because it is genuinely not
there, and the peek strip is the bar's own edge rather than a second surface to keep in step.

`on_hover` is what triggers the reveal on pointer contact; switched off, only a drag inward past
`[panels] drag_threshold` brings it in.

It stays out while a drawer or a layout panel opened from one of its chips is up, whatever the pointer does, and
hides once that closes with the pointer elsewhere. Hiding only when a window would actually cover it needs
`cosmic_overlap_notify_v1`, which is COSMIC-only today.

## Per monitor

*Which* bars a screen has, and what is on them, is an `OutputRule` in the layout, matched by connector glob
(`match = "DP-*"`) and merged by area id over the default — not a per-monitor config file. A screen with no
bar area at all in its output rule simply has none. See [Per-monitor setup](../../guides/per-monitor.md).

## What it needs

`wlr-layer-shell` — the one hard requirement of the whole shell.

## Known limit

The toolkit binds layer-shell at versions 1–4, so v5's `set_exclusive_edge` is out of reach: a bar anchored to
more than one edge cannot say which edge its exclusive zone belongs to, and the compositor decides.

## Related

- [Panels](panels.md) — what a chip opens.
- [Layout reference](../../reference/layout.md) — every area, group and style key a layout file holds.
- [Compositor rules](../../guides/compositor-rules.md) — what a `layer_rule` reaches now.
- [Modules](../modules/) — what goes in the zones.
