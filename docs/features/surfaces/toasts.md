---
id: toasts
kind: surface
title: Toasts
summary: The small, self-dismissing messages the shell says about itself.
status: stable
compositor: any
config: [stack, toasts]
commands: [toast, layout]
deps: [wlr-layer-shell]
see_also: [osd, notifications-daemon]
---

# Toasts

## What it is

Feedback about something you just did: Caps Lock came on, a screenshot was taken, the config reloaded, a VPN
came up.

**Deliberately not freedesktop notifications.** A notification is a record — it belongs to an application, it
goes into history, it can be acted on, and under do-not-disturb it is *kept* rather than shown. "Caps Lock is
on" is none of those: it is feedback about a key you just pressed, it is worthless a second later, and filing
it in the notification history would be filing your own keystrokes.

So toasts have their own queue and their own switches, and nothing here reaches the daemon.

## Which events raise one

`[toasts.events]` — one switch each:

`audio_input` `audio_output` `charging` `config_loaded` `dnd` `game_mode` `kb_layout` `lock_keys`
`now_playing` `recording` `screenshot` `vpn`

## From a script

```sh
hogar-shell toast show "backup finished"
hogar-shell toast clear
```

Which makes the shell's own feedback channel available to anything you write.

## Where it appears

On whichever monitor the compositor reports as focused **at the moment the toast is posted** — for feedback
about a keypress, that is the screen you are looking at.

The stack's window is opened on the first card and closed with the last, so an idle session carries no overlay
at all. Expiry runs on a thread of its own, because toasts are posted from wherever the event happened and the
one thing they cannot rely on is the stack being up to time them out.

## Configuring

`[toasts]` — `enabled`, plus `[toasts.events]`. Where a toast appears is not a toast setting: a toast, a
notification popup and an OSD are one column — the layout's `Stack` area, by default in the overlay layer,
pinned to the top right — and its place, size and which outputs it appears on are that area's own `anchor`,
`offset`, `width`, `flow` and `output_policy`.

A stack lays its cards down a column by default. With `flow = "row"` it lays them side by side, each `width` wide,
growing from the side its anchor names: a left anchor grows right, a right one grows left, and a middle one both ways.
A row holds only as many cards as fit across its box, or `max_visible` where that is fewer, and the edit mode's
[Try cards](../customization/edit-modes.md#overlay) follow the flow too.

A screen can have several stacks, and each card goes to one of them by its `routes`: the first stack of the
screen with a route that takes the card — a card kind (`notification`, `toast`, `osd`), the app a notification
came from, its urgency — and otherwise the first stack with no routes at all, wherever it is in the layer. So a
stack in the middle routed `critical` notifications takes those, while toasts and every other notification stay
in the corner stack that routes nothing. The overlay edit mode makes stacks (Shift+N), pins them by their first
card or to one of nine anchors, and edits their routes in the popover.

How many cards show at once and how long each stays is still `[stack]` — `max_visible`, `timeout_ms`,
`clear_threshold`. The space between two cards is the shell's `spacing` token, the same one that separates two
chips on a bar.

`max_visible` bounds the column but does not silence anyone: **each of the three — a notification, a toast, an
OSD — is guaranteed one card before the rest of the room is shared out**, so a brightness reading you asked for
by pressing a key is never queued behind notifications you did not. If all three are speaking at once and
`max_visible` is smaller than that, the column is three cards tall.

A card's timeout starts when it reaches the screen, not when it arrives — one that waited behind a full column
gets its whole life when it finally shows.

## Known limit

Turning a `[toasts.events]` switch off at runtime stops the toast, but leaves the watcher — and therefore the
service behind it — running. Restart the shell to actually quiet it.

## What it needs

`wlr-layer-shell`.

## Related

- [OSD](osd.md) — for levels rather than text.
- [Notification daemon](../system/notifications-daemon.md) — for messages that belong to an application.
- [Layout reference](../../reference/layout.md#areakindstack) — `AreaKind::Stack`, the column's own placement.
