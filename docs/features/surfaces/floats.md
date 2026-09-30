---
id: floats
kind: surface
title: Floats
summary: A panel as a free-standing, centred window with a title bar and close button.
status: stable
compositor: any
config: [modules]
commands: [panel]
deps: [wlr-layer-shell]
see_also: [panels, drawers, compositor-rules]
---

# Floats

## What it is

The other presentation for a panel: a free-standing, centred window with a frame, a title bar and a close
button. It is not moved or resized — it always opens centred, at the size its module's options give it.

Opening one closes the drawer, which it would otherwise open underneath. Nothing closes it back: opening a
drawer, pressing a chip, opening the notification centre or opening a second float all leave it where it is.
That is the whole difference between a float and a drawer — a drawer is a glance, a float stays until you close
it.

```toml
[modules.mixer]
open         = "float"
float_width  = 520
float_height = 640
```

## Sizing

The size that shows is `float_width` / `float_height` from the `options` of the module's instance in the
layout, then from `[modules.<id>]`, and 360 × 240 where neither says — or what an application panel such as the
settings needs. A float opened from a chip takes that chip's instance's; one opened by IPC or a keybind takes
the module's first instance on the focused screen. Set it there — there is no drag to resize with.

## Where it lives

A float has no chip to anchor to, so it is a node inside the shell's **Overlay** layer window
(`hogar-shell-overlay`) alongside the launcher, the notification centre and the region picker — never its own
`xdg-shell` toplevel, which is why it can be placed exactly and why it is unaffected by your window rules. See
[Compositor rules](../../guides/compositor-rules.md) for the namespace this used to have of its own, and where
a rule against it reaches now.

## What it needs

`wlr-layer-shell`.

## Related

- [Drawers](drawers.md) — the default, anchored to its chip.
- [Panels](panels.md).
- [Compositor rules](../../guides/compositor-rules.md).
