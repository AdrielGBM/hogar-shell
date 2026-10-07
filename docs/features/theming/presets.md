---
id: presets
kind: theming
title: Theme presets
summary: A whole `[theme]` kept under a name, and put back with one click.
status: stable
compositor: any
config: [theme]
commands: []
deps: []
see_also: [palettes, typography-and-icons]
---

# Theme presets

## What it is

A preset is the `[theme]` of `config.toml` saved as `themes/<name>.toml` beside it:

```
~/.config/hogar-shell/
├── config.toml
└── themes/
    ├── dusk.toml
    └── paper.toml
```

Each file is a `[theme]` table, the same one `config.toml` holds, so a preset can be written by hand or copied
into a config as it is.

## Saving, picking, deleting

- **Theme popover** (edit mode): the Presets rows list every preset; *Use* previews the whole of it on every
  window, its font family included, and the popover's own rows then adjust it further. Done writes it into
  `config.toml`; Esc or Cancel puts back the theme the popover opened with. *Save as preset* keeps what the
  popover shows now, *Delete* removes a preset.
- **Settings → Appearance → Theme presets**: *Apply* writes a preset into `config.toml` at once, *Save the
  theme as a preset* keeps the theme the file says now, *Delete* removes one.

Picking a preset rewrites `[theme]` key by key, so the comments in `config.toml` and every other section stay
as they were. A key the preset does not set is removed from `[theme]`, so the look is the preset's and not a
mix of the two.

## Names

Lowercase ASCII letters, digits, `-` and `_` — the rule bundles follow. A file in `themes/` whose name breaks
it is not listed.

## What a preset leaves out

`[theme.export]`: where the palette is written for other programs is not part of a look, so picking a preset
keeps the export the config already has. App icon shape and icon theme live in `[icons]`, not `[theme]`, and
are not part of a preset either.

A light/dark schedule that switches presets by time of day is not here yet.

## What it needs

Nothing.

## Related

- [Palettes](palettes.md) — what a `[theme]` says.
- [Typography and icons](typography-and-icons.md) — weights and app icons, from the same popover.
