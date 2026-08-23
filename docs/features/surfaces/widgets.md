---
id: widgets
kind: surface
title: Desktop widgets
summary: The clock face and audio visualiser drawn on the desktop, on a surface of their own.
status: stable
compositor: any
config: [widgets, visualiser, clock]
commands: []
deps: [wlr-layer-shell, libpipewire]
see_also: [wallpaper, clock, dynamic-scheme]
---

# Desktop widgets

One surface per monitor, over the wallpaper and under every window, carrying whatever `[widgets]` asks for. It
exists only while something is on it — no clock, no visualiser, no surface.

## Where it sits

**It is given the space the bars left, not the screen.** The wallpaper opts out of every exclusive zone and
covers the whole monitor; this surface respects them, so the compositor hands it exactly the area your windows
get, minus the same gap every panel keeps off a bar. Nothing here computes that: a bar appearing, an edge being
emptied or `[shape] frame` switching on moves the widgets with it.

So `position = "center"` is the centre of the *application* area. On a screen with bars on one side only, that
is deliberately not the centre of the glass.

## Why it is not the wallpaper's surface

A layer that changes forces its whole surface to be redrawn. The visualiser changes with the music, so drawing
it on the wallpaper meant rasterizing a screen-sized photograph up to sixty times a second on the CPU — on a
laptop, a core pinned and the machine throttling. On its own surface the same row repaints only itself.

## Clock

`[widgets.clock]` draws a clock face: `position`, `scale`, `format`, `date_format`, `show_date`, `invert`,
`margin`, `shadow`, `background`, `background_opacity`, `background_blur`.

`format` and `date_format` fall back to `[clock]`, so the face and the bar chip read the same unless you
deliberately give one its own — and the face drops the seconds the chip keeps, because a clock that ticks every
second is a surface that repaints every second.

**`background_blur` feathers the plate's own edge — it does not sample what is behind it.** No client-side
renderer can; asking the compositor is the route, through `ext-background-effect-v1` or a Hyprland
`layer_rule = blur, hogar-shell-widgets`, and neither is wired up yet.

## Visualiser

<a id="visualiser"></a>

`[widgets.visualiser]` draws the sound coming out of your speakers along an edge of that area: `enabled`,
`edge`, `reach`, `gap`, `margin`, `radius`, `opacity`, `accent`, `hide_when_silent`.

`[visualiser]` tunes the analysis itself — `bars`, `frame_rate`, `gain`, `smoothing`, `floor_db`,
`beat_sensitivity`.

It needs **`libpipewire`**, opened at runtime and read as the default sink's *monitor* — what is being played,
not what a microphone hears. Without it the bars stay silent.

This is the only service in the shell that publishes at a frame rate, and two things keep it from undoing an
idle desktop: nothing starts until something subscribes, and a frame identical to the one before it is not
published — so silence costs one final all-zero frame and then nothing at all. `hide_when_silent` is a reading
of that, not a timer.

## What it needs

`wlr-layer-shell`. `libpipewire` only for the visualiser.

## Related

- [Wallpaper layer](wallpaper.md) — the picture these are drawn over.
- [Clock](../modules/clock.md) — the same tick, as a chip on a bar.
