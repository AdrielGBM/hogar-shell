---
id: data-and-rules
kind: customization
title: Data and rules
summary: Bind what the shell draws to live readings with typed expressions, declare your own sources, keep variables, and run commands when something happens.
status: stable
compositor: any
config: [rules, automation, lock]
commands: [shell, var, rule, layout]
deps: [power-profiles-daemon]
see_also: [layouts, popovers, bundles, lock, scripting, idle]
---

# Data and rules

## What it is

Three ideas, all in the style of a desktop-widget toolkit, adapted to a shell that is driven by commands:

| Idea | In one line | Lives in |
| --- | --- | --- |
| **Readings** | A typed value that changes — the battery level, what is playing, the output of a command you declared. | the shell, and `[sources.<name>]` in a [layout](layouts.md) |
| **Expressions** | A small typed formula over readings: `if($battery.level < 20, #bf616a, $theme.accent)`. | a layout's bindings, `visible` and `repeat`; a rule's `when` and `store` |
| **Rules** | When something happens, check a condition and run commands. | `[[rules]]` in `config.toml` |

An expression can **read** but never **run** anything: no commands, no files, no clocks. Commands exist only where you
declare them — a source's `cmd`, a rule's `run`, an action — which is what keeps a [bundle](bundles.md) someone sent you
auditable.

## Readings

`$name` and `$source.field` name a reading. A reading has a type: `number`, `text`, `bool`, `colour`, or a list of one
of them. An expression is checked against those types before it runs, so a mistake is reported where it is written.

### The shell's own

Each module that has something to say declares **readings** with typed fields, fed by the service that owns them. They exist
whether or not the module is placed anywhere.

| Reading | Fields |
| --- | --- |
| `$battery` | `level` (percent), `charging` |
| `$power` | `profile` (`performance`, `balanced` or `power-saver`), `available` |
| `$clock` | `time` and `date` (as `[clock]` writes them), `now` (seconds since the epoch, for `df`) |
| `$cpu` | `usage` (percent), `frequency` (MHz) |
| `$gpu` | `usage` (percent), `vram` (bytes) |
| `$memory` | `used`, `total` (bytes) |
| `$netspeed` | `down`, `up` (bytes a second) |
| `$temperature` | `celsius`, `sensor` |
| `$volume` | `level` (percent), `muted` |
| `$media` | `title`, `artist`, `art`, `playing` |
| `$notifications` | `count`, `apps` (a list of text), `summary`, `body` |
| `$weather` | `place`, `temperature` (°C), `condition` (a stable name — `clear`, `rain`, `snow` — to compare against rather than to show) |
| `$workspace` | `active` (its id), `name`, `count` |
| `$user` | `name`, `avatar` (a path, empty where there is none) |
| `$spectrum` | `silent` |
| `$theme` | every token `[theme.colors]` can set, as a colour, plus `fg` (the text) and `bg` (the base) |

`$theme` follows the palette, so a binding on it re-colours when a wallpaper-derived palette lands. The expression field's
[source browser](popovers.md#the-expression-field) lists every reading with what it reads now.

### Events

`$event.<kind>` reads the last event of a kind as text — what it carried (a wallpaper's path, `dark` or `light`, the
profile, the threshold crossed) or the event's own name. The events are the fixed vocabulary of the
[scripting guide](../../guides/scripting.md#events); a [rule](#rules) can also be triggered by one.

### Variables

A **variable** is a typed value of your own, kept across restarts and read as `$name`. Setting one changes every binding that reads
it at once, with no reload. A variable is written from a script, a [rule](#rules)' `store` or an action; it is machine state in
`~/.local/state/hogar-shell/state.json`, not part of a layout, so a bundle never carries one.

```sh
hogar-shell var set mood calm
hogar-shell var set accent_override '#ff8800' --type colour
```

A name nothing answers to yet is a variable that has not been set: a layout may read it, and it has no value until `var set`
creates it. The types and the verbs are in the [scripting guide](../../guides/scripting.md#variables).

### Sources the layout declares

A layout can declare readings of its own under `[sources.<name>]`, read as `$name`:

```toml
[sources.forecast]
kind = "poll"
cmd = "curl -s 'wttr.in/?format=%t'"
every = "10m"
initial = ""

[sources.now_playing]
kind = "listen"
cmd = "playerctl --follow metadata --format '{{title}}'"

[sources.rate]
kind = "http"
url = "https://example.com/rate.json"
every = "1h"
parse = "json:.usd"
initial = 0
```

| `kind` | Reads |
| --- | --- |
| `poll` | A command, run through `sh -c` on an interval; each run's output is one reading. |
| `listen` | A command that keeps running and prints a line per update. It is started again, after a wait, if it exits. |
| `http` | An `http://` or `https://` address, fetched with a `GET` on an interval; each body is one reading. |

`parse` decides how output becomes a value — `text` (the default; all of it, trimmed), `lines` (a list, one item per line),
`json:<path>` or `regex:<pattern>` — and **`initial` decides the type**: a number makes the source a number, `true` or
`false` a bool, a list a list. It is also the reading before the first run answers. `every` is `500ms`, `5s`, `2m` or `1h`.
The keys of each kind are in the [Layout reference](../../reference/layout.md#sourcepoll).

- **Names.** Letters, digits and `_`, not starting with a digit. A source may not take a name a module's reading already has
  (`$battery`); that is an error. A source shadows a variable of the same name.
- **One producer per distinct source**, however many places read it, however it is spelled.
- **It runs only while it is wanted.** By default (`while = "visible"`) that means while something that reads it is on screen;
  `always` keeps it running while anything reads it at all. A hidden window's expressions stop counting, and while the
  session is locked only the lock layer's readings and rules do.
- **Limits.** Each source runs one command or request at a time. A run that outlasts `timeout_seconds` (5), prints a line
  past `max_line_kib` (64) or a run past `max_run_kib` (1024) is stopped, an `every` below `min_interval_seconds` (1) is
  raised to it, and a source that keeps failing waits twice as long before each retry, from `backoff_seconds` (2) up to
  `max_backoff_seconds` (300). These are `[automation]` keys in `config.toml` — a layout cannot loosen them, so a
  bundle cannot either.
- **Merging.** Along `extends`, a level keeps the keys it leaves out as long as both mean the same kind; one that changes the kind
  replaces the source, and one that changes `cmd` or `url` has to say `lock_safe` again.
- **A bundle's sources do not run until you trust them**, each command and address one by one, at the text you were shown. See [Bundles](bundles.md#trust).

## Expressions

```text
fmt("{}%", round($battery.level))
if($media.playing, mix($theme.accent, #ffffff, 20%), #888)
df("%H:%M", $clock.now) + " · " + $weather.condition
200 + 10%                               → 220
```

The language is a companion to the `telar` UI framework, `telar-expression`, so it behaves identically in every place it is
used — an editor popover, `layout check`, a rule.

- **Literals.** Numbers (`1_000`, `2.5e-3`), percentages (`10%`), texts (`'…'` or `"…"`), `true` and `false`, colours
  (`#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`) and lists (`{1, 2, 3}`). Write an expression in TOML inside a string, so a text
  literal in it uses the other kind of quote.
- **References.** `$name`, `$source.field`, `$event.<kind>`, and in a repeated group `$item` and `$index`.
- **Operators**, loosest to tightest: `||`, `&&`, `==` `!=`, `<` `<=` `>` `>=`, `+` `-`, `*` `/`, a leading `-` `+` `!`, `^`
  (right-associative, tighter than a leading minus, so `-2^2` is `-4`) and a postfix `%`. `+` also joins two texts. `(…)` and `[…]`
  both group, and `{…}` makes a list.
- **Percentages work as on a calculator.** On the right of `+` or `-` a percentage is taken of the left side (`200 + 10%` is 220);
  anywhere else it is a hundredth (`50 * 10%` is 5).
- **Conditionals.** `if(condition, then, else)` evaluates only the branch it takes, and `&&` and `||` short-circuit — an
  expression only subscribes to what its last evaluation actually read.
- **Typed.** `==` compares like with like, `if` needs a bool, and a binding must give the type of what it drives. Function
  and constant names ignore case.
- **Total.** An error — a division by zero, say — is a *value* an editor shows, never a crash, and
  what the expression drives keeps its last good value.

### Functions

Functions come in families, one function per operation:

| Family | Functions |
| --- | --- |
| text | `fmt` (each `{}` takes the next value, `{N}` the value at `N`), `upper`, `lower`, `trim`, `len`, `substring`, `replace`, `contains` |
| maths | `round` (also `round(x, decimals)`), `floor`, `ceil`, `abs`, `sqrt`, `cbrt`, `pow`, `min`, `max`, `ln`, `log`, `log2`, `exp`, `sin`, `cos`, `tan`, `asin`, `acos`, `atan`; constants `pi`, `e`, `tau` |
| list | `len`, `at` (from zero; negative counts from the end), `join` |
| colour | `mix` (in Oklch), `alpha`, `contrast` (the WCAG ratio) |
| the shell's | `df`, `tr`, `bytes`, `rate` |

The shell adds four that need to know it:

- **`df(format, moment)`** writes a moment — seconds since the epoch, such as `$clock.now` — with a `strftime` pattern, in local time,
  the way the clock does: `df("%H:%M", $clock.now)`.
- **`tr(text, language, text, …)`** is the text for the language the shell is speaking, the first argument where none is given for
  it: `tr("Battery", "es", "Batería", "de", "Akku")`. `es-MX` matches itself before it matches `es`.
- **`bytes(n)`** writes a byte count in binary units (`1.5 GiB`), **`rate(n)`** a byte rate in decimal ones (`1.2 MB/s`), as the
  system modules do. Neither takes a negative.

There is no function that runs a command. The numbers keep the launcher's calculator semantics, because it is the same parser.

## Bindings

A **binding** makes something the layout draws follow an expression.

### Instance options

An instance's `bindings` table maps an option key of its module — any key that `options` takes — or `accent` to an expression. The
value is laid over `options` as it changes, and has to be the type the option takes:

```toml
[[outputs.layers.top.areas.groups.children]]
id = "battery"
module = "battery"
representation = "chip"

[outputs.layers.top.areas.groups.children.bindings]
accent = "if($battery.level < 20, #bf616a, $theme.accent)"
```

Only this instance is drawn again when the value moves, and what it keeps — the state of its panel, a scroll position —
survives. An expression that does not check is reported and left out; one that fails while the shell runs keeps its last value.
From a script: `layout set battery bindings.accent "<expression>"`, and `layout set battery unset bindings.accent` to take back
one a broader level wrote.

### `visible`

An area's `visible` decides whether it is drawn at all:

```toml
[[outputs.layers.top.areas]]
id = "bar-top"
visible = "$workspace.count > 1"
```

While it is false the area paints nothing and takes no input, and flipping it rebuilds nothing else on the layer. It is shown until it
first has an answer, and through an evaluation error it keeps its last answer. An area that `reserve`s its edge **keeps it reserved
while hidden**, so windows stay clear of an empty strip; `layout check` warns. The lock's prompt cannot have one.

While its layer's edit mode is up, an area whose `visible` reads false is still there to edit: it is drawn at 30 % opacity and can be selected like any other, and its selection says why it is dim ("Hidden: visible = <expression> is false"). Outside the mode nothing changes: it paints nothing and takes no input.

### `repeat`

A group's `repeat` is an expression giving a list; its children are drawn once per item, in order, and each copy reads `$item` and
`$index` (from 0):

```toml
[[outputs.layers.top.areas.groups]]
id = "apps"
place = "zone"
zone = "end"
repeat = "$notifications.apps"

[[outputs.layers.top.areas.groups.children]]
id = "app-dot"
module = "clock"

[outputs.layers.top.areas.groups.children.bindings]
accent = "if($index == 0, $theme.accent, #888)"
```

A copy has the id `<id>#<index>` where it is drawn, with its own rect and its own state; IPC and the editor address the child as written. In a
`pages` group the copies are its pages. Until the list first answers, and while it is empty, the group draws nothing. A grid cell cannot
repeat, because a cell's footprint is fixed, and neither can a `grid` or `free` group, where each child has a place of its own. A list that grows or shrinks adds or drops copies at its end and builds nothing else.

**A group draws at most 256 copies.** A longer list draws its first 256 items and the `repeat` carries a finding that says how long the list is and how many are drawn, for as long as it is that long, so a source answering with a hundred thousand lines cannot stall the shell.

### Komponent parameters

A [komponent](bundles.md#komponents) declares typed parameters, read inside it as `$name` before anything else; each use sets them with
an expression of its own, read where the group is drawn. A group sets one with `parameters = { threshold = "15" }`, or
`layout set <area>.<group> parameters.threshold 15`.

## Actions

An instance, or an area's own background, can run commands on a gesture — `press`, `long_press`, `scroll_up`, `scroll_down`,
`middle` or `secondary` — as a chain of command lines in its `actions` table:

```toml
[outputs.layers.top.areas.groups.children.actions]
press = ["panel toggle battery"]
long_press = ["shell run notify-send hi", "var set seen true --type bool"]
```

Each line is a command of the same table `hogar-shell --list` prints, checked **when the layout loads** without being run, so a typo is a
located error rather than a gesture that silently does nothing. The chain runs in order and stops at the first line the shell refuses. A bound gesture
takes the place of whatever the shell itself would have done with it — a chip's own press, its wheel — and leaves every other gesture as it was;
a wheel chain runs once a notch rather than once per smooth-scroll event. From a script, `layout set <instance> actions.<gesture> "line; line"`.

**`shell run <command...>`** hands the rest of the line to `sh -c` and does not wait for it. It is the one command that executes
text of its own — every other command acts inside the shell. The [scripting guide](../../guides/scripting.md#running-a-command) has the quoting rules.

**In an imported bundle's file, an action line waits for your trust**: every line of a chain is an item, except the few that only
move what the shell shows (opening a panel, the volume, the player, the backlight, locking). The held line is taken out of its chain until
you accept it, at exactly the text you were shown; [Bundles](bundles.md#what-runs-without-asking) has the list and the reasons. A line
that is `layout trust …` does not belong in an action: it is refused when written, reported as an error wherever a file holds one, and dropped from an
imported file's chain, because trust is yours to give.

The lock layer has no actions.

## Rules

A **rule** is a few lines of `config.toml`: when something happens, check a condition, run commands, and optionally keep a value.

```toml
[[rules]]
id = "low-battery"
trigger = { edge = "$battery.level < 15 && !$battery.charging" }
run = ["toast show Battery low: plug in soon", "var set battery_low true --type bool"]
```

- **Triggers.** Exactly one of `event = "<name>"` (once for each such event after the rule is loaded), `edge = "<expression>"` (each time
  it turns from false to true — once per crossing, however often its readings change), `schedule = "<times> [days]"` (`HH:MM`, local time) or
  `every = "<interval>"` (`30s`, `5m`, `1h`, never more often than `[automation] min_interval_seconds`). The table is in the
  [scripting guide](../../guides/scripting.md#rules).
- **`when`** is an expression read as the rule fires, which has to give `true`. A `when` waiting on a reading that has not arrived yet does not
  hold, and is not a failure.
- **`run`** is a list of command lines, as everywhere else, stopping at the first the shell refuses.
- **`store`** keeps `{ var = "<name>", value = "<expression>" }` in a variable after the commands ran, the variable taking the value's type.
- **They read** module readings, `$theme`, variables and `$event.<kind>` — **not a layout's `[sources]`**, so a rule means the same thing
  whatever layout is drawn, and a bundle cannot add or change one. Rules live in `config.toml` because they are session behaviour, not placement.
- **They keep running while the session is locked.** They are session automation, not something on the lock screen, and a rule that watches
  a radio's or the power profile's event is what keeps that service running.
- **Reloading** `config.toml` does not fire an unchanged rule again or make it forget a crossing it is waiting on.

**Session-ending events hold the action.** `logging_out`, `rebooting` and `shutting_down` wait for the rules they trigger to finish
their commands before the action goes to logind, for at most `[automation] shutdown_grace_seconds` (3; `0` does not wait) — time
for a rule to save state, then it proceeds regardless.

**A loop is stopped, not run forever.** `config check` refuses a rule whose `run` has `rule run`, a rule asked to fire while it is
already firing is refused and reported, and a rule that fires more than **10 times within one second** — two rules setting each other
off through a variable — is **suspended** and reported until the config is loaded again. `rule list` shows it as `suspended`.

```sh
hogar-shell rule list        # id, trigger, state, and when it last fired
hogar-shell rule run <id>    # run its commands now, whatever its trigger, `when` and `enabled` say
hogar-shell config check     # what keeps a rule from loading, at the line and column it is written
```

## What the lock screen shows

The `lock` layer evaluates expressions normally, but over **lock views**: it is read by whoever is in the room, so a reading says only
what it is allowed to say to anyone.

- A reading's **private** field reads as its type's empty value — empty text, `0`, `false`, an empty list, a transparent colour — without its service being
  asked. A notification's `summary` and `body` are always private.
- Some fields follow `[lock]`. `$notifications.apps` is shown only under `notification_detail = "apps"` (the default,
  `"count"`, shows `count` alone). `$media.title`, `artist` and `art` are shown under `media_detail = "title"` (the
  default) and read empty under `"state"`, which leaves `$media.playing`. Every other reading is public.
- **A source of your own is refused** on the lock layer unless it says **`lock_safe = true`**: what a command prints or an address
  returns is unaudited text on a screen anyone can read. `lock_safe` is a promise the file makes about its own output; it is not a trust grant. The
  expression is reported and left out where it is drawn — it is no reason to fall back to the minimal lock.
- **`repeat` over a private list draws nothing**, since the list reads as empty.
- **Variables are readable**, so a rule that `store`s something the lock screen hides is warned about by `config check`. `var set` is unreachable from
  the layer, which has no actions.
- **Sources only another layer reads stop while locked**, because those layers are not visible; rules keep running.

The privacy keys are in the [Lock screen](../system/lock.md) page, and the layer itself in [Layouts](layouts.md#the-five-layers).

## When something fails

Failures are one registry, keyed by what failed — a source, an expression in a layout, a rule — and shown in the same notice as
the layout's and the config's other problems. A source or rule that starts failing while the shell runs is shown as it fails, and
withdrawn as it recovers.

- A source is named under the name the layout gave it, with why: it did not finish within the timeout and was stopped, it printed more than the
  limit, it exited with a status, it could not be started, an address did not answer.
- An expression that errors while running keeps its last good value, and the error — with a caret under the span — is the editor's to show.
- A rule that fails as it fires — a command refused, a `when` that could not be read — stays on the notice until it next fires cleanly.
- A source from a bundle you have not trusted is not failing: it reads as waiting, and nothing runs. See [Bundles](bundles.md#trust).

`hogar-shell layout check` and `hogar-shell config check` say the same things without a running shell.

## Related

- [Scripting guide](../../guides/scripting.md) — variables, rules, `shell run` and events from the command line.
- [Layouts](layouts.md), [Popovers](popovers.md) — where bindings are written and edited.
- [Bundles](bundles.md) — sharing sources and actions, and trusting them.
- [Lock screen](../system/lock.md).
