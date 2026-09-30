---
id: wallpaper
kind: surface
title: Wallpaper layer
summary: The background image and how one gives way to the next.
status: stable
compositor: any
config: [background, wallpaper, paths]
commands: [wallpaper]
deps: [wlr-layer-shell]
see_also: [dynamic-scheme, launcher, widgets]
---

# Wallpaper layer

One surface per monitor, at the bottom of the background layer. It paints the image this screen should show,
cover-cropped over the theme's base colour, and nothing else — a clock or a visualiser on the desktop is
[Desktop widgets](widgets.md), drawn on the desktop layer instead.

## Choosing an image

```sh
hogar-shell wallpaper set ~/pictures/x.jpg    # every screen
hogar-shell wallpaper set ~/pictures/x.jpg DP-2
hogar-shell wallpaper set ~/pictures/x.jpg --region left   # one region of the layout
hogar-shell wallpaper random [output]
hogar-shell wallpaper clear [output]          # back to what your config says
hogar-shell wallpaper list
```

The launcher's `@` mode is the same library as a grid.

## Which image a screen shows

Resolution order, most specific first:

1. the runtime per-output choice,
2. the runtime global one,
3. `[background.monitors]`,
4. `[background] image`.

An image you pinned in your config keeps showing until something sets one at runtime, and `wallpaper clear`
puts you back. The runtime choice lives in `state.json`, not in `config.toml`: a wallpaper picked at random is
state the shell owns, not a preference you hand-edited.

## Regions

The picture is drawn by the layout's `wallpaper_region` areas, and a screen can be split into several. A region
that names no `source` shows the image resolved above and follows every `wallpaper set`. One that names its own
keeps it: the runtime choice never reaches it. `wallpaper set <path> --region <area>` writes that `source` as a
layout edit, so `layout undo` takes it back; add an output to edit the region as that screen's own rule writes
it. Each region cross-fades on its own, and its fade repaints only its own box.

A region's `style` paints it too: `fill` is the colour wherever the picture does not reach, `radius` cuts its
corners, `opacity` fades the whole picture and `padding` holds it off the region's edges.

## The transition

A wallpaper change is an **event on the live surface**, not a rebuild. A layout or config edit does rebuild the
region, and it remembers the picture it was showing so that it still fades from it. `[background] transition`
and `transition_ms` control the cross-fade.

## The library

`[paths] wallpapers` is scanned recursively, with a thumbnail cache so a grid of two hundred images does not
decode two hundred full-resolution photographs.

`[wallpaper]` — `enabled`, `recursive`, `extensions`, `max_entries`, `thumbnail_size`.

## What it needs

`wlr-layer-shell`.

## Related

- [Desktop widgets](widgets.md) — the clock and visualiser that used to share this surface.
- [Dynamic scheme](../theming/dynamic-scheme.md) — deriving the palette from whatever is showing here.
