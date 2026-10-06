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
see_also: [wallpaper, clock, dynamic-scheme, compositor-rules]
---

# Desktop widgets

What the shell draws on the desktop itself — a clock face, a row of audio bars — placed on the **desktop
layer** of the active layout. They share that layer's window with everything else on it, and the window is on
screen only while the layer resolves something to show.

## Where they sit

**The layout says, not a config key.** A widget is an instance on a cell of a `grid` area. A grid has every
cell that fits its rectangle inside its `padding`, whatever is on them, so a widget stays where it was put: adding,
moving or removing another never shifts it. What the rectangle has left over once whole cells are taken is under
a cell's width, and the area's `anchor` says which side it is left on. A row of bars is an instance in a `dock`
area: the area's `edge` is the edge they stand on and its `thickness` how far the tallest reaches.

What has to sit somewhere that is no cell — the middle of the screen on every monitor — goes in a `free` area:
a rectangle, with an `anchor` saying where in it what it holds sits.

An area written `within = "usable"` is measured against **the space the bars left**, not the whole screen — so
`anchor = "center"` is the centre of the application area. On a screen with bars down one side only, that is
deliberately not the centre of the glass.

`hogar-shell layout show <name>` prints the areas a layout has (`layout list` marks the one being drawn); `hogar-shell layout check` says what is
wrong with one.

The built-in layout has an empty grid over the whole screen, 48 px off its edges, and a medium clock face in
the middle of the screen in a free area of its own.

## Arranging them

`hogar-shell layout edit desktop` edits the desktop layer on the screen it is run on. Drag a widget to other
cells, or onto the middle of another to stack the two into one Smart Stack (a group with `arrange = "pages"`); drag it
out of a stack to make it a widget again. A widget dropped where another one is never removes it: the one in the way moves to the free
cells nearest where it was, and nothing else moves. The corner handle on the selected widget steps it through
its sizes (S 2×2, M 4×2, L 4×4 cells, which are 80 px and 16 px apart unless the grid says otherwise).

`a` opens the palette of every module that draws as a widget. What is on a bar is another layer's and has
nothing to do with the desktop: a chip never becomes a widget, nor a widget a chip. Every drag has a key: Shift+arrows move a widget one cell, Ctrl+arrows step
its size, Ctrl+Shift+arrows stack it onto the widget that way, Alt+N makes a grid, Shift+N a container, and `w`
edits the workspace that is up alone. `?` lists them all.

### Containers

A container is a group that lays its widgets out in its own cells as a `row`, a `column`, a `grid` or `free`
(`arrange`, with each child's `weight`, `cell` or `rect`). Shift+N, or "Container" at the top of the palette, makes an
empty one on the largest span still free near the selection. Let a widget go anywhere over a container and it joins it
where the pointer is; drag a child inside to reorder it, move it to another cell or place it (a free child snaps to its
siblings unless Alt is held), or out to make it a widget of its own again at its smallest size. The selected child's
handle — at the end of a row's or column's child, at the corner of a grid's or free one's — sets its weight, span or
box, and Shift+arrows and Ctrl+arrows do the same by key; Alt+↑ selects the container. A child's menu has "Customize the
container…" (its arrangement, inner grid and gap) and "Take out of the container".

Outside edit mode each child is its own: it answers its own presses, wheel and menu. A child whose share holds no
widget is drawn as its chip, in the same chip shell a bar gives it and pressed the same way — what the layout binds
to its press, else the panel the layout gives it, else its module's own press or panel, which hangs off the chip. A
container claims the pointer only where its plate paints, so a press between the children of an unfilled one
(`style = { opacity = 0 }`) goes through to the window underneath. A Smart Stack keeps its wheel, arrow keys and dots.

### A desktop per workspace

Each workspace can have its own desktop, the way a launcher has pages: press `w` and further edits go into the rule
of the workspace that is up instead of every workspace, and `w` again switches back. See
[A desktop per workspace](../customization/layouts.md#a-desktop-per-workspace) for the TOML.

## Clock

`[clock.face]` is how a placed face is drawn: `scale`, `format`, `date_format`, `show_date`, `invert`,
`shadow`, `background`, `background_opacity`, `background_blur`. `[clock]`'s other keys dress the chip on a
bar.

`format` and `date_format` fall back to `[clock]`, so the face and the chip read the same unless you
deliberately give one its own — and the face drops the seconds the chip keeps, because a clock that ticks every
second is a surface that repaints every second.

**`background_blur` feathers the plate's own edge — it does not sample what is behind it.** For real blur an
area asks the compositor, through `backdrop = "blur"` and `ext-background-effect-v1` — see
[Compositor rules](../../guides/compositor-rules.md) for how that replaces a namespace-scoped `layer_rule`.

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
- [Compositor rules](../../guides/compositor-rules.md) — the namespace this used to be, and where it is now.
