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
  (chip to widget and back, keeping its id, options and state), **Save group as komponent…**, **Remove**, and
  **Edit <layer>…**. On a child of a [komponent](bundles.md#komponents) the menu has the module's actions, the use's
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
  explanation. It also has **Drawn as** (the size of the representation), the item's
  [bindings](data-and-rules.md#bindings), **Reset** and **Remove**.
- **An area's** card has the tools of its kind first — a bar's edge, thickness, length, offset, shape, auto-hide;
  a grid's cell size, gap and anchor; a stack's anchor, width, routes and screens; a region's picture, fit and transition;
  a texture's paint — and then what every area has:
  - **This workspace only**, which writes the change into that workspace's rule instead (see
    [Layouts](layouts.md#output-rules-and-workspace-rules)), except on the top layer;
  - **Shown while**, an expression that decides whether the area is drawn;
  - for each of its groups, **Repeated over a list** or **Drawn once** — the group's `repeat`;
  - **Look**: fill (a theme swatch), corner radius, opacity, padding and backdrop;
  - **Behaviour**: whether it keeps windows out of its edge, whether it stays above fullscreen windows — with the note that
    this keeps the screen off direct scanout — and which box it is measured in.
- **A group that draws a komponent** gets that komponent's parameters, each with the komponent's default shown beside it.
- **The theme**, from **Theme…** on an edit mode's strip or in-mode menu, has the palette, accent, base radius, plate opacity
  and text size. It edits `[theme]` in `config.toml` rather than the layout, so it previews on every window, is written
  when it closes and stays out of the undo history — see [Edit modes](edit-modes.md#the-theme).

Where the card goes: beside what it customizes, on the side that item faces, and always inside what the bars leave of the
screen.

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

## Handles and scrub fields

Handles are points laid over the real item, each the pointer's way to a value the popover also has as a row:

- **A bar** — its thickness, where it starts and ends along its edge, how far it floats off the edge, and a handle at
  each corner for its radius.
- **A texture** — the four cuts of a nine-sliced image, and for a gradient its angle and a handle on each stop.
- **A widget** — a corner handle that steps its size through S, M and L.
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

A handle you *drag* follows the pointer instead of stepping, so on a corner handle Alt means something else: it **isolates
that corner**, leaving the other three where they were, where a plain drag rounds all four together. The popover has a
row for each corner and one for all four. A corner radius stops at half the area's short side, where two arcs would meet,
and says it is clamped. Arrow keys on a focused corner handle move that corner alone.

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
- **Inherited expressions.** An expression a broader level of the layout wrote is marked as set by that level, and **Remove**
  takes it back here by writing an `unset`. Where a level that comes after the one the popover writes into gives the same
  expression, the popover says so rather than offering a change that would not show.

## Related

- [Edit modes](edit-modes.md) — the selection, the strip and the tools by layer.
- [Data and rules](data-and-rules.md) — what an expression can read, and what a binding can drive.
- [Layouts](layouts.md) — where each edit is written.
