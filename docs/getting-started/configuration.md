---
id: configuration
kind: guide
title: Configuration
summary: The files the shell reads and writes, and how a reload behaves.
status: stable
compositor: any
config: [general, paths]
commands: [config, shell]
see_also: [first-run, per-monitor, tokens]
---

# Configuration

## The files

| Path | What it is |
| --- | --- |
| `~/.config/hogar-shell/config.toml` | everything; hot-reloaded on save |
| `~/.config/hogar-shell/tokens.toml` | design-token overrides — see [Tokens](../features/theming/tokens.md) |
| `~/.config/hogar-shell/monitors/<output>/config.toml` | per-monitor overrides, same shape as the global file |
| `$XDG_STATE_HOME/hogar-shell/state.json` | runtime state the shell owns, not settings |

`XDG_CONFIG_HOME` moves the first three; `hogar-shell config path` prints where the shell is actually reading
from.

## Settings versus state

The split matters, and it is deliberate. `config.toml` is **yours** — you hand-edit it, and the shell only ever
writes it back through a form you used, preserving your comments and ordering. `state.json` is the shell's:
which wallpaper is up, whether do-not-disturb is on, how often each application was launched. A wallpaper
picked at random is not a preference you expressed, so it does not end up in your config file.

That is why `hogar-shell wallpaper clear` exists: it drops the runtime choice and puts `[background]` — the thing
you *did* write — back in charge.

## Reloading

Saving `config.toml` reloads it. The reload is non-destructive in both directions:

- A surface that is already up is **reused**, not replaced — its layer-shell configuration is adjusted in
  place. A bar does not blink because you changed a colour.
- What the user opened stays open. Panels and drawers are tracked separately from the surfaces the config
  describes.

`hogar-shell shell reload` does the same thing on demand.

## Every key, from the build

```sh
hogar-shell config schema              # every section
hogar-shell config schema launcher     # one section
man ./man/hogar-shell.5                # the same tree as a manual
```

Both are generated from the same walk over the config structs, so a key cannot reach one and go missing from
the other. [reference/config.md](../reference/config.md) is that tree as markdown.

## Sections and what they belong to

Config sections are named after the feature, not the surface — `[launcher]`, `[notifications]`, `[brightness]`.
Every feature page lists the sections it reads in its front matter, so the route from "I want to change this"
to "which section" is the page, and the route from "what can I set" to "what does it mean" is
`config schema <section>`.

Three sections are cross-cutting rather than one feature's:

- `[general]` — language, the terminal and default applications, whether surfaces show over fullscreen windows.
- `[paths]` — where wallpapers, screenshots, recordings, lyrics and assets live.
- `[modules.<id>]` — per-module presentation overrides (variant, accent, whether its panel opens as a drawer or
  a float, and that float's size). Keyed by module id, so it applies to every copy of that module on every bar.

## Unknown keys and unknown ids

An unknown module id, toggle id or status icon is **dropped with a log warning**, never a failure — a config
written for a newer build still starts on an older one. An unknown *key* is ignored by the same rule.

There is no validation report yet: `RUST_LOG=info` is where those warnings appear.
