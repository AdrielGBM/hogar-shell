---
id: popovers
kind: customization
title: Context menus and popovers
summary: Right-click anything to customize it with handles and scrub fields on the item itself, one undoable edit at a time.
status: stable
compositor: any
config: [keynav]
commands: [layout]
deps: [wlr-layer-shell]
see_also: [edit-modes, layouts, data-and-rules, bundles, bars]
---

# Context menus and popovers

## What it is

You customize an item by pointing at it. A **context menu** lists what you can do to it, and **Customize…** opens a
**popover**: a card beside the real item, with a row for each thing you can change and handles on the item itself, both
editing one value two ways. Values are manipulated directly — dragged, scrubbed, stepped — rather than typed, though
anything numeric can be typed too.

Neither needs an [edit mode](edit-modes.md). A context menu and a popover open on a bar chip, a widget, a card or an
area whether or not a mode is up; a mode adds the selection, the strip and the tools of its layer.

## Context menus

A secondary press opens one; so does the Menu key (or Shift+F10) on what has the keyboard focus. It appears at the pointer,
or on the item when the keyboard asked, and never past the edge of the screen.

- **On an item** — its module's own actions, **Customize…**, a move to an area that draws it the other way
  (chip to widget and back, keeping its id, options and state), **Save group as komponent…**, **Give it a panel** (and
  **Give it a panel along the whole bar** in a bar) or **Edit its panel**, **Remove**, and **Edit <layer>…**. On a child of a [komponent](bundles.md#komponents) the menu has the module's actions, the use's
  parameters, **Detach** and **Edit <layer>…** instead.
- **On an area** — its own bound actions, **Customize…**, what its kind adds (a bar's split and join, a stack's
  routes) and **Edit <layer>…**.
- **On a placeholder** — a module this build does not have, or cannot draw the way the layout asks — **Remove** and
  **Reset**, which act on the layout.

Empty space has a menu only where the area already answers the pointer by painting: a bar's strip, or an area with a visible
fill. A transparent area has none, because a press handler there would take presses from the windows underneath. Outside
an edit mode a chip is not focusable, so the keyboard route to a menu is the edit mode's. There are **no menus on the lock
layer**.

Each row that changes the layout is one edit, so Remove, Reset and a move are each taken back by one undo.

## Popovers

A popover is the thing you reach with **Customize…**, with Enter on the selection in an edit mode, and on the lock layer
only from its edit mode.

- **An item's** card is generated from its module's options: each option the module declares gets the row that fits its
  type — a number to scrub, a stepper, a toggle, a swatch, a choice — with the option's own documentation as its
  explanation. It also has its **Id** (see [Ids and rename](#ids-and-rename)), **Drawn as** (the size of the representation, not shown inside a container that arranges its children), the item's
  [bindings](data-and-rules.md#bindings), its **Look** — fill, corner radius (all four and each corner), opacity, border
  width and colour, shadow; its accent stays an option of its module — its **Actions**, **Reset options and look**,
  which takes its options and its style off where the popover writes, and **Remove**. Inside a container that arranges
  its children a line says its size is the container's to give.
- **An area's** card has its **Id** and the tools of its kind first — a bar's edge, thickness, length, offset, shape, auto-hide;
  a grid's cell size, gap and anchor; a panel's shape (beside its owner, or along its owner's bar), its columns and rows
  (only its depth along a bar), cell size and gap, with a handle on the corner it grows from (or the far side along a
  bar) that steps whole cells as it is dragged; a stack's anchor, width, flow (a column or a row of cards), routes and screens; a region's picture, fit and transition;
  a texture's paint — and then what every area has:
  - **This workspace only**, which writes the change into that workspace's rule instead (see
    [Layouts](layouts.md#output-rules-and-workspace-rules)), except on the top layer;
  - on a grid, how many cells it has room for on this screen — what its cell, gap, padding and rectangle come to, read
    only;
  - **Shown while**, an expression that decides whether the area is drawn;
  - for each of its groups, **Repeated over a list** or **Drawn once** — the group's `repeat`;
  - **Look**: fill, corner radius (all four, then each corner; a bar rounds itself by its shape instead), opacity,
    padding (all four sides, then each) where the area holds groups (a grid, a panel, a free area, a dock or a bar),
    border width and colour, shadow (its kind's, none, soft, medium or strong) and backdrop. Where nothing writes a
    radius or a padding, its row shows the one drawn, marked default, and writes only once you change it: a panel's
    radius is the theme's beside its owner and square along a bar. A picture or a texture has no border or shadow;
  - under **Opacity**, while a fill is set, the **contrast** of the theme's text colour on it, which follows the fill,
    the opacity and the theme as they change and turns to a warning below 4.5:1 (WCAG AA). Nothing is refused; only
    the lock's prompt, which has its own line, is kept to it. The fill is judged at its opacity over what is behind it,
    which is only approximated: the layer's base for an area, and the area's own fill over the base for a group or an
    item. A wallpaper, a picture, a backdrop or the group around an item is not counted. A group's and an item's
    **Look** has the same line;
  - **Actions**, below;
  - **Behaviour**: whether it keeps windows out of its edge, whether it stays above fullscreen windows — with the note that
    this keeps the screen off direct scanout — and which box it is measured in;
  - **Remove**, on every area but the lock screen's prompt.
- **A group's** card has its **Id**, then its arrangement — a loose run, a column, a row, a grid, free, or **One at a time (Smart Stack)**;
  a group in a bar's run is a loose run or one at a time — with the inner grid's columns and rows under a grid and the
  gap between children under any arrangement but one at a time (the plate's own gap, tighter on a bar, until you set
  one); what it repeats over; the parameters of the komponent it draws, each with the komponent's default shown beside it;
  its **Look**, as an area's without the backdrop and with padding only while it arranges its children; on a grid, how
  many columns and rows it covers, moving what it grows over out of its way; and **Remove**. A double-click on a selected
  group, or Enter on it, opens it.
- **The theme**, from **Theme…** on an edit mode's strip or in-mode menu, has the palette, accent, base radius, plate opacity
  and text size. It edits `[theme]` in `config.toml` rather than the layout, so it previews on every window, is written
  when it closes and stays out of the undo history — see [Edit modes](edit-modes.md#the-theme).

**Remove** on a card puts back whatever the popover previewed, takes the item away as an entry of its own in the undo
history, and closes the card.

Where the card goes: beside what it customizes, on the side that item faces, and always inside what the bars leave of the
screen.

### Colours

Every colour row — a fill, a border, a stop, an accent — is one field: a theme token or a `#rrggbb` typed as text, which
is the value itself, the theme's colours as swatches with the one it names lit, and red, green and blue fields that
write a hex. Text that is neither writes nothing, and the row says where its value comes from like any other (see
[Where a value comes from](#where-a-value-comes-from)).

### Ids and rename

The **Id** row of an area's, a group's and an item's card shows the id every other level, rule and script addresses it
by. Type another and press **Rename**: the shell rewrites every place the layout names it — the other levels, a
panel's owner, a komponent's children — as one undo entry named for the rename, and selects the item under its new id.
The card's own changes are kept first, since its rows address the item by the id it is losing, and the card closes. A
refused rename leaves the id alone and says why under the row: the id is empty or holds a `.`, `/`, `#` or space, it
is already used, or a `[[rules]]` command, an action line or a layout that extends this one names the old id and
cannot be rewritten. A child of a komponent has no row, since its id is the komponent's. The same edit from a script
is [`layout rename`](../../guides/scripting.md#editing-the-layout).

### Actions

An area's and an item's card list what each gesture runs: one row per bound gesture — press, long press, wheel up, wheel
down, middle and secondary press — with the chain of [commands](../reference/commands.md) it runs, `;` between them,
and **Add action**, which adds a row for the first gesture nothing binds yet. The row is written only once its chain
runs something, so a gesture keeps whatever answers it until then, and a chain emptied again takes its binding off here.
A gesture can be bound once: moving a row to one that is already bound is refused. Every line is checked as `layout set`
checks it over IPC — a line the shell has no command for is refused, and so is `layout trust …`, since trust is yours to
give and never a gesture's — and a refused line is said under its row while the chain keeps what it had. A row says where
its chain comes from, like every other row; **Remove** beside one this level binds takes it off here. On an area the note
says what its gestures are: **an area receives the gestures that land between its widgets**, never those that land on
one. Nothing is bound on the lock screen, on the background layer, or on a picture or a texture.

## One transaction

**A popover is one edit.** Every row and every handle previews live on the real item, and then:

| You do | Result |
| --- | --- |
| Enter, click outside, press Done, or close it any other way | Everything you changed is **one entry** in the undo history. |
| Esc, or the other mouse button on a gesture | The layout goes back **exactly** as it was when the popover opened. |

The theme popover and the lock screen's privacy popover take the same keys, but what they keep is written to
`config.toml` rather than added to the undo history, and Esc puts back the config they opened with.

The same convention runs everywhere: a drag previews every frame and commits when you let go; Esc or the other button
puts it back. Only one edit is open at a time — starting a second is refused with a message instead of being nested. A
value that does not check — an expression with a typo — is never written; the layout keeps what it had.

One thing is outside it: what the lock screen may reveal. Those two choices are `[lock]` config, previewed live in their
popover and written to `config.toml` when it closes, so they are not in the layout's undo history (Esc still puts them
back). See [Lock screen](../system/lock.md).

## Where a value comes from

A layout is written at several levels — a layout it extends, the `*` rule, a rule for one monitor, a workspace's rule —
and a popover writes into one of them: the narrowest rule that already writes the item (see
[Layouts](layouts.md#output-rules-and-workspace-rules)). So a line under each row says where the value it shows comes
from:

| The line says | Meaning |
| --- | --- |
| **Set here** | The level the popover writes into writes it. **Reset** beside it takes the key off that level, so the value is inherited again — the row and the item show the inherited value at once, and it is part of the popover's one undo entry. |
| **From `<rule>` in `<file>`** | A broader level writes it, such as `outputs.*` in `layouts/custom.toml`. Changing the row writes it here, over that. |
| **Overridden by `<rule>` in `<file>`** | A level that comes after this one writes it — usually a workspace's rule — so a change written here would not show. Change it there instead. |
| **From the komponent in `<file>`** | The group draws a komponent and the komponent's own file writes it. It cannot be changed here: change it in that file, or set a parameter the komponent reads. |
| **Default** | No level writes it: the value is the default for its kind, or for an item's option, its module's configuration. |

An item's options say the same, option by option, and their **Reset** is this one. An expression (**Shown while**,
**Repeated over a list**, a binding, a komponent parameter) cannot be inherited away by deleting it here, so an inherited one
has **Remove** instead, which writes an `unset` — see [the expression field](#the-expression-field).

## Handles and scrub fields

Handles are points laid over the real item, each the pointer's way to a value the popover also has as a row:

- **A bar** — its thickness, where it starts and ends along its edge, how far it floats off the edge, and a handle at
  each corner for its radius.
- **A texture** — the four cuts of a nine-sliced image, and for a gradient its angle and a handle on each stop.
- **A widget** — a corner handle that steps its size through S, M and L, unless a container arranges it.
- **A stack** — the nine anchor points of its screen, and its first card, dragged, to pin and offset it.

Every handle has a row for the same value, which is its alternative for a single pointer or a keyboard: you never need
to drag.

A **scrub field** is a number you change by dragging it, by the arrow keys once it has the focus, or by typing after a
double-click or Enter — **the one convention every number in the shell uses**:

| Hold | Step |
| --- | --- |
| nothing | the field's own step |
| Shift | ×10 |
| Alt | ×0.1 |

The four corners (and, with the padding tool, the four sides) are **linked** until you press `u`, which unlinks them —
and links them again — for the rest of the session. Linked, a corner handle dragged, or stepped by an arrow once it has
the focus, rounds all four corners together, and Alt held **isolates that corner**, leaving the other three where they
were; unlinked, each moves only its own. A handle you *drag* follows the pointer instead of stepping, so there Alt only
isolates; an arrow with Alt both isolates and steps a tenth. The popover has a row for each corner and one for all four.
A corner radius stops at half the area's short side, where two arcs would meet, and says it is clamped. On a short side,
such as a slim bar, the handles of its two ends slide apart along their edges so none covers another.

A handle asks the compositor for the matching cursor (`grab`, `ew-resize` …) through `wp-cursor-shape-v1`; without it the
handles still work and the pointer image just does not change.

## The expression field

**Shown while**, a binding's value, **Repeated over a list** and a komponent parameter's override are all one thing:
a line of the [expression language](data-and-rules.md#expressions), checked and evaluated as you type.

- **Under the line**, the value the expression has right now and its type, read the way the layer draws it — on the lock
  layer, through what a locked screen is allowed to see. An error is underlined where it is, with its message under the
  line; an evaluation error is underlined too, while the last good value stays shown.
- **It writes only what checks.** While the text does not compile to the type wanted, the popover keeps the layout's
  version and the field keeps your text and says what is wrong, so the popover's one transaction never holds a broken
  expression.
- **Completions** follow the word at the caret: `$` and a name offer the readings — module sources, the layout's own
  sources, variables, events and, inside a repeated group, `$item` and `$index` — and a bare word offers functions and
  constants. The arrows walk them, Enter or Tab takes one, Esc closes the list.
- **The source browser** under the field lists every reading with what it reads now; press one to put it in at the caret.
- **Keys.** Enter commits the popover, as it does from any control. Esc in the field puts its text back as the popover opened
  it and hands the keyboard back, so the next Esc reverts the popover.
- **Where it comes from.** An expression row carries the same line as every other row — set here, from the rule and file
  that wrote it, overridden by a later rule, or default (see [Where a value comes from](#where-a-value-comes-from)). On an
  inherited expression **Remove** is its reset: it takes the expression back here by writing an `unset`.

## Related

- [Edit modes](edit-modes.md) — the selection, the strip and the tools by layer.
- [Data and rules](data-and-rules.md) — what an expression can read, and what a binding can drive.
- [Layouts](layouts.md) — where each edit is written.
