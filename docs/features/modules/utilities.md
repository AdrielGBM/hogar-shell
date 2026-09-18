---
id: utilities
kind: module
title: Utilities
summary: The switches you reach for without opening anything.
status: stable
compositor: any
config: [utilities]
commands: [panel, screenshot, record]
deps: [gamemode, networkmanager, bluez, pw-dump]
panel: true
see_also: [notification-centre, screenshot, recording]
---

# Utilities

Every toggle here already exists as a service and, for most of them, as its own bar chip. What the panel adds
is *one place* — turning the microphone off and the VPN on should not mean putting two chips on a bar and
remembering which is which.

## What it shows

A grid of toggles, and optionally the recent captures and recordings.

| Toggle id | What it switches |
| --- | --- |
| `wifi` | the wireless radio |
| `bluetooth` | the adapter |
| `mic` | mute on the default source |
| `dnd` | do-not-disturb |
| `game_mode` | GameMode, held by the shell |
| `vpn` | the active tunnel |
| `idle_inhibit` | the idle timers |
| `screenshot` | opens the capture flow |
| `record` | starts or stops a recording |
| `settings` | opens the settings panel |

## Interacting

| Gesture | What happens |
| --- | --- |
| Click on the chip | opens the utilities panel |
| Click on a tile | performs that toggle |

## Configuring

`[utilities]` — `toggles` (the ids, in your order), `columns`, `show_capture`, `show_recordings`,
`window_preview_ms`.

An id this build does not know holds its place in the grid as a placeholder tile naming it, rather than failing
the panel; pressing it opens the settings window, and `hogar-shell config check` names it with its line.

## What it needs

Whatever each toggle needs — NetworkManager for Wi-Fi, BlueZ for Bluetooth, PipeWire for the microphone,
`gamemoded` for game mode. A toggle whose service is missing is greyed out rather than absent, so the grid does
not reflow depending on what is installed.

`game_mode` is worth one note: GameMode is a **reference count**, not a flag. Games register themselves while
they run, so "on" from the shell means the shell registering a client of its own, and "off" means dropping it —
a game that is already running keeps game mode on whatever the shell does.

## Related

- [Notification centre](../surfaces/notification-centre.md) — hosts *these* toggles rather than a second set,
  which is the whole reason the two were built together.
- [Screenshot](../system/screenshot.md), [Recording](../system/recording.md).
