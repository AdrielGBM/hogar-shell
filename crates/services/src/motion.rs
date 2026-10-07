//! The desktop's reduced-motion setting, read from the desktop portal's `Settings` interface.
//!
//! Optional: with no session bus, no portal, or a portal that names no such setting, the desktop does not prefer reduced motion and `[animation]` alone decides.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use platform_wayland::EventSender;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::OwnedValue;

use util::broadcast::{Broadcast, Service};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";

const APPEARANCE: &str = "org.freedesktop.appearance";
const REDUCED_MOTION: &str = "reduced-motion";
const GNOME: &str = "org.gnome.desktop.interface";
const ENABLE_ANIMATIONS: &str = "enable-animations";
const KDE: &str = "org.kde.kdeglobals.KDE";
const ANIMATION_DURATION_FACTOR: &str = "AnimationDurationFactor";

const METHOD_TIMEOUT: Duration = Duration::from_secs(2);

type Namespaces = HashMap<String, HashMap<String, OwnedValue>>;

static REDUCED: Service<bool> = Service::new("hogar-shell-motion", run);

fn run(out: &Arc<Broadcast<bool>>) {
    let signals = changes();
    let mut last = read();
    out.publish(last);
    let Some(signals) = signals else {
        return;
    };
    for message in signals {
        let Ok(message) = message else { continue };
        let Ok((namespace, key, _)) = message.body().deserialize::<(String, String, OwnedValue)>()
        else {
            continue;
        };
        if !is_watched(&namespace, &key) {
            continue;
        }
        let now = read();
        if now != last {
            last = now;
            out.publish(now);
        }
    }
}

fn changes() -> Option<MessageIterator> {
    let conn = crate::bus::private_session(None)?;
    let rule = zbus::MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(PORTAL)
        .ok()?
        .interface(SETTINGS)
        .ok()?
        .member("SettingChanged")
        .ok()?
        .path(PORTAL_PATH)
        .ok()?
        .build();
    MessageIterator::for_match_rule(rule, &conn, None).ok()
}

fn read() -> bool {
    crate::bus::session(Some(METHOD_TIMEOUT))
        .and_then(|conn| read_all(&conn))
        .is_some_and(|settings| prefers_reduced(&settings))
}

fn read_all(conn: &Connection) -> Option<Namespaces> {
    let reply = conn
        .call_method(
            Some(PORTAL),
            PORTAL_PATH,
            Some(SETTINGS),
            "ReadAll",
            &(vec![APPEARANCE, GNOME, KDE],),
        )
        .ok()?;
    reply.body().deserialize().ok()
}

fn is_watched(namespace: &str, key: &str) -> bool {
    matches!(
        (namespace, key),
        (APPEARANCE, REDUCED_MOTION)
            | (GNOME, ENABLE_ANIMATIONS)
            | (KDE, ANIMATION_DURATION_FACTOR)
    )
}

/// The portal's standard `reduced-motion` key when the portal has it, else the desktop's own switch exposed through the same interface: GNOME's `enable-animations` or KDE's `AnimationDurationFactor` at zero.
fn prefers_reduced(settings: &Namespaces) -> bool {
    let value = |namespace: &str, key: &str| settings.get(namespace)?.get(key);
    if let Some(reduced) = value(APPEARANCE, REDUCED_MOTION) {
        return u32::try_from(reduced).is_ok_and(|reduced| reduced == 1);
    }
    if let Some(enabled) = value(GNOME, ENABLE_ANIMATIONS) {
        return bool::try_from(enabled).is_ok_and(|enabled| !enabled);
    }
    value(KDE, ANIMATION_DURATION_FACTOR)
        .and_then(factor)
        .is_some_and(|factor| factor <= 0.0)
}

/// KDE's portal hands `kdeglobals` entries over as the text the file holds.
fn factor(value: &OwnedValue) -> Option<f64> {
    f64::try_from(value)
        .ok()
        .or_else(|| <&str>::try_from(value).ok()?.trim().parse().ok())
}

/// The desktop's setting as the shell starts, read on a thread of its own so the driver comes up while the portal answers — or is started to answer, at login.
pub fn read_in_background() -> Option<std::thread::JoinHandle<bool>> {
    std::thread::Builder::new()
        .name("hogar-shell-motion-seed".to_string())
        .spawn(read)
        .ok()
}

pub fn subscribe(tx: EventSender<bool>) {
    REDUCED.subscribe(tx);
}

pub fn on_change(reduced: bool) {
    config::motion::follow_desktop(reduced);
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    fn settings(entries: &[(&str, &str, Value<'_>)]) -> Namespaces {
        let mut settings = Namespaces::new();
        for (namespace, key, value) in entries {
            settings.entry(namespace.to_string()).or_default().insert(
                key.to_string(),
                value.try_to_owned().expect("a plain value"),
            );
        }
        settings
    }

    #[test]
    fn an_absent_portal_leaves_animation_as_configured() {
        assert!(
            !read(),
            "a test process reaches no bus, which is a desktop with no portal"
        );
        assert!(!prefers_reduced(&Namespaces::new()));

        config::motion::set_desktop_reduced(read());
        let animation = config::AnimationConfig::default();
        assert!(!animation.is_reduced());
        assert_eq!(
            animation.panel_tween(),
            telar::motion::tween(Duration::from_millis(180), telar::motion::Easing::EaseOut)
        );
        assert_eq!(
            animation.autohide_tween().duration,
            Duration::from_millis(160)
        );
    }

    #[test]
    fn the_portals_own_key_says_whether_motion_is_reduced() {
        assert!(prefers_reduced(&settings(&[(
            APPEARANCE,
            REDUCED_MOTION,
            Value::U32(1)
        )])));
        assert!(!prefers_reduced(&settings(&[(
            APPEARANCE,
            REDUCED_MOTION,
            Value::U32(0)
        )])));
        assert!(
            !prefers_reduced(&settings(&[(APPEARANCE, REDUCED_MOTION, Value::U32(7))])),
            "an unknown value is no preference"
        );
        assert!(
            !prefers_reduced(&settings(&[
                (APPEARANCE, REDUCED_MOTION, Value::U32(0)),
                (GNOME, ENABLE_ANIMATIONS, Value::Bool(false)),
            ])),
            "the standard key wins over the desktop's own"
        );
    }

    #[test]
    fn without_the_portals_key_the_desktops_own_switch_is_read() {
        assert!(prefers_reduced(&settings(&[(
            GNOME,
            ENABLE_ANIMATIONS,
            Value::Bool(false)
        )])));
        assert!(!prefers_reduced(&settings(&[(
            GNOME,
            ENABLE_ANIMATIONS,
            Value::Bool(true)
        )])));
        assert!(prefers_reduced(&settings(&[(
            KDE,
            ANIMATION_DURATION_FACTOR,
            Value::from("0")
        )])));
        assert!(!prefers_reduced(&settings(&[(
            KDE,
            ANIMATION_DURATION_FACTOR,
            Value::from("0.5")
        )])));
        assert!(prefers_reduced(&settings(&[(
            KDE,
            ANIMATION_DURATION_FACTOR,
            Value::F64(0.0)
        )])));
    }
}
