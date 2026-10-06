---
id: palettes
kind: theming
title: Palettes
summary: Twelve built-in schemes, light/dark modes, and your own colours on top.
status: stable
compositor: any
config: [theme]
commands: [scheme]
deps: []
see_also: [dynamic-scheme, export, tokens, presets, typography-and-icons]
---

# Palettes

```sh
hogar-shell scheme list             # what `scheme set` accepts
hogar-shell scheme set nord
hogar-shell scheme mode toggle
hogar-shell scheme status           # theme, mode, variant, and the wallpaper it came from
hogar-shell scheme colors           # every token now on screen, as name and hex
```

## The built-ins

`nord` `rose-pine` `rose-pine-moon` `rose-pine-dawn` `catppuccin-mocha` `catppuccin-macchiato`
`catppuccin-frappe` `catppuccin-latte` `gruvbox` `gruvbox-light` `tokyo-night` `everforest`

A name is reduced to what identifies it before it is matched, so `rose-pine`, `rose_pine` and `rosepine` are
one theme — a separator preference never makes a name unknown. A name that is unknown falls back to `nord`,
and `hogar-shell config check` says so.

## Light and dark

`[theme] mode` is `dark`, `light` or `auto`. Switching mode maps a palette to its counterpart where there is
one — `catppuccin-frappe` becomes `catppuccin-latte`, `gruvbox` becomes `gruvbox-light`.

**`auto` means "keep whatever the palette already is".** It is *not* a sunrise/sunset switch; there is no clock
driving a mode change yet.

## Your own colours

`[theme.colors]` overrides individual tokens on top of whichever palette is selected, as `#rrggbb`. Setting
`[theme] name = "custom"` starts from nord and lets the config say the rest.

`[theme] accent` picks the accent: one of the palette's hues by name, or an opaque `#rrggbb` (the `#` and six digits; a colour without its `#`, a three-digit one or one with alpha is reported by `config check` and the palette's own accent stays). The Theme popover and the settings page offer the same values, so a theme token such as `surface` is refused there too. A module can override it for itself with `[modules.<id>] accent`.

## Type and scale

`[theme] font_family` and `[theme.fonts.*]` — `display`, `title`, `body`, `caption`, each with a size, a
weight and a slant — set the type; [Typography and icons](typography-and-icons.md) has the weights.
`[theme.scale]` — `font`, `icon`, `rounding`, `spacing` — scales the whole shell without touching every key.

> `[theme.scale]` is a **manual** multiplier. The shell does not ask the compositor for a fractional scale
> (`wp-fractional-scale-v1` is unbound), so on a 1.5× output the compositor scales the shell's buffer rather
> than the shell rendering at the true device-pixel grid. Text is slightly soft as a result.

## Icons

`[icons]` — `provider`, `default_set`. Icons come from an Iconify-compatible endpoint
(`{provider}/{set}/{name}.svg`); a name may override the set inline as `mdi:home`. The provider is
configurable because Iconify is self-hostable. Applications' own icons — `app_icon_theme` and the `mask` they are
cut to — are in [Typography and icons](typography-and-icons.md).

## What it needs

Nothing.

## Related

- [Dynamic scheme](dynamic-scheme.md) — deriving a palette from your wallpaper.
- [Export](export.md) — handing the palette to the rest of the desktop.
- [Theme presets](presets.md) — keeping a whole `[theme]` under a name.
