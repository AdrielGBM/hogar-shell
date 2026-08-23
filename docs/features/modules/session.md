---
id: session
kind: module
title: Session
summary: The power chip and the menu it opens.
status: stable
compositor: any
config: [lock]
commands: [session, lock]
deps: [logind]
panel: true
see_also: [logo, lock, session-actions]
---

# Session

## What it shows

A power icon. The panel behind it is the session menu: lock, log out, suspend, hibernate, reboot, shut down.

Actions this machine cannot perform are **greyed out rather than offered**, because logind can be asked first
— `CanHibernate` is a question, so the menu does not show a button that would fail.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the session menu |

```sh
hogar-shell session list                 # what this machine supports
hogar-shell session do suspend
hogar-shell lock on
```

## Configuring

`[lock]` for what locking does — see [Lock screen](../system/lock.md).

## What it needs

**logind**, on the system bus. Every action goes through it rather than through `systemctl`, for two reasons:
it works without privileges, since logind decides what the active session's user may do, and it can be *asked*
first.

Without logind the session actions are unavailable.

## Related

- [logo](logo.md) — the same panel behind a distribution mark.
- [Session actions](../system/session-actions.md) — what each action does, and `lock_before_sleep`.
