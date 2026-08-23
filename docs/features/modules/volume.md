---
id: volume
kind: module
title: Volume
summary: The default output's level and mute.
status: stable
compositor: any
config: [audio, stack, toasts]
commands: [volume, audio]
deps: [pw-dump, wpctl]
popout: true
see_also: [mic, mixer, osd]
---

# Volume

## What it shows

The default sink's level, and whether it is muted.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | toggles mute, with the OSD |
| Scroll | adjusts by `[audio] increment`, with the OSD |
| Hover | a popout card with the level and the device |

The chip stays a level, a mute and a wheel on purpose — a chip that opened a panel could no longer toggle mute
with the same press. The pointer route to a non-default device is [mixer](mixer.md).

```sh
hogar-shell volume up
hogar-shell volume set 40
hogar-shell volume mute
```

## Configuring

`[audio]` — `increment`, `max_volume` (default 150 %, clamped to 100–300).

`[osd]` places the overlay; `[toasts.events] audio_output` decides whether a change also raises a toast.

## What it needs

**PipeWire**: `pw-dump` to read, `wpctl` to write. Without `pw-dump` the chip is empty; without `wpctl` it can
be read but not adjusted.

A mutation no longer re-reads afterwards — the graph monitor reports the real value on its own — so a set costs
one fork rather than two.

## Related

- [mic](mic.md), [mixer](mixer.md).
- [Visualiser](../surfaces/widgets.md#visualiser) — the other audio consumer, and the only service that
  publishes at a frame rate.
