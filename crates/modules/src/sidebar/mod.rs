//! The notification centre: a full-height surface that is the home for what has arrived and what can be switched.
//!
//! The bell drawer is a *glance* — it hangs off its chip, it is as tall as its content, and it closes when you
//! look away. This is the other thing: it takes the whole edge, it scrolls, and it is where a user goes to work
//! through a morning's notifications. It hosts the utilities panel's own toggles rather than a second set of
//! them, which is the whole reason the two were built together: two independent copies of "turn Wi-Fi off" would
//! drift the day one of them gained a toggle.

use std::sync::Arc;

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutScrollArea, LayoutStyle,
    RectStyle, SizeDimension, StyledContainer, SurfaceToken, Text, box_item, use_theme,
};

use config::Config;
use config::theme::{FontRole, NordTheme};
use ui::panel::PanelSurface;
use ui::placement::Placement;
use ui::scale::{corner, space};

pub const ID: &str = "sidebar";

/// Opens the centre, or closes it if it is up. Registered with the shell's surface registry under [`ID`], so a
/// press on the bell, `hogar-shell notifs center` and a keybind all reach the same surface rather than stacking
/// copies of it.
///
/// A standing window, not a glance: opening it takes the screen from whatever drawer was up — including the
/// bell's own, which is the same notifications seen the other way — and nothing takes it away again but the
/// user. Opening a drawer afterwards leaves it exactly where it was.
pub fn toggle() {
    surfaces::shell::toggle_standing_window(ID, open_sidebar);
}

pub fn open() {
    if !surfaces::shell::window_is_open(ID) {
        toggle();
    }
}

pub fn close() {
    surfaces::shell::close(ID);
}

pub fn is_open() -> bool {
    surfaces::shell::window_is_open(ID)
}

fn open_sidebar() -> SurfaceToken {
    let config = config::config().unwrap_or_else(|| Arc::new(Config::default()));
    let output = surfaces::shell::focused_output();
    PanelSurface::new(placement(&config, output), |env| {
        body(&env.config).expect("sidebar build failed")
    })
    .open()
}

/// A dock: spans its edge over the windows, at the shared panel margin off them. The zone a dock takes is
/// zero, not -1 — the compositor has already cleared the bars, and the margin is the only extra distance a
/// panel of any kind puts between itself and them.
fn placement(config: &Config, output: Option<String>) -> Placement {
    let sidebar = &config.sidebar;
    Placement::dock("hogar-shell-sidebar", sidebar.edge, sidebar.thickness())
        .margin(config.panel_margin(sidebar.edge))
        .output(output)
}

fn body(config: &Config) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let radius = config.panel_radius(config.sidebar.edge);

    let mut children: Vec<Box<dyn LayoutItem>> = vec![header(theme)?];
    if config.sidebar.show_toggles {
        children.push(crate::utilities::toggles_grid(theme)?);
    }
    if config.sidebar.show_history {
        children.push(crate::notifications::bell_panel()?);
    }

    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xl())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?;
    // Scrolled, because a morning's notifications are taller than any screen — the one thing the bell drawer,
    // which sizes to its content, cannot do. Kept: this surface is rebuilt by any config edit, and a history
    // that jumped back to the newest card each time would lose whatever the reader had scrolled down to.
    let scroll = LayoutScrollArea::new_kept(
        "sidebar.history",
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0)),
        |_| Ok(Box::new(column) as Box<dyn LayoutItem>),
    )?;
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .padding_all(space::xl())
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0)),
        move |_| RectStyle::filled(surfaces::drawer::panel_fill(), radius),
        vec![Box::new(scroll)],
    )?))
}

/// The title and the way out.
///
/// The close button is not decoration: a surface docked to an edge has no "outside" for a press to land in, and
/// this one takes no keyboard on purpose — a centre held open while the user works must not keep focus away from
/// what they are typing in — so Escape never reaches it either. Without the ✕ the only way to dismiss it is the
/// IPC command that opened it, which is not a way a user has.
fn header(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let title = Text::auto(
        || telar::t!("sidebar.title"),
        LayoutStyle::new().flex_grow(1.0),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?;
    let glyph = ui::icon::icon_view(|| "x".to_string(), move || theme.text, 18.0)?;
    let rounded = corner::md();
    let close_button = StyledContainer::new(
        LayoutStyle::new()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .padding_all(space::md())
            .flex_shrink(0.0),
        move |_| RectStyle::filled(theme.base, rounded),
        vec![glyph],
    )?
    .hover_style(move |_| RectStyle::filled(theme.overlay, rounded))
    // Through the registry rather than `request_close`, so `panel list` and a second `notifs center` agree with
    // what is on screen the moment the button is pressed.
    .on_press(close);

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(title), Box::new(close_button)],
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Edge;
    use platform_wayland::KeyboardInteractivity;

    fn config(edge: Edge) -> Config {
        Config {
            sidebar: config::SidebarConfig {
                edge,
                ..config::SidebarConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn the_centre_docks_full_length_on_every_edge() {
        for edge in Edge::ALL {
            let layer = placement(&config(edge), None).layer_config();
            let (across, along) = if edge.is_vertical() {
                (layer.size.0, layer.size.1)
            } else {
                (layer.size.1, layer.size.0)
            };
            assert_eq!(across, 400, "{edge:?} is as thick as it was configured");
            assert_eq!(along, 0, "{edge:?} spans the whole edge");
            assert_eq!(
                layer.exclusive_zone, 0,
                "the compositor has already cleared the bars; reserving again would double the gap"
            );
        }
    }

    #[test]
    fn a_hand_edited_size_cannot_cover_the_screen_or_vanish() {
        let tiny = config::SidebarConfig {
            size: 10,
            ..config::SidebarConfig::default()
        };
        assert_eq!(tiny.thickness(), 240);
        let huge = config::SidebarConfig { size: 9000, ..tiny };
        assert_eq!(huge.thickness(), 1200);
    }

    /// The regression this exists for: the centre shipped with no way to dismiss it. It is docked to an edge, so
    /// there is no outside to press, and it takes no keyboard, so Escape never arrives — the ✕ is the only way
    /// out a user has, and it must be in the tree.
    #[test]
    fn the_header_carries_the_only_way_out() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(header(NordTheme::new()).is_ok());

        let layer = placement(&config(Edge::Right), None).layer_config();
        assert!(
            matches!(layer.keyboard_interactivity, KeyboardInteractivity::None),
            "a centre held open while the user types must not hold their keyboard — which is exactly why it \
             cannot rely on Escape and needs the button above"
        );
    }

    #[test]
    fn the_body_builds_with_the_toggles_the_history_and_neither() {
        for (toggles, history) in [(true, true), (true, false), (false, false)] {
            telar::reset_layout_runtime();
            telar::set_theme(NordTheme::new());
            let config = Config {
                sidebar: config::SidebarConfig {
                    show_toggles: toggles,
                    show_history: history,
                    ..config::SidebarConfig::default()
                },
                ..Config::default()
            };
            assert!(
                body(&config).is_ok(),
                "toggles={toggles} history={history} builds"
            );
        }
    }
}
