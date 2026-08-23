---
id: lock
kind: system
title: Lock screen
summary: One surface per monitor, and the only thing on it that matters is the password field.
status: stable
compositor: any
config: [lock]
commands: [lock]
deps: [ext-session-lock, libpam, fprintd]
see_also: [idle, session-actions]
---

# Lock screen

## What it is

An `ext-session-lock-v1` surface covering every output. The compositor keeps it up **even if this process
dies**, and it gives it the keyboard — there is no scrim, no dismiss, and no way out but authenticating.

```sh
hogar-shell lock on
hogar-shell lock toggle
hogar-shell lock status     # locked? and can this machine lock at all?
```

## What is on it

The password field, the avatar and the clock are drawn only on the monitor the pointer or the compositor
focused; the rest stay a plain background. Optional readings — media, notifications, resources, weather — are
switched by `[lock]` keys.

**Every row on the lock screen is a reading, never a control.** A control on a lock screen reaches into another
application, which is the one thing a lock exists to prevent.

The screen never authenticates. It collects a password and hands it over.

## Two things are checked before the screen is covered

Never after:

1. that the compositor implements `ext-session-lock-v1`,
2. that **PAM** can be loaded.

A lock this process cannot undo is the one failure with no way out, so it is refused with a message instead.
`hogar-shell lock status` gives you that answer without locking.

## Asked to lock, and locked, are different questions

Between requesting a lock and the compositor granting it, the desktop may still be on screen. Anything
security-sensitive — suspending, for instance — has to wait for the *second*. `[lock] lock_before_sleep` is
what does that for the sleep case.

## Biometrics

Both are **alternatives** to the password, never replacements. They run alongside the field, they stop the
moment the screen unlocks, and each has its own attempt budget — after which the shell stops asking and leaves
the password as the only way in. A biometric that keeps retrying forever is a sensor an attacker can keep
feeding.

| | Needs | Keys |
| --- | --- | --- |
| Fingerprint | **fprintd** on the system bus | `fingerprint`, `max_fprint_tries` |
| Face | **`howdy`** installed | `howdy_command`, `max_howdy_tries` |

Neither is required, and neither has to be switched off explicitly when it is absent: fprintd is simply not on
the bus without a reader, and `howdy` is a command that is not installed.

## Configuring

`[lock]` — `pam_service`, `pam_library`, `max_tries`, `lockout_seconds`, `trigger_on_wake`,
`lock_before_sleep`, `show_avatar`, `show_media`, `show_notifications`, `show_resources`, `show_weather`,
`hide_notifs`, plus the biometric keys above.

`pam_library` names a path on a distribution that puts libpam outside the loader's search path.

## What it needs

- **`ext-session-lock`** — without it, `lock status` says the session cannot be locked.
- **libpam**, loaded at runtime rather than linked. Linking it would put PAM headers between a user and a
  working bar; loading it on demand turns "no PAM here" into a question the shell can ask *before* it locks the
  screen rather than a failure it discovers after.

Every PAM call runs on a worker thread. `pam_unix` sleeps for seconds after a wrong password by design, and may
talk to a fingerprint reader or a network directory — on the UI thread that is a frozen shell.

## Known limit

`LockHandle::is_locked` reports only locks hogar-shell performed. A lock taken by something else is not observed;
`hyprland-lock-notify-v1` is the protocol for that and is unbound.

## Related

- [Idle](idle.md) — the usual thing that triggers a lock.
- [Session actions](session-actions.md).
