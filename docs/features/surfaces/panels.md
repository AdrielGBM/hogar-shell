---
id: panels
kind: surface
title: Panels
summary: What a chip opens — as a drawer hanging off it, or as a movable float.
status: stable
compositor: any
config: [panels, modules]
commands: [panel]
deps: [wlr-layer-shell]
see_also: [drawers, floats, popouts]
---

# Panels

## What it is

The surface behind a module. Thirteen modules have one: clock, dashboard, battery, bluetooth, network, mixer,
notifications, notes, settings, utilities, windowinfo, session and logo.

A panel is reached three ways — a chip click, `hogar-shell panel toggle <module>`, or a keybind — and all three
reach the **same** surface rather than stacking three copies of it.

```sh
hogar-shell panel toggle settings
hogar-shell panel open network
hogar-shell panel list            # what is open right now
```

## Two presentations

`[modules.<id>] open` picks one per module:

| Value | What you get |
| --- | --- |
| `drawer` | anchored to the chip that opened it, sized to its content — see [Drawers](drawers.md) |
| `float` | a free-standing window you can move and resize — see [Floats](floats.md) |

The choice lives under `[modules.<id>]` rather than on the bar entry because a panel is toggled by module id
from three places and only one of them has a bar entry in hand. An entry-scoped answer would make the same
panel open differently depending on how you asked for it.

## Configuring

`[panels]` — `drag_threshold`.

How translucent a panel is, and how far it sits off the bar, are not panel settings: the opacity is
`[theme] opacity` for the whole shell at once, and the gap is the bar's own, so a panel floats off the bar by
exactly what the bar floats off the screen.
`[panels.drawer]` — `width`, `max_height`.
`[panels.float]` — `width`, `height`.

Per module, `[modules.<id>]` overrides `width` and `height` for that module's float, plus `variant` and
`accent` for how it is drawn.

## Keyboard

Most panels are display-only and take no keyboard focus, so the window behind them keeps it. Three take
focus because they have fields: **notes**, **settings** and **session**.

## What it needs

`wlr-layer-shell`.

## What closes one

Pressing the chip again, `hogar-shell panel close <module>`, and — for a drawer — a press outside it.

**A drawer is also closed by any window opening**: the [launcher](launcher.md), a float, the
[notification centre](notification-centre.md). A drawer is a glance, and while it is up its surface covers the
whole usable area — that is how a press beside it dismisses it — so a window opening underneath is a window that
is painted, unreachable, and dismissed rather than used by the first press that goes near it.

**Nothing closes a float.** It is the presentation you choose when you want a panel to stay put, so opening a
drawer, pressing a chip, opening the notification centre or opening a second float all leave it exactly where it
is. It closes by its ✕, by its chip, or by `hogar-shell panel close`.

Toasts, notification popups and the OSD are pinned to an edge and say something you did not open a window to be
told, so nothing closes them either. Neither does the region picker close a drawer: it is drawn over a still of
the screen taken the instant before it mapped, so closing one first would take out of the capture exactly what
you opened the picker to photograph.

## Lifecycle

A panel that has never been opened does not exist. What the *config* describes — bars, the wallpaper, the frame
— is reconciled on every reload; what the *user* opened is tracked separately, which is what keeps a reload
from closing what you had open.

## Related

- [Popouts](popouts.md) — the hover card, which is a different surface with different rules.
