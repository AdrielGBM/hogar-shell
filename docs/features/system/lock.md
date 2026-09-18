---
id: lock
kind: system
title: Lock screen
summary: One surface per monitor, and the only thing on it that matters is the password field.
status: stable
compositor: any
config: [lock]
commands: [lock]
deps: [ext-session-lock, libpam, fprintd, hyprland-lock-notify]
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
- **`hyprland-lock-notify`** — Hyprland only, and only for the restart below: without it, a lock hogar-shell dies
  holding is never taken back.
- **libpam**, loaded at runtime rather than linked. Linking it would put PAM headers between a user and a
  working bar; loading it on demand turns "no PAM here" into a question the shell can ask *before* it locks the
  screen rather than a failure it discovers after.

Every PAM call runs on a worker thread. `pam_unix` sleeps for seconds after a wrong password by design, and may
talk to a fingerprint reader or a network directory — on the UI thread that is a frozen shell.

## If hogar-shell dies while the screen is locked

The compositor keeps the session locked, which is what makes it a lock. With no locker left to draw, it shows a
fallback screen of its own — on Hyprland, a page saying the lockscreen app died. hogar-shell takes the lock back
the next time it starts:

- Once the compositor **grants** a lock, the shell writes `$XDG_RUNTIME_DIR/hogar-shell/held-lock`, naming the
  compositor session it holds the lock in: the device, inode and bind time of the Wayland socket it is connected
  through. `$WAYLAND_DISPLAY` alone would not do, because a restarted compositor binds `wayland-1` again. The file
  is removed when the lock is released, and when the compositor ends a lock it had granted.
- At startup, if that file names **this** compositor session **and the compositor says the session is still
  locked**, the shell takes the lock at once and mounts the **minimal** lock: the password field alone, without
  the configured clock, avatar or readings, because whatever killed the shell may be in them. Fingerprint and face
  unlock are not started for it.
- If the compositor says the session is **unlocked** — something else ended the lock, such as
  `hl.clear_crashed_lockscreen()` or another locker — the file is deleted and nothing is locked. If the file names
  another compositor session, it is deleted and nothing is locked.

Whether the session is still locked comes from `hyprland-lock-notify-v1`, Hyprland's protocol for exactly that
question. `ext-session-lock-v1` cannot be asked: requesting a lock to find out would lock an unlocked session.
**On a compositor without `hyprland-lock-notify-v1`, hogar-shell never takes a lock back.** It cannot tell a
session its dead predecessor still holds from one something else has unlocked since, and it does not guess; the
compositor's own recovery is the way back in. The file, for its part, is what keeps the restore to hogar-shell's
own crash: without it, a starting shell would reach for any locked session, including one a running hyprlock
holds.

hogar-shell does not restart itself. Something has to start it again: a supervisor, a keybind that works while
locked (below), or a command from another TTY.

### The compositor has to allow it

A compositor has to agree to hand a dead client's lock to a new one. On Hyprland that is
`misc:allow_session_lock_restore`, off by default:

```lua
-- hyprland.lua
hl.config({
  misc = {
    allow_session_lock_restore = true,
  },
})
```

```ini
# hyprland.conf
misc {
    allow_session_lock_restore = true
}
```

| When hogar-shell starts again | With the setting | Without it |
| --- | --- | --- |
| What you see | The password prompt, on every monitor | Hyprland's fallback page stays up |
| What the log says | that it took the lock back | that the compositor would not hand it back, and what to do |
| `held-lock` | kept until you unlock | kept, so a start after enabling the setting takes the lock back; forgotten by the first start that finds the session unlocked |

A bind marked to work while the screen is locked can start hogar-shell again without leaving the fallback page
(if hogar-shell is already running, the second copy exits at once):

```lua
hl.bind("SUPER + SHIFT + L", hl.dsp.exec_cmd("hogar-shell"), { locked = true })
```

```ini
bindl = SUPER SHIFT, L, exec, hogar-shell
```

### Getting back in without the setting

From another TTY (for example Ctrl+Alt+F3), logged in as the same user. `--instance 0` is the first entry of
`hyprctl instances`; a TTY has no `HYPRLAND_INSTANCE_SIGNATURE` to choose one for you.

**Hyprland's own recovery**, the command its fallback page shows, unlocks the session:

```sh
hyprctl --instance 0 eval 'hl.clear_crashed_lockscreen()'
```

Nothing else is needed. Start hogar-shell again as you normally would: it finds the session unlocked, forgets
`held-lock` by itself, and locks nothing.

**Or let hogar-shell take the lock back**, and unlock at its prompt:

1. Enable the setting in the running compositor. It lasts until Hyprland reloads its config.
   - Lua config: `hyprctl --instance 0 eval 'hl.config({ misc = { allow_session_lock_restore = true } })'`
   - `hyprland.conf`: `hyprctl --instance 0 keyword misc:allow_session_lock_restore 1`

   `keyword` is refused under a Lua config ("keyword can't work with non-legacy parsers").
2. Start hogar-shell inside that session:
   - Lua config: `hyprctl --instance 0 eval 'hl.exec_cmd("hogar-shell")'`
   - `hyprland.conf`: `hyprctl --instance 0 dispatch exec hogar-shell`
3. Switch back to Hyprland's TTY and unlock with your password.

## Known limits

`LockHandle::is_locked` reports only locks hogar-shell performed. `hyprland-lock-notify-v1` is bound for the whole
session and keeps the compositor's answer current, but the one thing that reads it is the startup decision whether
to take a lock back — kept live so that a lock or unlock landing between the first read and that decision still
counts. `lock status` and the lock screen still describe hogar-shell's own lock, not one another client holds.

One race is left at startup, and it is named rather than hidden: the compositor's answer is read, and the lock is
requested a loop turn later. A session unlocked in exactly that instant is locked again, behind the password
prompt.

## Related

- [Idle](idle.md) — the usual thing that triggers a lock.
- [Session actions](session-actions.md).
