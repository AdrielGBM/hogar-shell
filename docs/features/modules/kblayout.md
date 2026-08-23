---
id: kblayout
kind: module
title: Keyboard layout
summary: Which layout the main keyboard is on, and switching it.
status: stable
compositor: hyprland
config: [toasts]
commands: [keyboard]
deps: []
popout: true
see_also: [lockstatus, toasts]
---

# Keyboard layout

## What it shows

The main keyboard's active layout.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | cycles to the next layout |
| Hover | a popout card naming the layout in full |

```sh
hogar-shell keyboard layout    # which one
hogar-shell keyboard next      # switch
```

## Configuring

The layouts themselves are the compositor's — hogar-shell reads and cycles them, it does not define them.

`[toasts.events] kb_layout` decides whether a switch also raises a [toast](../surfaces/toasts.md), which is
the useful part when you switch with a keybind and the bar is on another screen.

## What it needs

Hyprland's IPC socket, for both halves: reading the active layout and switching it. On another compositor the
chip is hidden.

## Related

- [lockstatus](lockstatus.md) — the other keyboard-state indicator, and the one that does not need Hyprland.
