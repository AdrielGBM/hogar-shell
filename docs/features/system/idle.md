---
id: idle
kind: system
title: Idle
summary: Timers that fire a command when the seat goes quiet, and what holds them off.
status: stable
compositor: any
config: [idle]
commands: [idle]
deps: [ext-idle-notify, wlr-output-power-management]
see_also: [lock, session-actions, battery]
---

# Idle

## What it is

Each stage in `[idle]` becomes one `ext-idle-notify-v1` notification, and the compositor — the only thing that
sees your input devices — says when it elapses.

What a stage then *does* is a **request line the shell already answers**, so `hogar-shell --list` is the whole
vocabulary: anything bindable to a key is bindable to a timeout.

```toml
[[idle.stages]]
timeout = 300
action  = "lock on"

[[idle.stages]]
timeout = 600
action        = "shell dpms off"
return_action = "shell dpms on"
```

`return_action` runs when the seat wakes, if that stage had fired. Leaving it empty leaves the action standing
— which is right for a lock and wrong for a blanked screen, so a dpms stage pairs the two.

## Inhibiting

```sh
hogar-shell idle status          # armed? what is holding it off?
hogar-shell idle inhibit toggle
```

An inhibit is expressed by having **no notification at all** rather than by ignoring one that fires. An
inhibited stage is not armed, so the compositor never counts down for it, and un-inhibiting re-arms from zero
— which is what you expect after closing the film that was holding the screen awake, rather than the lock
screen appearing the moment it ends.

## Automatic inhibits

`[idle]` — `inhibit_when_audio`, `inhibit_when_charging`, `respect_inhibitors`.

`respect_inhibitors` picks which protocol request is used: honouring other clients' idle inhibitors, or
reporting raw input idleness. The second needs `ext-idle-notify` **v2**; on a v1 compositor the shell warns
once and uses the inhibitor-respecting request rather than silently reporting the wrong thing.

## What it needs

**`ext-idle-notify`.** Without it idle timers never arm, so nothing locks or blanks on its own.

Note what the shell *cannot* do: it can **observe** idleness and cannot **prevent** it.
`idle-inhibit-unstable-v1` is unbound, so `idle inhibit on` silences hogar-shell's own timers and leaves every
other idle consumer — including the compositor's — untouched.

## Known limit

`inhibit_when_charging` is a blanket disable on mains. A *different schedule* on battery is not expressible.

## Related

- [Lock screen](lock.md) — the usual thing an idle stage does.
