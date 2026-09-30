---
id: popouts
kind: surface
title: Popouts
summary: The readout a chip shows while the pointer rests on it.
status: stable
compositor: any
config: [popouts]
commands: []
deps: [wlr-layer-shell]
see_also: [panels, statusicons, compositor-rules]
---

# Popouts

Distinct from the drawer a click opens, and the shell's primary status interaction: a bar chip has room for one
glyph, and everything that glyph stands for — the level behind it, the sensor it came from, the whole window
title it truncated — lives here.

## Which chips have one

`volume` `mic` `brightness` `battery` `network` `bluetooth` `kblayout` `lockstatus` `activewindow` `media`
`cpu` `gpu` `memory` `temperature` `netspeed`

A chip with no card is never given a hover target, so nothing ever opens empty.

## Three things make it a popout rather than a flicker

- **Delays.** The pointer has to rest on a chip before anything opens, and the card survives long enough after
  you leave for the pointer to reach it.
- **One node.** Moving from chip to chip replaces the card rather than stacking a second one.
- **A carved input region.** The card's node is sized to the tallest card a popout may be, and everything it
  does not cover is click-through — so an invisible rectangle never eats a click meant for the window behind.

## Where it lives

A popout is a node inside the layer window its chip already lives in — `hogar-shell-top` for a bar chip — laid
out relative to the chip's own node by the same arithmetic a [drawer](drawers.md) is placed by, rather than a
surface of its own. If that window's layer is hidden on this output — under fullscreen, say — the popout still
opens: it is anchored, so it routes into `hogar-shell-overlay` instead and is laid out there against the same
chip rect. See [Compositor rules](../../guides/compositor-rules.md) for the namespace this used to be, and
where a rule against it reaches now.

## Live, not a snapshot

Every card subscribes to the service it reads, so it follows the value while it is up. Hovering the volume chip
and scrolling it is one gesture, and a card frozen at the level it opened with would be worse than no card.

Nothing polls: each subscription is bound to the popout's node and dies with it.

## Configuring

`[popouts]` — `enabled`, `open_delay`, `close_delay`.

How big a card is belongs to the module whose chip it rests on: `popout_width` and `popout_max_height`, in the
`options` of that chip's instance in the layout, or under `[modules.<id>]` for every instance of it.
`popout_max_height` is a ceiling: the card is as tall as it needs, and the rest stays click-through.

## What it needs

`wlr-layer-shell`, plus whatever the reading behind the card needs.

## Related

- [Panels](panels.md) — what a click opens instead.
- [Drawers](drawers.md) — placed by the same chip arithmetic.
- [Compositor rules](../../guides/compositor-rules.md).
