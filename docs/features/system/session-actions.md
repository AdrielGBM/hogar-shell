---
id: session-actions
kind: system
title: Session actions
summary: Lock, log out, suspend, hibernate, reboot and shut down, through logind.
status: stable
compositor: any
config: [lock]
commands: [session, shell]
deps: [logind]
see_also: [session, lock, idle]
---

# Session actions

```sh
hogar-shell session list                # what this machine supports
hogar-shell session do suspend
hogar-shell session do hibernate
hogar-shell shell quit                  # shut the shell down, not the session
```

## Through logind, not systemctl

Two reasons, and both are user-visible:

- **It works without privileges.** logind decides what the active session's user is allowed to do.
- **It can be asked first.** `CanHibernate` is a question, so the session menu greys out what this machine
  cannot do instead of offering a button that fails.

## Locking before sleep

`[lock] lock_before_sleep` locks and **waits for the lock to be granted** before suspending. "Asked to lock" and
"is locked" are different questions — between the request and the compositor granting it the desktop may still
be on screen — and waiting for the second is what makes this correct rather than racy.

## What it needs

**logind**, on the system bus. Without it the session actions are unavailable and the menu says so.

## Known limits

There is no `switch-user` and no `reboot-firmware`. Neither is a table entry yet.

## Related

- [session](../modules/session.md) — the menu.
- [Lock screen](lock.md).
