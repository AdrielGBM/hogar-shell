---
id: dashboard
kind: module
title: Dashboard
summary: One panel, four pages: overview, media, performance and weather.
status: stable
compositor: any
config: [dashboard, weather, gpu, temperature]
commands: [dashboard]
deps: [drm, libnvidia-ml]
panel: true
see_also: [sysinfo, media, weather]
---

# Dashboard

A panel like every other one — opened from a chip, from IPC or from a keybind, presented as a drawer or a
float per `[modules.dashboard] open`. What makes it worth its own page is that it is four pages behind one
chip.

## What it shows

| Page | What is on it |
| --- | --- |
| `dash` | the calendar, the profile and the day's overview |
| `media` | what is playing, with cover art and controls |
| `performance` | CPU, memory, storage, temperature and GPU |
| `weather` | current conditions and the forecast |

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the dashboard where you left it |

```sh
hogar-shell dashboard toggle
hogar-shell dashboard open weather     # straight to a page
hogar-shell dashboard tab              # which page is showing
```

Which page is showing survives closing the dashboard, and `dashboard tab weather` reaches the same state a
click would set. That is why it lives in a store rather than in the panel: the surface is rebuilt on every open
and does not exist in between.

## Configuring

`[dashboard]` — `tabs` (which pages exist and in what order), `avatar`, `first_day_of_week`,
`resource_update_interval`, `media_update_interval`.

The pages themselves read their own sections: `[weather]`, `[gpu]`, `[temperature]`.

## What it needs

Nothing for the calendar or the overview.

- **Performance** reads `/proc` and sysfs directly — no `lm-sensors`, nothing to install. GPU utilisation and
  VRAM come from `/sys/class/drm` on AMD and Intel; on NVIDIA they come from NVML, which the shell loads at
  runtime rather than linking. A card that cannot answer a field reports unknown, never zero.
- **Weather** needs network access and no API key — see [Weather](../system/weather.md).
- **Media** needs any MPRIS player.

## Related

- [sysinfo](sysinfo.md) — the same performance readings as individual bar chips.
- [media](media.md), [Weather](../system/weather.md).
