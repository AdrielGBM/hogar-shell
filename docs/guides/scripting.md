---
id: scripting
kind: guide
title: Scripting
summary: Driving the shell from a script, and reading its answers.
status: stable
compositor: any
commands: [shell, apps, audio, notifs, wallpaper, scheme, layout]
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

## Editing the layout

`layout` is a target like any other, so a script can read or change what the shell draws the same way it reads
or changes anything else:

```sh
hogar-shell layout list                       # every layout this shell can use
hogar-shell layout show [name]                 # print one as it is stored
hogar-shell layout check [name]                # what is wrong with one, without applying it
hogar-shell layout use <name>                  # draw this layout from now on
hogar-shell layout add <module> <area> [group] # place a module in an area of the layout being drawn
hogar-shell layout remove <id>                 # take a placed module, or a whole area, out
hogar-shell layout move <id> <group> [index]   # put a placed module in another group, or elsewhere in its own
hogar-shell layout set <instance> <key> <value> # change one property of a placed module
hogar-shell layout reset <id|layer|all>        # put a part of the layout back to the built-in one
hogar-shell layout undo                        # take back the last edit, whatever made it
hogar-shell layout redo                        # make the edit that was last taken back again
hogar-shell layout edit <layer|off> [output]   # edit one layer on one screen, or stop
```

Every edit — `add`, `remove`, `move`, `set`, `reset` — is one transaction, so `layout undo` takes back one
command whatever else made the edit before it: a gesture, a popover, or another line of a script.

`layout edit` is not an edit itself: it opens the edit mode of one layer — `background`, `desktop`, `top`,
`overlay` or `lock` — on the screen named, or the focused one. One layer is edited at a time, so entering a mode
leaves the last; Esc, the strip's Done button or `layout edit off` end it — a first Esc only clears what is
selected. Everything the pointer does in a mode has a key as well: the arrows select, Shift+arrows move, Ctrl+arrows
resize, Enter customizes, Delete removes, `m` switches mode, and `?` lists the rest. `lock` is a preview drawn over the
unlocked session and is refused while the session is locked. Under `--safe-layout` every mode is refused, like
every edit.

See the [Layout reference](../reference/layout.md) for what a layout file holds.

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
