---
id: ipc
kind: system
title: IPC
summary: Every action the shell has is a command on a socket.
status: stable
compositor: any
config: []
commands: [shell]
deps: []
see_also: [scripting, keybinds, global-shortcuts]
---

# IPC

## What it is

Started with no arguments, `hogar-shell` runs the shell. With arguments it is a **client**: the request goes to
the running shell over a Unix socket and the reply is printed.

That is why a keybind is just a shell command, and why anything the UI does is scriptable.

```sh
hogar-shell ping
hogar-shell --list        # every target, command and argument
```

## The socket

`$XDG_RUNTIME_DIR/hogar-shell/<instance>.sock`, one per compositor instance. `HYPRLAND_INSTANCE_SIGNATURE` names
it, so two compositors on one login session get one socket each; outside Hyprland the name is still stable and
the client still finds the shell.

## The reply format

Every reply starts with `ok` or `err`, and the payload follows on the same line. A caller can branch on the
outcome **without parsing prose**, and the exit status mirrors the reply.

```sh
hogar-shell lock status        # ok ...
hogar-shell wifi connect x     # err ...
```

Most replies are a single line, but a few are tables — `shell status`, `shell caches`, `shell screens`,
`shell clients`, `scheme colors` — and those continue over further lines. Only the first carries `ok`/`err`. A
reply ends when the shell closes its side of the connection, so a raw client should send its request, shut down
its write half, and read to end-of-file:

```sh
printf 'shell status\n' | socat - UNIX-CONNECT:$sock
```

## Three commands are answered locally

`config schema`, `deps` and `man` are answered by the binary rather than sent to the shell: each is a function
of this build and this machine rather than of a running shell. `deps` is the case that matters — a dependency
report is for the machine where something is missing, and "nothing starts" is exactly when there is no shell to
ask.

## One table

The same table backs `--list`, `hogar-shell(1)` and what actually dispatches, so they cannot drift from one
another. Looking a command up and running it are separate steps, which is what lets the shell check whether a
request line is valid — for a global shortcut, or for an `[idle]` stage — **without performing it**.

## What it needs

Nothing.

## Related

- [Scripting](../../guides/scripting.md) — patterns for using this from a script.
- [Global shortcuts](global-shortcuts.md) — the same lines, delivered by the portal.
