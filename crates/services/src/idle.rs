//! Idle timeouts and what holds them off.
//!
//! Every stage in `[idle]` becomes one `ext-idle-notify-v1` notification, and the compositor — the only thing
//! that sees the input devices — says when it elapses. What the stage then *does* is a request line the shell
//! already answers, so `hogar-shell --list` is the whole vocabulary and anything bindable to a key is bindable
//! to a timeout.
//!
//! An inhibit is expressed by having no notification at all rather than by ignoring one that fires. A stage
//! that is inhibited is not armed, so the compositor never counts down for it, and un-inhibiting re-arms from
//! zero — which is what a user expects after closing the film that was holding the screen awake, rather than
//! the lock screen appearing the moment it ends.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use platform_wayland::IdleHandle;

use config::IdleConfig;

thread_local! {
    // Armed on the driver thread, which is the only one that may create a Wayland object. Dropping a handle
    // destroys its notification, so replacing this vector is how every stage is disarmed at once.
    static ARMED: RefCell<Vec<IdleHandle>> = const { RefCell::new(Vec::new()) };
    // Whether the inhibit sources have been subscribed. App-level watches live for the process, so this is a
    // one-way latch rather than something reconcile toggles.
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    // Whether the last reconcile left the stages armed, so a log line is emitted on the change, not per tick.
    static ARMED_NOW: Cell<bool> = const { Cell::new(false) };
}

/// Why the timers are currently held off, if they are. Named rather than boolean because it is what
/// `hogar-shell idle status` prints and what a quick toggle shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inhibit {
    /// The user's own toggle.
    Manual,
    /// Something is playing.
    Audio,
    /// The machine is on mains power.
    Charging,
}

impl Inhibit {
    pub fn id(self) -> &'static str {
        match self {
            Inhibit::Manual => "manual",
            Inhibit::Audio => "audio",
            Inhibit::Charging => "charging",
        }
    }
}

/// What is holding the timers off right now, in the order a user would think of them.
pub fn inhibited_by(config: &IdleConfig) -> Option<Inhibit> {
    if crate::state::get().idle_inhibit {
        return Some(Inhibit::Manual);
    }
    if config.inhibit_when_audio && audio_is_playing() {
        return Some(Inhibit::Audio);
    }
    if config.inhibit_when_charging && is_charging() {
        return Some(Inhibit::Charging);
    }
    None
}

/// Whether anything is playing through an output. Read off the PipeWire graph the shell already keeps rather
/// than by asking a player: a browser tab playing video has no MPRIS entry, and it is exactly the case a user
/// means by "don't lock while something is playing".
fn audio_is_playing() -> bool {
    use crate::pipewire::{self, NodeKind};
    pipewire::current().is_some_and(|graph| {
        graph
            .of_kind(NodeKind::OutputStream)
            .any(|node| !node.muted && node.level > 0)
    })
}

/// Whether the machine is on mains power.
///
/// A machine with no battery reads as *not* charging, which is the opposite of the literal answer and the
/// right one here: a desktop is permanently plugged in, so taking that literally would make `[idle]` inert on
/// every desktop that ever copied this line from a laptop's config, with nothing on screen to say why.
fn is_charging() -> bool {
    crate::battery::read().is_some_and(|battery| battery.charging)
}

/// The user's own idle inhibit, persisted so it survives a reload — a toggle that quietly turned itself off
/// when the config changed would be worse than no toggle.
pub fn set_manual_inhibit(inhibit: bool) {
    crate::state::update(|state| state.idle_inhibit = inhibit);
    reconcile();
}

pub fn toggle_manual_inhibit() {
    set_manual_inhibit(!crate::state::get().idle_inhibit);
}

pub fn manual_inhibit() -> bool {
    crate::state::get().idle_inhibit
}

/// Disarms every stage and re-arms the ones that should be running. The single path in: a config reload, a
/// toggle, a charger plugged in and a track starting all end up here.
pub fn reconcile() {
    let config = config::config().map(|c| c.idle.clone()).unwrap_or_default();
    // Dropped first, and unconditionally: re-arming from scratch is what makes un-inhibiting restart the
    // countdown rather than resume it mid-way.
    ARMED.with(|armed| armed.borrow_mut().clear());

    if !config.enabled || config.stages.is_empty() {
        note_armed(false);
        return;
    }
    if !platform_wayland::idle_supported() {
        tracing::warn!(
            "this compositor does not implement ext-idle-notify-v1; [idle] does nothing"
        );
        note_armed(false);
        return;
    }
    ensure_inhibit_watches(&config);
    if let Some(reason) = inhibited_by(&config) {
        tracing::debug!("idle timers held off by {}", reason.id());
        note_armed(false);
        return;
    }

    let handles: Vec<IdleHandle> = config
        .stages
        .iter()
        .filter_map(|stage| arm(stage, config.respect_inhibitors))
        .collect();
    note_armed(!handles.is_empty());
    ARMED.with(|armed| *armed.borrow_mut() = handles);
}

fn note_armed(armed: bool) {
    if ARMED_NOW.with(|now| now.replace(armed)) != armed {
        if armed {
            tracing::info!("idle timers armed");
        } else {
            tracing::info!("idle timers disarmed");
        }
    }
}

/// Arms one stage. The `fired` flag is what keeps a return action honest: the compositor sends `resumed` on
/// every wake, including ones where this stage never elapsed, and running `shell dpms on` for a screen that
/// was never turned off is a request the compositor has to service on every keystroke.
fn arm(stage: &config::IdleStage, respect_inhibitors: bool) -> Option<IdleHandle> {
    let action = stage.action.trim().to_string();
    let return_action = stage.return_action.trim().to_string();
    if action.is_empty() {
        return None;
    }
    if !crate::command::resolves(&action) {
        tracing::warn!(
            "[[idle.stages]] action '{action}' is not a command this shell answers; see `hogar-shell --list`"
        );
        return None;
    }
    if !return_action.is_empty() && !crate::command::resolves(&return_action) {
        tracing::warn!(
            "[[idle.stages]] return_action '{return_action}' is not a command this shell answers"
        );
    }
    let fired = Rc::new(Cell::new(false));
    let on_idle = {
        let fired = Rc::clone(&fired);
        let action = action.clone();
        move || {
            fired.set(true);
            run(&action);
        }
    };
    let on_resume = move || {
        if !fired.replace(false) || return_action.is_empty() {
            return;
        }
        run(&return_action);
    };
    platform_wayland::idle_notification(stage.duration(), respect_inhibitors, on_idle, on_resume)
}

/// Runs a stage's action through the shell's own command surface, so an idle timeout and a keybind reach the
/// same code. Failures are logged rather than returned: there is nobody to report them to at 3 a.m.
fn run(line: &str) {
    let reply = crate::command::run(line);
    if let Some(error) = reply.strip_prefix("err ") {
        tracing::warn!("idle action '{line}': {error}");
    } else {
        tracing::info!("idle action '{line}'");
    }
}

/// Subscribes the sources an inhibit depends on, once. They are app-level watches, which live for the process,
/// so this latches on rather than following the config both ways — and it is only reached when `[idle]` is on,
/// so a shell with idle switched off never starts the PipeWire monitor for it.
fn ensure_inhibit_watches(config: &IdleConfig) {
    if WATCHING.with(|watching| watching.replace(true)) {
        return;
    }
    platform_wayland::watch(
        crate::state::subscribe,
        |_state: crate::state::ShellState| reconcile(),
    );
    if config.inhibit_when_audio {
        platform_wayland::watch(
            crate::pipewire::subscribe,
            |_graph: crate::pipewire::Graph| reconcile(),
        );
    }
    if config.inhibit_when_charging {
        platform_wayland::watch(
            crate::battery::subscribe,
            |_battery: crate::battery::Battery| reconcile(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::IdleStage;

    #[test]
    fn idle_is_off_until_asked_for() {
        let config = IdleConfig::default();
        assert!(
            !config.enabled,
            "a shell that locks a machine nobody told it to lock is a bug"
        );
        assert!(
            config.respect_inhibitors,
            "another app's inhibitor is honoured by default"
        );
    }

    #[test]
    fn a_stage_never_counts_down_for_less_than_a_second() {
        // `timeout = 0` would be a notification the compositor fires immediately and forever.
        let instant = IdleStage {
            timeout: 0,
            ..IdleStage::default()
        };
        assert_eq!(instant.duration().as_secs(), 1);
        assert_eq!(
            IdleStage {
                timeout: 90,
                ..IdleStage::default()
            }
            .duration()
            .as_secs(),
            90
        );
    }

    #[test]
    fn every_inhibit_reason_has_a_stable_id() {
        let ids: Vec<&str> = [Inhibit::Manual, Inhibit::Audio, Inhibit::Charging]
            .iter()
            .map(|reason| reason.id())
            .collect();
        assert_eq!(ids, vec!["manual", "audio", "charging"]);
    }
}
