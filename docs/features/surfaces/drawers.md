---
id: drawers
kind: surface
title: Drawers
summary: A panel anchored to the chip that opened it.
status: stable
compositor: any
config: [panels]
commands: [panel]
deps: [wlr-layer-shell]
see_also: [panels, floats, bars]
---

# Drawers

## What it is

The default presentation for a panel: a surface that hangs off the chip you clicked, on the same edge as its
bar, sized to its content up to a limit.

A drawer is positioned by the **chip's own place on the bar**, the same arithmetic a [popout](popouts.md) is
placed by — so what a click opens and what a hover opens land in the same spot, and a chip in the middle of a
bar no longer opens its panel at an end of it. Along a horizontal bar the drawer centres on its chip; along a
vertical one it lines up with the chip's top. Either way it is kept clear of the far end of the screen, so a
drawer never opens off the side.

Opened with no chip in hand — `hogar-shell panel toggle`, a keybind — there is nothing to follow, and the drawer
falls back to the zone the module is configured in (`start`, `end`, or centred for a module the config cannot
place).

## Configuring

`[panels.drawer]` — `width`, `max_height`.
The distance from the bar is the bar's own outer gap, and the translucency is `[theme] opacity` for the whole
shell — neither is a drawer setting.

`max_height` is a maximum, not a height: a drawer with two rows in it is two rows tall.

## When to use a float instead

A drawer closes when you look away and cannot be moved. If you want a panel to stay put while you work in the
window behind it — the mixer while you balance two applications, window info while you compare two windows —
set `[modules.<id>] open = "float"`. See [Floats](floats.md).

## What it needs

`wlr-layer-shell`.

## Related

- [Panels](panels.md) — the shared behaviour, and the list of what has one.
