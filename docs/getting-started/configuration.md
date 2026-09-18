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
| `~/.config/hogar-shell/config.toml` | everything; hot-reloaded when a save changes it |
| `~/.config/hogar-shell/tokens.toml` | design-token overrides — see [Tokens](../features/theming/tokens.md); hot-reloaded too |
| `~/.config/hogar-shell/monitors/<output>/config.toml` | per-monitor overrides, same shape as the global file; hot-reloaded too |
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

A save that changes what the three files above hold reloads them. The shell compares their contents, not their
modification times, so a save that leaves them as they were — an editor writing back a buffer you did not
change, a `touch` — reloads nothing. The reload is non-destructive in both directions:

- A surface that is already up is **reused**, not replaced — its layer-shell configuration is adjusted in
  place. A bar does not blink because you changed a colour.
- What the user opened stays open. Panels and drawers are tracked separately from the surfaces the config
  describes.

`hogar-shell shell reload` reloads on demand, and always rebuilds, whatever the files hold: it is how what they
do not cover reaches the shell, such as a font or an icon theme installed since.

A save that does not parse is not applied. The shell keeps running the last config that loaded — at startup,
with none yet, the starter config — and says why in its problems notice (below) until the file loads again.

## Every key, from the build

```sh
hogar-shell config schema              # every section
hogar-shell config schema launcher     # one section
man ./man/hogar-shell.5                # the same tree as a manual
```

Both are generated from the same walk over the config structs, so a key cannot reach one and go missing from
the other. [reference/config.md](../reference/config.md) is that tree as markdown.

## Checking what you wrote

```sh
hogar-shell config check
```

Reads `config.toml` and every monitor override beside it, and names — by file, line and key — what they ask for
that the shell cannot do: a module, dashboard page, utilities toggle or status icon it has no such id for; a
theme, accent or `[theme.colors]` token it has no such name for; a `[corners]` module on a corner no bar runs
along; and a section an override may not set. It exits non-zero when there is an error, so a script can run it
before putting a config in place, and it works whether or not the shell is running.

The running shell keeps a notice up while a problem lasts: one card listing what is wrong with the files now,
redrawn in place when that changes and withdrawn once nothing is left. A file that does not load is one of the
problems it lists, and while one is there the card says the configuration was not applied and waits to be
read. The shell also draws an unknown module or toggle as a placeholder where it was declared rather than
leaving it out.

`config check` fails only on an error — something asked for that the shell cannot do: a module, dashboard page,
utilities toggle, status icon or colour token it has no such name for, or a file that does not load. What the
shell does *instead* of what was asked is a warning: `nord` for an unknown theme, the palette's own accent for an
unknown accent, every dashboard page when no page listed is one it has, a corner module no bar draws, and a
section an override may not set.

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

An unknown id is never a failure — a config written for a newer build still starts on an older one — and it is
never silent either. An unknown module is drawn as a placeholder where it was declared, and an unknown utilities
toggle as a placeholder tile; an unknown dashboard page is left out, and a list with no page left in it shows
every page; an unknown status icon is left out of the cluster, an unknown theme falls back to `nord`, an
unknown accent to the palette's own, and an unknown `[theme.colors]` token is not applied.
`hogar-shell config check` names each of them with its file and line.

An unknown *key* is still ignored silently: checking keys against the schema is a different feature, not yet
built.
