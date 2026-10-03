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

`[theme.export]` — `enabled`, `dir`, `json`, `gtk`, `qt`, `terminal`.

Each format is a switch, so you write only what you use.

## Telling other programs

Once a new palette's files are on disk the shell raises the `colors_changed` event — the point at which a reload
is safe. A [rule](../../guides/scripting.md#rules) on that event runs whatever has to re-read them:

```toml
[[rules]]
id = "reload-gtk"
trigger = { event = "colors_changed" }
run = [
  "shell run gsettings set org.gnome.desktop.interface gtk-theme adw-gtk3-dark",
  "shell run makoctl reload",
]
```

Each line of `run` is a `hogar-shell` command, checked by `config check` without being run; `shell run` hands
the rest of its line to `sh -c`. The same rule can trigger on anything else the shell raises — a lock, a wallpaper,
the battery — or on a time of day, which is why the commands live in a rule rather than in this section.

## What it needs

Nothing.

## Related

- [Dynamic scheme](dynamic-scheme.md) — the usual reason a palette changes often enough to want this.
