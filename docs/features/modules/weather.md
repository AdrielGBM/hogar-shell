---
id: weather-widget
ids: [weather]
kind: module
title: Weather
summary: The sky and the temperature where you are, as a widget; not yet placeable on its own.
status: partial
compositor: any
config: [weather, temperature]
commands: []
deps: []
see_also: [weather, dashboard]
---

# Weather

## What you can do with it today

Nothing places this widget on screen yet: there is no config key that puts it on the desktop or the lock
screen, and the desktop's widget surface draws only the [clock](clock.md) and the
[visualiser](visualiser.md). Placing widgets arrives with the layout model.

The reading it draws is the one on the [dashboard](dashboard.md)'s Weather page, which is built from the same
parts: the place, the sky, the temperature, what it feels like, the humidity and the wind.

## What it shows

A small widget is the place, the condition's glyph, the temperature and a word for the sky. A medium one adds
what it feels like, the humidity and the wind.

It reads the last fetch of the weather service and never fetches on its own. Before the first reading every
value reads as a dash rather than a zero. With `[weather] enabled = false` it says the weather is switched off,
as the dashboard's page does.

## Configuring

`[weather]` chooses the place; `[temperature] unit` the scale, shared with the bar and the dashboard.

## What it needs

Network access, through the weather service.

## Related

- [Weather](../system/weather.md) — the service, its cache and how the place is found.
- [dashboard](dashboard.md) — the Weather page, with the forecast.
