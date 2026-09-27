---
id: notification-centre
kind: surface
title: Notification centre
summary: A full-height surface for what has arrived and what can be switched.
status: stable
compositor: any
config: [sidebar, notifications, utilities]
commands: [notifs]
deps: [wlr-layer-shell]
see_also: [notifications, utilities, notifications-daemon, compositor-rules]
---

# Notification centre

## What it is

The full-height counterpart to the bell drawer. It takes a whole screen edge, it scrolls, and it is where you
work through a morning's notifications.

```sh
hogar-shell notifs center toggle
hogar-shell notifs center open
```

## Drawer or centre?

| | Bell drawer | Notification centre |
| --- | --- | --- |
| Anchored to | the chip that opened it | a screen edge |
| Height | its content | the whole edge |
| Closes | when you look away | when you close it |
| For | a glance | working through a backlog |

Opening the centre closes whatever drawer was up — including the bell's own, which is the same notifications
seen the other way. Nothing closes the centre back: a chip pressed afterwards opens its drawer beside it and
leaves the centre where it is.

## What is on it

The notification history, grouped by application, and the **utilities panel's own toggles** — not a second set
of them. That is the whole reason the two were built together: two independent copies of "turn Wi-Fi off" would
drift the day one of them gained a toggle.

Both halves are switchable: `[sidebar] show_history` and `show_toggles`.

## Configuring

`[sidebar]` — `edge`, `size`, `show_history`, `show_toggles`.

The toggles themselves come from `[utilities] toggles`; the history's grouping and thresholds from
`[notifications]`.

## Where it lives

The centre is a node rather than a surface of its own, and it does not anchor to a chip: it always opens in
`hogar-shell-overlay` on the focused output, whether it was reached from the bell, `notifs center toggle` or a
keybind. See [Compositor rules](../../guides/compositor-rules.md) for the namespace this used to have of its
own, and where a rule against it reaches now.

## What it needs

`wlr-layer-shell`, plus whatever each toggle needs.

## Related

- [Notification daemon](../system/notifications-daemon.md) — where the history comes from.
- [utilities](../modules/utilities.md) — the toggles, and where they are configured.
- [Compositor rules](../../guides/compositor-rules.md).
