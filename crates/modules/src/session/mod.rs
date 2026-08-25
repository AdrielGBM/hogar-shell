//! The power chip and the menu it opens.

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, RectStyle,
    SizeDimension, StyledContainer, Text, box_item, signal, use_theme,
};
use ui::scale::{corner, space};

use ui::keynav::{self, Move};

use config::theme::{FontRole, NordTheme};
use services::session::{self, Action};

/// The bar chip: a power symbol that opens the session menu.
pub fn power_chip() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let fg = ui::module::module_fg();
    ui::icon::icon_view(
        || "power".to_string(),
        move || fg.get(),
        ui::module::icon_px(),
    )
}

/// The session menu: one tile per action this machine can actually perform.
///
/// Destructive actions confirm first — the tile arms, and only a second press on the armed tile goes through. A single mis-click on a panel that opens next to the clock should not end the session, and arming in place costs no extra surface and no extra keystroke for someone who meant it.
pub fn session_panel() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let armed = signal(String::new());
    let actions = session::available();

    let title = Text::auto(
        || telar::t!("session.title"),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Title, theme.text)
                .with_weight(700)
        },
    )?;

    // Which tile the keyboard is on. `None` until a key is pressed, so opening the panel with the pointer does not paint a selection ring nobody asked for.
    let selected = signal(None::<usize>);
    let mut tiles: Vec<Box<dyn LayoutItem>> = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        tiles.push(tile(
            *action,
            armed.clone(),
            theme,
            selected.read_only(),
            index,
        )?);
    }

    let grid = Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .justify_content(JustifyContent::CENTER)
            .width(SizeDimension::Percent(1.0)),
        tiles,
    )?;

    let panel = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::lg())
            .width(SizeDimension::Percent(1.0)),
        |_| RectStyle::default(),
        vec![box_item(title), box_item(grid)],
    )?
    .on_key(move |key| {
        let Some(movement) = navigation().interpret(key) else {
            return;
        };
        match movement {
            // Enter runs the selected tile through the same arm-then-confirm path a click takes, so the keyboard cannot end a session in one keystroke where the pointer needs two.
            Move::Activate => {
                let Some(action) = selected.peek().and_then(|at| actions.get(at).copied()) else {
                    return;
                };
                press(action, &armed);
            }
            Move::Cancel => armed.set(String::new()),
            movement => {
                // Arming survives a click elsewhere but not a move of the keyboard cursor: leaving an armed tile behind you is how the wrong Enter ends the session.
                armed.set(String::new());
                let at = keynav::apply(selected.peek().unwrap_or(0), actions.len(), movement);
                selected.set(Some(at));
            }
        }
    });
    Ok(Box::new(panel))
}

/// The session menu's key bindings. A wrapped row of tiles, so it reads the horizontal arrows as well: the tiles sit side by side, and Left/Right is what a hand on that row reaches for first.
fn navigation() -> keynav::KeyNav {
    let config = config::config().map(|c| c.keynav).unwrap_or_default();
    keynav::KeyNav {
        horizontal: true,
        ..keynav::from_config(&config)
    }
}

/// Whether pressing `action` ends work the user might not have saved. Suspend and lock are recoverable in a keystroke; the rest are not, so only the rest arm before they fire.
fn is_destructive(action: Action) -> bool {
    !matches!(action, Action::Lock | Action::Suspend)
}

/// Whether this machine can perform `action` right now. Everything but Lock is logind's answer, already filtered by [`session::available`]; Lock is the shell's own, because a compositor without `ext-session-lock-v1` or a machine with no PAM cannot be unlocked afterwards and so must not be offered.
fn is_offered(action: Action) -> bool {
    action != Action::Lock || services::lock::can_lock().is_ok()
}

fn label_for(action: Action) -> String {
    match action {
        Action::Lock => telar::t!("session.lock"),
        Action::Logout => telar::t!("session.logout"),
        Action::Suspend => telar::t!("session.suspend"),
        Action::Hibernate => telar::t!("session.hibernate"),
        Action::Reboot => telar::t!("session.reboot"),
        Action::Shutdown => telar::t!("session.shutdown"),
    }
}

/// Runs `action`, arming it first when it is destructive and not already armed. The one path both the pointer and the keyboard take, so a tile cannot end the session in fewer presses from one than from the other.
fn press(action: Action, armed: &telar::RwSignal<String>) {
    let id = action.id();
    if !is_destructive(action) || armed.peek() == id {
        // Lock goes to the lock service rather than to logind, so it works on a machine with no system bus — and logind's own `Lock` signal lands in the same place, so the two remain one lock.
        if action == Action::Lock {
            if let Err(reason) = services::lock::can_lock() {
                tracing::warn!("cannot lock: {reason}");
                return;
            }
            services::lock::lock();
        } else {
            session::perform(action);
        }
        surfaces::panel::close_panel("session");
        return;
    }
    // Arming one tile disarms any other, so two half-pressed tiles can never both be live.
    armed.set(id.to_string());
}

fn tile(
    action: Action,
    armed: telar::RwSignal<String>,
    theme: NordTheme,
    selected: telar::ReadSignal<Option<usize>>,
    index: usize,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let id = action.id();
    let selected_fill = selected.clone();
    // One handle per closure: a signal is not `Copy`, and each of the four readers below outlives the others.
    let armed_icon = armed.read_only();
    let armed_caption = armed.read_only();
    let armed_fill = armed.read_only();
    let armed_hover = armed.read_only();
    // Resolved once, at build time: whether this machine can lock is a fact about the compositor and the PAM stack, not something that changes while a menu is on screen.
    let offered = is_offered(action);

    let icon = ui::icon::icon_view(
        move || action.icon().to_string(),
        move || {
            if !offered {
                theme.muted
            } else if armed_icon.get() == id {
                theme.red
            } else {
                theme.text
            }
        },
        24.0,
    )?;

    let caption = Text::auto(
        move || {
            if !offered {
                // Says *why* rather than greying a tile out silently: a Lock that does nothing on press is indistinguishable from a broken shell.
                telar::t!("lock.unsupported")
            } else if armed_caption.get() == id {
                telar::t!("session.confirm")
            } else {
                label_for(action)
            }
        },
        LayoutStyle::new(),
        move || {
            let colour = if offered { theme.text } else { theme.muted };
            theme.text_style(FontRole::Caption, colour)
        },
    )?;

    let rounded = corner::xl();
    let tile = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .gap(space::md())
            .width(88.0)
            .padding_vertical(space::lg()),
        move |_| {
            // Armed wins over selected: a tile one press from ending the session must not be mistaken for one the cursor is merely resting on.
            let fill = if armed_fill.get() == id {
                theme.red
            } else if selected_fill.get() == Some(index) {
                theme.overlay
            } else {
                theme.base
            };
            RectStyle::filled(fill, rounded)
        },
        vec![icon, box_item(caption)],
    )?
    .hover_style(move |_| {
        let fill = if !offered {
            theme.base
        } else if armed_hover.get() == id {
            theme.red
        } else {
            theme.overlay
        };
        RectStyle::filled(fill, rounded)
    });
    // No press handler at all, rather than one that returns early: a tile with nothing behind it should not take the click away from the surface either.
    let tile = if offered {
        tile.on_press(move || press(action, &armed))
    } else {
        tile
    };
    Ok(Box::new(tile))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keyboard must not be a shortcut past the confirmation the pointer has to give.
    #[test]
    fn the_session_menu_reads_the_row_arrows_and_disarms_when_the_cursor_moves() {
        use telar::{Key, NamedKey};

        let nav = navigation();
        assert_eq!(
            nav.interpret(&Key::Named(NamedKey::ArrowRight)),
            Some(Move::Next),
            "the tiles sit in a row, so Right is the next one"
        );
        assert_eq!(
            nav.interpret(&Key::Named(NamedKey::Enter)),
            Some(Move::Activate)
        );
        assert_eq!(
            nav.interpret(&Key::Named(NamedKey::Escape)),
            Some(Move::Cancel)
        );

        let armed = signal(String::new());
        press(Action::Shutdown, &armed);
        assert_eq!(
            armed.peek(),
            Action::Shutdown.id(),
            "the first Enter arms rather than shutting down"
        );
        let unarmed = signal(String::new());
        assert!(!is_destructive(Action::Lock));
        assert!(unarmed.peek().is_empty());
    }

    #[test]
    fn only_unrecoverable_actions_ask_twice() {
        assert!(!is_destructive(Action::Lock), "a lock is undone by typing");
        assert!(
            !is_destructive(Action::Suspend),
            "a suspend is undone by a key"
        );
        for action in [
            Action::Logout,
            Action::Reboot,
            Action::Shutdown,
            Action::Hibernate,
        ] {
            assert!(
                is_destructive(action),
                "'{}' can lose unsaved work, so it must confirm",
                action.id()
            );
        }
    }
}
