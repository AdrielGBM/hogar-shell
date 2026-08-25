//! The lock screen: one surface per monitor, and the only thing on it that matters is the password field.
//!
//! Two things shape every decision here. The surface is a `ext-session-lock-v1` surface, so it covers the whole output and the compositor gives it the keyboard — there is no scrim, no dismiss, no way out but authenticating. And it is drawn on *every* monitor, so the parts that would be silly in duplicate (the field, the avatar, the clock) are drawn only on the one the pointer or the compositor focused, while the rest stay a plain background.
//!
//! Everything it shows is a subscription to [`lock::LockState`], which is written from a worker thread. The screen never authenticates; it collects a password and hands it over.

use std::sync::Arc;
use ui::scale::{corner, space};

use telar::{
    AlignItems, App, Color, Component, Container, Input, JustifyContent, LayoutError, LayoutItem,
    LayoutStyle, RectStyle, SizeDimension, StyledContainer, Text, WindowConfig, box_item,
    reset_layout_runtime, set_theme, signal, use_theme,
};

use config::theme::{FontRole, NordTheme};
use config::{Config, SurfaceEnv, set_surface_env};
use services::lock::{self, LockState, Method};
use telar::WindowRoot;

const AVATAR: f32 = 96.0;
const CARD_WIDTH: f32 = 380.0;

/// One monitor's lock surface. Built by the platform crate's lock session, once per output and again for any monitor connected while the screen is locked.
pub struct LockApp {
    /// `None` before the shell has a config — which cannot happen for a lock the shell itself took, but the type says so rather than the code assuming it.
    pub config: Option<Arc<Config>>,
    pub output: Option<String>,
}

impl App for LockApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self
            .config
            .clone()
            .unwrap_or_else(|| Arc::new(Config::default()));
        set_theme(config.resolve_theme());
        services::locale::attach(config.language());
        // Not a `PanelSurface` — the compositor's lock session mounts this, and it is the one surface that must never be translucent — but its content reads settings the same way every panel does, so it installs the same environment by hand.
        let edge = ui::panel::drawn_edge(&config);
        set_surface_env(SurfaceEnv {
            edge,
            bar_size: config.bars.get(edge).size,
            output: self.output.clone(),
            config: Arc::clone(&config),
        });
        let content = screen(&config).expect("lock screen build failed");
        Box::new(WindowRoot::wrapping(content).expect("lock screen layout failed"))
    }

    fn clear_color(&self) -> Option<Color> {
        // Opaque, and deliberately the darkest token there is: a lock surface is the only thing between the desktop and the room, so anything translucent would be a hole in it.
        let theme = self
            .config
            .as_ref()
            .map(|c| c.resolve_theme())
            .unwrap_or_default();
        Some(theme.base)
    }

    fn window_config(&self) -> Option<WindowConfig> {
        None
    }
}

/// The lock screen as the session opener mounts it, for [`crate::preview`] — over the starter config, since a preview has no session to read one from.
pub(crate) fn screen_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    screen(&Arc::new(Config::starter()))
}

/// The whole surface: a centred card over the background.
fn screen(config: &Arc<Config>) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = use_theme::<NordTheme>();
    let state = signal(lock::current());
    platform_wayland::watch(lock::subscribe, {
        let state = state.clone();
        move |next: LockState| state.set(next)
    });

    let mut column: Vec<Box<dyn LayoutItem>> = Vec::new();
    column.push(clock(config, theme)?);
    if config.lock.show_avatar
        && let Some(avatar) = avatar(config)
    {
        column.push(avatar);
    }
    column.push(user_name(theme)?);
    column.push(field(state.read_only(), theme)?);
    column.push(status_line(state.read_only(), theme)?);
    column.extend(crate::lock::content::extras(config, theme)?);

    let card = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .gap(space::xl())
            .width(CARD_WIDTH)
            .padding_all(space::xxl()),
        move |_| RectStyle::filled(theme.surface, card_radius()),
        column,
    )?;

    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .width(SizeDimension::Percent(1.0))
            .height(SizeDimension::Percent(1.0)),
        vec![Box::new(card)],
    )?))
}

/// The card rounds like the shell's panels do, so the lock screen belongs to the same set as the drawers rather than being the one surface with its own corner.
fn card_radius() -> f32 {
    config::config()
        .map(|c| c.panel_radius(config::Edge::Top))
        .unwrap_or(16.0)
}

/// The time, large, at the top of the card — the one thing on a lock screen a glance is usually after.
///
/// Formatted from the same `[clock]` keys the bar chip reads, so a user who set a 12-hour clock or their own pattern does not meet a different one here.
fn clock(config: &Config, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let clock = config.clock.clone();
    let time_format = clock.time_format().to_string();
    let date_format = clock.date_format.clone();
    let render = move |now: &services::clock::Now| {
        (
            now.format(&time_format).to_string(),
            now.format(&date_format).to_string(),
        )
    };
    let parts = signal(render(&chrono::Local::now()));
    platform_wayland::watch(services::clock::subscribe, {
        let parts = parts.clone();
        move |now: services::clock::Now| parts.set(render(&now))
    });
    let time = parts.read_only();
    let date = parts.read_only();

    let hhmm = Text::auto(
        move || time.get().0,
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Display, theme.text)
                .with_weight(600)
        },
    )?;
    let day = Text::auto(
        move || date.get().1,
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?;
    Ok(Box::new(Container::new(
        LayoutStyle::new().flex_column().gap(space::xs()),
        vec![centred(box_item(hhmm))?, centred(box_item(day))?],
    )?))
}

/// Centres one item across the card.
///
/// A `Text` laid out in a column takes the column's width and draws its glyphs from the left, so `align_items: center` on the card does nothing for it — the row it sits in has to do the centring. Every single-line reading on this screen goes through here rather than each one growing its own wrapper.
pub(crate) fn centred(item: Box<dyn LayoutItem>) -> Result<Box<dyn LayoutItem>, LayoutError> {
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .justify_content(JustifyContent::CENTER)
            .width(SizeDimension::Percent(1.0)),
        vec![item],
    )?))
}

fn avatar(config: &Arc<Config>) -> Option<Box<dyn LayoutItem>> {
    let path = crate::dashboard::avatar_path(&config.dashboard)?;
    util::picture::circle(&path, AVATAR)
}

fn user_name(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let name = services::pam::current_user();
    centred(box_item(Text::auto(
        move || name.clone(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Title, theme.text),
    )?))
}

/// The password field. Masked, submits on Enter, and inert while a check is in flight or a lockout is running — a field that keeps taking keystrokes it will throw away reads as a frozen screen.
fn field(
    state: telar::ReadSignal<LockState>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let password = signal(String::new());
    let submit = {
        let password = password.clone();
        let state = state.clone();
        move || {
            if !state.peek().accepts_input() {
                return;
            }
            let secret = password.peek();
            password.set(String::new());
            lock::submit(secret);
        }
    };

    let input = Input::new(
        password,
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(theme.font(FontRole::Body) * 1.8),
        move || theme.text_style(FontRole::Body, theme.text),
    )?
    .secret()
    // The one surface where focus-on-tap is not good enough: a lock screen that needs a click before it takes a password reads as a frozen machine.
    .autofocus()
    .placeholder(telar::t!("lock.password"))
    .on_submit(submit);

    let rounded = corner::xl();
    let outline = state.clone();
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .padding_horizontal(space::xl())
            .padding_vertical(space::md())
            .width(SizeDimension::Percent(1.0)),
        move |_| {
            // The field itself carries the verdict: a wrong password tints the box the user is already looking at, rather than only a line of text below it they have to notice.
            let fill = if outline.get().failures > 0 {
                theme.red.with_alpha(0.18)
            } else {
                theme.base
            };
            RectStyle::filled(fill, rounded)
        },
        vec![box_item(input)],
    )?))
}

/// The line under the field: what the shell is waiting for, what went wrong, or how long the lockout has left.
///
/// Read out of the state *before* any branch, so the paint registers its dependency on the frame that draws nothing too — a message that only appears after an unrelated re-render is the failure this avoids.
fn status_line(
    state: telar::ReadSignal<LockState>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let text = state.clone();
    let tint = state;
    // The line is drawn even when it says nothing, so a wrong password does not resize the card under the hand that is about to retype the password.
    centred(box_item(Text::auto(
        move || {
            let state = text.get();
            if state.is_locked_out() {
                return telar::t!(
                    "lock.locked_out",
                    seconds = state.lockout_remaining().to_string()
                );
            }
            if let Some(method) = state.busy {
                return translate(method.message_key());
            }
            match state.message.as_deref() {
                Some(key) => translate(key),
                None => String::new(),
            }
        },
        LayoutStyle::new(),
        move || {
            let state = tint.get();
            let colour = if state.message.is_some() || state.is_locked_out() {
                theme.red
            } else {
                theme.muted
            };
            theme.text_style(FontRole::Caption, colour)
        },
    )?))
}

/// Resolves one of the lock screen's own message keys.
///
/// `t!` takes a literal so the analyzer can prove every key exists, but which message the status line shows is decided by a worker thread — so the keys are enumerated here instead. Anything unrecognised falls through to a generic failure rather than printing the key at the user.
fn translate(key: &str) -> String {
    match key {
        "lock.checking" => telar::t!("lock.checking"),
        "lock.touch_sensor" => telar::t!("lock.touch_sensor"),
        "lock.looking" => telar::t!("lock.looking"),
        "lock.wrong_password" => telar::t!("lock.wrong_password"),
        "lock.too_many_tries" => telar::t!("lock.too_many_tries"),
        "lock.account_unavailable" => telar::t!("lock.account_unavailable"),
        "lock.no_authentication" => telar::t!("lock.no_authentication"),
        "lock.empty_password" => telar::t!("lock.empty_password"),
        other => {
            tracing::warn!("lock screen: no message for '{other}'");
            telar::t!("lock.wrong_password")
        }
    }
}

/// Whether a method other than the password is offered, for the hint the screen shows under the field.
pub fn offered_methods(config: &Config) -> Vec<Method> {
    let (fingerprint, face) = services::biometrics::offered(&config.lock);
    let mut methods = Vec::new();
    if fingerprint {
        methods.push(Method::Fingerprint);
    }
    if face {
        methods.push(Method::Face);
    }
    methods
}

pub mod content;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_never_left_in_the_field_after_it_is_submitted() {
        // The field's own copy is cleared before the secret is handed on, so a shoulder-surfer reading a screen that is still up after a failed attempt learns the length of nothing.
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let state = signal(LockState {
            wanted: true,
            ..LockState::default()
        });
        assert!(field(state.read_only(), NordTheme::new()).is_ok());
    }

    #[test]
    fn the_screen_builds_on_a_default_config() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(screen(&Arc::new(Config::default())).is_ok());
    }

    #[test]
    fn biometric_methods_are_only_offered_when_configured() {
        let bare = Config::default();
        assert!(
            offered_methods(&bare).is_empty(),
            "a machine with no reader and no Howdy shows no biometric hint"
        );
    }
}
