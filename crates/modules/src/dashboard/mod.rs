//! The dashboard: one panel, four pages.
//!
//! It is a panel like every other one — routed through `module_panel`, presented as a drawer or a float per `[modules.dashboard] open`, opened from a chip, from IPC or from a keybind through the same bookkeeping — so nothing here is a second way to put a surface on screen.
//!
//! Which page is showing lives in a [`Store`] rather than in the panel, for two reasons. Reopening the dashboard should land where it was left, and `hogar-shell dashboard tab weather` has to reach the tab a click would set; a signal owned by the surface could do neither, since the surface is rebuilt on every open and does not exist between them.

mod card;
mod dash;
mod media;
mod performance;
mod weather;

use std::sync::Arc;
use ui::scale::{corner, paint, space};

use platform_wayland::EventSender;
use telar::{
    AlignItems, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    ReactiveList, RectStyle, SizeDimension, StyledContainer, Text, box_item, signal, use_theme,
};

pub use config::DashboardTab;
pub use dash::avatar_path;

use config::Config;
use config::theme::{FontRole, NordTheme};
use ui::icon::icon_view;
use ui::module::{icon_px, module_fg, surface_env};
use util::broadcast::Store;

/// The module id, so the chip, the panel routing and the IPC target cannot spell it three ways.
pub const ID: &str = "dashboard";

const TAB_ICON: f32 = 16.0;

/// The page currently showing. Producerless: the shell owns it, nothing polls, and it survives the panel.
static TAB: Store<DashboardTab> = Store::new(DashboardTab::default);

pub fn tab() -> DashboardTab {
    TAB.get()
}

pub fn set_tab(tab: DashboardTab) {
    TAB.update(|current| *current = tab);
}

pub fn subscribe_tab(tx: EventSender<DashboardTab>) {
    TAB.subscribe(tx);
}

/// The bar chip.
pub fn dashboard_chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = module_fg();
    icon_view(
        || "layout-dashboard".to_string(),
        move || fg.get(),
        icon_px(),
    )
}

/// The panel: a tab strip over the configured pages, and the active one under it.
pub fn dashboard_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = live_config();
    services::locale::attach(config.language());
    let theme = use_theme::<NordTheme>();
    let tabs = config.dashboard.tabs();

    // A page can be dropped from `[dashboard] tabs` while it is the one showing, and a stored page the config no longer offers would leave the strip with nothing highlighted and the panel on a page it never listed.
    let active = signal(match tabs.contains(&TAB.get()) {
        true => TAB.get(),
        false => tabs[0],
    });
    if active.peek() != TAB.get() {
        set_tab(active.peek());
    }
    let sink = active;
    let offered = tabs.clone();
    platform_wayland::watch(subscribe_tab, move |tab| {
        if offered.contains(&tab) {
            sink.set(tab);
        }
    });

    let source = active.read_only();
    let page_config = Arc::clone(&config);
    let body = ReactiveList::new(
        move || vec![source.get()],
        |tab: &DashboardTab| tab.id().to_string(),
        move |tab: DashboardTab| page(tab, &page_config, theme),
        0.0,
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        vec![strip(&tabs, active, theme)?, Box::new(body)],
    )?))
}

fn page(
    tab: DashboardTab,
    config: &Config,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    match tab {
        DashboardTab::Dash => dash::page(config, theme),
        DashboardTab::Media => media::page(config, theme),
        DashboardTab::Performance => performance::page(config, theme),
        DashboardTab::Weather => weather::page(config, theme),
    }
}

/// The config the panel resolves against: the bar's when a chip opened it, the running one when IPC or a keybind did — never the defaults, which would silently ignore everything the user configured.
fn live_config() -> Arc<Config> {
    surface_env()
        .map(|env| env.config)
        .or_else(config::config)
        .unwrap_or_default()
}

fn strip(
    tabs: &[DashboardTab],
    active: telar::RwSignal<DashboardTab>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut pills: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(tabs.len());
    for tab in tabs {
        pills.push(pill(*tab, active, theme)?);
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
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let selected_ink = theme.accent.most_readable(&[theme.text, theme.base]);
    // One handle per closure: a signal is not `Copy`, and each of the three readers below outlives the others.
    let (icon_state, label_state, fill_state) =
        (active.read_only(), active.read_only(), active.read_only());

    let ink = move |current: DashboardTab| {
        if current == tab {
            selected_ink
        } else {
            theme.subtle
        }
    };
    let icon = icon_view(
        move || tab.icon().to_string(),
        move || ink(icon_state.get()),
        TAB_ICON,
    )?;
    let label = Text::new(
        move || tab_label(tab),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Caption, ink(label_state.get()))
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
                let fill = if fill_state.get() == tab {
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
        .on_press(move || set_tab(tab)),
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
    use config::DashboardConfig;

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
        assert!(dashboard_chip().is_ok(), "the bar chip builds");

        let config = Config::default();
        let theme = NordTheme::new();
        for tab in DashboardTab::ALL {
            telar::reset_layout_runtime();
            telar::set_theme(theme);
            assert!(page(tab, &config, theme).is_ok(), "the {tab:?} page builds");
        }

        telar::reset_layout_runtime();
        telar::set_theme(theme);
        assert!(dashboard_panel().is_ok(), "the panel builds around them");
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
        assert!(page(DashboardTab::Weather, &config, theme).is_ok());
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
}
