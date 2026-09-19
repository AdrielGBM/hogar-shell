//! The weather as a widget, and the parts of a reading it and the dashboard's Weather page draw alike: the last fetch of the weather service, never a fetch of its own.

use telar::{LayoutError, LayoutItem, RwSignal, signal};

use config::{TemperatureConfig, TemperatureUnit, WeatherConfig};
use services::weather::{self, Weather};
use ui::card::{Card, Density};
use ui::glyph;
use ui::host::Host;
use util::reactive::{Live, derive, derive_pair, fixed, fixed_text};

pub fn widget(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    if !host.options::<WeatherConfig>().enabled {
        return off().build(Density::Widget);
    }
    let state = reading();
    let unit = signal(host.options::<TemperatureConfig>().unit);
    let card = Card::new(place(state))
        .icon(sky(state))
        .icon_tint(fixed(host.accent));
    let card = if host.is_small() {
        card.figure(temperature(state, unit))
            .detail(condition(state))
    } else {
        with_rows(
            card.trailing(temperature(state, unit))
                .detail(condition(state)),
            state,
            unit,
        )
    };
    card.build(Density::Widget)
}

/// What stands in for a reading while `[weather] enabled = false`: nothing fetches, and it says so.
pub(crate) fn off() -> Card {
    Card::bare().detail(fixed_text(telar::t!("dashboard.weather_off")))
}

/// The last fetch, followed while the surface is up; `None` until the service has one.
pub(crate) fn reading() -> RwSignal<Option<Weather>> {
    let state = signal(weather::current());
    let sink = state;
    platform_wayland::watch(weather::subscribe, move |w: Weather| sink.set(Some(w)));
    state
}

pub(crate) fn place(state: RwSignal<Option<Weather>>) -> Live<String> {
    derive(state, |w| {
        w.map(|w| w.place.trim().to_string())
            .filter(|place| !place.is_empty())
            .unwrap_or_else(|| telar::t!("sysinfo.no_reading"))
    })
}

pub(crate) fn sky(state: RwSignal<Option<Weather>>) -> Live<String> {
    derive(state, |w| match w {
        Some(w) => glyph::weather(w.condition(), w.is_day).to_string(),
        None => "cloud".to_string(),
    })
}

pub(crate) fn temperature(
    state: RwSignal<Option<Weather>>,
    unit: RwSignal<TemperatureUnit>,
) -> Live<String> {
    measured(state, unit, |w| w.temperature)
}

pub(crate) fn condition(state: RwSignal<Option<Weather>>) -> Live<String> {
    shown(state, |w| w.condition().label())
}

/// What it feels like, the humidity and the wind, as rows under whatever `card` already shows.
pub(crate) fn with_rows(
    card: Card,
    state: RwSignal<Option<Weather>>,
    unit: RwSignal<TemperatureUnit>,
) -> Card {
    card.row(
        fixed_text(telar::t!("dashboard.feels_like")),
        measured(state, unit, |w| w.feels_like),
    )
    .row(
        fixed_text(telar::t!("dashboard.humidity")),
        shown(state, |w| format!("{}%", w.humidity)),
    )
    .row(
        fixed_text(telar::t!("dashboard.wind")),
        shown(state, |w| format!("{:.0} km/h", w.wind)),
    )
}

fn shown(state: RwSignal<Option<Weather>>, read: fn(&Weather) -> String) -> Live<String> {
    derive(state, move |w| match w {
        Some(w) => read(&w),
        None => telar::t!("sysinfo.no_reading"),
    })
}

fn measured(
    state: RwSignal<Option<Weather>>,
    unit: RwSignal<TemperatureUnit>,
    celsius: fn(&Weather) -> f32,
) -> Live<String> {
    derive_pair(
        state.read_only(),
        unit.read_only(),
        move |w, unit| match w {
            Some(w) => unit.format(celsius(&w)),
            None => telar::t!("sysinfo.no_reading"),
        },
    )
}
