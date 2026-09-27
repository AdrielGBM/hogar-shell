---
id: compositor-rules
kind: guide
title: Compositor rules
summary: What a namespace, a layer rule or an animation rule targets now that every layer is one window.
status: stable
compositor: any
deps: [wlr-layer-shell]
see_also: [per-monitor, bars, widgets, panels]
---

# Compositor rules

## What changed

Every wlr layer used to be a surface per item: a bar was its own namespace, a drawer another, a hover popout
another again — fourteen namespaces in all. Writing `layer_rule = blur, ^hogar-shell-drawer$` or
`layerrule = noanim, ^hogar-shell-popout$` reached exactly one kind of thing.

That is gone. The shell now opens **one layer-shell window per wlr layer per output** —
`hogar-shell-background`, `hogar-shell-desktop`, `hogar-shell-top` and `hogar-shell-overlay`, plus
`hogar-shell-reserve` for every reservation strip. A drawer, a hover popout, a tray or context menu, and the
customization popover are not surfaces at all: each is a node drawn inside whichever of those four windows its
anchor already lives in, or inside `hogar-shell-overlay` when it has no anchor. A compositor rule can no longer
single one of them out — it targets the whole window, which is also everything else sharing it.

**This is a breaking change with no alias.** A rule written against one of the fourteen old namespaces now
matches nothing.

## The old namespaces, and where that thing lives now

| Old namespace | Where it is now |
| --- | --- |
| `hogar-shell-top`, `hogar-shell-bottom`, `hogar-shell-left`, `hogar-shell-right` | `hogar-shell-top` — every bar, on every edge, shares the one Top window. Only `-top` survives; `-bottom` is deliberately not reused for it, since a user's own rules would read "bottom" as "the bottom bar". |
| `hogar-shell-reserve-top` / `-bottom` / `-left` / `-right` | `hogar-shell-reserve` — one namespace for every reservation strip, still a separate surface per `(output, edge)`, just no longer one namespace per edge. |
| `hogar-shell-wallpaper` | `hogar-shell-background` — the wallpaper is a `WallpaperRegion` area drawn in the Background window. |
| `hogar-shell-widgets` | `hogar-shell-desktop` — the clock face and visualiser are areas drawn in the Desktop window. |
| `hogar-shell-frame` | Nothing. The frame ring is decoration inside the Top window's own tree, not a surface a rule can address. |
| `hogar-shell-popout`, `hogar-shell-drawer` | Nothing of their own, usually: a drawer, a hover popout, a tray/context menu and the customization popover are nodes inside the window of the chip they hang off — `hogar-shell-top` for a bar chip. They land in `hogar-shell-overlay` only when that chip's bar is hidden under a fullscreen window. |
| `hogar-shell-float` | `hogar-shell-overlay` — a float has no anchor, so it always lives there. |
| `hogar-shell-overlay` *(old meaning: the launcher)* | `hogar-shell-overlay` — the string is unchanged, but the meaning is not: it now names the one shared window for everything unanchored — the launcher, a float, the notification centre, the region picker, and any drawer/popout/menu whose anchor is currently hidden. A rule against this namespace today reaches all of those at once, not the launcher alone. |
| `hogar-shell-stack` | Wherever the active layout puts the `stack` area — by default `hogar-shell-overlay`, top right, but a layout can move it into any layer. `hogar-shell layout show` prints where it is. |
| `hogar-shell-sidebar` | The notification centre does not anchor to a chip: it is a node that always opens in `hogar-shell-overlay` on the focused output, whether reached from the bell, IPC or a keybind. |
| `hogar-shell-picker` | `hogar-shell-overlay` — the region picker has no anchor. |

## Blur

A whole-window `layer_rule = blur, ^hogar-shell` still works exactly as before: it blurs everything the window
draws, which is now several kinds of thing at once rather than one.

Item-level blur — a single drawer or card blurred while its bar is not — is no longer a compositor rule. It is
requested per area, in the layout, with `style.backdrop = "blur"`; the shell asks for it through
`ext-background-effect-v1` and unions the rounded rects of every area styled that way into one blur region. On
a compositor that does not bind the protocol, those areas render translucent instead of blurred, and
`hogar-shell layout check` says why. See the [Layout reference](../reference/layout.md#areastyle) for
`AreaStyle`.

## Animation

Per-namespace compositor animations (`layerrule = animation …`) go the same way as blur: they applied to one
kind of item and now apply to a whole window shared by several. Enter and exit motion for a drawer, a popout, a
float and the rest is drawn in-client instead, and does not need a compositor rule at all.

## Related

- [Per-monitor setup](per-monitor.md) — arranging what each output shows.
- [Bars](../features/surfaces/bars.md), [Desktop widgets](../features/surfaces/widgets.md).
- [Layout reference](../reference/layout.md) — every area kind and style key.
