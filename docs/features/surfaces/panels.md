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
see_also: [drawers, floats, popouts, compositor-rules]
---

# Panels

## What it is

The surface behind a module. Thirteen modules have one: clock, dashboard, battery, bluetooth, network, mixer,
notifications, notes, settings, utilities, windowinfo, session and logo.

A panel is reached three ways — a chip click, `hogar-shell panel toggle <module>`, or a keybind — and all three
reach the **same** node rather than stacking three copies of it.

```sh
hogar-shell panel toggle settings
hogar-shell panel open network
hogar-shell panel list            # what is open right now
```

## Two presentations

`open` picks one — in the `options` of the module's instance in the layout, or under `[modules.<id>]` for every
instance of it:

| Value | What you get |
| --- | --- |
| `drawer` | anchored to the chip that opened it, sized to its content — see [Drawers](drawers.md) |
| `float` | a free-standing window you can move and resize — see [Floats](floats.md) |

A panel is toggled by module id from three places and only a press has a chip in hand, so which instance's
options apply is fixed: a press takes the pressed chip's instance, and IPC or a keybind the module's first
instance on the focused screen, then on any screen, then `[modules.<id>]` alone.

## A panel the layout arranges

A layout can give one instance a `panel` area of its own, sized in cells and holding widgets like a grid (see
[Layouts](../customization/layouts.md)). It opens beside its owner, or along the whole length of its owner's bar
with `along`, and is drawn in the owner's window as a drawer is.

### What a press opens

A press on an instance — a chip on a bar or a dock, a widget on the desktop, a widget inside a panel — runs the
first of these that answers:

1. the action the layout binds to its `press`;
2. the panel the layout gives the instance;
3. for a chip, the module's own: its own press, else its module's panel. A placeholder chip, standing in for a
   module this build does not have, opens the settings window here instead.

What a widget draws that answers a press itself — a button, a slider — keeps that press; the order is for the
rest of it.

Pulling a chip away from its bar opens what the second and third would, and never runs a bound action.
`hogar-shell panel toggle <instance>` follows the same order from the second step: the panel the layout gives the
instance, else its module's panel hung off that instance's chip. Given a module id instead, it toggles the module's
panel as it always has. An instance on the focused screen is found first.

The panel is drawn from its own `style`, over the theme's surface where the style says nothing:

- `visible` is honoured as it is on any area. A panel whose `visible` reads false does not open, and one that is open
  closes when it turns false. In the edit mode of its layer it opens anyway, drawn dim so it can be selected, and
  leaving the mode closes it again if the expression still hides it.
- `backdrop = "blur"` asks the compositor to blur behind the panel's box through `ext-background-effect-v1`, together
  with whatever the window's areas ask for, and takes it back when the panel goes. The protocol takes rectangles, so
  the blur runs past the rounded corners; without the protocol the panel stays translucent and unblurred, as an area
  does. On the background layer the shell blurs what its own surface drew under the panel instead.
- What the panel holds is cut to its `radius`, never rounder than half its short side.
- While the panel slides in or out, the travel is cut to the box it ends in, so nothing is repainted outside it; once
  it has settled nothing is cut that was not before. Drawers are cut the same way.

`panel list` names it as `<instance>@<output>` (`<instance>@` where the layout has a single, unnamed output), and `hogar-shell panel close <instance>@<output>` closes it.

## Configuring

`[panels]` — `drag_threshold`.

How translucent a panel is, and how far it sits off the bar, are not panel settings: the opacity is
`[theme] opacity` for the whole shell at once, and the gap is the bar's own, so a panel floats off the bar by
exactly what the bar floats off the screen.

How big a panel opens is its module's: `drawer_width` and `drawer_max_height` for a drawer, `float_width` and
`float_height` for a float, set on an instance or under `[modules.<id>]` like `open`, beside `variant` and
`accent` for how its chip is drawn.

## Keyboard

Most panels are display-only and take no keyboard focus, so the window behind them keeps it. Three take
focus because they have fields: **notes**, **settings** and **session**.

## Where it lives

Neither presentation opens a layer-shell surface of its own. A drawer is a node in the window of the chip it
hangs off (`hogar-shell-top` for a bar chip), and a float — having no chip to anchor to — is a node in
`hogar-shell-overlay` alongside the launcher and the notification centre. See
[Compositor rules](../../guides/compositor-rules.md) for what a `layer_rule` written against the old
per-panel namespaces reaches now.

## What it needs

`wlr-layer-shell`.

## What closes one

Pressing the chip again, `hogar-shell panel close <module>`, and — for a drawer — a press outside it. A layout's
panel closes on everything that closes a drawer, and on Esc once a press inside it has given it the keyboard.

**A drawer is also closed by any window opening**: the [launcher](launcher.md), a float, the
[notification centre](notification-centre.md). A drawer is a glance, and while it is up its window's input
region covers the whole usable area — that is how a press beside it dismisses it — so a window opening
underneath is a window that is painted, unreachable, and dismissed rather than used by the first press that
goes near it.

What opens from inside another — a drawer opened from a chip inside a layout's panel — is its child rather than
the next drawer in turn: opening it leaves the panel open, Esc and a press outside close the child first, and
closing the panel closes the child with it.

**Nothing closes a float.** It is the presentation you choose when you want a panel to stay put, so opening a
drawer, pressing a chip, opening the notification centre or opening a second float all leave it exactly where it
is. It closes by its ✕, by its chip, or by `hogar-shell panel close`.

Toasts, notification popups and the OSD are pinned to an edge and say something you did not open a window to be
told, so nothing closes them either. Neither does the region picker close a drawer: it is drawn over a still of
the screen taken the instant before it mapped, so closing one first would take out of the capture exactly what
you opened the picker to photograph.

## Lifecycle

A panel that has never been opened does not exist. What the *layout* describes — bars, the wallpaper, desktop
widgets — is reconciled on every reload; what the *user* opened is tracked separately, which is what keeps a
reload from closing what you had open. A layout's panel stays open across a reload for as long as the layout still
gives its owner one, and follows what the reload changed about it.

## Related

- [Popouts](popouts.md) — the hover card, which is a different node with different rules.
- [Compositor rules](../../guides/compositor-rules.md).
