---
id: notes
kind: module
title: Notes
summary: A scratchpad that survives a restart.
status: stable
compositor: any
config: []
commands: [panel]
deps: []
panel: true
see_also: [dashboard]
---

# Notes

## What it shows

A chip that opens a panel you can type in. The panel takes keyboard focus — it is one of the three surfaces
that do, alongside the launcher and the session menu.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the notes panel |

```sh
hogar-shell panel toggle notes
```

## Configuring

No section of its own. `[modules.notes]` carries the usual presentation overrides — whether the panel opens as
a drawer or a float, and that float's size.

## What it needs

Nothing. Notes are stored in `notes.toml` under the shell's data directory, written as you type.

## Related

- [Panels and drawers](../surfaces/panels.md) — how `[modules.notes] open` changes the surface it appears in.
