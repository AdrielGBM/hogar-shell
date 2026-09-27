---
id: spacer
kind: module
title: Spacer
summary: An empty module that takes up whatever room is left.
status: stable
compositor: any
config: []
commands: [layout]
deps: []
see_also: [bars]
---

# Spacer

A bar's three zones only give three anchor points. `spacer` buys every arrangement in between — pinning one
module hard left and the next just off it, or splitting a zone in two — by growing to fill the slack instead of
hugging its content like every other module.

## What it shows

Nothing. It gets no chip shell, no padding, no hover state and no press state, so it is genuinely a gap rather
than an empty chip.

## Configuring

Put it in a zone's `children` like any other module, as many times as you need:

```toml
{ id = "start", place = "zone", zone = "start", children = [
    { id = "workspaces", module = "workspaces" },
    { id = "gap",        module = "spacer" },
    { id = "activewindow", module = "activewindow" },
] }
```

## What it needs

Nothing.

## Related

- [Bars](../surfaces/bars.md) — zones, shapes and per-monitor placement.
