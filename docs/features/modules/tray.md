---
id: tray
kind: module
title: System tray
summary: One icon per running tray application.
status: stable
compositor: any
config: [tray]
commands: []
deps: []
see_also: [tray-host, statusicons]
---

# System tray

## What it shows

One icon per application that publishes a StatusNotifierItem. The row is drawn by the module itself rather
than inside a chip shell, because each icon carries its own click, middle-click, right-click and scroll — a
single chip-level handler would act on the row instead of on the application you clicked.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | the application's primary action |
| Middle-click | its secondary action |
| Right-click | its menu |
| Scroll | passed through to the application |

The right-click menu matters more than it looks: applications built on libappindicator — Steam among them —
implement no `Activate` at all and expose only a menu, so without it their icon is decoration.

## Configuring

`[tray]` — `enabled`, `compact`, `background`, `recolour`, `hidden` (ids to leave out), plus `[tray.icon_subs]`
for substituting an icon by glob pattern, most-specific-pattern-wins.

`recolour` is what makes a tray of mismatched vendor icons look like part of your bar.

## What it needs

Nothing to install: hogar-shell **is** the tray host. It owns `org.kde.StatusNotifierWatcher` — the registry
every tray application looks for before it will show itself — and registers as a host so applications that stay
hidden until one exists come out.

If another shell already owns the watcher, hogar-shell degrades to a plain client and reads the item list off
that watcher instead. Everything downstream is identical.

## Related

- [Tray host](../system/tray-host.md) — the D-Bus side, and what to do when two shells fight over the watcher.
