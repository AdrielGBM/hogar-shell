---
id: tokens
kind: theming
title: Design tokens
summary: The unstable surface underneath `[theme]`, for when a key does not exist yet.
status: partial
compositor: any
config: [theme]
commands: []
deps: []
see_also: [palettes, shape]
---

# Design tokens

## What it is

`~/.config/hogar-shell/tokens.toml` overrides the design tokens the UI is drawn from — the layer beneath every
`[theme]` key.

**It is deliberately unstable.** `[theme]` is the supported surface; a token name can change between builds
without that being a breaking change. Use this when there is no key for what you want, and expect to revisit it.

## Why it is a separate file

Two reasons, both practical:

- It is **skipped from serialization**, so a settings form saving a section can never write tokens into your
  `config.toml`.
- It keeps the unstable thing physically apart from the stable one, so a config you share is a config that
  keeps working.

## What is stable instead

| Want | Use |
| --- | --- |
| a colour | `[theme.colors]` |
| an accent | `[theme] accent`, or `[modules.<id>] accent` |
| type | `[theme] font_family`, `[theme.fonts.*]` |
| size | `[theme.scale]` |
| roundness and gaps | `[shape]`, `[panels]`, `[corners]` |

Reach for `tokens.toml` only when none of those covers it.

## What it needs

Nothing.

## Related

- [Palettes](palettes.md) — the supported surface.
- [Shape](shape.md).
