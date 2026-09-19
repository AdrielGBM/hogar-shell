//! The machine's readings as widgets: a small one is the number and its meter, a medium one is the card the dashboard or the hover shows, with its chart or its rows.

use telar::{LayoutError, LayoutItem, signal, use_theme};

use config::TemperatureConfig;
use config::theme::NordTheme;
use services::gpu;
use ui::card::{Card, Density};
use ui::glyph;
use ui::host::Host;
use util::reactive::{derive, fixed, fixed_text};

use crate::dashboard::cards;
use crate::popout_cards::{self as popouts, percent, resource_signal};

type Built = Result<Box<dyn LayoutItem>, LayoutError>;

pub fn cpu(host: &Host) -> Built {
    if !host.is_small() {
        return cards::cpu(host).build(Density::Widget);
    }
    let state = resource_signal();
    Card::titled(telar::t!("sysinfo.cpu"))
        .icon(fixed_text("cpu"))
        .figure(derive(state, |r| percent(r.as_ref().map(|r| r.cpu))))
        .meter(
            derive(state, |r| r.as_ref().map_or(0.0, |r| r.cpu / 100.0)),
            fixed(host.accent),
        )
        .build(Density::Widget)
}

pub fn memory(host: &Host) -> Built {
    if !host.is_small() {
        return cards::memory(host).build(Density::Widget);
    }
    let state = resource_signal();
    Card::titled(telar::t!("sysinfo.memory"))
        .icon(fixed_text("memory-stick"))
        .figure(derive(state, |r| {
            percent(r.as_ref().map(|r| r.memory.used_percent()))
        }))
        .meter(
            derive(state, |r| {
                r.as_ref().map_or(0.0, |r| r.memory.used_percent() / 100.0)
            }),
            fixed(host.accent),
        )
        .build(Density::Widget)
}

pub fn gpu(host: &Host) -> Built {
    if !host.is_small() {
        return cards::gpu(host).build(Density::Widget);
    }
    let state = signal(gpu::current().unwrap_or_default());
    let sink = state;
    platform_wayland::watch(gpu::subscribe, move |g| sink.set(g));
    Card::titled(telar::t!("sysinfo.gpu"))
        .icon(fixed_text(glyph::gpu()))
        .figure(derive(state, |g| percent(g.usage)))
        .meter(
            derive(state, |g| g.usage.unwrap_or(0.0) / 100.0),
            fixed(host.accent),
        )
        .build(Density::Widget)
}

/// Tinted as the chip is, so a hot machine reads as hot from across the room.
pub fn temperature(host: &Host) -> Built {
    if !host.is_small() {
        return popouts::temperature(host).build(Density::Widget);
    }
    let theme = use_theme::<NordTheme>();
    let accent = host.accent;
    let settings = host.options::<TemperatureConfig>().clone();
    let (unit, critical) = (settings.unit, settings.critical);
    let state = resource_signal();
    let sensor = settings.sensor.clone();
    let celsius = derive(state, move |r| {
        r.as_ref().and_then(|r| r.temperature_of(&sensor))
    });
    let figure_settings = settings.clone();
    Card::titled(telar::t!("sysinfo.temperature"))
        .icon(fixed_text("thermometer"))
        .figure_tinted(
            derive(celsius, move |c| match c {
                Some(c) => unit.format(c),
                None => telar::t!("sysinfo.no_reading"),
            }),
            derive(celsius, move |c| match c {
                Some(_) => glyph::heat_tint(&figure_settings, c, theme, accent),
                None => theme.text,
            }),
        )
        .meter(
            derive(celsius, move |c| {
                (c.unwrap_or(0.0) / critical.max(1.0)).clamp(0.0, 1.0)
            }),
            derive(celsius, move |c| {
                glyph::heat_tint(&settings, c, theme, accent)
            }),
        )
        .build(Density::Widget)
}

/// Down and up as rows even when small: a rate is too wide a number to stand at display size in a square.
pub fn netspeed(host: &Host) -> Built {
    if host.is_small() {
        popouts::netspeed(host).build(Density::Widget)
    } else {
        cards::netspeed(host).build(Density::Widget)
    }
}
