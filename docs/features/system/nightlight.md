---
id: nightlight
kind: system
title: Night light
summary: Warm the screen by setting each output's gamma ramp, with no helper process running alongside.
status: stable
compositor: any
config: []
commands: [nightlight]
deps: [wlr-gamma-control]
see_also: [brightness, idle]
---

# Night light

Warms every screen by setting its gamma ramp over `wlr-gamma-control`, which every wlroots compositor speaks.
Nothing else has to be running: no `hyprsunset`, no `gammastep`, no `wlsunset`.

```sh
hogar-shell nightlight on          # 4000K, the default
hogar-shell nightlight on 3200     # or name a temperature
hogar-shell nightlight toggle      # what a keybind binds to
hogar-shell nightlight off
hogar-shell nightlight status
```

Temperatures run from 1000K to 10000K. A value outside that is refused by name rather than clamped: a caller
who typed `400` meant something, and silently warming to 1000K would hide the typo behind a screen that went
orange.

## What holds the tint

The compositor restores the original ramp the moment the shell's gamma control is destroyed. That is the
protocol keeping a crashed client from leaving a screen orange for ever, and it has two consequences worth
knowing:

- **Turning the night light off is dropping the control**, not sending a neutral ramp. So does the shell
  exiting — quitting hogar-shell gives every screen its own colour back.
- **The shell holds one control per output for as long as the tint lasts.** A monitor plugged in while the
  night light is on is warmed to match the others rather than staying blue.

## One client at a time

A compositor grants gamma control to one client per output. If something else already holds it — a
`wlsunset` left running, or Hyprland's own `hyprsunset` — the compositor refuses, and `nightlight` says so
instead of fighting for it. Stop the other program and try again.

`status` reports what the shell asked for, not a reading from the compositor: the protocol has no way to ask
what the gamma currently is, deliberately, since the ramp is per-client state.

## Not yet

There is no schedule. Turning the night light on at sunset means a timer of your own for now — a systemd
timer, or a `cron` line calling `hogar-shell nightlight on`.
