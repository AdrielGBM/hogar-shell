---
id: launcher
kind: surface
title: Launcher
summary: A full-screen modal that owns the keyboard: applications, actions, a calculator, schemes, wallpapers and the windows already open.
status: stable
compositor: any
config: [launcher, general]
commands: [launcher, apps]
deps: [wlr-layer-shell, qalc, wlr-foreign-toplevel-management]
see_also: [apps, palettes, wallpaper]
---

# Launcher

## What it is

A modal surface that holds the keyboard while it is up. Type to filter; press Enter to run.

The keyboard, not the pointer: **the bar keeps working with the launcher open.** Hovering a chip still opens its
popout, and pressing one opens that chip's panel rather than dismissing the launcher. Pressing anywhere else
dismisses it, as does Escape.

Opening it closes whatever drawer was up, which it would otherwise cover. A [float](floats.md), the
[notification centre](notification-centre.md), toasts and notification popups are left where they are.

```sh
hogar-shell launcher toggle
hogar-shell launcher close
```

## Six modes, chosen by a prefix

| Prefix | Mode | What it lists |
| --- | --- | --- |
| *(none)* | Applications | installed `.desktop` entries |
| `>` | Actions | the actions you declared in `[launcher] actions` |
| `=` | Calculator | the result of an expression |
| `#` | Schemes | the palettes `scheme set` accepts |
| `@` | Wallpapers | the images in your library, as a grid |
| `/` | Windows | the windows already open — choosing one switches to it |

The prefix is stripped and the rest is the query, so `>reboot` and `> reboot` are the same thing.

The window mode matches on both the window's title and its application, so `/kitty` finds every terminal and
`/README` finds the one editing that file. It is the difference between starting a second copy of something and
going back to the copy you already have.

It reads `wlr-foreign-toplevel-management`, not Hyprland's socket, so it works on any compositor that speaks
that protocol — and lists nothing at all on one that does not, rather than falling back to the applications and
launching what you were trying to switch to.

## Ranking

Applications are ranked by **how often you have launched them**, which is state the shell keeps in
`state.json`. `[launcher] favourites` pins entries above that; `hidden` removes them.

`fuzzy` switches matching between fuzzy and substring. It is one global switch — there is no per-mode setting.

```sh
hogar-shell apps search firefox    # the same ranking, over IPC
hogar-shell apps reload            # re-scan, though a watcher usually does it for you
```

Applications are scanned once and cached for the process — a few hundred entries, parsed in a few milliseconds
— and a watcher notices an install, so new software appears without a reload.

## The calculator

A built-in evaluator handles ordinary arithmetic with no dependency. **`qalc`** adds currencies, constants and
dates; without it those queries simply do not resolve. `[launcher] calculator` and `qalc` switch each half.

## Actions

`[launcher] actions` is a list of things to run — the extension surface of the shell, alongside
`[theme.export] hooks`. `enable_dangerous_actions` gates the ones that can end a session.

## Configuring

`[launcher]` — `width`, `height`, `max_results`, `fuzzy`, `calculator`, `qalc`, `actions`,
`favourites`, `hidden`, `enable_dangerous_actions`, plus `[launcher.icons]`.

## What it needs

Nothing to open, and nothing to install to launch with. What you start from the launcher is put in a session of
its own, so it outlives the shell rather than dying with it; that is a system call the shell makes itself
rather than a helper it needs on the path. **`qalc`** is optional, as above.

## Known limits

No emoji picker, no clipboard history, no open-window switcher, no web-search mode and no file search. Each is
a mode over machinery that already exists, and none is built yet.

## Related

- [Palettes](../theming/palettes.md) — what `#` lists.
- [Wallpaper](wallpaper.md) — what `@` lists.
