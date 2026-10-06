---
id: edit-modes
kind: customization
title: Edit modes
summary: One mode per layer — background, desktop, top, overlay, lock — each with the tools that layer needs, all reachable from the keyboard.
status: stable
compositor: any
config: [keynav, lock, theme]
commands: [layout]
deps: [wlr-layer-shell, ext-session-lock]
see_also: [layouts, popovers, data-and-rules, bars, widgets, wallpaper, lock, keybinds]
---

# Edit modes

## What it is

An **edit mode** is the shell with its hands showing: the real layer, on the real screen, with handles and a strip of
controls laid over it, so you arrange the [layout](layouts.md) by moving what you can see. There is one mode per layer,
because the five layers have different things worth doing to them and are never edited together.

| Mode | What you arrange there |
| --- | --- |
| `background` | Wallpaper regions and the textures laid over them. |
| `desktop` | Grids of widgets. |
| `top` | Bars, and the chips on them. |
| `overlay` | Stacks of cards: notifications, toasts, volume and brightness; where the launcher opens. |
| `lock` | The lock screen, previewed. |

## Entering and leaving

```sh
hogar-shell layout edit desktop          # the desktop layer of the focused screen
hogar-shell layout edit top DP-1         # the top layer of a named screen
hogar-shell layout edit off              # leave
```

Any of these gets you in:

- the `layout edit` command, which is what a keybind should run — see [Keybinds](../../guides/keybinds.md);
- **Edit <layer>…** in the context menu of an item or an area, which opens the mode of the layer that item is on;
- **Edit ▸** in the shell menu, which a right-click on the empty desktop opens (see Context menus below);
- the mode strip's switcher, or `m` with a mode up, which opens a pie of the other modes: press one, or press the arrow
  that points at it.

**One mode is up at a time**, process-wide: entering a mode leaves the last. It leaves on its own if its screen is
unplugged. To leave on purpose, press **Esc**, press **Done** on the strip, or run `layout edit off`. Esc closes
whatever is open above the mode first — a popover, a menu, an edit you are holding — then clears the selection, and only
then leaves, so a stray Esc never throws away your place.

The **strip** sits at the foot of the screen, under the mode's toolbar, and is dragged anywhere by its grip; it stays
where you put it, across modes, until the shell restarts. In the top mode it sits higher while the bottom edge has no
bar, so the edge a new bar is pulled out of stays reachable. It names the mode — press the name for every layer to
switch to — and the screen; `+` opens the mode's Add (the palette, a texture in the background mode, or a new stack in the overlay mode); **History ▸**
lists the undo history to jump through; **Theme…** opens the [theme popover](#the-theme); and **Keys** and **Done**. Under it, one line says what the last thing you asked
for did where that is not plain to see — "Undone: …", "Redone: …", a fork of the built-in layout — or why it was not
done, for six seconds. A thin accent border runs round the edited screen so it is never unclear which one is being
edited. While a mode is up it holds the keyboard, so modifiers and Esc arrive without a click.

On the background and desktop layers the mode raises that layer's window above your application windows for the
session, so you edit the real items rather than copies of them; on a compositor that cannot move a layer window, the
same areas are drawn inside the mode's own window instead. Leaving puts every window back exactly as it was.

**What refuses a mode.** Under [`--safe-layout`](layouts.md#when-a-layout-goes-wrong) every mode is refused, like every
edit. `lock` is refused while the session is locked.

## What every mode shares

- **Select, then act.** Click an item, area or group to select it; the tools of the mode act on the selection. As the
  pointer passes over the layer, a thin outline shows what a click there would select. A selected group or widget has
  its size under it — in cells where it sits on a grid's cells, then in pixels — and a double-click customizes what it
  selects, as Enter does.
- **Drag with the pointer** — and everything a drag does has a key too, so nothing needs a pointer:

  | Keys | Does |
  | --- | --- |
  | Arrows (and `h` `j` `k` `l` under `[keynav] vim`) | select the nearest thing of the same depth that way |
  | Home / End (and `g` / `G` under `[keynav] vim`) | select the first or the last |
  | Alt+Down / Alt+Up | select what is inside / what holds it |
  | Tab / Shift+Tab | next or previous area |
  | Enter | [customize](popovers.md) the selection |
  | Menu key, or Shift+F10 | open its context menu |
  | Shift+arrows | move it one slot, cell or edge over |
  | Ctrl+arrows | make it one step bigger or smaller |
  | Delete or Backspace | remove it |
  | Ctrl+D | duplicate it: a widget on a grid onto the nearest free cells of its size, a child of a container or a chip right after itself, a group of a grid with what it holds, an area right after itself (a bar into the free stretch of its edge) |
  | Ctrl+] / Ctrl+[ | bring it forward or send it backward among what it overlaps; with Shift, to the front or the back (see below for what has an order) |
  | Ctrl+Z · Ctrl+Shift+Z or Ctrl+Y | undo · redo |
  | `m` | the mode pie |
  | `w` | edit this workspace alone, or every workspace again (background, desktop and overlay) |
  | `r` · `i` | the radius tool · the padding tool for the selection, pressed again to put it away |
  | `u` | move the four corners or sides together, or each on its own |
  | `.` | reach the quick bar: arrows, Home and End walk its buttons, Enter or Space presses one, Esc or `.` hands the keyboard back |
  | `?` or F1 | list the keys of this mode |
  | Ctrl+K, or Ctrl+Shift+P | the command palette (only Ctrl+Shift+P under `[keynav] vim`, where Ctrl+K makes the selection taller) |
  | `\` (held) | before and after: show the layout as it was when the mode opened, on every layer, until you let go or press Esc; the strip says "As it was when you started", editing keys do nothing meanwhile, and nothing is recorded or written |

  A held key is one undo entry however long the keyboard repeats it, and is previewed until you let go.
  No tool takes `h`, `j`, `k`, `l`, `g` or `G`, which `[keynav] vim` keeps for moving the selection.
  A focused control — a handle, a quick bar button — answers its own keys first, and the mode's keys get what it leaves.
- **The command palette.** Ctrl+K opens one searchable list of everything the mode can do: every key of the mode's
  key list with its chord — a key that goes four ways, or both ends, is one entry per way — every widget, container
  and komponent the add palette offers, the strip's actions (Theme…), each step of the history to jump to, the other
  modes and Done. Typing narrows it — the letters in order, word starts counting most, or a chord such as
  `Shift+←` — the arrows walk it, and Enter or a click runs the entry against the selection, as one undo entry; Esc
  closes it and hands the keyboard back. What the selection would refuse stays listed, dimmed, with the reason.
- **Copies and stacking order.** Duplicate (Ctrl+D, or the menu) gives every instance it copies an id of its own, and a
  copied komponent use stays a use of that komponent. What there is one of or that tiles its layer is refused with why:
  the lock's prompt, a panel (it belongs to the widget that opens it), a wallpaper region (split one instead), a grid
  (duplicate its widgets or containers) and a dock. Free areas, textures and card stacks, and the children of a `free` container, overlap, so they have an
  order — Ctrl+] and Ctrl+[, Ctrl+Shift+] and Ctrl+Shift+[ all the way (on any keyboard layout that types the brackets),
  or "Order ▸" in their menu. Bars, wallpaper regions, grids and docks tile their layer or their edge, so they have
  none, and the lock's prompt is always drawn last. Something a layout this one extends places keeps its place, since a layout laid over another never
  restacks it.
- **Rectangles.** A selected texture, free area, grid or lock prompt, on any layer that has them, has a handle on each corner
  that resizes it from there, the opposite corner staying put, with its size shown beside the pointer as a share of its
  box ("40 × 25 %"). Dragging a texture's body moves it, as dragging the prompt does. Whatever you move or resize snaps
  its dragged edges (or, when moved whole, its edges and centre) to the areas, textures and free areas beside it and to
  the edges and middle of its box within 6 pixels, with a guide drawn where it snapped. The prompt never gets smaller
  than the least a prompt may be and never leaves its screen. A selected free area also shows its nine anchors: press
  one, or drag the area's body, to pin what it holds to that ninth. Every one of these has a popover row, and
  Shift+arrows and Ctrl+arrows move and resize the rectangle a step at a time. A child of a free container
  snaps the same way to its siblings and its container's box. Hold **Alt** while dragging to place anything freely. The corner handles are hidden while the
  radius or padding tool is up.
- **Corners and padding.** The radius tool (`r`) puts a handle over each corner's arc of the selection — any area, group or
  widget — with its radius beside it; the padding tool (`i`), for an area that holds groups (a grid, a panel, a free area, a
  dock, a bar) or a container, tints the padding along each edge, outlines the box what it holds is laid out in and puts
  a handle on each side. While the four are **linked** a drag, or an arrow on a focused handle, moves all four together and
  writes one number; **unlinked** (`u`, remembered for the session) each moves on its own and all four are written, and
  **Alt** isolates the one dragged or stepped while they are linked. A corner let go within a few pixels of its corner is
  squared off; no corner rounds past half the short side, and no padding past that less 4 pixels, which the pointer is
  told. A bar's corners are its `shape.radius`, everything else's its `style.radius`, and padding is `style.padding` —
  the same values the popover's rows set, within the same limits. An arrow held on a focused handle is one undo entry,
  as every held key is. Changing the selection or opening a popover puts the tool away.
- **The quick bar.** The selection has a small bar of buttons beside it: a grip, **Customize** (as Enter), **Panel** for
  an item off the lock that is not inside a panel (lit once it owns one), the **radius** and **padding** tools where they
  apply, **Linked / Independent** while a tool is up, and **Remove** wherever removing the selection is not refused —
  never on the lock screen's prompt. It lies above the selection where there is room, else below it clear of the size
  tag; beside a selection taller than it is wide, upright; and only inside one that leaves no room around it, past its
  padding. It never covers a selection that leaves it room, stays on screen, steps aside while a popover is open, and
  its grip drags it out of the way for that selection alone, until the mode changes. `.` gives it the keyboard, and each button's name says the
  key that does the same, as the mode's key list has it.
- **Panels.** **Panel** on the quick bar, or **Give it a panel** in an item's menu, writes a panel owned by that item on
  its layer at the panel's defaults (4 × 3 cells), opens it and selects it, as one undo entry; an item in a bar also has
  **Give it a panel along the whole bar**, a panel that runs along the bar's whole length. Once it owns one, the row reads **Edit its panel**
  and opens and selects it. Removing the panel is its own undo entry and leaves its owner; removing the owner takes the
  panel with it. The lock layer has no panel rows.
- **Context menus.** Right-click an item for its own actions plus Customize, Remove, Reset, the move to an area that draws
  it the other way (chip to widget and back, keeping its id, options and state), its panel, and save as a [komponent](bundles.md#komponents).
  Right-click an area for Customize and what its kind adds. On the layer being edited, both menus end with Undo, Redo,
  History and Theme…. Right-click the empty desktop — the wallpaper where no window covers it, in a mode or not — for
  the shell menu: **Add ▸** (a widget and a container wherever the layer has a grid, a group on a plate on the top layer, and in a mode
  the layer's add tools — a new grid, bar, texture, region or stack, never its other toolbar buttons), **Customize grid…**, **Edit ▸** a layer, and then, in a mode, the same Undo, Redo, History and Theme… with
  **Keys** and **Done**, or outside one Theme… and **Lock**. Outside a mode it acts on the desktop layer, and a row
  that needs a mode enters it first. A `secondary` action the layout binds to the wallpaper runs instead.
- **Every change is a layout edit.** One gesture, one popover, one key press, one script line is one undo entry; Ctrl+Z
  and `hogar-shell layout undo` take back the same history, whatever made the change. The history list on the strip
  and in the context menu shows every entry, the one the layout is at marked, each with how many steps back or forward it
  is (redo entries dimmed); picking one walks there one ordinary undo or redo at a time. The first edit of the built-in
  layout [forks it](layouts.md#files), and the strip says so.
- **Where the edit is written.** Into the narrowest output rule of the layout that already writes the thing, which is where
  the value it replaces came from. Something only a layout it extends writes gets a partial entry in the narrowest rule
  that covers the screen, naming just what changed, so the change lies over the inherited area instead of copying it.
  With `w`, the change goes into that workspace's rule instead, made for it if there is none — and resolves on that
  workspace only. The top layer has no workspace variant: bars reserve space, which a workspace rule may not change.

## The theme

**Theme…** on the strip, or in the context menu of anything on the layer being edited, opens a popover for how the whole
shell looks: the palette (one tile per palette, painted in its own colours), the accent (a hue swatch or a `#rrggbb`),
the base radius, the opacity of the plates and the size of the text. Every change shows at once on every bar, widget and
other area. The theme is `[theme]` config rather than layout, so it is **outside the layout's undo history**: closing the
popover any way but Esc writes it to `config.toml`, keeping the comments and everything else the file says, and **Esc**
or **Cancel** puts back the theme it opened with.

Before the theme is saved, the popover runs the lock screen's own check against it: where the lock would fall back to
the minimal one with it — a prompt fill its text would no longer be readable on, say — the popover says so and why.

## Tools by layer

### Background

Wallpaper regions: split a region side by side (`s`) or top and bottom (Shift+S), join it with the region that way
(Alt+Shift+arrows, as bars are joined), drag the grip on the edge two regions share, or move and resize it with the generic keys — neighbours follow,
so no gap opens. Each region has its own picture, fit and transition; "Follow the background" puts it back on whatever
`[background]` says. Split and join are different hotspots on purpose.

Region edges and cuts **snap** to the cell lines of the desktop grid (the lock's grid on the lock layer), in the middle of
the gap between two cells: a dragged edge within about 1 % of a line, a cut — dragged from its split button, or made by
`s` — within 4 %. The lines are drawn while you drag.

Textures lay an image or a gradient over a region (`t`). An image can be tiled or nine-sliced (`n`), with a handle for
each cut in the image's own pixels; a gradient has its axis drawn over it, a handle that turns it (`[` and `]` turn 45°; Shift snaps a
drag to 45°), and a handle per stop, at most eight. See [Wallpaper](../surfaces/wallpaper.md).

### Desktop

Grids of widgets placed by explicit cells, which are where the grid's rectangle puts them whatever is on it. `a` opens
the **palette** of every module that draws as a widget and the komponents you have saved. Each entry shows the sizes
its module draws, S, M and L: press one (or use ←/→ on the entry the arrows point at) to choose the size a drag, a press
or Enter puts. Drag an entry out and let go on free cells, on the middle of a widget to make a Smart Stack with it, into
a container at the slot under the pointer, or on the cells of an open panel; a tag beside the pointer names it and says
where, over a copy of the cells it covers. Press an entry to pick it and then press where it goes, or press Enter to put
the first match near the selection. A widget
dropped where another is never deletes it: the one in the way moves to the free cells nearest, and nothing else moves.
A widget being dragged leaves a translucent copy under the pointer, with a note beside it saying where it would land:
the cell, counted from one, or that it goes into a container or onto a stack.
Drop a widget on the middle of another to stack the two into a **Smart Stack**, and drag it out to make it a widget again;
the corner handle steps its size through S, M and L, or use Ctrl+arrows. Alt+N makes a grid; Ctrl+Shift+arrows stack a
widget onto the one that way, or put it into the container that way.

**Containers.** Shift+N — or "Container" at the top of the palette — puts an empty container on the first span of
6×4, 4×3, 6×1, 4×1, 1×4, 2×2, 2×1, 1×2 or 1×1 cells still free near the selection: a row where it is wider than tall, a
column otherwise. A widget let go anywhere over a container joins it where the pointer is — a slot of a row or a column,
a cell of a grid container, a box centred on the pointer in a free one — and the note beside the pointer says "Into the
container" rather than "Stack". Inside, drag a child to another slot, cell or place (a free child snaps; Alt frees it),
or out of the container to make it a widget of its own at its smallest size. A selected child has one handle: the end
of a row's or a column's child sets its weight, the corner of a grid's or a free one's its span or its box, with the
share it takes shown beside the pointer. Shift+arrows and Ctrl+arrows do the same a step at a time (a row's child grows
only across, a column's only down), Alt+↑ selects the container, and a child's menu has "Customize the container…" and
"Take out of the container". An emptied container stays, saying it is empty, until you remove it. See
[Desktop widgets](../surfaces/widgets.md).

### Top

Bars: Ctrl+Shift+arrows make one on the edge the arrow points at, `s` splits one in half (or just before the selected chip),
Alt+Shift+arrows join it with the bar beside it along its edge, and handles set its thickness, length and offset. Several bars
can share an edge. Drag a chip between zones and between bars — a live line shows where it will land — or off every bar to
take it off the layout, which one undo brings back. Nothing moves between layers: a chip stays a chip. `o` and "Move to <screen>" in a bar's menu send it to another screen. A bar's popover
holds its shape, `reserve`, auto-hide and `above_fullscreen`, with the cost of that last one written beside it. Reservation
re-tiles your windows once, when you let go, not during the drag. Shift+N adds a group of its own on a plate to the
selected bar, in the zone of what is selected there (the start zone otherwise), for chips you drag into it; a zone lays
its chips out itself, so this is a plated group, not a container — with an open panel selected, Shift+N makes a
container on its cells instead. `a` (or `+`) opens the palette of the modules that draw a chip, after **Group on a
plate** (dropped into a bar's zone) and **Container** (for an open panel's cells): drag one onto a bar, where the same line shows the place between two
chips, or onto the cells of an open panel at the size chosen on the entry; press it and then the bar; or press Enter to
put it at the end of the zone of the selected chip. See
[Bars](../surfaces/bars.md).

### Overlay

Stacks: Shift+N makes one, dragging pins it to the ninth of the screen you let go over (or Shift+arrows step its anchor), Alt+Shift+arrows
move it a few pixels off the anchor, Ctrl+arrows change its width. Its popover holds the flow (a column or a row of cards), the routes — which cards it takes by
kind, app and urgency — and the screens it appears on. A card goes to the first stack on its screen with a route that
takes it, and a stack with no routes takes whatever nothing else takes, so a critical stack in the middle and a corner for
the rest are two stacks. `v` shows volume and brightness in a stack, and Shift+O opens the launcher there. `t` (or **Try cards** in a
stack's menu, or the strip's button) tries the routes: it sends a sample notification, a critical one, a toast, a
volume OSD and the launcher in turn, each drawn in the stack its routes send it to and staying as long as the real card would: `[stack] timeout_ms`, and for a critical notification under `[notifications] critical_sticky`, `critical_max_secs`. Samples are previews only: they never reach
the notification daemon or its history, and leaving the mode clears them. Once the overlay has a grid, `a` opens the
palette there too, with "Card stack" after "Container". See
[Toasts](../surfaces/toasts.md), [OSD](../surfaces/osd.md) and [Launcher](../surfaces/launcher.md).

### Lock

The lock layer is edited on a **preview** drawn over your unlocked session. It never takes a real lock, and its prompt is a
picture with no field and a "Preview" badge. It has little of its own: it uses the background tools for regions and
textures and the desktop tools for grids and containers, and its palette offers only modules that are readings. Shift+B adds a region covering the whole screen, behind everything. Containers and the style of an instance (fill, border, shadow) are as free here as elsewhere; panels and actions are refused, and the prompt is always drawn above everything else. The prompt is moved by
dragging it, resized by its corners, has a style popover and cannot be removed. `p` opens a popover for what the lock screen may reveal about
notifications and media; those two choices are `[lock]` config rather than layout, so they are written to `config.toml`
when the popover closes and are **outside the layout's undo history** — Esc puts them back.

The preview runs the lock's own check: where a real lock would fall back to the minimal one, the preview shows that and
says why, and an edit that would cause it is refused with the reason. On a machine that cannot lock, the mode still opens
and says why instead of offering tools. See [Lock screen](../system/lock.md).

## Related

- [Popovers](popovers.md) — what Enter and Customize open, and how a drag or a popover is committed or put back.
- [Layouts](layouts.md) — the file every edit lands in.
- [Bundles](bundles.md) — saving a group as a komponent.
