---
id: battery
kind: module
title: Battery
summary: Charge, and the warnings and actions that hang off it.
status: stable
compositor: any
config: [battery]
commands: []
deps: [upower, power-supply, logind]
panel: true
popout: true
see_also: [statusicons, idle, session-actions]
---

# Battery

Charge level as an icon chip, a detail panel behind it, and the one part of the shell that acts on its own
when a reading crosses a line you set.

## What it shows

Level and charging state. The chip is **hidden entirely on a machine with no battery** — a desktop does not
get a permanently-full icon.

The panel adds health, time remaining and the power supply's own reported state, where the source publishes
them.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the battery panel |
| Hover | a popout card with the level, the state and the estimate |

## Configuring

`[battery]` — `enabled`, `critical_level`, `critical_action`, plus `[[battery.warn_levels]]`, a list of
thresholds each with its own message.

`critical_action` is a request line, so anything in `hogar-shell --list` can be what happens at 5 %: suspend,
lock, hibernate, a toast. See [Scripting](../../guides/scripting.md) for the vocabulary.

## What it needs

**UPower** for the full reading, over D-Bus. Without it the service falls back to `/sys/class/power_supply`,
which answers level and charging state and not much else. Without *both*, the chip is hidden.

**logind** is what carries out `critical_action` when it is a session action — suspend, hibernate — and it
works without privileges because logind decides what the active session may do.

This is the fallback pattern the shell uses everywhere: a richer source when it is there, a kernel interface
when it is not, hidden when neither is.

## Related

- [statusicons](statusicons.md) — the same reading as one glyph in a shared chip.
- [Idle](../system/idle.md) — the other thing that acts on its own after a threshold.
