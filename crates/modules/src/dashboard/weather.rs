//! The Weather page. The unit toggle is local to the surface: checking a reading in the other scale is a glance, and writing `[temperature] unit` from it would change what the bar shows because someone looked at a number.

use chrono::NaiveDate;
use telar::{
    AlignItems, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RwSignal, SizeDimension, StyledContainer, Text, TextStyle, box_item, signal,
};
use ui::scale::{paint, space};

use super::{PageCard, cards_page};
use crate::weather;
use config::TemperatureUnit;
use config::theme::{FontRole, NordTheme};
use services::weather::{Day, Weather};
use ui::card::Card;
use ui::glyph;
use ui::host::Host;
use ui::icon::icon_view;
use util::reactive::{Live, derive, fixed_text};

const CONDITION_ICON: f32 = 52.0;
const FORECAST_ICON: f32 = 20.0;

pub fn page(host: &Host, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = host.config();
    if !config.weather.enabled {
        return cards_page(host, vec![PageCard::Own(weather::off())]);
    }

    let state = weather::reading();
    let unit = signal(config.temperature.unit);

    cards_page(
        host,
        vec![
            PageCard::Own(current_card(state, unit, theme)),
            PageCard::Own(forecast_card(
                state,
                unit,
                config.weather.forecast_days(),
                theme,
            )),
        ],
    )
}

fn current_card(
    state: RwSignal<Option<Weather>>,
    unit: RwSignal<TemperatureUnit>,
    theme: NordTheme,
) -> Card {
    weather::with_rows(
        Card::new(weather::place(state))
            .icon(fixed_text("map-pin"))
            .child(move || headline(state, unit, theme)),
        state,
        unit,
    )
}

fn headline(
    state: RwSignal<Option<Weather>>,
    unit: RwSignal<TemperatureUnit>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon_state = weather::sky(state);
    let icon = icon_view(
        move || icon_state.get(),
        move || theme.accent,
        CONDITION_ICON,
    )?;

    let temperature = weather::temperature(state, unit);
    let condition = weather::condition(state);

    let headline = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::xl())
            .width(SizeDimension::Percent(1.0)),
        vec![
            icon,
            Box::new(Container::new(
                LayoutStyle::new()
                    .flex_column()
                    .flex_grow(1.0)
                    .gap(space::xs()),
                vec![
                    unit_toggle(temperature, unit, theme)?,
                    box_item(Text::new(
                        move || condition.get(),
                        LayoutStyle::new(),
                        move || theme.text_style(FontRole::Body, theme.subtle),
                    )?),
                ],
            )?),
        ],
    )?;
    Ok(Box::new(headline))
}

fn unit_toggle(
    reading: Live<String>,
    unit: RwSignal<TemperatureUnit>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = Text::new(
        move || reading.get(),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Display, theme.text)
                .with_font_weight(700)
        },
    )?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new().padding_horizontal(space::sm()),
            paint::xs(Color::TRANSPARENT),
            vec![box_item(text)],
        )?
        .hover_style(paint::xs(theme.overlay))
        .on_press(move || unit.set(other_unit(unit.peek()))),
    ))
}

fn other_unit(unit: TemperatureUnit) -> TemperatureUnit {
    match unit {
        TemperatureUnit::Celsius => TemperatureUnit::Fahrenheit,
        TemperatureUnit::Fahrenheit => TemperatureUnit::Celsius,
    }
}

fn forecast_card(
    state: RwSignal<Option<Weather>>,
    unit: RwSignal<TemperatureUnit>,
    limit: u32,
    theme: NordTheme,
) -> Card {
    let empty = derive(state, |w| {
        if w.is_none_or(|w| w.days.is_empty()) {
            telar::t!("dashboard.no_forecast")
        } else {
            String::new()
        }
    });
    Card::titled(telar::t!("dashboard.forecast"))
        .icon(fixed_text("calendar-days"))
        .child(move || forecast_days(state, unit, limit, theme))
        .detail(empty)
}

fn forecast_days(
    state: RwSignal<Option<Weather>>,
    unit: RwSignal<TemperatureUnit>,
    limit: u32,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let source = state.read_only();
    let unit_source = unit.read_only();
    let days = ReactiveList::new(
        move || {
            let unit = unit_source.get();
            source
                .get()
                .map(|w| w.days)
                .unwrap_or_default()
                .into_iter()
                .take(limit as usize)
                .map(|day| (day, unit))
                .collect()
        },
        |(day, unit): &(Day, TemperatureUnit)| format!("{}|{}", day.date, unit.suffix()),
        move |(day, unit): (Day, TemperatureUnit)| forecast_row(day, unit, theme),
        6.0,
    )?;
    Ok(Box::new(days))
}

/// One day: when, what, how likely to rain, and the range. Kept to a single row so a week reads as a column of comparable lines rather than seven small cards.
fn forecast_row(
    day: Day,
    unit: TemperatureUnit,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let label = weekday_label(&day.date);
    let icon = icon_view(
        {
            let condition = day.condition();
            move || glyph::weather(condition, true).to_string()
        },
        move || theme.subtle,
        FORECAST_ICON,
    )?;
    let rain = if day.precipitation > 0 {
        format!("{}%", day.precipitation)
    } else {
        String::new()
    };
    let range = format!("{} / {}", unit.format(day.high), unit.format(day.low));
    let caption = theme.font(FontRole::Caption);

    let cells: Vec<Box<dyn LayoutItem>> = vec![
        box_item(Text::new(
            move || label.clone(),
            LayoutStyle::new().width(40.0).flex_shrink(0.0),
            move || TextStyle::new(caption, theme.text).with_font_weight(700),
        )?),
        icon,
        box_item(Text::new(
            move || rain.clone(),
            LayoutStyle::new().flex_grow(1.0),
            move || TextStyle::new(caption, theme.info),
        )?),
        box_item(Text::new(
            move || range.clone(),
            LayoutStyle::new().flex_shrink(0.0),
            move || TextStyle::new(caption, theme.subtle),
        )?),
    ];

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        cells,
    )?))
}

/// The API's `YYYY-MM-DD` as a weekday name. An unparseable date falls back to the raw string rather than to a weekday it made up.
fn weekday_label(date: &str) -> String {
    match NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        Ok(date) => super::dash::weekday_label(chrono::Datelike::weekday(&date)),
        Err(_) => date.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forecast_date_reads_as_its_weekday_and_a_broken_one_reads_as_itself() {
        telar::set_locale("en");
        // 2026-07-27 is a Monday.
        assert_eq!(
            weekday_label("2026-07-27"),
            telar::t!("dashboard.weekday.mon")
        );
        assert_eq!(
            weekday_label("not-a-date"),
            "not-a-date",
            "a shape the API changed under us shows through instead of becoming Monday"
        );
    }

    #[test]
    fn the_unit_toggle_returns_to_where_it_started() {
        assert_eq!(
            other_unit(TemperatureUnit::Celsius),
            TemperatureUnit::Fahrenheit
        );
        assert_eq!(
            other_unit(other_unit(TemperatureUnit::Celsius)),
            TemperatureUnit::Celsius
        );
    }
}
