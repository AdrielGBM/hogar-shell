//! The dashboard: one panel, four pages. The page showing is per-instance state in an [`InstanceStore`] rather than in the panel, so a reopen or a reload lands where it was left and `hogar-shell dashboard tab` reaches the tab a click sets; the surface does not exist between opens.

mod dash;
mod media;
mod performance;
mod weather;

use ui::scale::{corner, paint, space};

use telar::{
    AlignItems, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RectStyle, SizeDimension, StyledContainer, Text, box_item, signal, use_theme,
};

pub use config::DashboardTab;

use config::DashboardConfig;

use config::theme::{FontRole, NordTheme};
use ui::card::{Card, Density};
use ui::host::{Host, InstanceId, InstanceStore, Representation};
use ui::icon::icon_view;

/// The module id, so the chip, the panel routing and the IPC target cannot spell it three ways.
pub const ID: &str = "dashboard";

const TAB_ICON: f32 = 16.0;

/// Tall enough that a minute of history reads as a shape rather than a jagged line, short enough that six of them stack inside one drawer.
const CHART_HEIGHT: f32 = 40.0;

static TAB: InstanceStore<DashboardTab> = InstanceStore::new(DashboardTab::default);

pub fn tab(instance: &InstanceId) -> DashboardTab {
    TAB.get(instance)
}

pub fn set_tab(instance: &InstanceId, tab: DashboardTab) {
    TAB.set(instance, tab);
}

/// The dashboard's cards one at a time, as the `card` representation of the module each one reads — which is also how its pages place them.
pub mod cards {
    use super::*;
    use config::{ClockConfig, MediaConfig, TemperatureConfig, VisualiserConfig};

    pub fn clock(host: &Host) -> Card {
        dash::clock_card(
            host.options::<ClockConfig>().clone(),
            use_theme::<NordTheme>(),
        )
    }

    pub fn cpu(host: &Host) -> Card {
        performance::cpu_card(
            performance::machine(host.options::<DashboardConfig>()),
            host.options::<TemperatureConfig>(),
            use_theme::<NordTheme>(),
        )
    }

    pub fn gpu(host: &Host) -> Card {
        performance::gpu_card(
            host.options::<TemperatureConfig>().unit,
            use_theme::<NordTheme>(),
        )
    }

    pub fn memory(host: &Host) -> Card {
        performance::memory_card(
            performance::machine(host.options::<DashboardConfig>()),
            use_theme::<NordTheme>(),
        )
    }

    pub fn netspeed(_host: &Host) -> Card {
        performance::network_card(use_theme::<NordTheme>())
    }

    pub fn battery(_host: &Host) -> Card {
        performance::battery_card(use_theme::<NordTheme>())
    }

    pub fn media(host: &Host) -> Card {
        media::now_playing_card(
            host.options::<DashboardConfig>(),
            media::ring_bands(
                host.options::<MediaConfig>(),
                host.options::<VisualiserConfig>(),
            ),
            use_theme::<NordTheme>(),
        )
    }

    pub fn track(host: &Host) -> Card {
        media::track_card(
            media::ring_bands(
                host.options::<MediaConfig>(),
                host.options::<VisualiserConfig>(),
            ),
            use_theme::<NordTheme>(),
        )
    }
}

/// A reading the cards on one page share, started by the page before its cards build so they find it; a card built anywhere else starts its own.
fn shared<T: Clone + 'static>(start: impl FnOnce() -> T) -> T {
    util::state::context::<T>().unwrap_or_else(|| {
        let reading = start();
        util::state::set_context(reading.clone());
        reading
    })
}

pub fn dashboard_chip(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = host.foreground;
    icon_view(
        || "layout-dashboard".to_string(),
        move || fg,
        host.icon_size(),
    )
}

pub fn dashboard_panel(host: &Host) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let tabs = host.options::<DashboardConfig>().tabs();

    // A page can be dropped from `[dashboard] tabs` while it is the one showing, and a stored page the config no longer offers would leave the strip with nothing highlighted and the panel on a page it never listed.
    let instance = host.instance.clone();
    let stored = TAB.get(&instance);
    let active = signal(match tabs.contains(&stored) {
        true => stored,
        false => tabs[0],
    });
    if active.peek() != stored {
        set_tab(&instance, active.peek());
    }
    let sink = active;
    let offered = tabs.clone();
    let followed = instance.clone();
    platform_wayland::watch(
        move |tx| TAB.subscribe(&followed, tx),
        move |tab| {
            if offered.contains(&tab) {
                sink.set(tab);
            }
        },
    );

    let source = active.read_only();
    let page_host = host.clone();
    let body = ReactiveList::new(
        move || vec![source.get()],
        |tab: &DashboardTab| tab.id().to_string(),
        move |tab: DashboardTab| page(tab, &page_host, theme),
        0.0,
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        vec![strip(&tabs, active, theme, &instance)?, Box::new(body)],
    )?))
}

/// One card on a page: a module's `card` representation from the installed table, or one only the page draws.
enum PageCard {
    Module(&'static str),
    Own(Card),
}

fn cards_page(host: &Host, cards: Vec<PageCard>) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let full_width = || {
        LayoutStyle::new()
            .flex_column()
            .width(SizeDimension::Percent(1.0))
    };
    let mut built: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(cards.len());
    for card in cards {
        built.push(match card {
            PageCard::Module(id) => ui::descriptor::place(
                id,
                &host.inner(id, Representation::Card, host.extent),
                full_width(),
            )?,
            PageCard::Own(card) => card.build(Density::Page)?,
        });
    }
    Ok(Box::new(Container::new(
        full_width().gap(space::lg()),
        built,
    )?))
}

fn page(
    tab: DashboardTab,
    host: &Host,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    match tab {
        DashboardTab::Dash => dash::page(host, theme),
        DashboardTab::Media => media::page(host, theme),
        DashboardTab::Performance => performance::page(host, theme),
        DashboardTab::Weather => weather::page(host, theme),
    }
}

fn strip(
    tabs: &[DashboardTab],
    active: telar::RwSignal<DashboardTab>,
    theme: NordTheme,
    instance: &InstanceId,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut pills: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(tabs.len());
    for tab in tabs {
        pills.push(pill(*tab, active, theme, instance.clone())?);
    }
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        pills,
    )?))
}

fn pill(
    tab: DashboardTab,
    active: telar::RwSignal<DashboardTab>,
    theme: NordTheme,
    instance: InstanceId,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let selected_ink = theme.accent.most_readable(&[theme.text, theme.base]);
    let state = active.read_only();

    let ink = move |current: DashboardTab| {
        if current == tab {
            selected_ink
        } else {
            theme.subtle
        }
    };
    let icon = icon_view(
        move || tab.icon().to_string(),
        move || ink(state.get()),
        TAB_ICON,
    )?;
    let label = Text::new(
        move || tab_label(tab),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Caption, ink(state.get()))
                .with_font_weight(700)
        },
    )?;

    let rounded = corner::md();
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .flex_row()
                .flex_grow(1.0)
                .flex_basis(0.0)
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER)
                .gap(space::md())
                .padding_vertical(space::md())
                .padding_horizontal(space::md()),
            move |_r| {
                let fill = if state.get() == tab {
                    theme.accent
                } else {
                    Color::TRANSPARENT
                };
                RectStyle::filled(fill, rounded)
            },
            vec![icon, box_item(label)],
        )?
        .hover_style(paint::md(theme.overlay))
        // Through the store, not the local signal: a click and `hogar-shell dashboard tab …` must land in the same place, and the watch above is what brings the change back to this surface.
        .on_press(move || set_tab(&instance, tab)),
    ))
}

fn tab_label(tab: DashboardTab) -> String {
    match tab {
        DashboardTab::Dash => telar::t!("dashboard.tab.dash"),
        DashboardTab::Media => telar::t!("dashboard.tab.media"),
        DashboardTab::Performance => telar::t!("dashboard.tab.performance"),
        DashboardTab::Weather => telar::t!("dashboard.tab.weather"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Config;
    use std::sync::Arc;

    fn panel_host(config: Config) -> Host {
        ui::preview::host_on(
            Arc::new(config),
            ID,
            Representation::Panel,
            ui::host::Size {
                width: 420.0,
                height: 600.0,
            },
        )
    }

    #[test]
    fn every_tab_has_a_label_a_glyph_and_a_stable_id() {
        telar::set_locale("en");
        for tab in DashboardTab::ALL {
            assert!(!tab_label(tab).is_empty(), "{tab:?} has no label");
            assert!(!tab.icon().is_empty(), "{tab:?} has no glyph");
            assert_eq!(
                DashboardTab::from_id(tab.id()),
                Some(tab),
                "{tab:?} does not round-trip through its config id"
            );
        }
        assert_eq!(DashboardTab::from_id("nope"), None);
    }

    #[test]
    fn an_unknown_or_empty_tab_list_still_leaves_a_page_to_open() {
        let all: Vec<DashboardTab> = DashboardTab::ALL.to_vec();
        let unknown = DashboardConfig {
            tabs: vec!["nope".to_string()],
            ..DashboardConfig::default()
        };
        assert_eq!(unknown.tabs(), all, "a list of nothing valid falls back");
        let empty = DashboardConfig {
            tabs: Vec::new(),
            ..DashboardConfig::default()
        };
        assert_eq!(empty.tabs(), all, "so does an explicitly empty one");
        let partial = DashboardConfig {
            tabs: vec![
                "weather".to_string(),
                "nope".to_string(),
                "dash".to_string(),
            ],
            ..DashboardConfig::default()
        };
        assert_eq!(
            partial.tabs(),
            vec![DashboardTab::Weather, DashboardTab::Dash],
            "the known ids keep the order the user wrote them in"
        );
    }

    #[test]
    fn the_update_intervals_are_clamped_to_something_a_surface_can_survive() {
        let reckless = DashboardConfig {
            media_update_interval: 1,
            resource_update_interval: 1,
            ..DashboardConfig::default()
        };
        assert_eq!(
            reckless.media_interval(),
            std::time::Duration::from_millis(100),
            "a D-Bus round-trip per frame is not a poll interval"
        );
        assert_eq!(
            reckless.resource_interval(),
            std::time::Duration::from_millis(1000),
            "asking faster than the service publishes cannot produce a new reading"
        );
    }

    /// The only kind of test that runs a surface's closures. Every page reads a service and the theme at once, which is the shape that panics on a re-entrant borrow, and none of it fires until something builds.
    #[test]
    fn the_chip_and_every_page_build() {
        telar::set_locale("en");
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(
            dashboard_chip(&ui::preview::bar_chip()).is_ok(),
            "the bar chip builds"
        );

        let host = panel_host(Config::default());
        let theme = NordTheme::new();
        for tab in DashboardTab::ALL {
            telar::reset_layout_runtime();
            telar::set_theme(theme);
            assert!(page(tab, &host, theme).is_ok(), "the {tab:?} page builds");
        }

        telar::reset_layout_runtime();
        telar::set_theme(theme);
        let host = ui::preview::surface_host(
            ID,
            ui::host::Representation::Panel,
            ui::host::Size {
                width: 420.0,
                height: 600.0,
            },
        );
        assert!(
            dashboard_panel(&host).is_ok(),
            "the panel builds around them"
        );
    }

    /// The weather page has a second shape: `[weather] enabled = false` means no service to subscribe to, and the page has to say so rather than subscribe to a producer that was switched off.
    #[test]
    fn the_weather_page_builds_with_the_service_switched_off() {
        telar::set_locale("en");
        telar::reset_layout_runtime();
        let theme = NordTheme::new();
        telar::set_theme(theme);
        let mut config = Config::default();
        config.weather.enabled = false;
        assert!(page(DashboardTab::Weather, &panel_host(config), theme).is_ok());
    }

    #[test]
    fn the_first_day_of_week_accepts_what_a_user_would_write() {
        let with = |value: &str| {
            DashboardConfig {
                first_day_of_week: value.to_string(),
                ..DashboardConfig::default()
            }
            .first_weekday()
        };
        assert_eq!(with("sunday"), chrono::Weekday::Sun);
        assert_eq!(with("Sun"), chrono::Weekday::Sun);
        assert_eq!(with("saturday"), chrono::Weekday::Sat);
        assert_eq!(with("monday"), chrono::Weekday::Mon);
        assert_eq!(with("nonsense"), chrono::Weekday::Mon, "the common default");
    }

    fn with_tabs(ids: &[&str]) -> Config {
        let mut config = Config::default();
        config.dashboard.tabs = ids.iter().map(|id| id.to_string()).collect();
        config
    }

    /// Builds `instance`'s panel on a fresh runtime against `config`, then tears it down as a closing surface would.
    fn build_panel(instance: &str, config: Config) {
        telar::set_locale("en");
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let host = ui::preview::host_on(
            Arc::new(config),
            instance,
            ui::host::Representation::Panel,
            ui::host::Size {
                width: 420.0,
                height: 600.0,
            },
        );
        let scope = telar::owner_scope();
        let owner = scope.id();
        assert!(dashboard_panel(&host).is_ok(), "{instance}'s panel builds");
        drop(scope);
        telar::dispose_owner(owner);
    }

    #[test]
    fn two_dashboards_keep_a_page_each_through_a_rebuild_and_a_reload() {
        const EVERY: [&str; 4] = ["dash", "media", "performance", "weather"];
        let (left, right) = (
            InstanceId::new("dashboard-left"),
            InstanceId::new("dashboard-right"),
        );
        build_panel(left.as_str(), with_tabs(&EVERY));
        build_panel(right.as_str(), with_tabs(&EVERY));
        set_tab(&left, DashboardTab::Weather);
        set_tab(&right, DashboardTab::Media);
        assert_eq!(
            tab(&left),
            DashboardTab::Weather,
            "a click on one moves only that one"
        );
        assert_eq!(tab(&right), DashboardTab::Media);

        build_panel(left.as_str(), with_tabs(&EVERY));
        build_panel(right.as_str(), with_tabs(&EVERY));
        assert_eq!(
            tab(&left),
            DashboardTab::Weather,
            "a rebuild keeps the page"
        );
        assert_eq!(tab(&right), DashboardTab::Media);

        let reloaded = ["weather", "media", "dash"];
        build_panel(left.as_str(), with_tabs(&reloaded));
        build_panel(right.as_str(), with_tabs(&reloaded));
        assert_eq!(tab(&left), DashboardTab::Weather, "a reload keeps the page");
        assert_eq!(tab(&right), DashboardTab::Media);
    }

    #[test]
    fn a_reload_that_drops_a_page_moves_only_the_dashboard_showing_it() {
        let (showing, other) = (
            InstanceId::new("dashboard-showing"),
            InstanceId::new("dashboard-other"),
        );
        set_tab(&showing, DashboardTab::Performance);
        set_tab(&other, DashboardTab::Media);

        build_panel(showing.as_str(), with_tabs(&["media", "dash"]));
        build_panel(other.as_str(), with_tabs(&["media", "dash"]));
        assert_eq!(
            tab(&showing),
            DashboardTab::Media,
            "onto the first page still offered"
        );
        assert_eq!(tab(&other), DashboardTab::Media);

        build_panel(other.as_str(), with_tabs(&["dash", "media"]));
        assert_eq!(
            tab(&other),
            DashboardTab::Media,
            "a page still offered stays"
        );
    }
}
