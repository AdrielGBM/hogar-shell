---
id: compositor-constraints
kind: guide
title: Compositor constraints
summary: What Hyprland does with a layer-shell window, measured on 0.56.2, and the design rules that follow from it.
status: stable
compositor: hyprland
deps: [wlr-layer-shell]
see_also: [compositor-rules, developing]
---

# Compositor constraints

The shell is one fullscreen layer-shell window per wlr layer per output, so what the compositor does with such a
window decides the design. This page is the evidence: Hyprland's source at `hyprwm/Hyprland` @`1b85c7a`, and
measurements taken live on **Hyprland 0.56.2** where a decision rests on them. For what a compositor *rule* can
and cannot reach, see [Compositor rules](compositor-rules.md).

## The overlay layer costs direct scanout, mapped or not

Any existing surface on the overlay layer disables direct scanout and tearing. The check is the bare emptiness of
the monitor's overlay layer list (`src/output/Monitor.cpp:1793`), which a surface enters when it asks for its
layer surface and leaves only when it is destroyed. Confirmed live with a fullscreen video.

So the overlay window is **opened** only while it has content and closed when it has none — unmapping is not
enough — and an item that must persist above fullscreen costs its output scanout. That is why "above fullscreen"
is a per-area flag whose inspector and `hogar-shell layout check` say so, and why the default layout keeps the
overlay window unopened.

Mapping an overlay window takes 19 to 27 ms to the first frame when cold, and still about 20 ms with the buffer
retained, because the compositor's next refresh dominates. While mapped over a fullscreen video Hyprland's CPU
goes from 1.7–2.0 % to 3.1–3.4 % and the video is composited instead of scanned out. Anything that has to appear
within a frame therefore lives in a window that is already mapped: a drawer, a hover popout or a menu is drawn in
the window of the chip it hangs off and falls back to the overlay window only when that layer is hidden.

## Fullscreen hides the top layer and keeps the overlay

A fullscreen window hides the top layer and leaves the overlay in place. A bar, and everything anchored to it, is
gone under a fullscreen window; what must stay reachable is drawn in the overlay window.

## Damage is honoured per rectangle

The compositor uploads each damaged rectangle with `glTexSubImage2D`, so a quiet fullscreen surface is cheap only
if its damage is tight. At 60 Hz, full-surface damage cost Hyprland 12.0 % of a core against 7.9 % at 1080p. Every
region costs an upload too, which is why the benchmark caps how many a frame may have (see
[Developing](developing.md#the-window-benchmark)).

Blur cost scales with the surface's box intersected with its damage. `ext-background-effect-v1` is in Hyprland
0.56 and later, KWin 6.7, Mutter 51 and niri 26.04 and later; its region is a `wl_region`, so a rounded area
blurs square at its corners.

## Rules and animations are per namespace

A compositor's layer rules and animations are keyed by namespace, so with a few shared windows they cannot single
out a drawer or a card. Enter and exit motion is drawn in-client.

## Changing a window's layer

`set_layer` is cheap and quiet: no configure, a frame callback in under 1 ms (3.5 ms worst case), a hard cut
with no fade, and focus, windows and the cursor unmoved. With `exclusive` keyboard interactivity the surface
gains focus on promotion and loses it on demotion; `on_demand` focuses only after a pointer interaction.
It needs layer-shell v2; on an older compositor edit mode takes a fallback path that does not restack.

Hyprland appends a restacked surface to the end of its new layer's list (`LayerSurface.cpp:325-332`), so a window
raised into the overlay layer would cover whatever is already there. Edit mode therefore raises the edited
layer's window to the top layer, not the overlay: the top layer sits above every application window, and the
cost is that the background and desktop edit modes are hidden under a fullscreen window.

## Exclusive zones

An `exclusive_zone` is independent of the surface's size, and changing one re-tiles synchronously with window
animations. Reservation comes from the output-level layout only, never from a workspace variant, so switching
workspaces never re-tiles the desktop.

## Keyboard

- `exclusive` blocks refocusing any window while the surface is mapped.
- `on_demand` needs a click first.
- `wl_keyboard.modifiers` reaches only the focused surface, so a gesture gated on a modifier key is not portable
  outside an edit mode, and Caps and Num Lock state cannot be read by a surface that never holds focus.

## Input bugs

Hyprland issues #16156, #14136, #4281 and #6858 did not reproduce on 0.56.2 with one output. **Dragging across
outputs is untested**, so every drag in the shell keeps a keyboard or menu alternative, and a cross-output drag
should be checked live on two outputs before relying on it.

## A lock surface is not a layer surface

The session lock is `ext-session-lock-v1`: one surface per output from the moment the lock exists, a second is a
protocol error, and after `locked` only unlocking is legal. Hyprland 0.56.2 disconnects a client whose
`get_lock_surface` request crosses a refusal, so the shell creates lock surfaces only after a `wl_display.sync`
answers without `finished`. This is worth reporting upstream.
