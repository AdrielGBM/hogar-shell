---
id: mic
kind: module
title: Microphone
summary: The default input's level and mute.
status: stable
compositor: any
config: [audio, stack, toasts]
commands: [mic]
deps: [pw-dump, wpctl]
popout: true
see_also: [volume, mixer, osd]
---

# Microphone

## What it shows

The default source's level, and whether it is muted. This is the *volume* of the microphone — not whether
something is currently using it. There is no privacy indicator yet.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | toggles mute, with the OSD |
| Scroll | adjusts the level by `[audio] increment`, with the OSD |
| Hover | a popout card with the level and the device name |

```sh
hogar-shell mic mute
hogar-shell mic set 60
hogar-shell mic step -5
```

## Configuring

`[audio]` — `increment`, `max_volume`. Shared with [volume](volume.md), because the step you want on a
function key is the same step either way.

`[toasts.events] audio_input` raises a toast on a mute change; `[osd]` decides where the overlay appears.

## What it needs

**PipeWire.** `pw-dump` for the graph — devices, streams, levels and mutes — and `wpctl` for mutations.

Without `pw-dump` every audio module stays empty. Without `wpctl` audio can be read but not adjusted.

Reading and writing are deliberately different tools: `pw-dump --monitor` is *one* long-lived process that
parses JSON only when the graph changes, and `wpctl` is one fork per mutation. A level changed from another
mixer reaches the bar as PipeWire reports it, not up to two seconds later.

## Related

- [volume](volume.md) — the output side, same shape.
- [mixer](mixer.md) — every device and stream, with a pointer.
