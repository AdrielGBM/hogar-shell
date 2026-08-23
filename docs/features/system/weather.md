---
id: weather
kind: system
title: Weather
summary: Current conditions and a forecast, with no API key.
status: stable
compositor: any
config: [weather, temperature]
commands: [weather]
deps: []
see_also: [dashboard, lock]
---

# Weather

```sh
hogar-shell weather now         # place, temperature, sky
hogar-shell weather forecast    # one line per day
```

## Where it comes from

Open-Meteo, **because it needs no API key**. A shell that asked you to register for one before it could show a
temperature would ship with the feature effectively off.

Two calls, both cached: a geocoding lookup that turns a place name into coordinates once and remembers it, and
the forecast itself.

## Configuring

`[weather]` — `enabled`, `location`, `forecast_days`, `refresh_minutes`, plus `latitude` and `longitude`.

`location` is a place name, geocoded once and remembered. Setting `latitude`/`longitude` skips that step, which
is what to do when the place name is ambiguous or you would rather not send one.

The unit is **not** here. Readings are always fetched in Celsius and km/h, and `[temperature] unit` is the one
place that turns a temperature into text — so every surface shows the same unit without having to know which
one the service happened to be configured in.

## What it needs

Network access. There is no dependency row: nothing is installed, and a machine that is offline gets a card
saying so rather than an empty one.

## Where it shows

The [dashboard](../modules/dashboard.md)'s weather page, and — with `[lock] show_weather` — the lock screen.

## Related

- [Dashboard](../modules/dashboard.md).
