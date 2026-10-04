---
id: bundles
kind: customization
title: Komponents and bundles
summary: Save a group to use again, share a whole layout as a directory, and decide item by item what an imported one is allowed to run.
status: stable
compositor: any
config: [lock]
commands: [layout, komponent]
deps: [wlr-layer-shell]
see_also: [layouts, edit-modes, popovers, data-and-rules, lock, scripting]
---

# Komponents and bundles

## What it is

Two ways to reuse a piece of a [layout](layouts.md), and one safeguard for taking someone else's.

- A **komponent** is a group you saved, with the values a use of it can set. Use it in several places, change it once.
- A **bundle** is a layout with everything it needs to draw on another machine, as one plain directory you can read before you import it.
- **Trust** is what stops an imported bundle from running anything you have not seen: every command, address and action
  line it brings stays off until you accept it, one by one, at exactly the text you were shown — except the few actions
  that only move what the shell shows, such as opening a panel or changing the volume.

## Komponents

A komponent is a file at `~/.config/hogar-shell/components/<name>.toml`: some typed **parameters**, and the instances they
configure.

```toml
[parameters.threshold]
type = "number"
default = "20"

[[children]]
id = "level"
module = "battery"
representation = "chip"

[children.bindings]
accent = "if($battery.level < $threshold, #bf616a, $theme.accent)"
```

- **`parameters`** — each has a `type` (`text`, `number`, `bool`, `colour` or `list:<one of those>`) and a `default`, an expression of that type.
  The name is what the komponent's expressions read as `$threshold`, before any of the shell's own names; it is letters, digits and `_`,
  and neither `item` nor `index` (which a copy of a [repeated group](data-and-rules.md#repeat) reads) nor the name of a module's reading.
  A picture's path and a font's family are `text`.
- **`children`** are written as a group's are. Their ids only have to be unique within the file. `stacked` and `repeat` may be set on
  the komponent too, and `repeat` may read the parameters.
- **Name.** Lowercase ASCII letters, digits, `-` and `_`: the name is the file's, and a letter of another script cannot pass for a Latin one ([names](#names)).
- **On the lock layer** a komponent may hold readings only and no actions; a use that would break that is refused.

### Using one

A group draws a komponent by naming it, and sets its parameters with an expression of its own, read where the group is drawn:

```toml
[[outputs.layers.top.areas.groups]]
id = "pill"
place = "zone"
zone = "end"
komponent = "battery-pill"
parameters = { threshold = "15" }
```

A parameter left out reads its default; a name the komponent does not declare is an error. **The komponent supplies the group's
`children`, `stacked` and `repeat`**, so naming one replaces what the group held, and a group that holds more is a located error and the komponent wins.
Each child is drawn as `<area>.<group>/<child>` (`bar-top.pill/level`), so two uses never share an id or an instance's state.
A komponent that is missing or cannot be read is one placeholder that says which file, plus a finding; the area still draws.

Ways to put one in a layout, each one undo entry:

| From | How |
| --- | --- |
| a grid | The edit mode's palette lists them under **Komponents**, with their parameter counts; a drop, a pick or Enter places one in the free cells. |
| a bar | The bar's menu has **Add <komponent>**, which puts it at the end of the zone you pick. |
| anywhere | `hogar-shell komponent use <area>[.<group>] <name> [--zone start\|center\|end] [parameter=<expression>...]` — on a bar, `--zone` picks the zone as the menu does (the end zone by default); the only way into a dock or a free area. |

A use never replaces what a group holds: it goes into a new group with a readable id made from its name, or into a group that holds nothing,
and a group that holds modules or already draws a komponent is refused. The area's popover has **Parameters of <komponent>…**, each row showing
the komponent's default beside your override, and `layout set <area>.<group> parameters.<name> <expression>` is the script's way.

### Saving and detaching

Right-click a group's item and choose **Save group as komponent…** — or run `hogar-shell komponent save <area.group> <name> [parameter...]`. It
writes the group as the group is drawn, every level merged, and makes the group draw the komponent from then on. You pick which values become
parameters: an option becomes a parameter whose default is its current value (and a binding of the same key reads it back), and a binding
becomes a parameter whose default is its expression. A name that is taken is refused.

**Detach** is the other direction: **Detach <komponent>** in the menu, or `hogar-shell komponent detach <area.group>`. The group gets the
komponent's children back as instances of its own, under ids the layout does not use yet, with each parameter's value at that use written
into the expressions that read it. It draws what the use drew. A komponent's children cannot be edited through the use — `layout set` on one says to set the
use's parameters, edit the file, or detach.

```sh
hogar-shell komponent list     # each komponent, its parameters, and the layouts that draw it
hogar-shell komponent show battery-pill
```

The file is watched like a layout: save it and every use redraws.

## Bundles

A bundle is a directory:

```text
<bundle>/manifest.toml            name, and optionally description and author
<bundle>/layouts/<name>.toml      the layout, and each layout of its `extends` chain but the built-in one
<bundle>/components/<name>.toml   every komponent those layouts draw
<bundle>/assets/<file>            every picture they name, as `assets/<file>`
```

A directory rather than an archive, because a directory is what you can read, diff and audit before importing it — which is the point of
a trust prompt.

**There is no version field**, in the manifest, a layout or a komponent: one in the manifest is refused. A bundle written for a different build
fails to import with the same located findings any broken layout gets — an unknown module, an unknown key — rather than being converted.

### Exporting

```sh
hogar-shell layout export ~/shared/work-bar work      # the layout named, or the one being drawn
```

- Carries the layout, every layout of its `extends` chain except the built-in one (every installation has that already), every komponent any of
  those layouts draws, and every picture they name — a wallpaper region's `source` and a texture's `image` — copied into `assets/` and renamed to
  `assets/<file>`. Those paths are the only thing rewritten.
- Carries what a layout file holds, so it carries `[sources.<name>]` and `actions`. It does **not** carry `[[rules]]`, `[automation]` limits or
  variables: those are the installation's, so a bundle cannot add a rule or loosen a limit.
- Refuses the built-in layout, a picture that is not a plain file (missing, a link, or over the size limit), an `extends` or komponent the store does not hold, a layout or
  komponent whose name is not lowercase ASCII (see [names](#names)), and a destination that exists and is not empty. A bundle is made whole, never merged into another.
- Says how many commands, addresses and actions it runs, which whoever imports it is asked to trust one by one.

It reads files and answers from the command line, so it works whether or not the shell is running. The manifest's `name` defaults to the layout's
id and is what `layout trust` and the pictures' directory use; add a `description` and an `author` by hand if you like.

### Importing

```sh
hogar-shell layout import ~/shared/work-bar
hogar-shell layout use work
```

`import` goes to the running shell and does nothing until **all** of the bundle checks: the manifest and every file parse, every picture it
names is in it, its name and its file names are lowercase ASCII, no id, name or key holds a character that hides or rewrites what is around it, no command, address or action line holds a bidirectional control, and each layout, with the komponents it draws, validates against the built-in layout and what the bundle itself holds — so a bundle
that needs anything of *your* installation cannot be imported. Then:

- Pictures are copied to `~/.local/share/hogar-shell/bundles/<name>/`, and layouts and komponents are written to your `layouts/` and `components/` with
  their picture paths pointing there. Only the pictures a layout names are copied: anything else under `assets/` is never read.
- **Nothing of yours is overwritten.** A layout or komponent name you already use is refused, with the file named; the only exception is an unchanged
  copy the same bundle wrote, which a reimport replaces. A file you edited since is yours and is refused.
- **A bundle is its name and the files it writes.** Its identity is the manifest's `name` together with the names of its layout and komponent files, recorded
  at its first import. An update — other contents, other pictures, a new description or author — is a reimport of it, and a reimport keeps an answer you gave for
  an item whose text did not change and asks again for one whose text did. A bundle of the same name that writes other layouts or komponents is an impostor, a different bundle,
  and is refused, naming what the first one wrote; once every one of those files is deleted the name is free again, and the first bundle's pictures are deleted with what is remembered of it.
- Which files came from which bundle is remembered in `state.json`, along with every answer you give.
- **Everything the bundle runs is left pending**, listed in the reply, and the [trust dialog](#the-dialog) opens on the focused screen.
- It does not switch layouts: `layout use <name>` does, when you have looked.

**A bundle is somebody else's directory**, so it is read as plain files only and within limits:

| Refused | Because |
| --- | --- |
| a symbolic link anywhere in it, a FIFO, a socket, a device | a link would carry whatever you can read, a key or a password store, into a directory you may share next; a FIFO would leave the shell waiting on its other end |
| a manifest, layout or komponent file over 1 MiB (`LAYOUT_FILE_LIMIT`) | far more than any layout holds |
| a picture over 64 MiB (`ASSET_LIMIT`), or the pictures together over 256 MiB (`ASSETS_LIMIT`) | enough for any picture a screen can show, and no more than a disk should be asked to take |
| more than 256 files (`ENTRIES_LIMIT`) in `layouts/`, in `components/`, or among the pictures the layouts name | far more than a shared layout has |
| an `extends` chain more than 16 layouts deep (`EXTENDS_DEPTH`) | more than anybody layers by hand; a cycle is refused too |

Every directory of the bundle is opened once, and every file under it is opened relative to that directory, refusing a link at each step, so a directory swapped for a link after it was looked at — `assets` turned into a link to your home — redirects nothing.

#### Names

A bundle's `name`, and the name of every layout and komponent file it carries, is **lowercase ASCII**: `a`–`z`, `0`–`9`, `-` and `_`. That is how you tell one bundle from another, and a letter of another script shaped like a Latin one (`nоrd` with a Cyrillic `о`) would let a stranger's bundle pass for one you know. Komponents you save yourself follow the same rule.

Every id, name and key a layout or a komponent writes — an area, a group or an instance id, a source or parameter name, an option key, an output pattern — is something you read and type, so one holding a control character, a terminal escape, a bidirectional control or an invisible character is an error, in a bundle at import and in any layout of yours at load. A layout's `name` may hold any text you can read, emoji included, but no control character and no bidirectional control.

The reading, and the check of what the bundle's files say, happen on a worker thread, never on the one that draws the screen, and the shell reads **one bundle at a time**: an import started while another is reading is refused until it finishes. `import` waits for the worker — up to 60 seconds — and then prints what it wrote, lists what waits, and says whether the dialog opened. Past that, or where nobody waits for a reply, such as an action line that runs the import, a notice says how it went. Imports open the dialog at most once every ten seconds. A fault in the shell while a bundle is read is reported as that import failing, and the next import is not held up by it.

If a layout or komponent cannot be written — a full disk, a directory you made read-only — what was written stays and is remembered as the bundle's, what was not is neither in the shell nor in `state.json`, and when nothing could be written the bundle's record and pictures are as they were before the import.

Under `--safe-layout` an import is refused, like every edit.

## Trust

**What a bundle can run.** Everything a bundle's files write that makes the shell do something is an **item**:

| Item | Is |
| --- | --- |
| `poll` source | the source's `cmd`, run through `sh -c` on an interval |
| `listen` source | the source's `cmd`, kept running |
| `http` source | the source's `url`, fetched on an interval |
| action line | **every** line of an `actions` chain, in a layout or a komponent, except the few listed [below](#what-runs-without-asking) |

An [expression](data-and-rules.md#expressions) can only read, so it is never an item. A bundle cannot add a `[[rules]]` entry, which could run commands too.

### What runs without asking

An action line that only moves what the shell shows is not an item and runs as soon as the layout is drawn. The list is short and fixed in the command table, and a line is judged by the command it resolves to, never by running it, so spacing and arguments do not change the answer:

| Target | Commands |
| --- | --- |
| `panel` | `toggle`, `open`, `close` |
| `launcher` | `toggle`, `close` |
| `dashboard` | `toggle`, `open`, `close`, `tab` |
| `notifs` | `center` |
| `toast` | `clear` |
| `volume` | `up`, `down`, `mute`, `set`, `step` |
| `media` | `play-pause`, `next`, `previous`, `stop`, `seek`, `shuffle`, `loop` |
| `brightness` | `up`, `down`, `set`, `step` |
| `keyboard` | `next` |
| `lock` | `on` |

Every other command waits for your answer, because each can do something this list may not: run a program (`shell run`), fetch something or change the network, write a layout, the config, a wallpaper, a variable or any other state, run a rule, end or change the session, or hand trust out. Three that look harmless are on purpose not on the list:

- `toast show` puts text of the bundle's choosing on your screen, which can pose as a message from the system.
- `mic mute` toggles, so it can switch your microphone on.
- `lock off` unlocks the session; `lock on` only locks it, which is why that one is listed.

**`layout trust` never runs from an action.** Trust is yours to give, and a file that could press it for you would trust itself. `layout set` refuses to write such a line, any other edit that would add one is refused too, a file that holds one gets an error from `layout check` and at load, and in a bundle's file the line is dropped from its chain.

### Per item, bound to its exact text

Each item is accepted or declined on its own, and the answer is kept with the text that was shown. An edit or a reimport that changes an item's text asks again for **that item alone**; the rest of the file is unaffected. A source's `lock_safe` is part of what is shown and accepted: a file that starts saying a command is safe for the lock screen asks again, and the promise is honoured only once the item is accepted.

**An answer is for what you were shown, and for nothing else.**

- An item's id is 32 hexadecimal characters (128 bits) of a SHA-256 over its file, key, kind, text and `lock_safe`. An id you copied answers for exactly that item; once an edit changes the text, the id no longer exists.
- **`--all <set>`** takes the id of the whole list `layout trust <bundle>` printed, 32 hexadecimal characters of a hash of every item id in it. It answers for the list as printed, so an item that came to run since is never accepted with the rest.
- The dialog's rows work the same way: a row answers for exactly the item, or the bundle's items, it showed.
- When the bundle's items are no longer what was shown — edited, reimported or answered in the meantime — the answer is **refused whole**, nothing is recorded, and the reply says to look again.

**Until you accept, an item is off, with a finding that says so** — never a runtime failure:

- a held source is never declared, so no producer ever starts for it, and an expression reading it reads as waiting;
- a held action line is taken out of its chain, so the gesture does nothing rather than something else;
- `layout check` names each held item and the command that would accept it, and a notice counts what waits.

A file a bundle brought stays that bundle's however you edit it, so a command edited into it asks too; a file you wrote yourself is never held. An edit that would copy a held text into a file of yours is refused, and a text is judged with its `lock_safe`: a command your file already runs off the lock screen does not make the bundle's lock-safe copy of it yours.

### What you read is what runs

A command can be made to look like another. Wherever an item's text is shown — the dialog, the `layout trust` listing, the reply to `layout import`, a finding — every control character, line break, tab, terminal escape, bidirectional control, invisible or formatting character (zero-width spaces and joiners, variation selectors, tag characters, Hangul fillers and the like) and every space that is not a plain space is written out as `\n`, `\r`, `\t` or `\u{…}`, and a run of more than three spaces as `\u{20}{×n}`, so a wide gap cannot push the rest of a command out of sight. The text on screen is the text that runs, character for character. In the dialog the command is also laid out left to right as one block, so letters of a right-to-left script inside it cannot carry a pipe or a path into another order.

Everything the command line prints from the shell goes through the same escaping, keeping only its line breaks, tabs and padding, so no reply — a finding quoting a bundle's id, a listing — can repaint your terminal; and the notices the shell raises are plain text, never read as markup.

A bidirectional control has no use in a command, an address or an action line, so an import **refuses** a bundle that holds one, with a finding that shows the line with the character written out and names its code point.

### The dialog

The shell opens a dialog listing every item that waits: its text exactly as written, where it is written (the file and the key), what kind it is, and whether the file says the lock screen may show what it reads. Each item has **Accept** and **Decline**; each bundle has **Accept all** and **Decline all**. It opens right after an import, from the **Review…** button of the notice that says something awaits your decision, and from `hogar-shell layout trust --dialog`. What waits at startup raises the notice alone.

- **Closing decides nothing.** Esc, **Decide later** and the session locking all close it and leave every item waiting, so there is no way out of it that runs something. It has no default button: Enter answers only the control the keyboard was moved to.
- It never opens while the session is locked, and the lock closes it.
- Imports open it at most once every ten seconds, so a run of imports cannot keep taking the keyboard; the notice and `layout trust --dialog` always can.
- A declined item stays off, and `layout check` reports it as declined rather than waiting.

### From a script

```sh
hogar-shell layout trust                       # every imported bundle, and how many items are pending, accepted, declined
hogar-shell layout trust work                  # what the bundle runs: each item's id, state, kind, where it is written, and its text
hogar-shell layout trust work 9f3a1c2e5b7d4086 # accept one item, by the id that listing gives
hogar-shell layout trust work --all 41c0d2a97e3b58f6   # accept everything that listing printed, by the set it ends with
hogar-shell layout trust work 9f3a1c2e5b7d4086 --decline
hogar-shell layout trust --dialog              # open the dialog
```

An item's id is the same in every process for the same file, key, kind, text and `lock_safe`. The listing ends with the line that says how to accept one item or everything listed; read the list before you accept all of it. **A bundle cannot accept for itself**: `layout trust` is never an action.

### Edits never copy held text

Moving things around inside a bundle's own file keeps what is held: an editor move or a bar split writes a held line along with the chip it belongs to, and it stays held and listed where it lands, rather than the move erasing it.

An edit that would copy a command, address or line a bundle brought and you have not accepted into a file of **your own** is refused, because your files run whatever they hold and the copy would make the bundle's text yours without your ever trusting it. That is moving an instance out of an imported area, saving an imported group as a komponent (the menu's card says so and stays open, and `komponent save` says so too), and detaching an imported komponent. The refusal names the item and the command that trusts it, and changes nothing, so there is nothing to undo; the edit never drops the line to get past the refusal. Accept it first, or leave it out of what you copy. A text your own file already ran stays yours.

### Notifications cannot run shell lines

The **Review…** button is a button of the shell's own notice, and a notice's buttons run a line in the shell only when the shell raised it. A button on a notification sent by any other program is sent back to that program as the freedesktop `ActionInvoked` signal and never runs anything here, and a program cannot take over one of the shell's notices, buttons included, by naming its id.

## Related

- [Layouts](layouts.md) — the files a bundle carries, and `extends`.
- [Data and rules](data-and-rules.md) — sources, actions and the lock screen's `lock_safe`.
- [Edit modes](edit-modes.md), [Popovers](popovers.md) — saving a group as a komponent from the menu.
