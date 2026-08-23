---
id: brightness
kind: module
title: Brightness
summary: Screen brightness, for the internal panel and for external monitors.
status: stable
compositor: any
config: [brightness]
commands: [brightness]
deps: [backlight, ddcutil, logind]
popout: true
see_also: [osd, dependencies]
---

# Brightness

A laptop has one panel behind a sysfs backlight; a desk has two or three monitors, none of which has one. Both
are the same question — "make that screen dimmer" — so the service publishes every controllable display and
the chip shows the primary one.

## What it shows

The primary display's level. Primary is the **internal panel** where there is one, because that is the screen a
laptop's brightness keys mean.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | shows the OSD without changing anything |
| Scroll | adjusts by `[brightness] increment`, with the OSD |
| Hover | a popout card with the level |

`hogar-shell brightness up` with no display named means the primary panel, **not** every screen. It is the one
mutation where an unnamed target is not "all of them", because it is overwhelmingly a laptop's function key.
Name a connector (`brightness up DP-2`) or spell out `all` for the rest.

## Configuring

`[brightness]` — `increment`, `external`.

## What it needs

Two routes, and they are independent:

- **Internal panel** — `/sys/class/backlight` to read, and **logind** to write. Going through logind is what
  makes it work without root and without a udev rule: logind permits the active session to set a backlight.
  The chip notices a change made by something else from the **kernel's uevent socket**, so the reading follows
  the function keys instead of drifting out of step. That needs nothing installed: it is the same broadcast
  `udevadm monitor` reads, listened to directly, and it falls back to a five-second poll where the socket
  cannot be opened.
- **External monitors** — **`ddcutil`**, which speaks DDC/CI over the I²C bus behind each output. There is no
  library worth binding; the CLI is the interface.

Without `/sys/class/backlight` the internal panel is unavailable and external monitors still work. Without
`ddcutil` only internal panels are dimmable. Without either the chip has nothing to show.

## Known limit

An external monitor's level is read **once at detection** and then tracked optimistically. A `getvcp` costs
tens to hundreds of milliseconds per monitor, so polling one would be a permanent background cost for a value
that only changes when somebody changes it — which means a change made with the monitor's own buttons is not
noticed. `hogar-shell brightness refresh` re-detects, which is also what to run after plugging a monitor in.

## Related

- [OSD](../surfaces/osd.md) — the overlay this chip and its keybinds show.
