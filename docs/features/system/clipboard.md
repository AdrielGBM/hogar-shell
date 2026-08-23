---
id: clipboard
kind: system
title: Clipboard
summary: The shell owns the selection itself — no `wl-copy` in the loop.
status: partial
compositor: any
config: [screenshot]
commands: [screenshot]
deps: []
see_also: [screenshot, dependencies]
---

# Clipboard

## What it is

When `[screenshot] copy` is on, a capture goes to the clipboard. The shell puts it there **itself**, over a
Wayland protocol, rather than spawning `wl-copy` to hold the selection for it.

## How

`ext-data-control-v1` where the compositor has it, `zwlr-data-control-unstable-v1` where it does not. Both let
a client own the selection **without holding focus**, which is exactly what a bar needs — it never has focus.

A copy registers a source and returns as soon as the selection is registered, leaving one thread to serve the
bytes on demand. The compositor's `cancelled` event ends that thread when something else copies.

## What it needs

One of those two protocols. Without either, `[screenshot] copy` does nothing and `save` still works.

Neither has a row in `hogar-shell deps list`, because neither is probed as a named dependency — the shell asks
the compositor for the manager it wants at the moment it copies.

## Known limit

**Write only.** Nothing here reads the current selection or watches it change, which is the whole other half of
both protocols. That is also why there is no clipboard history and no `clipboard` IPC target: the reading side
is what a history would be built on.

## Related

- [Screenshot](screenshot.md) — the one feature that copies today.
