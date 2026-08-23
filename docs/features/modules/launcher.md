---
id: launcher
kind: module
title: Launcher chip
summary: A search icon that opens the launcher.
status: stable
compositor: any
config: [launcher]
commands: [launcher]
deps: []
see_also: [../surfaces/launcher.md]
---

# Launcher chip

A square icon chip whose only job is to open the launcher. Everything the launcher *is* — the modes, the
ranking, the actions — is on its own page: **[Launcher](../surfaces/launcher.md)**.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | toggles the launcher |

Equivalent to `hogar-shell launcher toggle`, which is what most people bind to a key instead of putting the chip
on a bar.

## Configuring

The chip has no settings of its own. `[launcher]` configures the surface it opens.

## What it needs

Nothing.

## Related

- [Launcher](../surfaces/launcher.md) — the modes, the ranking and what it can run.
