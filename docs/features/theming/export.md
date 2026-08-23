---
id: export
kind: theming
title: Theme export
summary: Writing the palette out so the rest of the desktop matches it.
status: stable
compositor: any
config: [theme]
commands: [scheme]
deps: []
see_also: [palettes, dynamic-scheme]
---

# Theme export

`[theme.export]` writes the current palette to disk whenever it changes, so applications that are not this
shell can follow it.

```sh
hogar-shell scheme export     # write now, ignoring `enabled`
```

## What it writes

| File | For |
| --- | --- |
| `scheme.json` | anything that can read JSON |
| `scheme.css` | GTK |
| `scheme.conf` | Qt / Kvantum |
| `scheme.sh` | shell variables |
| terminal OSC sequences | live terminals |

`[theme.export]` — `enabled`, `dir`, `json`, `gtk`, `qt`, `terminal`, `hooks`.

Each format is a switch, so you write only what you use.

## Hooks

`hooks` is a list of commands run **after** the files are written — the point at which a reload is safe.

```toml
[theme.export]
hooks = ["gsettings set org.gnome.desktop.interface gtk-theme adw-gtk3-dark"]
```

This is one of the shell's two extension surfaces, alongside `[launcher] actions`. Both are scripts rather than
loaded code, which is deliberate: there is no plugin runtime, and the answer to "hogar-shell cannot do X" is meant
to be a command rather than a module.

## Known limit

Hooks run on **one** event: a scheme was written. There is no general event vocabulary — nothing fires on lock,
unlock, wallpaper change or network up.

## What it needs

Nothing.

## Related

- [Dynamic scheme](dynamic-scheme.md) — the usual reason a palette changes often enough to want this.
