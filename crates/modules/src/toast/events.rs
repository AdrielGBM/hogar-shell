//! What the shell says a toast about.
//!
//! Every watcher here follows the same two rules, and both are the difference between feedback and noise.
//!
//! **Only a change is an event.** `watch` hands a subscriber the current reading immediately, so a handler that toasted on delivery would put a card on screen for every service the moment the shell started. Each watcher remembers what it last saw and says nothing about the first one.
//!
//! **A watcher only exists if its toast is switched on.** Subscribing starts the service behind it — a D-Bus connection, or in the lock keys' case a poll — so a user who switched an event off pays nothing for it. This is why the set is decided from the config here rather than filtered at the point the toast is posted, and why switching one off *removes* its watcher: leaving it installed and silent would leave the service it started running for nobody, which is the cost this rule exists to avoid rather than a cosmetic detail.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use platform_wayland::{WatchToken, unwatch, watch};

use config::Config;
use services::toaster::{self, Event};
use ui::glyph;

thread_local! {
    /// Which watchers are up, and the registration that takes each one back. Installing twice would double-toast and start a second subscription, so a reload adds only what is missing and removes only what is no longer asked for. A thread-local because this only ever runs on the driver thread, which is also where [`unwatch`] has to run.
    static INSTALLED: RefCell<HashMap<&'static str, WatchToken>> = RefCell::new(HashMap::new());
}

/// An event's id, whether the config asks for it, and how to install its watcher.
type Wanted = (&'static str, bool, fn() -> Option<WatchToken>);

/// Reconciles the installed watchers with the ones the config asks for. Called from the startup path and again on every reload, so turning an event on gets it without restarting the shell — and turning one off gives back the service behind it.
pub fn watch_events(config: &Config) {
    let events = &config.toasts;
    let wanted: [Wanted; 8] = [
        ("charging", events.events.charging, charging),
        ("game_mode", events.events.game_mode, game_mode),
        ("dnd", events.events.dnd, dnd),
        (
            "audio",
            events.events.audio_output || events.events.audio_input,
            audio,
        ),
        ("lock_keys", events.events.lock_keys, lock_keys),
        ("kb_layout", events.events.kb_layout, keyboard_layout),
        ("vpn", events.events.vpn, vpn),
        ("now_playing", events.events.now_playing, now_playing),
    ];
    for (id, asked_for, install) in wanted {
        let asked_for = asked_for && events.enabled;
        let installed = INSTALLED.with(|installed| installed.borrow().contains_key(id));
        match (asked_for, installed) {
            (true, false) => {
                if let Some(token) = install() {
                    INSTALLED.with(|installed| installed.borrow_mut().insert(id, token));
                }
            }
            (false, true) => {
                if let Some(token) = INSTALLED.with(|installed| installed.borrow_mut().remove(id)) {
                    unwatch(token);
                }
            }
            _ => {}
        }
    }
}

/// A change watcher: remembers the last reading and calls `report` only when the next one differs. The seed delivery is recorded and not reported, which is what keeps startup quiet.
fn on_change<T, S>(subscribe: S, report: impl Fn(&T, &T) + 'static) -> Option<WatchToken>
where
    T: Clone + PartialEq + Send + 'static,
    S: FnOnce(platform_wayland::EventSender<T>) + Send + 'static,
{
    let last: Rc<RefCell<Option<T>>> = Rc::new(RefCell::new(None));
    watch(subscribe, move |current: T| {
        let previous = last.borrow_mut().replace(current.clone());
        if let Some(previous) = previous
            && previous != current
        {
            report(&previous, &current);
        }
    })
}

fn charging() -> Option<WatchToken> {
    use services::battery::{self, Battery};
    on_change(
        battery::subscribe,
        |previous: &Battery, current: &Battery| {
            if previous.charging == current.charging {
                return;
            }
            let title = if current.charging {
                telar::t!("toast.charging")
            } else {
                telar::t!("toast.on_battery")
            };
            toaster::post(
                Event::Charging,
                glyph::battery(current.charging),
                title,
                format!("{}%", current.level),
            );
        },
    )
}

fn game_mode() -> Option<WatchToken> {
    use services::gamemode::{self, GameMode};
    on_change(
        gamemode::subscribe,
        |previous: &GameMode, current: &GameMode| {
            if previous.active == current.active {
                return;
            }
            toaster::post(
                Event::GameMode,
                glyph::game_mode(current.active),
                telar::t!("toast.game_mode"),
                on_off(current.active),
            );
        },
    )
}

fn dnd() -> Option<WatchToken> {
    use services::notifications::{self, SharedSnapshot};
    // Compared on the flag alone: the snapshot changes on every notification, and none of those is a DND event.
    let last: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    watch(notifications::subscribe, move |snapshot: SharedSnapshot| {
        let previous = last.borrow_mut().replace(snapshot.dnd);
        if previous == Some(snapshot.dnd) || previous.is_none() {
            return;
        }
        toaster::post(
            Event::Dnd,
            glyph::dnd(snapshot.dnd),
            telar::t!("toast.dnd"),
            on_off(snapshot.dnd),
        );
    })
}

/// The default output and input *devices* — which is a different question from their level, and the one worth a toast: plugging a headset in changes where sound goes without anything on screen saying so.
///
/// One watcher for both halves, since both come off the same graph reading. Which half is wanted is read live rather than captured, so switching one off in the config takes effect on the next change.
fn audio() -> Option<WatchToken> {
    use services::pipewire::{self, Graph};
    let last: Rc<RefCell<Option<(String, String)>>> = Rc::new(RefCell::new(None));
    watch(pipewire::subscribe, move |graph: Graph| {
        let devices = (
            graph
                .default_sink()
                .map(|node| node.label())
                .unwrap_or_default(),
            graph
                .default_source()
                .map(|node| node.label())
                .unwrap_or_default(),
        );
        let Some(previous) = last.borrow_mut().replace(devices.clone()) else {
            return;
        };
        let (output, input) = config::config()
            .map(|c| (c.toasts.events.audio_output, c.toasts.events.audio_input))
            .unwrap_or((true, true));
        if output && previous.0 != devices.0 && !devices.0.is_empty() {
            toaster::post(
                Event::AudioOutput,
                "volume-2",
                telar::t!("toast.audio_output"),
                devices.0.clone(),
            );
        }
        if input && previous.1 != devices.1 && !devices.1.is_empty() {
            toaster::post(
                Event::AudioInput,
                "mic",
                telar::t!("toast.audio_input"),
                devices.1.clone(),
            );
        }
    })
}

fn lock_keys() -> Option<WatchToken> {
    use services::lockkeys::{self, LockKeys};
    on_change(
        lockkeys::subscribe,
        |previous: &LockKeys, current: &LockKeys| {
            if previous.caps != current.caps {
                toaster::post(
                    Event::LockKeys,
                    glyph::caps_lock(),
                    telar::t!("toast.caps_lock"),
                    on_off(current.caps),
                );
            }
            if previous.num != current.num {
                toaster::post(
                    Event::LockKeys,
                    glyph::num_lock(),
                    telar::t!("toast.num_lock"),
                    on_off(current.num),
                );
            }
        },
    )
}

fn keyboard_layout() -> Option<WatchToken> {
    use services::hyprland::{self, KeyboardLayout};
    on_change(
        hyprland::subscribe_keyboard,
        |_: &KeyboardLayout, current: &KeyboardLayout| {
            toaster::post(
                Event::KbLayout,
                glyph::keyboard_layout(),
                telar::t!("toast.kb_layout"),
                current.name.clone(),
            );
        },
    )
}

fn vpn() -> Option<WatchToken> {
    use services::vpn::{self, Vpn};
    on_change(vpn::subscribe, |previous: &Vpn, current: &Vpn| {
        if previous.is_connected() == current.is_connected() {
            return;
        }
        let body = match current.active() {
            Some(tunnel) => tunnel.name.clone(),
            None => on_off(false),
        };
        toaster::post(
            Event::Vpn,
            glyph::vpn(current.is_connected()),
            telar::t!("toast.vpn"),
            body,
        );
    })
}

fn now_playing() -> Option<WatchToken> {
    use services::mpris::{self, Player};
    on_change(mpris::subscribe, |previous: &Player, current: &Player| {
        // The track, not the position: a player republishes on every progress tick, and none of those is a new song. An empty title is a player that stopped, which the media chip already shows.
        if previous.title == current.title || current.title.trim().is_empty() {
            return;
        }
        let artist = if current.artist.trim().is_empty() {
            current.identity.clone()
        } else {
            current.artist.clone()
        };
        toaster::post(
            Event::NowPlaying,
            glyph::now_playing(),
            current.title.clone(),
            artist,
        );
    })
}

/// The shell's own config reload. Not a service watcher: the reload path is what knows a save was applied, and only it can tell an applied config from the one the shell started with.
pub fn config_reloaded() {
    toaster::post(
        Event::ConfigLoaded,
        "refresh-cw",
        telar::t!("toast.config_loaded"),
        String::new(),
    );
}

fn on_off(value: bool) -> String {
    if value {
        telar::t!("common.on")
    } else {
        telar::t!("common.off")
    }
}
