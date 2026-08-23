---
id: settings
kind: module
title: Settings
summary: A full settings application, in a panel.
status: stable
compositor: any
config: []
commands: [panel, config]
deps: []
panel: true
see_also: [configuration, dependencies]
---

# Settings

Not a preferences dialog with the ten keys somebody thought were important — a nav pane, twelve pages and a
search box over **every** key, including the ones no form displays.

## What it shows

| Page | Covers |
| --- | --- |
| Appearance | `[theme]`, `[theme.colors]` |
| Bars | `[bars]`, `[modules.*]`, `[[battery.warn_levels]]` |
| Audio | `[audio]`, `[media]`, `[media.aliases]` |
| Network | `[network]` |
| Bluetooth | `[bluetooth]` |
| Applications | `[launcher]`, `[general.apps]` |
| Notifications | `[notifications]`, `[toasts]` |
| Lock | `[lock]`, `[idle]`, `[[idle.stages]]` |
| Wallpaper | `[wallpaper]`, `[background]` |
| General | `[general]`, language |
| System | paths, capture, recorder |
| About | the build, and the dependency report |

The search box searches keys, not page titles, which is what makes the twelfth page findable.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the settings panel |

```sh
hogar-shell panel toggle settings
```

The panel takes keyboard focus, since it has text fields.

## What it needs

Nothing. The **About** page runs the dependency probe, which is a second or two of process starts and bus round
trips — done on a thread of its own, and re-probed rather than cached, because the gesture that reaches that
page is usually someone who has just installed something.

## Writing back

Every form writes to `config.toml` **non-destructively**: your comments and ordering survive. Only one form
owns any given key, so two pages can never disagree about what is set.

## Related

- [Configuration](../../getting-started/configuration.md) — the files, and how a reload behaves.
- `hogar-shell config schema` — the same key set as text.
