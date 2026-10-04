---
id: edit-modes
kind: customization
title: Edit modes
summary: One mode per layer — background, desktop, top, overlay, lock — each with the tools that layer needs, all reachable from the keyboard.
status: stable
compositor: any
config: [keynav, lock]
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
- the mode strip's switcher, or `m` with a mode up, which opens a pie of the other modes: press one, or press the arrow
  that points at it.

**One mode is up at a time**, process-wide: entering a mode leaves the last. It leaves on its own if its screen is
unplugged. To leave on purpose, press **Esc**, press **Done** on the strip, or run `layout edit off`. Esc closes
whatever is open above the mode first — a popover, a menu, an edit you are holding — then clears the selection, and only
then leaves, so a stray Esc never throws away your place.

The strip names the mode and the screen, and says in one line why the last thing you asked for was not done. A thin
accent border runs round the edited screen so it is never unclear which one is being edited. While a mode is up it holds
the keyboard, so modifiers and Esc arrive without a click.

On the background and desktop layers the mode raises that layer's window above your application windows for the
session, so you edit the real items rather than copies of them; on a compositor that cannot move a layer window, the
same areas are drawn inside the mode's own window instead. Leaving puts every window back exactly as it was.

**What refuses a mode.** Under [`--safe-layout`](layouts.md#when-a-layout-goes-wrong) every mode is refused, like every
edit. `lock` is refused while the session is locked.

## What every mode shares

- **Select, then act.** Click an item, area or group to select it; the tools of the mode act on the selection.
- **Drag with the pointer** — and everything a drag does has a key too, so nothing needs a pointer:

  | Keys | Does |
  | --- | --- |
  | Arrows (and `h` `j` `k` `l` under `[keynav] vim`) | select the nearest thing of the same depth that way |
  | Home / End | select the first or the last |
  | Alt+Down / Alt+Up | select what is inside / what holds it |
  | Tab / Shift+Tab | next or previous area |
  | Enter | [customize](popovers.md) the selection |
  | Menu key, or Shift+F10 | open its context menu |
  | Shift+arrows | move it one slot, cell or edge over |
  | Ctrl+arrows | make it one step bigger or smaller |
  | Delete or Backspace | remove it |
  | Ctrl+Z · Ctrl+Shift+Z or Ctrl+Y | undo · redo |
  | `m` | the mode pie |
  | `w` | edit this workspace alone, or every workspace again (background, desktop and overlay) |
  | `?` or F1 | list the keys of this mode |

  A held key is one undo entry however long the keyboard repeats it, and is previewed until you let go.
- **Context menus.** Right-click an item for its own actions plus Customize, Remove, Reset, the move to an area that draws
  it the other way (chip to widget and back, keeping its id, options and state), and save as a [komponent](bundles.md#komponents).
  Right-click an area for Customize and what its kind adds.
- **Every change is a layout edit.** One gesture, one popover, one key press, one script line is one undo entry; Ctrl+Z
  and `hogar-shell layout undo` take back the same history, whatever made the change. The first edit of the built-in
  layout [forks it](layouts.md#files).
- **Where the edit is written.** Into the narrowest output rule of the layout that already writes the thing, which is where
  the value it replaces came from. Something only a layout it extends writes gets a partial entry in the narrowest rule
  that covers the screen, naming just what changed, so the change lies over the inherited area instead of copying it.
  With `w`, the change goes into that workspace's rule instead, made for it if there is none — and resolves on that
  workspace only. The top layer has no workspace variant: bars reserve space, which a workspace rule may not change.

## Tools by layer

### Background

Wallpaper regions: split a region side by side (`s`) or top and bottom (Shift+S), join it with the region that way
(Alt+arrows), drag the grip on the edge two regions share, or move and resize it with the generic keys — neighbours follow,
so no gap opens. Each region has its own picture, fit and transition; "Follow the background" puts it back on whatever
`[background]` says. Split and join are different hotspots on purpose.

Textures lay an image or a gradient over a region (`t`). An image can be tiled or nine-sliced (`n`), with a handle for
each cut in the image's own pixels; a gradient has its axis drawn over it, a handle that turns it (`[` and `]` turn 45°; Shift snaps a
drag to 45°), and a handle per stop, at most eight. See [Wallpaper](../surfaces/wallpaper.md).

### Desktop

Grids of widgets placed by explicit cells, which are where the grid's rectangle puts them whatever is on it. `a` opens
the **palette** of every module that draws as a widget and the komponents you have saved. A widget
dropped where another is never deletes it: the one in the way moves to the free cells nearest, and nothing else moves.
Drop a widget on the middle of another to stack the two into a **Smart Stack**, and drag it out to make it a widget again;
the corner handle steps its size through S, M and L, or use Ctrl+arrows. Shift+N makes a grid; Ctrl+Shift+arrows stack a
widget onto the one that way. See [Desktop widgets](../surfaces/widgets.md).

### Top

Bars: Ctrl+Shift+arrows make one on the edge the arrow points at, `s` splits one in half (or just before the selected chip),
Alt+Shift+arrows join it with the bar beside it along its edge, and handles set its thickness, length and offset. Several bars
can share an edge. Drag a chip between zones and between bars — a live line shows where it will land — or off every bar to
take it off the layout, which one undo brings back. Nothing moves between layers: a chip stays a chip. `o` and "Move to <screen>" in a bar's menu send it to another screen. A bar's popover
holds its shape, `reserve`, auto-hide and `above_fullscreen`, with the cost of that last one written beside it. Reservation
re-tiles your windows once, when you let go, not during the drag. See [Bars](../surfaces/bars.md).

### Overlay

Stacks: Shift+N makes one, dragging pins it to the ninth of the screen you let go over (or Shift+arrows step its anchor), Alt+arrows
move it a few pixels off the anchor, Ctrl+arrows change its width. Its popover holds the routes — which cards it takes by
kind, app and urgency — and the screens it appears on. A card goes to the first stack on its screen with a route that
takes it, and a stack with no routes takes whatever nothing else takes, so a critical stack in the middle and a corner for
the rest are two stacks. `v` shows volume and brightness in a stack, and Shift+L opens the launcher there. See
[Toasts](../surfaces/toasts.md), [OSD](../surfaces/osd.md) and [Launcher](../surfaces/launcher.md).

### Lock

The lock layer is edited on a **preview** drawn over your unlocked session. It never takes a real lock, and its prompt is a
picture with no field and a "Preview" badge. It has no tools of its own: it uses the background tools for regions and
textures and the desktop tools for the grid, and its palette offers only modules that are readings. The prompt has a move
handle and a style popover and cannot be removed. `p` opens a popover for what the lock screen may reveal about
notifications and media; those two choices are `[lock]` config rather than layout, so they are written to `config.toml`
when the popover closes and are **outside the layout's undo history** — Esc puts them back.

The preview runs the lock's own check: where a real lock would fall back to the minimal one, the preview shows that and
says why, and an edit that would cause it is refused with the reason. On a machine that cannot lock, the mode still opens
and says why instead of offering tools. See [Lock screen](../system/lock.md).

## Related

- [Popovers](popovers.md) — what Enter and Customize open, and how a drag or a popover is committed or put back.
- [Layouts](layouts.md) — the file every edit lands in.
- [Bundles](bundles.md) — saving a group as a komponent.
