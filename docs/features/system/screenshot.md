---
id: screenshot
kind: system
title: Screenshot
summary: Capturing a screen, a monitor or a region, without forking a helper.
status: stable
compositor: any
config: [screenshot, paths]
commands: [screenshot]
deps: [ext-image-copy-capture, wlr-screencopy]
see_also: [recording, clipboard, windowinfo]
---

# Screenshot

The shell speaks the capture protocol itself — there is no `grim` in the loop.

## Taking one

```sh
hogar-shell screenshot screen         # every monitor, composed into one image
hogar-shell screenshot output [name]  # one monitor, focused by default
hogar-shell screenshot region         # pick with the pointer
hogar-shell screenshot cancel         # close the picker without capturing
hogar-shell screenshot last           # where the last one went, or why it failed
```

## Where it goes is config, not a flag

`[screenshot] copy` and `save` decide whether a capture reaches the clipboard, a file, or both — so **one
keybind behaves the way you set it up** rather than needing a different bind per destination.

`[paths] screenshots` is the folder; `file_name` is the pattern.

## Configuring

`[screenshot]` — `backend`, `copy`, `save`, `file_name`, `freeze`, `include_cursor`, `notify`, `annotator`.

`annotator` is a command a saved capture is handed to, with `{file}` where the path goes (appended when you
leave the placeholder out). That is the hook for an editor.

`freeze` holds the screen still while you drag out a region, so a video behind the picker does not move under
your selection.

## What it needs

A capture protocol, and there are two:

- **`ext-image-copy-capture`** — tried first.
- **`wlr-screencopy`** — the fallback, and the only route that crops compositor-side.

`[screenshot] backend` names either explicitly, which is how you debug one without silently being given the
other. `Auto` is the default and tries them in that order. Without both, capture is unavailable.

## Two things the newer protocol changed

- **It has no region request.** A selection is captured whole and cropped in the shell — against the output's
  *true device-pixel ratio* rather than its announced integer scale, which is the more correct of the two.
- **It reports a transform**, so a rotated screen comes back the right way up. The older route did not.

Both routes hand back tightly-packed RGBA8 in the output's physical pixels, always upright.

## Known limit

`screenshot screen` composes each output at its logical position times its own scale. That is exact when every
screen runs at the same scale, and can overlap by a row when they do not — the same root as the missing
fractional-scale support.

Capture is **output-only**: capturing a single window needs a toplevel handle, which needs
`ext-foreign-toplevel-list-v1`, which is not bound.

## Related

- [Clipboard](clipboard.md) — how `copy` puts the image on the selection.
- [Recording](recording.md).
