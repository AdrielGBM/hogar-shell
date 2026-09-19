//! The Performance page. Every series is the `History` ring its service already keeps, so a card opens showing the last minute; `[dashboard] resource_update_interval` only throttles how often the page redraws, leaving the bar chips on the same service alone.

use std::time::{Duration, Instant};
use ui::scale::space;

use telar::{
    Container, LayoutError, LayoutItem, LayoutStyle, ReactiveList, RwSignal, SizeDimension, signal,
};

use super::{CHART_HEIGHT, PageCard, cards_page, shared};
use config::theme::NordTheme;
use config::{DashboardConfig, TemperatureConfig, TemperatureUnit};
use services::{battery, gpu, netspeed, resources};
use ui::card::{Card, Parts};
use ui::glyph;
use ui::host::Host;
use ui::widget;
use util::reactive::{derive, fixed, fixed_text};

/// A percentage series has a natural full scale; a byte rate does not, and is scaled to its own peak instead.
const FULL_SCALE: f32 = 100.0;

pub fn page(host: &Host, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let machine = machine(host.options::<DashboardConfig>());
    cards_page(
        host,
        vec![
            PageCard::Module("cpu"),
            PageCard::Module("gpu"),
            PageCard::Module("memory"),
            PageCard::Own(storage_card(machine, theme)),
            PageCard::Module("netspeed"),
            PageCard::Module("battery"),
        ],
    )
}

pub(super) fn machine(config: &DashboardConfig) -> RwSignal<Option<resources::Resources>> {
    shared(|| throttled_resources(config.resource_interval()))
}

/// Accepted at most once per `interval`, dropped here rather than slowing the service, which publishes every second for the bar chips.
fn throttled_resources(interval: Duration) -> RwSignal<Option<resources::Resources>> {
    let state = signal(resources::current());
    let sink = state;
    let mut last = Instant::now() - interval;
    platform_wayland::watch(resources::subscribe, move |r| {
        let now = Instant::now();
        if now.duration_since(last) < interval {
            return;
        }
        last = now;
        sink.set(Some(r));
    });
    state
}

pub(super) fn cpu_card(
    machine: RwSignal<Option<resources::Resources>>,
    temperature: &TemperatureConfig,
    theme: NordTheme,
) -> Card {
    let unit = temperature.unit;
    let sensor = temperature.sensor.clone();
    let chart = derive(machine, |r| {
        r.map(|r| r.cpu_history.values()).unwrap_or_default()
    });
    let detail = derive(machine, move |r| {
        let Some(r) = r else {
            return telar::t!("sysinfo.no_reading");
        };
        let temperature = r
            .temperature_of(&sensor)
            .map(|c| unit.format(c))
            .unwrap_or_else(|| telar::t!("sysinfo.no_reading"));
        let clock = match r.cpu_mhz {
            Some(mhz) if mhz >= 1000.0 => format!("{:.2} GHz", mhz / 1000.0),
            Some(mhz) => format!("{mhz:.0} MHz"),
            None => telar::t!("sysinfo.no_reading"),
        };
        format!(
            "{} · {} · {}",
            cores_label(r.cores.len()),
            clock,
            temperature
        )
    });

    Card::titled(telar::t!("sysinfo.cpu"))
        .icon(fixed_text("cpu"))
        .trailing(derive(machine, |r| percent(r.map(|r| r.cpu))))
        .child(move || widget::sparkline(chart, fixed(FULL_SCALE), theme.accent, CHART_HEIGHT))
        .detail(detail)
}

/// Which of usage, temperature and VRAM a card answers is a property of its driver, so each field says "—" rather than a zero it never measured — the same rule the GPU service itself follows.
pub(super) fn gpu_card(unit: TemperatureUnit, theme: NordTheme) -> Card {
    let state = signal(gpu::current().unwrap_or_default());
    let sink = state;
    platform_wayland::watch(gpu::subscribe, move |g| sink.set(g));

    let chart = derive(state, |g| g.usage_history.values());
    let detail = derive(state, move |g| {
        let name = g.name.trim();
        let name = if name.is_empty() {
            telar::t!("sysinfo.gpu")
        } else {
            name.to_string()
        };
        format!(
            "{name} · {} · {}",
            temperature_label(g.temperature, unit),
            vram_label(&g)
        )
    });

    Card::titled(telar::t!("sysinfo.gpu"))
        .icon(fixed_text(glyph::gpu()))
        .trailing(derive(state, |g| percent(g.usage)))
        .child(move || widget::sparkline(chart, fixed(FULL_SCALE), theme.accent, CHART_HEIGHT))
        .detail(detail)
}

pub(super) fn memory_card(
    machine: RwSignal<Option<resources::Resources>>,
    theme: NordTheme,
) -> Card {
    let chart = derive(machine, |r| {
        r.map(|r| r.memory_history.values()).unwrap_or_default()
    });
    let detail = derive(machine, |r| {
        let Some(r) = r else {
            return telar::t!("sysinfo.no_reading");
        };
        let used = format!(
            "{} / {}",
            resources::format_bytes(r.memory.used),
            resources::format_bytes(r.memory.total)
        );
        if r.memory.swap_total == 0 {
            return used;
        }
        format!(
            "{used} · {} {} / {}",
            telar::t!("popout.swap"),
            resources::format_bytes(r.memory.swap_used),
            resources::format_bytes(r.memory.swap_total)
        )
    });

    Card::titled(telar::t!("sysinfo.memory"))
        .icon(fixed_text("memory-stick"))
        .trailing(derive(machine, |r| {
            percent(r.map(|r| r.memory.used_percent()))
        }))
        .child(move || widget::sparkline(chart, fixed(FULL_SCALE), theme.accent, CHART_HEIGHT))
        .detail(detail)
}

/// Storage has no history ring — a filesystem does not move fast enough for one to say anything — so the card is a meter per mount instead, which is also what answers the question it is opened for.
fn storage_card(machine: RwSignal<Option<resources::Resources>>, theme: NordTheme) -> Card {
    let mounts = derive(machine, |r| r.map(|r| r.disks.clone()).unwrap_or_default());
    let io = derive(machine, |r| match r {
        Some(r) => format!(
            "{} {} · {} {}",
            telar::t!("popout.down"),
            netspeed::format_rate(r.disk_read),
            telar::t!("popout.up"),
            netspeed::format_rate(r.disk_write)
        ),
        None => telar::t!("sysinfo.no_reading"),
    });

    Card::titled(telar::t!("dashboard.storage"))
        .icon(fixed_text("hard-drive"))
        .composed(move |parts| {
            let bars = ReactiveList::new(
                move || mounts.get(),
                |disk: &resources::Disk| disk.mount.to_string_lossy().into_owned(),
                move |disk: resources::Disk| disk_row(disk, parts, theme),
                space::md(),
            )?;
            Ok(Box::new(bars) as Box<dyn LayoutItem>)
        })
        .detail(io)
}

fn disk_row(
    disk: resources::Disk,
    parts: Parts,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fraction = (disk.used_percent() / 100.0).clamp(0.0, 1.0);
    // A filesystem past ninety per cent is the one a user opens this card to find.
    let tint = if fraction >= 0.9 {
        theme.red
    } else if fraction >= 0.75 {
        theme.yellow
    } else {
        theme.accent
    };
    let label = disk.mount.to_string_lossy().into_owned();
    let value = format!(
        "{} / {}",
        resources::format_bytes(disk.used),
        resources::format_bytes(disk.total)
    );
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::sm())
            .width(SizeDimension::Percent(1.0)),
        vec![
            parts.row(fixed_text(label), fixed_text(value))?,
            parts.meter(fixed(fraction), fixed(tint))?,
        ],
    )?))
}

/// Down and up share one chart because they share one scale: a card that drew them separately would show a 50 KB/s upload as tall as a 50 MB/s download, which is the opposite of what a throughput chart is for.
pub(super) fn network_card(theme: NordTheme) -> Card {
    let state = signal(netspeed::current().unwrap_or_default());
    let sink = state;
    platform_wayland::watch(netspeed::subscribe, move |s| sink.set(s));

    let chart = derive(state, |s| s.down_history.values());
    let ceiling = derive(state, |s| s.down_history.peak().max(s.up_history.peak()));
    let detail = derive(state, |s| {
        format!(
            "↓ {} · ↑ {} · {} {} / {}",
            netspeed::format_rate(s.down),
            netspeed::format_rate(s.up),
            telar::t!("popout.total"),
            resources::format_bytes(s.total_down),
            resources::format_bytes(s.total_up)
        )
    });

    Card::titled(telar::t!("dashboard.network"))
        .icon(fixed_text("arrow-down-up"))
        .trailing(derive(state, |s| netspeed::format_rate(s.down)))
        .child(move || widget::sparkline(chart, ceiling, theme.accent, CHART_HEIGHT))
        .detail(detail)
}

/// On a desktop the battery service reports nothing, and the card says so rather than drawing an empty meter that reads as a flat battery.
pub(super) fn battery_card(theme: NordTheme) -> Card {
    let state = signal(battery::details());
    let sink = state;
    platform_wayland::watch(battery::stream_details, move |d| sink.set(Some(d)));

    let fraction = derive(state, |d| d.map(|d| d.level as f32 / 100.0).unwrap_or(0.0));
    let tint = derive(state, move |d| match d {
        Some(d) => glyph::battery_tint(d.level, d.state.is_charging(), theme, theme.accent),
        None => theme.muted,
    });
    let detail = derive(state, |d| match d {
        Some(d) => battery_detail(&d),
        None => telar::t!("battery.none"),
    });

    Card::titled(telar::t!("dashboard.battery"))
        .icon(derive(state, |d| {
            glyph::battery(d.is_some_and(|d| d.state.is_charging())).to_string()
        }))
        .icon_tint(derive(state, move |d| match d {
            Some(d) => glyph::battery_tint(d.level, d.state.is_charging(), theme, theme.subtle),
            None => theme.muted,
        }))
        .trailing(derive(state, |d| match d {
            Some(d) => format!("{}%", d.level),
            None => telar::t!("sysinfo.no_reading"),
        }))
        .meter(fraction, tint)
        .detail(detail)
}

fn battery_detail(details: &battery::BatteryDetails) -> String {
    use battery::ChargeState;
    let status = match details.state {
        ChargeState::Charging => match remaining_label(details.time_to_full) {
            Some(time) => telar::t!("battery.until_full", time = time),
            None => telar::t!("battery.charging"),
        },
        ChargeState::Discharging => match remaining_label(details.time_to_empty) {
            Some(time) => telar::t!("battery.remaining", time = time),
            None => telar::t!("battery.on_battery"),
        },
        ChargeState::Full => telar::t!("battery.full"),
        ChargeState::Empty => telar::t!("battery.empty"),
        ChargeState::Pending => telar::t!("battery.pending"),
        ChargeState::Unknown => telar::t!("battery.unknown"),
    };
    if details.energy_rate > 0.0 {
        format!("{status} · {:.1} W", details.energy_rate)
    } else {
        status
    }
}

fn remaining_label(seconds: i64) -> Option<String> {
    if seconds <= 0 {
        return None;
    }
    let (hours, minutes) = (seconds / 3600, (seconds % 3600) / 60);
    Some(if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    })
}

fn cores_label(count: usize) -> String {
    format!("{count} {}", telar::t!("popout.cores"))
}

fn temperature_label(celsius: Option<f32>, unit: TemperatureUnit) -> String {
    match celsius {
        Some(c) => unit.format(c),
        None => telar::t!("sysinfo.no_reading"),
    }
}

fn vram_label(card: &gpu::Gpu) -> String {
    match (card.vram_used, card.vram_total) {
        (Some(used), Some(total)) if total > 0 => format!(
            "{} / {}",
            resources::format_bytes(used),
            resources::format_bytes(total)
        ),
        _ => telar::t!("sysinfo.no_reading"),
    }
}

fn percent(value: Option<f32>) -> String {
    match value {
        Some(v) => format!("{v:.0}%"),
        None => telar::t!("sysinfo.no_reading"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_a_driver_does_not_publish_reads_as_unknown_not_zero() {
        assert_eq!(percent(None), telar::t!("sysinfo.no_reading"));
        assert_eq!(
            percent(Some(0.0)),
            "0%",
            "a measured zero is still a measurement"
        );
        let blind = gpu::Gpu::default();
        assert_eq!(vram_label(&blind), telar::t!("sysinfo.no_reading"));
        assert_eq!(
            temperature_label(None, TemperatureUnit::Celsius),
            telar::t!("sysinfo.no_reading")
        );
    }

    #[test]
    fn a_battery_with_no_estimate_still_says_what_it_is_doing() {
        let charging = battery::BatteryDetails {
            level: 40,
            state: battery::ChargeState::Charging,
            time_to_full: 0,
            time_to_empty: 0,
            energy_rate: 0.0,
        };
        assert_eq!(battery_detail(&charging), telar::t!("battery.charging"));
        let estimated = battery::BatteryDetails {
            time_to_full: 5_400,
            energy_rate: 21.5,
            ..charging
        };
        let line = battery_detail(&estimated);
        assert!(
            line.contains("1h 30m"),
            "the estimate is spelled out: {line}"
        );
        assert!(line.contains("21.5 W"), "and so is the rate: {line}");
    }
}
