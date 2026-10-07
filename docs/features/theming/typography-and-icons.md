---
id: typography-and-icons
kind: theming
title: Typography and icons
summary: A weight for each kind of text, the icon theme applications are drawn from, and one shape for every app icon.
status: stable
compositor: any
config: [theme, icons]
commands: []
deps: []
see_also: [palettes, presets, tokens]
---

# Typography and icons

## Weight per role

The shell draws its text in four roles: `display` (the clock face), `title` (panel and section headers), `body`
and `caption` (chip labels, badges, notification bodies). Each takes a weight of its own, from `100` to `900`:

```toml
[theme.fonts.title]
weight = 600

[theme.fonts.caption]
weight = 300
```

A role left unset keeps its own weight. A label the shell makes bold on purpose — a heading among rows —
stays bold whatever its role's weight, because that emphasis is relative to the role.

`[theme] font_family` sets the one family every role is drawn in; the font has to be installed, and the weights
it does not have are drawn at the nearest it does. A family this machine does not have is drawn in the platform's
sans-serif instead, and the shell says so in its log. Every window takes a new family as it is reloaded or
previewed, without being opened again.

## App icon theme

`[icons] app_icon_theme` names the freedesktop icon theme applications' own icons come from — on the tray, in
the launcher, on notifications and window chips. Empty follows the GTK settings, then `hicolor`. The Theme
popover and the settings page list the themes installed on this machine; one that asks to be hidden from a
picker, and a cursor theme, are not offered.

## App icon shape

`[icons] mask` cuts every application's icon to one silhouette:

| Value | Shape |
| --- | --- |
| `none` | As the icon theme drew it (the default). |
| `circle` | A circle the size of the icon. |
| `squircle` | A superellipse: a square whose sides curve continuously into its corners. |

It is never applied to what is not an application's artwork: a symbolic icon (its name ends in `-symbolic`), an
icon beside a menu entry, and a tray icon recoloured to the bar's ink with `[tray] recolour` are drawn whole.

## Live preview

The Theme popover in edit mode has a row for each of these, and a *Font family* row listing the families
installed on this machine. Every change is shown on every window as it is made, written to `config.toml` when
the popover closes, and put back by Esc or Cancel.

## What it needs

Nothing.

## Related

- [Palettes](palettes.md) — the colours the text and icons are drawn in.
- [Theme presets](presets.md) — keeping a whole look under a name.
