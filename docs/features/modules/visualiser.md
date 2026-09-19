---
id: visualiser
kind: module
title: Visualiser
summary: Bars that follow the music, stood along an edge of the desktop.
status: partial
compositor: any
config: [visualiser, widgets]
commands: []
deps: [libpipewire]
see_also: [widgets, media]
---

# Visualiser

## What you can do with it today

Switch it on with `[widgets.visualiser] enabled = true` and the desktop's
[widget surface](../surfaces/widgets.md) draws it across the free area of the screen, its bars standing in a
row on `[widgets.visualiser] edge`, reaching at most `reach` into the screen. That surface builds this module
and only decides where it goes.

The module also draws a ring of bars at the small widget size; nothing places a small widget yet, so the ring
is not something a user can put on screen today. Placing widgets arrives with the layout model.

## What it shows

The audio spectrum, one bar per band.

## Configuring

`[visualiser]` is what the bars are — how many, how smooth — and is shared with every consumer, the media
card's ring included. `[widgets.visualiser]` is how they look: gap, radius, opacity, accent and whether they fade
out when nothing plays.

## What it needs

PipeWire, opened at runtime. Without it the bars stay silent, and a silent visualiser that hides when silent
draws nothing at all.

## Related

- [Desktop widgets](../surfaces/widgets.md) — the surface that places it on the desktop.
