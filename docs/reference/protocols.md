---
id: protocols
kind: guide
title: Wayland protocols
summary: What the backend speaks, what it does not, and which features that decides.
status: stable
compositor: any
deps: [wlr-layer-shell, ext-session-lock, ext-idle-notify, ext-image-copy-capture, wlr-screencopy]
see_also: [dependencies, install]
---

# Wayland protocols

## The authoritative list

**[`crates/platform-wayland/README.md`](../../crates/platform-wayland/README.md)** is the survey: every
protocol, what it unlocks, and whether the backend speaks it — grouped by that state and kept next to the code,
so it cannot drift. This page is the short version for a reader deciding whether hogar-shell will work for them.

## What is bound

| Protocol | What it gives you |
| --- | --- |
| `wlr-layer-shell-unstable-v1` | every surface the shell draws. **The only hard requirement.** |
| `ext-session-lock-v1` | [the lock screen](../features/system/lock.md) |
| `ext-idle-notify-v1` | [idle stages](../features/system/idle.md) |
| `ext-image-copy-capture-v1` | [screenshots](../features/system/screenshot.md) and the window preview |
| `wlr-screencopy-unstable-v1` | the same captures on an older compositor |
| `ext-data-control-v1` / `zwlr-data-control-unstable-v1` | [owning the clipboard](../features/system/clipboard.md) without focus |
| `ext-output-image-capture-source-v1` | naming an output as a capture source |
| `xdg-output-unstable-v1` | output geometry on a compositor predating `wl_output` v4 |

Where two protocols answer one need, the newer is tried first and the older is the fallback — and naming one
explicitly (`[screenshot] backend`) means that route or nothing, so debugging one never silently gives you the
other.

## What is not bound, and what that costs

| Protocol | What is missing without it |
| --- | --- |
| `ext-workspace-v1` | [workspaces](../features/modules/workspaces.md) runs on Hyprland IPC instead |
| `ext-foreign-toplevel-list-v1` | [activewindow](../features/modules/activewindow.md) and [windowinfo](../features/modules/windowinfo.md) likewise; also blocks per-window capture |
| `wlr-foreign-toplevel-management-v1` | acting on another window portably |
| `wp-fractional-scale-v1` + `wp-viewporter` | rendering on the true device-pixel grid; `[theme.scale]` is a manual multiplier |
| `zwlr-gamma-control-v1` | night light / colour temperature |
| `zwlr-output-management-v1` | configuring outputs from the shell |
| `ext-background-effect-v1` | blur behind a panel |
| `idle-inhibit-unstable-v1` | preventing idleness, as opposed to observing it |
| `wp-cursor-shape-v1` | setting a cursor at all — the pointer keeps whatever image it entered with |

## Which compositors this adds up to

Everything except the four modules above works anywhere `wlr-layer-shell`, `ext-session-lock` and
`ext-idle-notify` do. **Those four need Hyprland today** — not because no protocol exists, but because the
protocol route is not implemented yet. Every page that depends on it says `compositor: hyprland` in its front
matter, and the [feature index](../features/README.md) marks them.

## Asking your compositor

```sh
hogar-shell deps list      # the protocols with a dependency row, probed against your session
```

A protocol can only be asked of a compositor, so from a process with no session the answer is **unknown**
rather than absent.

## Related

- [Dependencies](../getting-started/dependencies.md) — the other four kinds of dependency.
