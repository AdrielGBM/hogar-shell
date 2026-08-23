---
id: recording
kind: system
title: Screen recording
summary: Driving a recorder that already exists, and stopping it properly.
status: stable
compositor: any
config: [recorder, paths]
commands: [record]
deps: [wf-recorder, gpu-screen-recorder]
see_also: [screenshot, utilities]
---

# Screen recording

Unlike a screenshot, a recording is not something a shell can do itself: it is an encoder, a muxer and a frame
pump, and every Wayland session already has one. So hogar-shell owns the **session** — which backend, what it is
recording, since when — rather than the pixels.

## Recording

```sh
hogar-shell record start [screen|output|region]
hogar-shell record stop
hogar-shell record toggle          # stop, or start one of the whole screen
hogar-shell record pause           # on a backend that can
hogar-shell record status
hogar-shell record list            # newest first
```

A region opens the picker first, the same one screenshots use.

## Stopping properly

`stop` sends **`SIGINT`** and lets the encoder write its own trailer. A recorder that is killed rather than
interrupted leaves an unplayable file, which is the one thing a wrapper around a recorder has to get right.

One process at a time, tracked by pid, owned by a waiter thread rather than by whoever pressed stop — so a
recorder that exits on its own (a full disk, a missing codec) updates the shell exactly like one you stopped.

## Configuring

`[recorder]` — `backend`, `fps`, `audio`, `audio_device`, `file_name`, `max_entries`, `notify`.
`[paths] recordings` is the folder.

## What it needs

One of two programs:

| Backend | What it gives you |
| --- | --- |
| **`wf-recorder`** | CPU encoding |
| **`gpu-screen-recorder`** | GPU encoding, and the only backend that can **pause** |

With neither installed, recording is unavailable — the toggle is present and refuses. With only `wf-recorder`,
everything works except `record pause`.

## Related

- [Screenshot](screenshot.md) — the same region picker.
- [utilities](../modules/utilities.md) — `record` as a toggle tile.
