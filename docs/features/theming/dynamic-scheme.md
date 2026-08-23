---
id: dynamic-scheme
kind: theming
title: Dynamic scheme
summary: A palette derived from the current wallpaper, in OkLCH, with a contrast pass over the result.
status: stable
compositor: any
config: [theme, background]
commands: [scheme, wallpaper]
deps: []
see_also: [palettes, wallpaper, export]
---

# Dynamic scheme

```sh
hogar-shell scheme set dynamic
hogar-shell scheme variant vibrant
hogar-shell scheme refresh          # re-derive from the current wallpaper
```

Once it is on, changing the wallpaper changes the palette.

## How it is derived

Colours are extracted from the image and worked in **OkLCH**, so lightness and chroma can be adjusted without
the hue shifting under them — then a **WCAG-AA contrast pass** runs over the result. That pass is what makes
the difference between a palette that matches a photograph and one you can read text on.

## Variants

`[theme] variant` decides how much colour the result carries:

`vibrant` `content` `expressive` `fidelity` `muted`

## The fallback

`[theme] fallback` is the palette used when a dynamic scheme cannot be derived — no wallpaper, or an image
nothing useful can be pulled from. Set it to something you are happy with rather than leaving the shell to
pick.

## Interaction with the wallpaper

The wallpaper is state (`state.json`), not config, so a dynamic palette follows whatever is *showing* —
including one picked at random. `hogar-shell wallpaper clear` puts `[background]` back in charge and re-derives.

## What it needs

Nothing. The extraction and the contrast pass are in-process; there is no `matugen` or helper in the loop.

## Related

- [Palettes](palettes.md) — the built-ins, and `[theme.colors]`.
- [Export](export.md) — sending the derived palette to GTK, Qt and your terminal.
- [Wallpaper layer](../surfaces/wallpaper.md).
