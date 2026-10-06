---
id: scripting
kind: guide
title: Scripting
summary: Driving the shell from a script, and reading its answers.
status: stable
compositor: any
commands: [shell, apps, audio, notifs, wallpaper, scheme, layout, komponent, var, rule]
config: [rules, automation]
deps: [power-profiles-daemon]
see_also: [ipc, keybinds, layouts, data-and-rules, bundles]
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
hogar-shell layout set <instance|area|area.group> <key> <value...> # change one property: the keys are below
hogar-shell layout duplicate <id|area.group>   # copy a placed module, a group or an area beside itself
hogar-shell layout order <id> <up|down|front|back> # draw an area, or a child of a free group, over or under the others
hogar-shell layout panel <instance> [--along]  # give a placed module a panel of its own, beside it or along its bar
hogar-shell layout rename <id|area.group> <new> # give a module, an area or a group another id, everywhere the layout names it
hogar-shell layout reset <id|layer|all>        # put a part back to what the layout it extends says, or the built-in one
hogar-shell layout undo [n]                    # take back the last edit, or the last n, whatever made them
hogar-shell layout redo [n]                    # make the last edit taken back again, or the last n
hogar-shell layout history                     # every edit undo and redo walk through
hogar-shell layout edit <layer|off> [output]   # edit one layer on one screen, or stop
hogar-shell layout export <bundle-path> [name] # write a layout and what it needs to a new directory
hogar-shell layout import <bundle-path>        # add a bundle's layouts, komponents and pictures; waits up to 60 s and prints the outcome
hogar-shell layout trust [bundle] [item|--all <set>] [--decline] # list, accept or decline what an imported bundle runs
hogar-shell layout trust --dialog              # open the dialog that answers for what waits
```

`list`, `show`, `check` and `export` read the files and answer in your terminal, so they work when the shell will not start. `show` and `check` with no name mean the built-in layout, not the one being drawn. Every other verb is answered by the running shell.

What `layout set` takes as its key. The first word names a placed module, an area or a group (`<area>.<group>`, or
the group's own id where only one area has it); an id that several of them share names the first, in that order,
that takes the key:

| Of | Key | Sets |
| --- | --- | --- |
| an instance | `module`, `representation` | what it shows and how big it is drawn |
| an instance | `options.<key>` | one option, the value read as TOML where it parses as TOML and as text otherwise (`options.show_date true`) |
| an instance | `bindings.<key>` | an [expression](../features/customization/data-and-rules.md#bindings) that drives an option, or `accent` |
| an instance or an area | `actions.<gesture>` | a chain of command lines separated by `;` (`actions.press "panel toggle battery; var set seen true"`); `press`, `long_press`, `scroll_up`, `scroll_down`, `middle` or `secondary` |
| an instance | `weight`, `cell`, `cell.<key>`, `rect`, `rect.<key>` | where it sits in a `row` or `column`, `grid` or `free` group (`cell.col 2`, `cell {col = 0, row = 1, col_span = 2}`, `rect.x 0.5`) |
| an instance | `unset bindings.<key>` | takes back a binding a broader level wrote |
| an instance, an area or a group | `style.<key>` | its look: `style.fill`, `style.radius`, `style.opacity`, `style.padding`, `style.border.width`, `style.border.color`, `style.shadow`, and on an area `style.backdrop` (`style.radius [8, 8, 0, 0]`); a bar's `style.radius` is written as its `shape.radius`, as the editor writes it |
| an area | `visible`, `unset visible` | the expression that decides whether it is drawn |
| an area | any key of its kind (`thickness`, `shape.gap`, `flow`, `offset`, `rect.x`, `owner`, `cols`, …) | its geometry, as the [layout file](../reference/layout.md) names it for that kind: a bar's edge and shape, a stack's flow and routes, a panel's owner and cells; an area only a layout it extends writes gets an entry naming its kind and that key alone |
| an area | `reserve`, `above_fullscreen`, `within` | whether it keeps windows off its edge, whether it stays over a fullscreen window, and which box it is measured in |
| a group | `arrange`, `cols`, `rows`, `gap` | how it lays its children out: `column`, `row`, `grid`, `free` or `pages`, and a `grid` group's tracks |
| a group | `col`, `row`, `col_span`, `row_span`, `zone` | where it sits: its cells on a grid or a panel, or its zone on a bar |
| a group | `unset arrange` | takes back an arrangement a broader level wrote, with its `cols`, `rows` and `gap`, so the group is a loose run again |
| a group | `repeat`, `unset repeat` | the list its children are drawn once per item of |
| a group | `parameters.<name>`, `unset parameters.<name>` | what a [komponent](../features/customization/bundles.md#komponents) parameter reads |

An expression is checked the way the editor checks it, so a typo is refused with a caret under it rather than written.
Any other value is refused where `layout check` would report it at that key — a colour that is neither `#rrggbb`,
`#rrggbbaa` nor a theme token, a `weight` in a group that is not a `row` or a `column`, an arrangement a bar's zone
does not take — and so is anything that would make a locked screen fall back to the minimal lock, as the lock's edit mode refuses it. An
action is checked as the editor's Actions rows check it, in the same order: the lock layer and the pictures behind
the desktop take none, the gesture has to be one of those above, and each line has to be a command the shell has
and never `layout trust`. A colour is written lowercased, as the editor's colour rows write it.
`unset` is written where it takes back what every screen draws; where a level it cannot be laid over still writes
the key, it is refused naming that level.

`duplicate`, `order` and `panel` do what the edit modes' Duplicate, Order and Panel do, on the focused screen
where it draws the id and on the first screen that does otherwise. `duplicate` takes an instance, a group as
`<area>.<group>` or an area, and answers with the copy's id. `order` moves a free area, a texture or a card stack
over or under the others of its layer, or a child of a `free` group over or under its siblings: `up` and `down` a
step, `front` and `back` all the way. `panel` gives an instance a panel of its own, opened beside it, or along the
whole bar it is in with `--along`; `panel toggle`, `panel open` and `panel close` take the instance's id to reach it.

`layout history` prints one line per entry: how many steps away it is — below 0 back, 0 where the layout is now,
above 0 forward — then a tab and what the edit was. `layout undo 3` and `layout redo 3` walk that many at once, and
are refused when the history holds fewer. A walk refused part way says which steps it took before it stopped,
and why.

`layout rename` rewrites every level of the layout that names the id, and what names it by id — a panel's owner, a
komponent child — with it. It never rewrites a line that runs a command, so a `[[rules]]` command, an action or a
layout extending this one that names the old id keeps it, and the rename is refused with each of them listed; an
id the layout it extends names too is refused as well, since it is that layout's to rename.

The komponents have a target of their own:

```sh
hogar-shell komponent list                                   # each komponent, its parameters, and the layouts that draw it
hogar-shell komponent show <name>                            # print one as it is stored
hogar-shell komponent save <area.group> <name> [parameter...] # save a group, making the values named into parameters
hogar-shell komponent use <area>[.<group>] <name> [--zone start|center|end] [parameter=<expression>...]
hogar-shell komponent detach <area.group>                    # turn a use back into instances of its own
```

`save`, `use` and `detach` are each one transaction too. On a bar, `use` puts the new group at the end of the zone `--zone` names, the end zone by default.
`save` and `detach` refuse to copy a command, address or line that came with an imported bundle and that you have not accepted into a file of your own,
and say which `layout trust` line accepts it.

Every edit — `add`, `remove`, `move`, `set`, `duplicate`, `order`, `panel`, `rename`, `reset` — is one transaction, so `layout undo` takes back one
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

A name is letters, digits and `_`, and does not start with a digit, which is what `$name` can spell. A number has to
be finite, a colour is written `#rrggbb`, and a list cannot hold a list. In an expression an `image` is a text (a path)
and a `font` is a text (a family), so they compare and join like any other.

`var set` writes one, from a script, a rule's `run` or a layout's [actions](../features/customization/data-and-rules.md#actions), and so does a
rule's `store`. A layout cannot hold one, and a [bundle](../features/customization/bundles.md) never carries one: they are machine state in
`~/.local/state/hogar-shell/state.json`. A layout may read `$name` before it is set: it has no value until `var set`
makes it, and then every binding that reads it follows. A source a layout declares under the same name wins over
the variable while the layout is drawn. The lock screen can read variables, so keep nothing there you would not show
it; `var set` itself cannot be reached from the lock layer, which has no actions.

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

`when` is checked as the rule fires, and the rule runs only if it gives true; one waiting on a reading that has not
arrived yet does not hold, which is not a failure. `run` is a list of the same command lines as everywhere else, in
order, stopping at the first one the shell refuses. `store` evaluates its `value` as the rule fires and keeps it in a
[variable](#variables) of the value's type. `enabled = false` keeps a rule written down without loading it, though
`rule run` still runs its commands.

Expressions read module readings (`$battery.level`), the palette (`$theme.accent`), variables (`$name`) and events
(`$event.session_locked`) — not a layout's own `[sources]`, so a rule means the same thing whatever layout is drawn, and
a bundle cannot change one. Rules keep running while the session is locked: they are session automation, not
something on the lock screen. A `store` whose value reads something the lock screen hides is warned about by
`config check`, since a variable is readable there.

```sh
hogar-shell rule list        # id, trigger, state (on, off, invalid or suspended) and when it last fired, tab-separated
hogar-shell rule run <id>    # run a rule's commands now, whatever its trigger, `when` and `enabled` say
hogar-shell config check     # what keeps a rule from loading, at the line and column it is written
```

`rule run` prints each command it ran with the shell's reply, and does not write the rule's `store`.

A rule written with a mistake — an unknown event, an expression that does not compile, a command line the shell
does not have, a name two rules share — does not load, and `config check` and the problems notice say why. One that
fails as it fires — a command refused, a `when` or `store` that could not be read — is on the problems notice until it
next fires cleanly. Saving `config.toml` reloads the rules without firing an unchanged one again or forgetting a crossing it is
waiting on; a rule whose own text is unchanged but reads a variable that has appeared, gone or changed type is checked again.

**A rule cannot run a rule.** `rule run` inside `run` is an error, and a rule asked to fire while it is already
firing is refused and reported. Two rules setting each other off through a variable are caught by rate instead: a rule
that fires more than **10 times within one second** is **suspended** — `rule list` says `suspended` and the problems
notice says why — until the config is loaded again.

**Logging out, rebooting and powering off wait for the rules they trigger.** `logging_out`, `rebooting` and `shutting_down`
hold the session action until the rules triggered by that event have finished their commands, up to
`[automation] shutdown_grace_seconds` (3; `0` does not wait), and then go ahead whether they have or not.

A rule runs its commands but cannot read their output: [`shell run`](#running-a-command) discards it, and an expression cannot run
anything. To bring a command's output into a layout, declare a `poll` or `listen` [source](../features/customization/data-and-rules.md#sources-the-layout-declares).

The model — readings, expressions, bindings, what the lock screen shows — is [Data and rules](../features/customization/data-and-rules.md).

## Running a command

```sh
hogar-shell shell run notify-send "build done"
```

`shell run` hands the rest of the line to `sh -c` and does not wait for it. It is also what a layout action or a
rule uses to run something: every action is a line like these, checked when the layout loads. From the command
line each argument stays the word your shell made it, so `"build done"` reaches `notify-send` as one argument with
its spacing intact; to run a pipeline, hand it to `sh` yourself: `hogar-shell shell run sh -c 'ls | wc -l'`.

**What the reply means.** `ok` says the command was handed off, not that it worked: the process is detached in a session of its
own, with stdin, stdout and stderr closed, so it outlives the shell, writes nothing over its output, and its exit status and
anything it prints are not reported to you, to a rule or to a layout. A command that cannot be started is only a
line in the log. For output you want to read, declare a `poll` or `listen`
[source](../features/customization/data-and-rules.md#sources-the-layout-declares), whose commands are run with limits.

**It is the one command that runs text of its own.** Every other command acts inside the shell, so `shell run` is the
verb to be careful with wherever a line is not yours. In an imported [bundle](../features/customization/bundles.md#trust)'s file it is not
alone in being held, though: every action line waits for your trust at exactly the text you were shown, except the few
[commands that only move what the shell shows](../features/customization/bundles.md#what-runs-without-asking), and a held line is taken out
of its action chain until you accept it.

### Importing and trusting from a script

```sh
hogar-shell layout import ~/shared/work-bar
hogar-shell layout trust work
hogar-shell layout trust work --all <set>
```

`layout import` is answered when the shell has read the bundle, up to 60 seconds, with what it wrote and everything that waits for trust;
only one import is read at a time. `layout trust <bundle>` lists each item with a 32-hex-digit id (128 bits) and ends with the set id of the list. An id answers for
that item at that text, and `--all <set>` answers for exactly the list it names: if the bundle's items changed since the listing, nothing is answered and you list again.
A `layout trust` line cannot be written into an action, so a script's own gestures cannot accept for you either.

## Saying something

```sh
hogar-shell toast show "backup finished"
```

A [toast](../features/surfaces/toasts.md) rather than a notification, deliberately — see that page for which
one you want. For something that should be *recorded*, send a real notification with `notify-send`; hogar-shell is
the daemon that receives it. A notification from another program can carry buttons, but they only answer back to that program: only
the shell's own notices run a line in the shell, so no client can trigger a command through one.

## Two rules worth knowing

**Where a screenshot goes is config, not a flag.** `[screenshot] copy` and `save` decide whether a capture
reaches the clipboard, a file, or both, so one command behaves the way you set it up.

**`brightness up` with no display named means the primary panel, not every screen.** It is the one mutation
where an unnamed target is not "all of them". Name a connector, or spell out `all`. Every other mutation —
`wallpaper set`, `wallpaper clear` — does mean all of them when nothing is named.

## Running a script from the shell

Four places take a command line, and all four take the *same* vocabulary:

| Where | What it runs |
| --- | --- |
| `[[idle.stages]] action` / `return_action` | on a timeout, and on wake |
| `[launcher] actions` | from the launcher's `>` mode |
| `[[rules]] run` | when a rule fires |
| a layout's `actions` | on a gesture on an instance or an area: press, long press, scroll, middle or right click |

Anything in `hogar-shell --list` is valid in all four, and a request line is validated **without being run** — so
a typo in an idle stage is a warning rather than a surprise at 3 a.m.

## Events

The shell names what happens to it with one fixed set of events, each raised by the part of the shell that
owns it:

| Event | When |
| --- | --- |
| `started` | the shell has finished starting |
| `wallpaper_changed` | a wallpaper was set or cleared, and the picture actually changed |
| `colors_changed` | the colours the shell paints with changed — an edited theme, a switch of palette, or a wallpaper-derived palette once its `[theme.export]` files are on disk |
| `theme_mode_changed` | the palette switched between dark and light |
| `session_locked`, `session_unlocked` | a lock **this shell** took was confirmed by the compositor, or ended; another locker's lock raises neither |
| `logging_out`, `rebooting`, `shutting_down` | just before the session action is asked of logind, which [waits for the rules they trigger](#rules) |
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
