---
id: layouts
kind: customization
title: Layouts
summary: Everything the shell draws, as one file of areas, groups and modules — merged by id, checked before it is applied, and recoverable.
status: stable
compositor: any
config: [lock]
commands: [layout]
deps: [wlr-layer-shell, ext-session-lock, ext-workspace]
see_also: [edit-modes, popovers, data-and-rules, bundles, bars, widgets, lock, per-monitor, compositor-rules, scripting]
---

# Layouts

## What it is

A **layout** is the answer to "what does the shell draw, and where?" — which bars exist and what is on them, what sits on
the desktop, where notifications land, what the lock screen shows. It is one TOML file, and the shell redraws as soon as
the file or an edit mode changes it. `config.toml` keeps behaviour, theme and what each module does by default; the
layout keeps placement.

You can write one by hand, build it by pointing at the screen in an [edit mode](edit-modes.md), or both: they are two
views of the same file, and every edit — a drag, a popover, a script line — is one entry in the same undo history.

Every key a layout file can hold is in the [Layout reference](../../reference/layout.md). This page is how the pieces
fit together.

## The model

```text
Layout
└─ Output rule          match = "*" or a glob over the connector name
   └─ Layer             background · desktop · top · overlay · lock
      └─ Area           a region with a geometry of its own: a bar, a grid, a stack, a wallpaper region …
         └─ Group       a run of instances, and how the area places it: a zone of a bar, a cell of a grid
            └─ Instance one placed module: which module, how big, its options, bindings and actions
```

An **area** has a `kind` that says what it is, and the keys of that kind follow it in the same table:

| Kind | What it is | Holds modules |
| --- | --- | --- |
| `bar` | A strip along one edge, in three zones (`start`, `center`, `end`). Several per edge are fine. | yes |
| `dock` | A strip that sizes itself to its contents, like the audio visualiser's. | yes |
| `grid` | Cells that widgets are placed into by explicit coordinates. | yes |
| `stack` | A column that notification, toast and volume cards land in. | yes |
| `free` | A rectangle placed by hand. | yes |
| `wallpaper_region` | A rectangle with a picture of its own. | no |
| `texture` | An image or gradient painted over what is behind it. | no |
| `prompt` | The lock screen's password field. Lock layer only; exactly one per output. | no |

A **group** is written with `place = "zone"` (and a `zone`) on a bar, or `place = "cell"` (with `col`, `row` and spans)
on a grid. An **instance** is `module`, plus `representation` — `chip`, `widget_s`, `widget_m`, `widget_l` or `card` — and
optionally `options`, `bindings` and `actions`. Wallpaper regions and textures are paint: a group put in one is dropped
with a finding.

Ids are what everything addresses. An area's id is unique on its layer, a group's within its area, and an instance's
across the whole layout — `clock`, then `clock-2` — which is what IPC, the editor and [rules](data-and-rules.md#rules)
use to name a thing. Two areas on a layer, or two instances in a layout, with one id is an error.

The built-in layout is the smallest usable desktop: a top bar with the workspaces, the clock and the notes chip; a clock face
in the middle of the desktop; the stack notifications arrive in, top right; and a lock layer with the clock, who is
signed in, what is playing and how many notifications wait, above the prompt.

## Files

```text
~/.config/hogar-shell/layouts/<name>.toml     one file per layout
~/.config/hogar-shell/components/<name>.toml  saved groups — see Komponents
~/.local/state/hogar-shell/state.json         which layout is drawn (machine state)
```

The file name is the layout's id — it is what `layout use`, `extends` and the last-good copy name — so an `id` written
inside the file changes nothing. `name` is a label for you. **Which layout is drawn is not a config
key**: `hogar-shell layout use <name>` writes it to machine state and the shell redraws. There is one active layout; how
screens differ is [output rules](#output-rules-and-workspace-rules), not a layout per screen.

The file is watched: saving it applies it. A file that has stopped parsing is reported and the layout already on screen
stays, rather than the screen going blank. Edits made in the shell are written about a quarter second after they stop,
and `shell quit` writes whatever is waiting.

The built-in layout is named `default`. It is read-only, and **the first edit to it forks it** — to `custom`, or
`custom-2` and so on if that is taken — and switches to the copy, so there is always a layout that works to go back to.
No file stands in for it: a `layouts/default.toml` is never read, whether at startup or on a reload, and a warning names the file and says
to give it another name, so `extends = "default"` and `layout use default` always mean the layout that ships. `layout check` prints the same warning.

## Building on another layout

A layout starts from nothing unless it says `extends`. The built-in layout is a layout like any other, reachable by name
rather than an implicit base:

```toml
name = "Work"
extends = "default"

[[outputs]]
match = "*"

[[outputs.layers.top.areas]]
id = "bar-top"                      # the area the built-in layout already has: only what is written changes

[[outputs.layers.top.areas.groups]]
id = "end"

[[outputs.layers.top.areas.groups.children]]
id = "battery"                      # a new id, so this is added after what is there
module = "battery"
representation = "chip"
```

A level is laid over the one before it, and **merging is by id, never by position**: a level names the area, group or
instance it means and writes only what it changes, so "this monitor also has a battery chip" is the few lines above and
keeps following the layout it refines.

- An id the earlier levels did not place is **added**, after what is already there — order is z-order, and an override
  never restacks.
- An id they did place is **overridden** field by field.
- An id in a `remove` list (a layer's `remove` takes areas, an area's takes groups, a group's takes instances) is
  **removed**. Removals apply before additions, so a level that removes an id and names it again is placing a new one.
- A level takes back an expression it inherited with `unset` on the area, group or instance that holds it: `visible`
  on an area; `repeat` and `parameters.<name>` on a group; `bindings.<path>` on an instance. Writing a key and
  unsetting it at one level is an error, and unsetting what nothing under the level writes is reported.

The levels, loosest to tightest:

1. The `extends` chain, root first.
2. Every [output rule](#output-rules-and-workspace-rules) whose glob matches the screen, broadest first.
3. The workspace rule for the workspace that is up on that screen.

A layout's `[sources.<name>]` tables merge by name along the `extends` chain only; see
[Data and rules](data-and-rules.md#sources-the-layout-declares).

## Output rules and workspace rules

Each `[[outputs]]` entry has a `match`: `*` for every screen, or a glob over the connector name (`DP-*`, `eDP-1`). Every
rule that matches a screen is applied to it, a literal name after a wildcard one and a longer pattern after a shorter,
so a `*` rule is the base and a named monitor refines it.

```toml
[[outputs]]
match = "DP-*"

[outputs.layers.desktop]
remove = ["widgets"]                 # no clock face on the external monitors
```

A `[[outputs.workspaces]]` entry refines an output rule while a workspace is up on that screen:

```toml
[[outputs.workspaces]]
match = "3"                          # a workspace name; `id:<n>` and `special:<name>` need Hyprland

[outputs.workspaces.layers.background]
remove = ["background"]
```

- `match` is a workspace **name**, which every compositor reports. `id:<n>` and `special:<name>` are resolved through
  Hyprland; without it a rule using either is reported as inactive by `layout check`, never silently ignored.
- A workspace rule may change contents and non-reserving areas. It may not add, remove or resize an area that
  reserves space, or change what an area reserves: reservation comes from the output rules alone, so switching workspaces
  never re-tiles your windows.
- It has no lock layer. No workspace is visible while the screen is locked.

See [Per-monitor setup](../../guides/per-monitor.md) for the same idea from the monitor's side.

## The five layers

| Layer | Window | What lives there |
| --- | --- | --- |
| `background` | one layer-shell window per screen | wallpaper regions, textures |
| `desktop` | one per screen | grids of widgets: the clock face, the visualiser |
| `top` | one per screen | bars, and everything that hangs off a chip: drawers, popouts, menus |
| `overlay` | opened only while something needs it | stacks of cards, the launcher, floats, an area flagged `above_fullscreen` |
| `lock` | one `ext-session-lock` surface per screen, between lock and unlock | readings, and the prompt |

The four session layers are layer-shell windows, one each per screen, and a window is on screen only while its layer
draws something. See [Compositor rules](../../guides/compositor-rules.md) for what that means for a rule of your own.

**The `lock` layer is the same model on a different surface**, with four differences that exist so the lock stays a lock:

- **Readings only.** Only a module drawn as something that does not take the pointer or keys may be placed there:
  `layout add` refuses the rest, validation refuses a hand-edited one, and `actions` are refused outright. The lock has no
  context menus.
- **The prompt is mandatory.** Exactly one `prompt` area exists on each screen's lock layer, with the password field, the
  status line and the fingerprint hint. It can be moved and restyled, never removed, hidden by an expression, faded below 0.9
  opacity, put off the screen, stacked under another area, or given a fill its text cannot be read on (WCAG AA).
- **It fails safe.** A lock layer that does not resolve or validate is replaced whole by the built-in minimal lock — a plain
  background and the prompt — rather than a half-corrected one. The lock never waits on a layout to appear: coverage is
  the compositor's, and a surface is made for every screen whatever the layout says.
- **It is resolved once, when the lock is taken.** A layout change made while locked applies at the next lock. Output rules
  apply; workspace rules do not.

What a reading may show on the lock screen, and how expressions are evaluated there, is in
[Data and rules](data-and-rules.md#what-the-lock-screen-shows). The lock page is [Lock screen](../system/lock.md).

## Working with layouts from a script

`layout` is an IPC target; every verb is in the [command reference](../../reference/commands.md#layout) and the
[scripting guide](../../guides/scripting.md#editing-the-layout) has the patterns.

| Verb | Does |
| --- | --- |
| `layout list`, `layout show [name]` | The layouts on disk, and one as it is stored. With no name, `show` and `check` mean the built-in layout, not the one being drawn. |
| `layout use <name>` | Draw this layout from now on. |
| `layout check [name]` | Everything wrong with a layout, without applying it. |
| `layout add`, `remove`, `move`, `set`, `reset` | Edit the layout being drawn; each is one transaction. |
| `layout undo`, `layout redo` | Take back or redo the last edit, whatever made it. |
| `layout edit <layer\|off> [output]` | Open or close an [edit mode](edit-modes.md). |
| `layout export`, `layout import`, `layout trust` | Share a layout as a [bundle](bundles.md): `export` writes one, `import` reads one (and waits for the outcome, up to a minute), `trust` answers for what an imported bundle runs. |

`list`, `show`, `check` and `export` read the files and answer in the command-line process, so they work when the shell
will not start — which is exactly when you want to read a layout. The rest go to the running shell, which owns the layout
being edited.

## Checking a layout

```sh
hogar-shell layout check work
```

`layout check` parses, validates and resolves one layout without applying it, and prints each problem with the file, the
line and column, and the key path. It resolves against one nominal screen, so it finds everything that does not depend on
a monitor's name — a missing prompt, an area with no `kind`, an instance with no `module`, an expression that does not
check, a komponent that is missing — and leaves the per-monitor half to the running shell's own notice. For a layout that came
with an imported [bundle](bundles.md#trust) it also warns about each command, address and action that is still held or was declined,
with the `layout trust` line that would accept it.

```text
layouts/broken.toml:8:12: error: outputs.*.layers.top.areas.bar-top.visible: `$battery` has no `levl`: it has `level`, `charging`
```

The running shell does the same on every reload and shows what is wrong in one notice that replaces itself and goes away
when the layout is clean. An item that cannot be drawn does not take the rest with it: an unknown module becomes a
visible placeholder where you put it, whose menu has Remove and Reset, and an area missing a required key is left
out with a finding that names the key. An `above_fullscreen` area gets a warning that it keeps its screen off direct
scanout. `hogar-shell config check` is the same idea for `config.toml`, and it also names a retired config key and
where its setting lives now.

## When a layout goes wrong

| Situation | What happens |
| --- | --- |
| The file does not parse after a hand edit, while the shell is running | The layout on screen stays and the problem is reported. |
| The active layout's file is missing or unreadable at startup | The **last good copy** is drawn and a warning says so; the file is left exactly as you left it, so nothing is overwritten. |
| There is no last good copy, or the name is unknown | The built-in layout is drawn and the notice names the layout it could not find. |
| You cannot fix it from inside the shell | `hogar-shell --safe-layout` starts on the built-in layout and refuses every edit, so it cannot write over the layout you are trying to rescue. |

The last good copy lives at `~/.config/hogar-shell/layouts/.last-good/<name>.toml`. It is refreshed whenever a layout is
proven to resolve cleanly, and only when it would differ from what is already there.

Under `--safe-layout` the store holds the built-in layout alone: every edit mode and every editing verb is refused with a
message saying so, and `layout import` is refused too. Restart the shell without the flag to edit again.

## Related

- [Edit modes](edit-modes.md), [Popovers](popovers.md) — building a layout by pointing at the screen.
- [Data and rules](data-and-rules.md) — readings, expressions and automation a layout can bind to.
- [Bundles](bundles.md) — sharing a layout, and what an imported one may run.
- [Bars](../surfaces/bars.md), [Desktop widgets](../surfaces/widgets.md), [Wallpaper](../surfaces/wallpaper.md).
