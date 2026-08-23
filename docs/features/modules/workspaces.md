---
id: workspaces
kind: module
title: Workspaces
summary: Which workspaces the bar shows, and what each pill says.
status: stable
compositor: hyprland
config: [workspaces]
commands: []
deps: [ext-workspace]
see_also: [activewindow, bars]
---

# Workspaces

## What it shows

A pill per workspace. What is in a pill is entirely yours: a number, a label, the icons of the windows on it,
or an indicator that slides to the active one.

`per_monitor` decides whether a bar shows every workspace or only the ones on its own screen — the setting that
matters most on a multi-monitor desk.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click a pill | switches to that workspace |
| Scroll | moves between workspaces, if `scroll` is on |

The module manages its own layout rather than sitting in a chip shell, since the pills are the layout.

## Configuring

`[workspaces]` — `shown`, `per_monitor`, `scroll`, `show_special`, `active_label`, `occupied_label`,
`occupied_background`, `capitalize`, `indicator`, `indicator_trail`, `label`, `window_icons`,
`max_window_icons`, plus `[workspaces.special_icons]`.

`hogar-shell config schema workspaces` is the annotated version.

## What it needs

**Hyprland's IPC.** Workspaces are read from the compositor's event socket, and switching is a dispatcher.

This is the largest of the four Hyprland-bound modules, and the one whose absence is most felt on another
compositor. `ext-workspace-v1` is the protocol that would replace it — it exists, Hyprland advertises it, and
the backend does not bind it yet. Until it does, this module is hidden outside Hyprland.

## Related

- [activewindow](activewindow.md) — the other compositor reading on a bar.
- [Bars](../surfaces/bars.md) — where the pills sit, and what happens in each shape mode.
