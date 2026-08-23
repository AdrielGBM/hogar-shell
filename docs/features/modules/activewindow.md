---
id: activewindow
kind: module
title: Active window
summary: What the focused window is called, and how much of that fits on a bar.
status: stable
compositor: hyprland
config: [active_window]
commands: [shell]
deps: [wlr-foreign-toplevel-management]
popout: true
see_also: [windowinfo, workspaces]
---

# Active window

The title of whatever has focus, truncated to fit a bar rather than pushing everything else off it.

## What it shows

The focused window's title, optionally with its application icon. `compact` drops to the application name
instead of the title, which is the reading that stays a fixed width as you move between windows.

`inverted` swaps the two, so the application name leads and the title follows.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | focuses the window it names |
| Hover | a popout card with the untruncated title, the class and the workspace |

The chip holds one line, and nothing bounds what a title may be, so it takes the width its side of the bar can
spare and ends in `…` where that runs out. Nothing to set: the limit is the room, measured in the pixels the
room is actually made of.

## Configuring

`[active_window]` — `compact`, `inverted`, `show_icon`. Run
`hogar-shell config schema active_window` for what each does.

## What it needs

Hyprland's IPC socket. The reading is the compositor's focused client, and Hyprland's `.socket2.sock` event
stream is the only route implemented — there is no `ext-foreign-toplevel-list-v1` binding yet. On another
compositor the chip is hidden rather than blank.

Nothing else: no external program, no D-Bus peer.

## Related

- [windowinfo](windowinfo.md) — the panel with the preview and the four actions on the same window.
- `hogar-shell shell clients` — every open window, over IPC.
