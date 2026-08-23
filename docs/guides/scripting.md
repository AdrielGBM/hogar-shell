---
id: scripting
kind: guide
title: Scripting
summary: Driving the shell from a script, and reading its answers.
status: stable
compositor: any
commands: [shell, apps, audio, notifs, wallpaper, scheme]
deps: []
see_also: [ipc, keybinds]
---

# Scripting

Every action the shell has is a command on a socket, so anything the UI does, a script can do.

```sh
hogar-shell --list        # the complete menu; this page is patterns, not a copy of it
```

## Branching on the answer

Replies start with `ok` or `err`, and the exit status mirrors that — so you never have to parse prose:

```sh
if hogar-shell lock status >/dev/null; then
  echo "the shell answered"
fi

state=$(hogar-shell media status) || state="no player"
```

## Reading state

The commands that answer rather than act:

```sh
hogar-shell shell outputs          # the compositor's monitors
hogar-shell shell screens          # with mode, scale and make
hogar-shell shell clients          # every open window
hogar-shell audio sinks
hogar-shell brightness list        # every controllable display
hogar-shell wifi list
hogar-shell record status
hogar-shell scheme colors          # every palette token, name and hex
```

`scheme colors` is the one worth knowing about: it is how a script themes something the export files do not
cover.

## Saying something

```sh
hogar-shell toast show "backup finished"
```

A [toast](../features/surfaces/toasts.md) rather than a notification, deliberately — see that page for which
one you want. For something that should be *recorded*, send a real notification with `notify-send`; hogar-shell is
the daemon that receives it.

## Two rules worth knowing

**Where a screenshot goes is config, not a flag.** `[screenshot] copy` and `save` decide whether a capture
reaches the clipboard, a file, or both, so one command behaves the way you set it up.

**`brightness up` with no display named means the primary panel, not every screen.** It is the one mutation
where an unnamed target is not "all of them". Name a connector, or spell out `all`. Every other mutation —
`wallpaper set`, `wallpaper clear` — does mean all of them when nothing is named.

## Running a script from the shell

Three places take a command line, and all three take the *same* vocabulary:

| Where | What it runs |
| --- | --- |
| `[[idle.stages]] action` / `return_action` | on a timeout, and on wake |
| `[launcher] actions` | from the launcher's `>` mode |
| `[theme.export] hooks` | after a palette is written |

Anything in `hogar-shell --list` is valid in all three, and a request line is validated **without being run** — so
a typo in an idle stage is a warning rather than a surprise at 3 a.m.

## Related

- [IPC](../features/system/ipc.md) — the socket, the reply format, and what is answered locally.
- [Keybinds](keybinds.md).
