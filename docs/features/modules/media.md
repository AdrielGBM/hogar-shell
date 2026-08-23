---
id: media
kind: module
title: Media
summary: What is playing, on the bar.
status: stable
compositor: any
config: [media, lyrics]
commands: [media]
deps: []
popout: true
see_also: [mixer, dashboard, volume]
---

# Media

## What it shows

The active player's track, scrolled as a marquee when it does not fit. Which player is "active" is decided in
one order: the player named in `preferred_player` if it is running, else the first one actually *playing*, else
the first that exists. That ordering is what makes the chip feel right when a browser tab and a music player
are both alive.

## Interacting

| Gesture | What happens |
| --- | --- |
| Click | play / pause |
| Scroll | previous / next track |
| Hover | a popout card with the track, the artist and the player |

```sh
hogar-shell media play-pause
hogar-shell media get title
hogar-shell media seek -10
hogar-shell media loop cycle
```

## Configuring

`[media]` — `preferred_player`, `marquee`, `marquee_speed_ms`, `max_chars` (the marquee's window, in
characters, since it steps in characters — the resting label is bounded by the room it has, not by a count),
`scroll`, `seek_seconds`, `visualiser`, plus `[media.aliases]` for renaming a player's bus name to something
readable.

`[lyrics]` — `enabled`, `online`. A `.lrc` next to the track always wins over the network: it is what you
chose to keep, and it needs no connection and no waiting.

## What it needs

Any MPRIS player. There is no dependency row for it because MPRIS is a per-application interface — every
player owns an `org.mpris.MediaPlayer2.<app>` bus name — so what is required is *a player*, not a daemon.
With none running, the chip is hidden.

Cover art is fetched off the UI thread and cached **by URL** rather than by track, because that is what
identifies the image: two tracks from one album share an `artUrl` and should share one download.

## Known limit

Playback **position** is deliberately not published. It advances continuously, so broadcasting it would wake
every subscribed surface many times a second for a value only a progress bar cares about — a consumer that
wants it asks on its own cadence.

## Related

- [Dashboard](dashboard.md) — the media page, with cover art and full controls.
- [mixer](mixer.md) — per-application volume for the same players.
