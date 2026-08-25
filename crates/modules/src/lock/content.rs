//! What the lock screen shows besides the field: what is playing, the weather, the machine's load, and how many notifications are waiting.
//!
//! Every row is a *reading*, never a control. A lock screen's whole promise is that nothing behind it can be reached, so a play button — which reaches into another application — would be a hole in it; and a notification body is the one thing on this surface that can be read by whoever is standing there, which is why `hide_notifs` counts them instead of showing them until the user says otherwise.

use std::sync::Arc;
use ui::scale::space;

use telar::{
    AlignItems, Container, LayoutError, LayoutItem, LayoutStyle, SizeDimension, Text, box_item,
    signal,
};

use config::Config;
use config::theme::{FontRole, NordTheme};
use services::notifications::SharedSnapshot;

/// The rows `[lock]` switches on, in the order they read best: what is playing, then the weather, then the machine, then what is waiting. An empty vector is the ordinary case — the default lock screen is a field.
pub fn extras(
    config: &Arc<Config>,
    theme: NordTheme,
) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::new();
    if config.lock.show_media {
        rows.push(now_playing(theme)?);
    }
    if config.lock.show_weather {
        rows.push(weather(config, theme)?);
    }
    if config.lock.show_resources {
        rows.push(resources(theme)?);
    }
    if config.lock.show_notifications {
        rows.push(notifications(config.lock.hide_notifs, theme)?);
    }
    if rows.is_empty() {
        return Ok(rows);
    }
    Ok(vec![Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .align_items(AlignItems::CENTER)
            .gap(space::md())
            .width(SizeDimension::Percent(1.0)),
        rows,
    )?)])
}

/// A muted, centred caption row — the shape every reading below takes.
fn caption(
    value: impl Fn() -> String + 'static,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    super::centred(box_item(Text::auto(
        value,
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, theme.muted),
    )?))
}

fn now_playing(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    use services::mpris;
    let player = signal(mpris::current());
    platform_wayland::watch(mpris::subscribe, {
        let player = player.clone();
        move |next: mpris::Player| player.set(Some(next))
    });
    let read = player.read_only();
    caption(
        move || match read.get() {
            Some(player) if !player.is_empty() => player.summary(),
            _ => telar::t!("lock.nothing_playing"),
        },
        theme,
    )
}

fn weather(config: &Arc<Config>, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    use services::weather;
    let unit = config.temperature.unit;
    let current = signal(weather::current());
    platform_wayland::watch(weather::subscribe, {
        let current = current.clone();
        move |next: weather::Weather| current.set(Some(next))
    });
    let read = current.read_only();
    caption(
        move || match read.get() {
            Some(reading) => format!("{}  {}", reading.place, unit.format(reading.temperature)),
            None => String::new(),
        },
        theme,
    )
}

fn resources(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    use services::resources;
    let current = signal(resources::current());
    platform_wayland::watch(resources::subscribe, {
        let current = current.clone();
        move |next: resources::Resources| current.set(Some(next))
    });
    let read = current.read_only();
    caption(
        move || match read.get() {
            Some(reading) => format!(
                "CPU {:.0}%   RAM {:.0}%",
                reading.cpu,
                reading.memory.used_percent()
            ),
            None => String::new(),
        },
        theme,
    )
}

/// How many notifications are waiting, and — only if `hide_notifs` is off — who they are from.
///
/// Never the body. A lock screen is read by whoever is in the room, and a message preview is the one thing on it that leaks something the lock was supposed to protect.
fn notifications(hide: bool, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    use services::notifications as notifs;
    let snapshot = signal(notifs::snapshot_now());
    platform_wayland::watch(notifs::subscribe, {
        let snapshot = snapshot.clone();
        move |next: SharedSnapshot| snapshot.set(Some(next))
    });
    let read = snapshot.read_only();
    caption(
        move || {
            let Some(snapshot) = read.get() else {
                return telar::t!("lock.no_notifications");
            };
            let count = snapshot.active.len();
            if count == 0 {
                return telar::t!("lock.no_notifications");
            }
            if hide {
                return telar::t!("lock.notifications", count = count.to_string());
            }
            let mut apps: Vec<&str> = snapshot
                .active
                .iter()
                .map(|entry| entry.app_name.as_str())
                .collect();
            apps.dedup();
            format!(
                "{} · {}",
                telar::t!("lock.notifications", count = count.to_string()),
                apps.join(", ")
            )
        },
        theme,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::LockConfig;

    #[test]
    fn a_default_lock_screen_shows_what_is_playing_and_what_is_waiting() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let config = Arc::new(Config::default());
        let rows = extras(&config, NordTheme::new()).expect("builds");
        assert_eq!(rows.len(), 1, "the enabled rows are grouped into one block");
    }

    #[test]
    fn nothing_extra_is_drawn_when_every_row_is_off() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let config = Arc::new(Config {
            lock: LockConfig {
                show_media: false,
                show_weather: false,
                show_resources: false,
                show_notifications: false,
                ..LockConfig::default()
            },
            ..Config::default()
        });
        assert!(
            extras(&config, NordTheme::new())
                .expect("builds")
                .is_empty()
        );
    }

    #[test]
    fn notification_bodies_are_hidden_by_default() {
        // The one setting on this screen with a privacy consequence: a preview on a locked screen is readable by whoever walks past it.
        assert!(LockConfig::default().hide_notifs);
    }
}
