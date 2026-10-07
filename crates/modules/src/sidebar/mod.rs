//! The notification centre: a full-height transient that is the home for what has arrived and what can be switched.
//!
//! The bell drawer is a *glance* — it hangs off its chip, it is as tall as its content, and it closes when you look away. This is the other thing: it takes the whole edge, it scrolls, and it is where a user goes to work through a morning's notifications. It hosts the utilities panel's own toggles rather than a second set of them, which is the whole reason the two were built together: two independent copies of "turn Wi-Fi off" would drift the day one of them gained a toggle.

use std::rc::Rc;

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutScrollArea, LayoutStyle,
    RectStyle, SizeDimension, StyledContainer, Text, box_item, use_theme,
};

use config::Config;
use config::theme::{FontRole, NordTheme};
use surfaces::transient::{self, Motion, Place, Slot, Spec};
use ui::chrome::Chrome;
use ui::scale::{corner, space};

pub const ID: &str = "sidebar";

/// Opens the centre, or closes it if it is up. Registered with the shell's transient registry under [`ID`], so a press on the bell, `hogar-shell notifs center` and a keybind all reach the same transient rather than stacking copies of it.
///
/// A standing window, not a glance: opening it takes the screen from whatever drawer was up — including the bell's own, which is the same notifications seen the other way — and nothing takes it away again but the user. Opening a drawer afterwards leaves it exactly where it was.
pub fn toggle() {
    transient::toggle(spec());
}

pub fn open() {
    if !is_open() {
        toggle();
    }
}

pub fn close() {
    transient::close(ID);
}

pub fn is_open() -> bool {
    transient::is_open(ID)
}

/// The module whose centre this is: its `center` action opens it, so how deep it is (`sidebar_size`) is one of that module's options, read off its first instance on the screen it opens on (TA-2).
const OWNER: &str = "notifications";

fn spec() -> Spec {
    let output = transient::focused_output();
    let config = config::config_for(output.as_deref());
    let sidebar = config.sidebar.clone();
    let owner = surfaces::reconcile::instance_of(OWNER, output.as_deref());
    let thickness = owner
        .options::<config::NotificationsConfig>(&config)
        .sidebar_thickness();
    Spec::new(
        ID,
        Place::Docked {
            edge: sidebar.edge,
            thickness: thickness as f32,
        },
        Rc::new(|chrome: &Chrome| body(&chrome.config)),
    )
    .slot(Slot::Standing)
    .output(output)
    .motion(Motion::Slide(sidebar.edge))
}

fn body(config: &Config) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let radius = ui::chrome::content_radius();

    let mut children: Vec<Box<dyn LayoutItem>> = vec![header(theme)?];
    if config.sidebar.show_toggles {
        children.push(crate::utilities::toggles_grid(&config.utilities, theme)?);
    }
    if config.sidebar.show_history {
        children.push(crate::notifications::bell_view(
            &config.notifications,
            &config.stack,
        )?);
    }

    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::xl())
            .width(SizeDimension::Percent(1.0)),
        children,
    )?;
    // Scrolled, because a morning's notifications are taller than any screen — the one thing the bell drawer, which sizes to its content, cannot do. Kept: this transient is rebuilt by any config edit, and a history that jumped back to the newest card each time would lose whatever the reader had scrolled down to.
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
        move |_| RectStyle::filled(ui::chrome::panel_fill(), radius),
        vec![Box::new(scroll)],
    )?))
}

/// The title and the way out.
///
/// The close button is not decoration: a transient docked to an edge has no "outside" for a press to land in, and this one takes no keyboard on purpose — a centre held open while the user works must not keep focus away from what they are typing in — so Escape never reaches it either. Without the ✕ the only way to dismiss it is the IPC command that opened it, which is not a way a user has.
fn header(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let title = Text::declaring(
        || telar::t!("sidebar.title"),
        LayoutStyle::new().flex_grow(1.0),
        move |inherited| {
            theme
                .text_over(inherited, FontRole::Title, theme.text)
                .with_font_weight(700)
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
    // Through the registry rather than closing the node directly, so `panel list` and a second `notifs center` agree with what is on screen the moment the button is pressed.
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
    #[test]
    fn a_hand_edited_size_cannot_cover_the_screen_or_vanish() {
        let tiny = config::NotificationsConfig {
            sidebar_size: 10,
            ..config::NotificationsConfig::default()
        };
        assert_eq!(tiny.sidebar_thickness(), 240);
        let huge = config::NotificationsConfig {
            sidebar_size: 9000,
            ..tiny
        };
        assert_eq!(huge.sidebar_thickness(), 1200);
    }

    /// The regression this exists for: the centre shipped with no way to dismiss it. It is docked to an edge, so there is no outside to press, and it takes no keyboard, so Escape never arrives — the ✕ is the only way out a user has, and it must be in the tree.
    #[test]
    fn the_header_carries_the_only_way_out() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(header(NordTheme::new()).is_ok());

        assert!(
            matches!(spec().keyboard, platform_wayland::KeyboardMode::None),
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
