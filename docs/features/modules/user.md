---
id: user
kind: module
title: User
summary: Who is signed in — the user's picture and name — drawn as the lock screen and the dashboard draw it; not yet placeable on its own.
status: partial
compositor: any
config: [dashboard, lock]
commands: []
deps: []
see_also: [dashboard, lock]
---

# User

## What you can do with it today

Nothing places this module on screen yet: there is no config key that puts it on the desktop or the lock
screen, and the desktop's widget surface draws only the [clock](clock.md) and the
[visualiser](visualiser.md). Placing widgets arrives with the layout model.

What you see today is the same picture and name the module draws, on the [lock screen](../system/lock.md) and on the
[dashboard](dashboard.md)'s user card — all three are built by this module, so they never disagree.

## What it shows

The user's picture, round, and their login name from their account's passwd entry. With no picture it draws a
generic silhouette rather than a gap. As a widget, a small one stacks the two and a medium one sets them side
by side.

It is a reading and nothing else: no chip, no panel and no press. Changing the picture is the dashboard's user
card, the one place that takes that input.

## Configuring

The picture is `[dashboard] avatar`, or the first of `~/.face`, `~/.face.icon` and the AccountsService icon
when that is empty. `[lock] show_avatar` decides whether the lock screen shows it.

## What it needs

Nothing. Both readings are local.

## Related

- [dashboard](dashboard.md) — where the picture is chosen.
- [lock](../system/lock.md) — the other place it is drawn.
