---
id: mixer
kind: module
title: Mixer
summary: Every device and every stream in the audio graph, each with its own level and mute.
status: stable
compositor: any
config: [audio]
commands: [audio]
deps: [pw-dump, wpctl]
panel: true
see_also: [volume, mic, media]
---

# Mixer

The shell has been able to *read* the whole audio graph since the PipeWire service replaced the `wpctl` poll,
and `hogar-shell audio` has been able to drive all of it. With a pointer there was no way to reach anything but
the default sink. This is that missing half.

## What it shows

One row per adjustable node: output devices, input devices, and the applications currently playing — each with
its own level, mute and default marker.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the mixer panel |

Inside the panel, drag a row's slider to set its level, or click its icon to mute it. Making a device the
default is a click on its row.

```sh
hogar-shell audio sinks
hogar-shell audio streams
hogar-shell audio default <id>
hogar-shell audio set <id> 40
```

## Configuring

`[audio]` — `max_volume` (default 150 %, clamped to 100–300) is the one that matters here, since a per-stream
slider is where you would push past 100 %.

## What it needs

**PipeWire**: `pw-dump` to read the graph, `wpctl` to change it. Without the first the panel is empty.

Nothing in the panel holds its own state — every row's level, mute and default marker is read out of the live
graph by node id. That is what lets a row survive its own drag: a slider that rebuilt on every value it set
would drop the gesture that was setting it.

## Related

- [volume](volume.md) — the default sink only, on a chip.
- [media](media.md) — the same applications, as players rather than as streams.
