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

In the background edit mode a region's popover picks its picture from the library as thumbnails, narrowed by a
search, takes any path typed, or sets it back to following `[background]`. Its "This workspace only" switch
writes whatever the popover changes into the rule of the workspace that is up, so a picture per workspace is a
workspace rule like any other: that workspace shows it, and every other one shows the region's own.

## Focus, dimming and parallax

```toml
[[outputs.layers.background.areas]]
id = "background"
kind = "wallpaper_region"
focus = { x = 0.3, y = 0.4 }   # the part of the picture `cover` keeps in view
dim = 0.35                     # how dark it goes under a window, 0 to 1
blur = 16                      # how far it blurs under a window, up to 64
parallax = 0.1                 # how far it slides across the workspaces, up to 0.5
```

- **`focus`** is a point of the picture, as fractions of its width and height. `cover` crops the picture to
  fill the region, and it centres the crop on the focus as far as the picture reaches. The middle unless set,
  which is the crop every other wallpaper tool makes. The popover has a row for each fraction and a dot on the
  region: drag it, or focus it and step it with the arrows (Shift ×10, Alt ×0.1).
- **`dim`** and **`blur`** apply while an application window covers the screen. Where the compositor counts the
  windows on a workspace (Hyprland), the screen is covered while the workspace up on it holds one; everywhere
  else, while a maximized or fullscreen window is on it. The picture eases in and out of it with `[animation]`.
- **`parallax`** widens the box the picture covers by that share of the region and slides the picture across
  it by the workspace up: the screen's first workspace shows its left edge, its last its right. It needs
  `fit = "cover"`, and reduced motion (`[animation] reduced`) holds the picture still.

Values past what a key runs to are drawn at its end and reported by `hogar-shell layout check`, as is a
`parallax` on a region whose `fit` is not `cover`, which has no picture to spare and is not drawn.

## The transition

A wallpaper change is an **event on the live surface**, not a rebuild. A layout or config edit does rebuild the
region, and it remembers the picture it was showing so that it still fades from it. `[background] transition`
and `transition_ms` control the cross-fade.

## The library

`[paths] wallpapers` is scanned recursively, with a thumbnail cache so a grid of two hundred images does not
decode two hundred full-resolution photographs.

`[wallpaper]` — `enabled`, `recursive`, `extensions`, `max_entries`, `thumbnail_size`.

## What it needs

`wlr-layer-shell`. Dimming, blurring and parallax read the workspaces from `ext-workspace-v1` and the windows from
`wlr-foreign-toplevel-management`, with Hyprland's socket counting windows where there is one; a compositor
without them leaves the picture as it is.

## Related

- [Desktop widgets](widgets.md) — the clock and visualiser that used to share this surface.
- [Dynamic scheme](../theming/dynamic-scheme.md) — deriving the palette from whatever is showing here.
