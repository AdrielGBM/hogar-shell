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
see_also: [panels, floats, bars, compositor-rules]
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
falls back to wherever the layout placed that instance (`start`, `end`, or centred for a module the layout has
not placed on any bar).

## Where it lives

A drawer is a node inside the layer window its chip already lives in — `hogar-shell-top` for a bar chip — laid
out relative to the chip's own node rather than opened as a surface of its own. If that window's layer is not
visible on this output (its bar is hidden under a fullscreen window), or the drawer has no chip to anchor to, it
lives in `hogar-shell-overlay` instead. See [Compositor rules](../../guides/compositor-rules.md) for what that
means for a `layer_rule` you write yourself.

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
