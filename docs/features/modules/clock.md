---
id: clock
kind: module
title: Clock
summary: The time on the bar, and the calendar behind it.
status: stable
compositor: any
config: [clock, dashboard]
commands: [panel]
deps: []
panel: true
see_also: [dashboard, widgets]
---

# Clock

## What it shows

Time, optionally with the date, in whichever format you set. `format` and `date_format` take strftime
patterns; `twelve_hour` and `show_date` are the shortcuts for the common cases.

The panel behind it is a calendar.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | opens the calendar panel |

## Configuring

`[clock]` — `format`, `date_format`, `show_date`, `twelve_hour`.

The calendar's week start comes from `[dashboard] first_day_of_week`, shared with the dashboard rather than
duplicated.

## What it needs

Nothing. It is the only reading in the shell that genuinely has to *tick* rather than wait for an event.

That is also why it is a service rather than a timer per surface: the whole shell ticks **once**, and the bar
chip, the calendar panel and the desktop clock all read one broadcast. The producer sleeps to the next second
boundary, so the displayed second changes when the system second does instead of drifting by however long the
shell took to start.

## Related

- [Desktop widgets](../surfaces/widgets.md) — `[clock.face]` is a second, larger clock a layout places on the desktop
  itself.
- [dashboard](dashboard.md) — where the calendar lives when you want it beside everything else.
