---
id: notifications-daemon
kind: system
title: Notification daemon
summary: hogar-shell is the freedesktop notification daemon — nothing else to install.
status: stable
compositor: any
config: [stack, notifications]
commands: [notifs]
deps: []
see_also: [notifications, notification-centre, toasts]
---

# Notification daemon

hogar-shell owns `org.freedesktop.Notifications` itself. There is no second daemon to install, and running one
alongside means whichever claims the bus name first wins.

## What it does

Receives notifications, pops them, groups them by application, stores them, and keeps a history that survives a
restart.

```sh
hogar-shell notifs dnd toggle
hogar-shell notifs mute <app> [on|off|toggle]
hogar-shell notifs muted
hogar-shell notifs clear [app]
hogar-shell notifs center toggle
```

## Do-not-disturb, and what it means

Under DND a notification is **kept, not shown** — it goes to history and does not pop. That is the property
that distinguishes a notification from a [toast](../surfaces/toasts.md): a toast under DND would be
meaningless, because a toast is feedback about something you just did.

Per-application mute is a separate, binary switch.

## Configuring

`[notifications]` — `body_lines`, `group_by_app`, `group_preview_num`, `open_expanded`, `critical_sticky`,
`clear_threshold`, `action_on_click`, `fullscreen`, `sound`.

Where a popup appears and how wide it is are not the daemon's, or `[stack]`'s: a notification popup, a toast
and an OSD are one column, and where that column sits and how wide it is are the layout's own `stack` area.
How many show at once and how long each stays are still `[stack]` — `max_visible`, `timeout_ms`.

`critical_sticky` keeps urgency-critical notifications up until they are dismissed. `fullscreen` is the policy
for what happens while a window is fullscreen.

## Interacting

Actions, swipe-to-dismiss and click-through-to-the-application all work from the popup and from the history.

## Replacing and closing

A notification can be replaced in place by naming its id, as `notify-send -r <id>` does, from any process — the spec lets any client name any live id, and `notify-send` runs as a new process every time. An id that names nothing live is treated as 0 and gets a fresh id, so a client never picks one in advance. `CloseNotification` likewise closes any notification. The one exception is the shell's own notices: a client naming one gets a fresh id instead, and cannot close it, since their buttons run the shell's own commands. The shell's notices are plain text, never read as markup.

## What it needs

Nothing. The daemon is part of the shell, and the popup host is always mapped — a notification can arrive at any
moment and the daemon owns the timing.

## Known limits

- **Per-application rules stop at mute.** There is no matching on summary or body, no forcing an urgency, and
  no routing straight to history.
- **A notification's action cannot be invoked over IPC.** `notifs` answers `clear`, `mute`, `muted`, `dnd` and
  `center`; acting on an action is UI-only.

## Related

- [Notification bell](../modules/notifications.md) — the chip and its drawer.
- [Notification centre](../surfaces/notification-centre.md) — the full-height surface.
