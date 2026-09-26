---
id: visualiser
kind: module
title: Visualiser
summary: Bars that follow the music, stood along an edge of the desktop.
status: partial
compositor: any
config: [visualiser]
commands: []
deps: [libpipewire]
see_also: [widgets, media]
---

# Visualiser

## What you can do with it today

Place it in a `dock` area on the desktop layer and its bars stand in a row on that area's `edge`, reaching
as far into the screen as the area is thick — see [desktop widgets](../surfaces/widgets.md). The area decides
where the row goes; this module only draws it.

The module also draws a ring of bars at the small widget size, which a `grid` area places on a cell.

## What it shows

The audio spectrum, one bar per band.

## Configuring

`[visualiser]` is what the bars are — how many, how smooth — and is shared with every consumer, the media
card's ring included. `[visualiser.face]` is how a placed row looks: gap, radius, opacity, accent and whether
they fade out when nothing plays.

## What it needs

PipeWire, opened at runtime. Without it the bars stay silent, and a silent visualiser that hides when silent
draws nothing at all.

## Related

- [Desktop widgets](../surfaces/widgets.md) — the surface that places it on the desktop.
