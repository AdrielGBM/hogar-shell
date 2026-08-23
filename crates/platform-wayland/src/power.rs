//! Blanking and waking screens over `zwlr-output-power-management-v1`.
//!
//! What `hyprctl dpms` does, asked of the compositor directly. The protocol is better than a dispatcher at the
//! one thing that matters here: **it reports the mode back**. A `mode` event arrives when the control object is
//! created and again after every change, whoever made it — so switching a screen off is verified rather than
//! hoped for, and reading the current state needs no separate query.
//!
//! **A short-lived connection, not a watcher.** Destroying a `zwlr_output_power_v1` does not restore anything:
//! the protocol says a mode change is effective immediately and says nothing about the object outliving it,
//! which is the opposite of `zwlr-gamma-control` and the reason this needs no thread. Releasing the objects
//! also releases the exclusivity the compositor grants with them, so a `wlopm` run afterwards is not refused.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};

use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
    zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1},
};

/// The global a compositor advertises when its outputs can be blanked this way.
pub const OUTPUT_POWER_INTERFACE: &str = "zwlr_output_power_manager_v1";

/// How long to wait for every output to report the mode it was asked for. Long enough for a panel to come back
/// from sleep, short enough that a caller waiting on a keybind is not left wondering.
const DEADLINE: Duration = Duration::from_millis(1500);

/// Whether the compositor can blank an output this way, asked over a connection of its own so it answers
/// outside a running shell. `None` means no compositor could be reached.
pub fn output_power_supported() -> Option<bool> {
    crate::globals::advertises(OUTPUT_POWER_INTERFACE)
}

/// Switches every output on or off, returning once the compositor has confirmed it.
///
/// `Err` distinguishes the two failures a caller can act on: a compositor without the protocol, and one that
/// has it but would not do it — an output that cannot be blanked, or one another client holds.
pub fn set_output_power(on: bool) -> Result<(), String> {
    let wanted = if on { Mode::On } else { Mode::Off };
    let mut session = Session::open()?;
    session.request(wanted);
    session.settle(wanted)
}

/// Whether every output is currently on, or `None` where that cannot be asked. An output the compositor
/// refuses to report is left out rather than counted as either.
pub fn output_power_on() -> Option<bool> {
    let mut session = Session::open().ok()?;
    session.roundtrip().ok()?;
    let modes: Vec<Mode> = session.state.modes.values().flatten().copied().collect();
    (!modes.is_empty()).then(|| modes.iter().all(|mode| *mode == Mode::On))
}

#[derive(Default)]
struct State {
    /// The last mode each control reported, by its protocol id. `None` until the compositor has said.
    modes: HashMap<u32, Option<Mode>>,
    /// Controls the compositor disowned — an output that cannot be blanked, or one another client holds.
    failed: Vec<u32>,
}

struct Session {
    connection: Connection,
    queue: wayland_client::EventQueue<State>,
    controls: HashMap<u32, ZwlrOutputPowerV1>,
    state: State,
}

impl Session {
    /// Takes a power control for every output. The compositor reports each one's current mode as soon as it
    /// exists, so the round trip that follows is also the reading.
    fn open() -> Result<Self, String> {
        let connection = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, queue) =
            registry_queue_init::<State>(&connection).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let manager = globals
            .bind::<ZwlrOutputPowerManagerV1, _, _>(&qh, 1..=1, ())
            .map_err(|_| {
                "this compositor does not implement wlr-output-power-management".to_string()
            })?;

        let mut controls = HashMap::new();
        let mut state = State::default();
        globals.contents().with_list(|list| {
            for global in list {
                if global.interface != "wl_output" {
                    continue;
                }
                let output: wl_output::WlOutput =
                    globals
                        .registry()
                        .bind(global.name, global.version.min(4), &qh, ());
                let control = manager.get_output_power(&output, &qh, ());
                state.modes.insert(control.id().protocol_id(), None);
                controls.insert(control.id().protocol_id(), control);
            }
        });
        if controls.is_empty() {
            return Err("the compositor reports no outputs".to_string());
        }
        Ok(Session {
            connection,
            queue,
            controls,
            state,
        })
    }

    fn roundtrip(&mut self) -> Result<(), String> {
        self.queue
            .roundtrip(&mut self.state)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn request(&mut self, wanted: Mode) {
        for control in self.controls.values() {
            control.set_mode(wanted);
        }
        let _ = self.connection.flush();
    }

    /// Waits until every control the compositor still owns reports `wanted`.
    fn settle(&mut self, wanted: Mode) -> Result<(), String> {
        let until = Instant::now() + DEADLINE;
        loop {
            self.roundtrip()?;
            let live: Vec<u32> = self
                .state
                .modes
                .keys()
                .copied()
                .filter(|id| !self.state.failed.contains(id))
                .collect();
            if live.is_empty() {
                return Err(
                    "every output refused: nothing here can be blanked, or another client holds them"
                        .to_string(),
                );
            }
            if live
                .iter()
                .all(|id| self.state.modes.get(id).copied().flatten() == Some(wanted))
            {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err("the compositor did not report the outputs changing".to_string());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        for control in self.controls.values() {
            control.destroy();
        }
        let _ = self.connection.flush();
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ZwlrOutputPowerV1,
        event: zwlr_output_power_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            zwlr_output_power_v1::Event::Mode { mode } => {
                state.modes.insert(id, mode.into_result().ok());
            }
            // The protocol asks that this object be dropped, and says nothing about the output's mode. So it
            // stops counting toward "every output changed" rather than failing the whole request.
            zwlr_output_power_v1::Event::Failed => state.failed.push(id),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputPowerManagerV1,
        _: <ZwlrOutputPowerManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A control the compositor disowned must not hold the whole request open, and must not be counted as
    /// having changed either — the two ways this could report a lie.
    #[test]
    fn a_failed_output_stops_counting_without_failing_the_rest() {
        let mut state = State::default();
        state.modes.insert(1, Some(Mode::On));
        state.modes.insert(2, None);
        state.failed.push(2);

        let live: Vec<u32> = state
            .modes
            .keys()
            .copied()
            .filter(|id| !state.failed.contains(id))
            .collect();
        assert_eq!(live, vec![1]);
        assert!(
            live.iter()
                .all(|id| state.modes.get(id).copied().flatten() == Some(Mode::On)),
            "the output that answered is the only one the request waits on"
        );
    }

    /// Reading the mode, which needs the protocol but disturbs nothing — so it runs where the blanking test
    /// below would be too rude, and tells a missing protocol apart from a broken one.
    ///
    /// `HOGAR_SHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland power_reads -- --nocapture`
    #[test]
    fn power_reads_back_from_the_compositor() {
        if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_err() {
            eprintln!("set HOGAR_SHELL_WAYLAND_LIVE to ask the real compositor; skipping");
            return;
        }
        let advertised = output_power_supported();
        eprintln!("advertises the manager: {advertised:?}");
        assert_eq!(advertised, Some(true));
        let on = output_power_on();
        eprintln!("every output on: {on:?}");
        assert_eq!(
            on,
            Some(true),
            "the screen this is running on is on, so the reading has to say so"
        );
    }

    /// Blanking every screen and waking it again, against the real compositor.
    ///
    /// `HOGAR_SHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland power -- --nocapture`
    ///
    /// **It turns the screen off and back on.** There is no gentler way to prove a screen blanked: the mode is
    /// exactly what this reads back, and reading it without setting it proves only that the protocol answers.
    #[test]
    fn a_screen_can_be_blanked_and_woken() {
        if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_err() {
            eprintln!("set HOGAR_SHELL_WAYLAND_LIVE to blank the real screen; skipping");
            return;
        }
        assert_eq!(output_power_supported(), Some(true));
        assert_eq!(
            output_power_on(),
            Some(true),
            "the screen this is running on should be on"
        );

        set_output_power(false).expect("the screen blanks");
        assert_eq!(output_power_on(), Some(false));
        std::thread::sleep(Duration::from_millis(600));

        set_output_power(true).expect("and wakes again");
        assert_eq!(output_power_on(), Some(true));
    }
}
