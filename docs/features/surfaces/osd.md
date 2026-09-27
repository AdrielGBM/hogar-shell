---
id: osd
kind: surface
title: On-screen display
summary: The overlay that shows a level while you are changing it.
status: partial
compositor: any
config: [stack]
commands: [volume, mic, brightness, layout]
deps: [wlr-layer-shell]
see_also: [toasts, volume, brightness]
---

# On-screen display

## What it is

The overlay that appears when you change volume, microphone level or brightness — from a keybind, from a chip
click, or from a scroll on a chip.

Three kinds, and only three: `volume`, `mic` and `brightness`. Other state changes raise a
[toast](toasts.md) instead, which is a defensible different answer rather than an omission — a toast carries
text, an OSD carries a bar.

## Configuring

An OSD is a card in the shell's one column: a `Stack` area in the layout, by default an area in the overlay
layer, pinned to the top right. Where it is, how wide it is and which outputs it appears on are that area's
`anchor`, `width` and `output_policy` — a layout edit, not a config key — and `hogar-shell layout show` prints
where the running layout put it. How the column behaves — `[stack]` — `max_visible`, `timeout_ms`,
`clear_threshold` — is unchanged. See [Toasts](toasts.md) for the column itself, and the
[Layout reference](../../reference/layout.md#areakindstack) for `AreaKind::Stack`.

## Showing one without changing anything

Clicking the [brightness](../modules/brightness.md) chip shows the OSD without moving the level, which is the
gesture for "what is it at". There is **no IPC command** that does the same thing — `volume osd` / `mic osd` /
`brightness osd` do not exist yet, so a keybind that reports a level without changing it is not available.

## What it needs

`wlr-layer-shell`. An OSD is a card in the stack column, and the overlay window it lives in is held open only
while the column has cards.

## Related

- [Toasts](toasts.md) — the text equivalent, for everything that is not a level.
