---
id: scripting
kind: guide
title: Scripting
summary: Driving the shell from a script, and reading its answers.
status: stable
compositor: any
commands: [shell, apps, audio, notifs, wallpaper, scheme, layout, var, rule]
config: [rules]
deps: [power-profiles-daemon]
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
hogar-shell layout set <instance|area|area.group> <key> <value> # change one property of a placed module, an area's visible or a group's repeat
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

## Variables

A variable is a typed value that layouts and rules read as `$name`. It is kept across restarts, and setting one
changes every binding that reads it at once, with no reload.

```sh
hogar-shell var set accent_override '#ff8800' --type colour  # the first set fixes the type
hogar-shell var set accent_override '#88c0d0'                # later sets are read as that type
hogar-shell var get accent_override
hogar-shell var list                                         # name, type and value, tab-separated
hogar-shell var remove accent_override
```

A type is `text`, `number`, `bool`, `colour`, `image`, `font` or `list:<type>`, a list being its items separated by
commas. A new variable is text unless `--type` says otherwise, and a value that does not fit the type it already
has is refused: `var remove` it first to change its type. The value is the rest of the line as written, so a text keeps
its spacing.

## Rules

A rule is a few lines of `config.toml`: when something happens, check a condition, run commands, and keep a value.

```toml
[[rules]]
id = "low-battery"
trigger = { edge = "$battery.level < 15 && !$battery.charging" }
run = ["toast show Battery low: plug in soon", "var set battery_low true --type bool"]

[[rules]]
id = "evening"
trigger = { schedule = "19:00 mon-fri" }
when = "$power.profile != 'power-saver'"
run = ["shell run powerprofilesctl set power-saver"]
store = { var = "evening_from", value = "$clock.time" }
```

A trigger is exactly one of four:

| Trigger | Fires |
| --- | --- |
| `event = "<name>"` | on each [event](#events) of that name |
| `edge = "<expression>"` | each time the expression turns from false to true — once per crossing, however often its readings change; one already true when the rule loads waits for the next crossing |
| `schedule = "<times> [days]"` | at each `HH:MM`, local time, on the days named: `mon` … `sun`, a range such as `mon-fri`, `weekdays` or `weekends`; every day when none is named. A time the machine slept through is skipped, not run late |
| `every = "<interval>"` | every `30s`, `5m`, `1h`, counted from when the rule loaded; never more often than `[automation] min_interval_seconds` |

`when` is checked as the rule fires, and the rule runs only if it gives true. `run` is a list of the same command
lines as everywhere else, in order, stopping at the first one the shell refuses. `store` evaluates its `value` as the
rule fires and keeps it in a [variable](#variables) of the value's type. `enabled = false` keeps a rule written down
without loading it.

Expressions read module readings (`$battery.level`), variables (`$name`) and events (`$event.session_locked`) —
not a layout's own `[sources]`, so a rule means the same thing whatever layout is drawn. Rules keep running while
the session is locked: they are session automation, not something on the lock screen.

```sh
hogar-shell rule list        # id, trigger, state (on, off or invalid) and when it last fired, tab-separated
hogar-shell rule run <id>    # run a rule's commands now, whatever its trigger, `when` and `enabled` say
hogar-shell config check     # what keeps a rule from loading, at the line and column it is written
```

A rule written with a mistake — an unknown event, an expression that does not compile, a command line the shell
does not have, a name two rules share — does not load, and `config check` and the problems notice say why. One that
fails as it fires — a command refused, a `when` with no reading yet — is on the problems notice until it next fires
cleanly. Saving `config.toml` reloads the rules without firing an unchanged one again or forgetting a crossing it is
waiting on.

## Running a command

```sh
hogar-shell shell run notify-send "build done"
```

`shell run` hands the rest of the line to `sh -c` and does not wait for it. It is also what a layout action or a
rule uses to run something: every action is a line like these, checked when the layout loads. From the command
line each argument stays the word your shell made it, so `"build done"` reaches `notify-send` as one argument with
its spacing intact; to run a pipeline, hand it to `sh` yourself: `hogar-shell shell run sh -c 'ls | wc -l'`.

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
| `[[rules]] run` | when a rule fires |

Anything in `hogar-shell --list` is valid in all three, and a request line is validated **without being run** — so
a typo in an idle stage is a warning rather than a surprise at 3 a.m.

## Events

The shell names what happens to it with one fixed set of events, each raised by the part of the shell that
owns it:

| Event | When |
| --- | --- |
| `started` | the shell has finished starting |
| `wallpaper_changed` | a wallpaper was set or cleared, and the picture actually changed |
| `colors_changed` | a wallpaper-derived palette was published, once its `[theme.export]` files are on disk |
| `theme_mode_changed` | the palette switched between dark and light |
| `session_locked`, `session_unlocked` | a lock **this shell** took was confirmed by the compositor, or ended; another locker's lock raises neither |
| `logging_out`, `rebooting`, `shutting_down` | just before the session action is asked of logind |
| `wifi_enabled`, `wifi_disabled`, `bluetooth_enabled`, `bluetooth_disabled` | the radio was switched, from the shell or from anywhere else |
| `battery_state_changed` | the charger was plugged in or pulled |
| `battery_under_threshold` | the charge crossed down through a `[battery] warn_levels` threshold, once per crossing |
| `power_profile_changed` | power-profiles-daemon switched profile |

A rule consumes them two ways. `trigger = { event = "<name>" }` fires the rule once for every such event raised
after the rule is loaded — two palettes landing are two firings, since an event is a thing that happened rather than
a state. `$event.<name>` reads the last one of a kind in an expression, as text: what it carried (the wallpaper's
path, the profile, `dark` or `light`, `charging` or `discharging`, the threshold crossed) or the event's own name.

The radios and the power profile are read only while something is watching them. A rule triggered by one of their
events is something watching them: it keeps that service running for as long as the rule is loaded, locked or not.

## Related

- [IPC](../features/system/ipc.md) — the socket, the reply format, and what is answered locally.
- [Keybinds](keybinds.md).
