---
id: widgets
kind: surface
title: Desktop widgets
summary: The clock face and audio visualiser drawn on the desktop, placed by the layout.
status: stable
compositor: any
config: [visualiser, clock]
commands: [layout]
deps: [wlr-layer-shell, libpipewire]
see_also: [wallpaper, clock, dynamic-scheme]
---

# Desktop widgets

What the shell draws on the desktop itself — a clock face, a row of audio bars — placed on the **desktop
layer** of the active layout. They share that layer's window with everything else on it, and the window is on
screen only while the layer resolves something to show.

## Where they sit

**The layout says, not a config key.** A clock face is a widget instance in a `grid` area: the area's `anchor`
picks which of the nine places it is pinned to and its `padding` how far it is held off them. A row of bars is
an instance in a `dock` area: the area's `edge` is the edge they stand on and its `thickness` how far the
tallest reaches.

An area written `within = "usable"` is measured against **the space the bars left**, not the whole screen — so
`anchor = "center"` is the centre of the application area. On a screen with bars down one side only, that is
deliberately not the centre of the glass.

`hogar-shell layout show` prints the areas the running layout has; `hogar-shell layout check` says what is
wrong with one.

## Clock

`[clock.face]` is how a placed face is drawn: `scale`, `format`, `date_format`, `show_date`, `invert`,
`shadow`, `background`, `background_opacity`, `background_blur`. `[clock]`'s other keys dress the chip on a
bar.

`format` and `date_format` fall back to `[clock]`, so the face and the chip read the same unless you
deliberately give one its own — and the face drops the seconds the chip keeps, because a clock that ticks every
second is a surface that repaints every second.

**`background_blur` feathers the plate's own edge — it does not sample what is behind it.** For real blur an
area asks the compositor, through `backdrop = "blur"` and `ext-background-effect-v1`.

## Visualiser

<a id="visualiser"></a>

`[visualiser.face]` is how a placed row is drawn: `gap`, `radius`, `opacity`, `accent`, `hide_when_silent`.

`[visualiser]` tunes the analysis itself — `bars`, `frame_rate`, `gain`, `smoothing`, `floor_db`,
`beat_sensitivity` — and is shared with every other consumer, the media card's ring included.

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
