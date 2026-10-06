---
id: windows
kind: module
title: Windows
summary: Every open window as one entry on a strip — a taskbar, or a dock with pinned apps.
status: stable
compositor: any
config: [windows, dock]
commands: []
deps: [wlr-foreign-toplevel-management]
see_also: [activewindow, workspaces, bars]
---

# Windows

## What it shows

One entry per open window: the application's icon, and its title where the strip has room for it. The focused
window's entry rests on the accent colour, so the strip also says where you are.

The strip takes the shape of the place it is put in:

| Where | How it lays out |
| --- | --- |
| A top or bottom bar | a row along the bar, icon and title |
| A left or right bar | a column of icons; titles too once the bar is wider than 64 px |
| A tall widget or container cell | a list, one row per window |
| A narrow box (64 px or less) | a column of icons |
| A short, wide box | a row, with titles only when it is roomy |

It follows the box as it changes: a container resized in the editor turns the same strip from a row into a list
without being placed again. A list shows as many windows as fit.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click an entry | focuses that window; a minimised one is restored first |
| Drag an entry along the strip | moves it; the others make room as it passes |

The order you drag into is the shell's own and is kept per screen. It is remembered across restarts by
application, since a window has no identity that outlives the session: a newly opened window joins the other
windows of its application, or takes the place its application was last dragged to, or goes last.

Each entry also tells the compositor where it is, on the screen the window is on, which a compositor that
animates minimising uses as the place the window goes to.

## In a dock

Placed in a `kind = "dock"` area, the strip becomes a dock, on whichever edge the dock hangs off:

- **Pinned apps** come first, in the order `[dock] pinned` lists them, whether or not they are running. A pinned
  app that is running is the entry its windows show on; one that is not shows its icon alone.
- **One entry per application.** The windows of an application share its entry, and the applications that are
  not pinned follow the pins in the order you dragged them into.
- **Running indicators**: a dot per window on the side of the entry that faces the edge, up to three. The
  focused application's dots and entry are on the accent.
- **Magnification**: the entries near the pointer grow towards `[dock] magnification` and their neighbours make
  room along the dock. Only what is drawn grows — the dock keeps its thickness and what it reserves, so no window
  moves. The entries ease in and out with `[animation]`, and snap while motion is reduced.

| Gesture | What happens |
| --- | --- |
| Click an entry | launches a pinned app that is not running; otherwise focuses its window, and the next one of its windows on each click |
| Secondary click an entry | pins it, or unpins it — unless the layout binds a `secondary` action to the instance, which runs instead |
| Drag a pin along the dock | moves the pin; the order is written to `[dock] pinned` |
| Drag any other entry | moves the application, remembered as the taskbar remembers it |

A pin is the desktop entry's id (`firefox`, `org.gnome.Nautilus`), matched to windows by that id, by the
entry's `StartupWMClass`, or by the last part of a reverse-DNS name, without regard to case. Without the
compositor listing its windows, the pins still show and launch.

## Configuring

`[windows]` — `titles` writes each window's title beside its icon where there is room; off, every entry is the
icon alone. Like any module option it can be set on one placed instance rather than for all of them.

`[dock]` — `pinned`, the desktop-entry ids a dock keeps, and `magnification`, how large the entry under the
pointer grows (`1` turns it off, up to `2.5`). One list for every dock, since pinning from one is pinning.

```toml
[dock]
pinned = ["firefox", "org.gnome.Nautilus", "kitty"]
magnification = 1.5
```

`hogar-shell config schema windows` is the annotated version.

## What it needs

`wlr-foreign-toplevel-management`, read on its own: it is the protocol that says which window has focus and can
act on one, which `ext-foreign-toplevel-list` cannot, and the two describe the same windows without anything to
match them by. Every wlroots compositor and Hyprland speak it; no compositor IPC is involved.

Without it the strip is an empty chip, and a widget says the compositor does not list its windows; a dock still
shows and launches its pins. Nothing else in the shell depends on the strip.

## Related

- [activewindow](activewindow.md) — the focused window's title alone.
- [workspaces](workspaces.md) — the strip of workspaces beside it.
- [Bars](../surfaces/bars.md) — where the strip usually sits.
