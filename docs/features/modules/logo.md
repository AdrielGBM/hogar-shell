---
id: logo
kind: module
title: Logo
summary: The distribution mark that opens the shell's own menu.
status: stable
compositor: any
config: [general]
commands: [panel, session]
deps: []
see_also: [session, launcher]
---

# Logo

A square icon chip carrying a distribution mark. Clicking it opens the session menu — it is an alias for the
[session](session.md) chip with a different face, for people who want the start-button gesture.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the session panel |

## Configuring

`[general] logo` names the glyph. `hogar-shell config schema general` lists what it accepts.

## What it needs

Nothing to display. The menu it opens needs **logind** for the actions that end the session — see
[Session actions](../system/session-actions.md).

## Related

- [session](session.md) — the same panel behind a power icon.
