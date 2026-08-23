---
id: floats
kind: surface
title: Floats
summary: A panel as a free-standing window you can move and resize.
status: stable
compositor: any
config: [panels, modules]
commands: [panel]
deps: [wlr-layer-shell]
see_also: [panels, drawers]
---

# Floats

## What it is

The other presentation for a panel: a free-standing surface with a frame, a drag region and a resize grip in
its corner.

Opening one closes the drawer, which it would otherwise open underneath. Nothing closes it back: opening a
drawer, pressing a chip, opening the notification centre or opening a second float all leave it where it is.
That is the whole difference between a float and a drawer — a drawer is a glance, a float stays until you close
it.

```toml
[modules.mixer]
open   = "float"
width  = 520
height = 640
```

## Interacting

| Gesture | What happens |
| --- | --- |
| Drag the frame | moves it |
| Drag the corner grip | resizes it |

`[panels] drag_threshold` is how far the pointer must travel before a press becomes a drag rather than a
click — the same distinction a click inside the panel depends on.

## A resize is not saved

**A float resized by its grip keeps that size only while it is open.** It deliberately does not write back,
because that would be a config write and a config reload *per drag*.

The size that persists is `[modules.<id>] width` / `height`, or `[panels.float]` where the module sets none.
Set it there once rather than dragging every time.

## What it needs

`wlr-layer-shell`. A float is still a layer surface — hogar-shell opens no `xdg-shell` toplevels at all — which
is why it can be placed exactly and why it is unaffected by your window rules.

## Related

- [Drawers](drawers.md) — the default, anchored to its chip.
- [Panels](panels.md).
