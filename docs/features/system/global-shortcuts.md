---
id: global-shortcuts
kind: system
title: Global shortcuts
summary: Registering the shell's actions with the desktop portal so the compositor can bind them by name.
status: stable
compositor: any
config: []
commands: []
deps: [xdg-desktop-portal]
see_also: [keybinds, ipc]
---

# Global shortcuts

## What it is

hogar-shell registers its most-bound actions with `xdg-desktop-portal`, so the compositor can bind them by name
rather than by spawning a process.

```sh
hyprctl globalshortcuts    # what is registered, and under what name
```

Registered ids: `launcher` `dashboard` `notifications` `session` `dnd` `volume-up` `volume-down` `volume-mute`
`mic-mute` `brightness-up` `brightness-down`.

The name is `<appid>:<id>`, and on a non-sandboxed install the app id is empty — so it is `:launcher`, not
`hogar-shell:launcher`.

## Why it exists

Keybinds already work without it: `bind = SUPER, N, exec, hogar-shell panel toggle notifications` spawns the
client, which talks to the running shell over its socket. What that costs is a **process launch per press** — a
fork, an exec, a dynamic link and a connect, to deliver one line the shell answers in microseconds. A portal
shortcut is the same line over a connection that is already open.

## What you give up

The *binding* moves out of the shell's hands. hogar-shell says "I have an action called `launcher`"; the
compositor decides which keys reach it. **The portal registers actions, never keys** — it has no way to ask for
a particular one — so you still write the bind either way.

What you gain is that the compositor's own settings UI can list them by description, and that two applications
cannot silently claim the same chord.

## Why the list is short

Deliberately shorter than the IPC table. `hogar-shell audio set 40` is a scripting command, not a shortcut, and
registering every command would bury the ten anyone binds.

## What it needs

**`xdg-desktop-portal`** with the GlobalShortcuts interface. Without it, bind the IPC commands directly — that
route covers everything this one does and more.

## Related

- [Keybinds](../../guides/keybinds.md) — a starting set, and the Hyprland syntax.
- [IPC](ipc.md).
