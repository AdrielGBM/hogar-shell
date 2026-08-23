---
id: tray-host
kind: system
title: Tray host
summary: hogar-shell is the StatusNotifierWatcher, and a host besides.
status: stable
compositor: any
config: [tray]
commands: []
deps: []
see_also: [tray, notifications-daemon]
---

# Tray host

## Two D-Bus roles in one service

- It **owns `org.kde.StatusNotifierWatcher`** — the registry every tray application looks for before it will
  show itself.
- It **is a host**, registering `org.kde.StatusNotifierHost-<pid>`, so applications that stay hidden until a
  host exists (most of them) come out.

## Running alongside another shell

If something else already owns the watcher, hogar-shell **degrades to a plain client**: the item list is read off
that watcher's property instead of the local registry, and everything downstream is identical. You do not have
to pick one.

## Menus

A good part of the tray is only reachable through `com.canonical.dbusmenu`. Applications built on
libappindicator — Steam among them — implement no `Activate` at all and expose a menu instead, so without menu
support their icon would be decoration.

A menu is fetched when you ask to see one and thrown away when it closes: it is not ambient state, and the
fetch is a round trip to another application, so it never happens on the UI thread.

## What it needs

Nothing to install. Applications speaking StatusNotifierItem are what fill it; with none running the tray is
empty.

## Related

- [tray](../modules/tray.md) — the bar module, and `[tray]`.
