//! Colour temperature over `zwlr-gamma-control-unstable-v1`: warming the screen without a helper process.
//!
//! A night light is a gamma ramp per output, and this protocol is the portable way to set one. Every wlroots
//! compositor carries it, so nothing here needs `hyprsunset`, `gammastep` or `wlsunset` running alongside.
//!
//! **The control object *is* the setting.** The compositor restores the original ramp the moment the
//! `zwlr_gamma_control_v1` is destroyed — which is the protocol keeping a crashed client from leaving a screen
//! orange for ever, and which means turning the tint off is dropping the object rather than sending a neutral
//! ramp. It also means the objects have to be held for as long as the tint lasts, so this owns a connection and
//! a thread.
//!
//! **And for no longer than that.** The shell's standing rule is that nothing runs unless something is asking
//! for it, and here the two are the same fact: the controls are the tint, so a thread with nothing to hold has
//! nothing to do. Turning the night light off therefore ends the thread and closes the connection rather than
//! parking them until the process exits, and the next `warm` starts a fresh one. [`retire`] is that hand-over,
//! and the lock it takes is what keeps a `warm` arriving mid-retirement from being answered by nobody.
//!
//! **Only one client at a time.** A compositor grants gamma control to one client per output; a second gets
//! `failed`. That is reported rather than retried, because the honest answer to "something else already owns
//! the gamma" is to say so and leave that something else alone.

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop::channel::{
    Channel, Event as ChannelEvent, Sender, channel,
};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

/// The global a compositor advertises when its gamma can be set at all.
pub const GAMMA_INTERFACE: &str = "zwlr_gamma_control_manager_v1";

/// The range a caller may ask for, in kelvin. Below the floor the screen is unreadably red and above the
/// ceiling the ramp is clipping rather than cooling — both ends are the point at which the setting stops
/// meaning anything, not an implementation limit.
pub const MIN_TEMPERATURE: u32 = 1000;
pub const MAX_TEMPERATURE: u32 = 10000;

/// The temperature at which the ramp is the identity, which is what "off" restores.
pub const NEUTRAL_TEMPERATURE: u32 = 6500;

/// The live producer's channel, and `None` whenever no tint is held — which is what lets a later [`warm`] start
/// a fresh thread instead of talking into one that has already gone.
static REQUESTS: Mutex<Option<Sender<Request>>> = Mutex::new(None);
/// Set when starting finds no compositor or no gamma protocol. A machine that cannot do this at all would
/// otherwise open a connection on every call, since there is no producer left behind to say it already failed.
static UNSUPPORTED: AtomicBool = AtomicBool::new(false);
static APPLIED: Mutex<Option<u32>> = Mutex::new(None);

enum Request {
    Warm(u32),
    Neutral,
}

/// Whether the compositor lets a client set gamma at all, asked over a connection of its own so it answers
/// outside a running shell. `None` means no compositor could be reached.
pub fn gamma_supported() -> Option<bool> {
    crate::globals::advertises(GAMMA_INTERFACE)
}

/// Warms every output to `kelvin`, clamped to the range this protocol is useful over.
///
/// Reports whether the request could be sent, not whether the screen changed: the compositor answers a refused
/// gamma grab with an event, not a reply, so [`current`] is what says whether a tint is actually held.
pub fn warm(kelvin: u32) -> bool {
    let kelvin = kelvin.clamp(MIN_TEMPERATURE, MAX_TEMPERATURE);
    let mut slot = REQUESTS.lock().unwrap();
    let Some(requests) = producer(&mut slot) else {
        return false;
    };
    // Recorded here rather than on the watcher thread. The request is asynchronous, so a caller that warmed the
    // screen and immediately asked what it was holding would be told `None` — which is what `nightlight status`
    // straight after `nightlight on` is.
    //
    // Written while the producer slot is held, and read back under the same lock by [`retire`]: that overlap is
    // the whole of what stops a `warm` landing on a thread already on its way out.
    *APPLIED.lock().unwrap() = Some(kelvin);
    requests.send(Request::Warm(kelvin)).is_ok()
}

/// Restores every output's original ramp, which also ends the thread holding them.
pub fn neutral() -> bool {
    let slot = REQUESTS.lock().unwrap();
    // Nothing is warmed, so there is nothing to restore and no reason to open a connection to say so.
    let Some(requests) = slot.as_ref() else {
        return true;
    };
    *APPLIED.lock().unwrap() = None;
    requests.send(Request::Neutral).is_ok()
}

/// The temperature the shell is holding, or `None` when the screens are at their own.
///
/// What was asked for, not a reading: this protocol has no way to ask the compositor what the gamma currently
/// is — deliberately, since the ramp is per-client state — so a client's own intent is the only answer there
/// can be. An output whose gamma another client already owns is dropped from the tint and logged rather than
/// changing this.
pub fn current() -> Option<u32> {
    *APPLIED.lock().unwrap()
}

/// The channel to talk to a producer over, starting one if none is live.
///
/// Takes the slot the caller already holds rather than locking again, because a caller has to keep that lock
/// across recording its intent — see [`warm`].
fn producer(slot: &mut Option<Sender<Request>>) -> Option<Sender<Request>> {
    if let Some(requests) = slot.as_ref() {
        return Some(requests.clone());
    }
    if UNSUPPORTED.load(Ordering::Relaxed) {
        return None;
    }
    match start() {
        Some(requests) => Some(slot.insert(requests).clone()),
        None => {
            UNSUPPORTED.store(true, Ordering::Relaxed);
            None
        }
    }
}

/// Gives up the producer slot, for a thread that is about to return.
///
/// Answering `true` is final: the caller returns, its controls go with it and every screen is restored, so the
/// slot has to be free before that happens or the next [`warm`] would talk to a corpse. Answering `false` is a
/// `warm` having landed since the tint was dropped — it has already recorded what it wants and put its request
/// in the channel, and retiring now would leave the user's night light asked for and never applied.
fn retire() -> bool {
    let mut slot = REQUESTS.lock().unwrap();
    if APPLIED.lock().unwrap().is_some() {
        return false;
    }
    *slot = None;
    true
}

/// Gives the slot up whatever was asked for, for a producer whose connection has failed under it. Leaving the
/// sender behind would have every later [`warm`] report success into a channel nobody is reading, and leaving
/// [`APPLIED`] set would have the shell claim a tint that died with the connection.
fn forget() {
    let mut slot = REQUESTS.lock().unwrap();
    *slot = None;
    *APPLIED.lock().unwrap() = None;
}

fn start() -> Option<Sender<Request>> {
    let connection = Connection::connect_to_env().ok()?;
    let (globals, queue) = registry_queue_init::<Gamma>(&connection).ok()?;
    let qh = queue.handle();
    let manager = match globals.bind::<ZwlrGammaControlManagerV1, _, _>(&qh, 1..=1, ()) {
        Ok(manager) => manager,
        Err(e) => {
            tracing::debug!("no wlr-gamma-control: {e}");
            return None;
        }
    };

    let mut outputs = HashMap::new();
    globals.contents().with_list(|list| {
        for global in list {
            if global.interface == "wl_output" {
                let output: wl_output::WlOutput =
                    globals
                        .registry()
                        .bind(global.name, global.version.min(4), &qh, ());
                outputs.insert(global.name, output);
            }
        }
    });

    let (requests, channel) = channel();
    let gamma = Gamma {
        connection: connection.clone(),
        manager,
        qh,
        outputs,
        controls: HashMap::new(),
        wanted: None,
    };
    std::thread::Builder::new()
        .name("hogar-shell-wlr-gamma".to_string())
        .spawn(move || run(gamma, connection, queue, channel))
        .ok()?;
    Some(requests)
}

fn run(
    mut gamma: Gamma,
    connection: Connection,
    queue: EventQueue<Gamma>,
    requests: Channel<Request>,
) {
    let Ok(mut event_loop) = EventLoop::<Gamma>::try_new() else {
        return forget();
    };
    let handle = event_loop.handle();
    if WaylandSource::new(connection, queue)
        .insert(handle.clone())
        .is_err()
    {
        return forget();
    }
    let registered = handle.insert_source(requests, |event, _, gamma: &mut Gamma| {
        if let ChannelEvent::Msg(request) = event {
            gamma.apply(request);
        }
    });
    if registered.is_err() {
        return forget();
    }
    // The controls this thread holds are the tint: returning drops them and gives every screen back. That is
    // exactly what is wanted once nothing is tinted and never before, so both halves are checked — a control
    // still held is a screen still warm, whatever was last asked for.
    while event_loop.dispatch(None, &mut gamma).is_ok() {
        if gamma.wanted.is_none() && gamma.controls.is_empty() && retire() {
            return;
        }
    }
    forget();
}

/// One output's gamma control, and the ramp length the compositor asked for.
struct Control {
    control: ZwlrGammaControlV1,
    /// The output it was taken out against, so a second control is never opened for one already held.
    output: u32,
    size: Option<u32>,
}

struct Gamma {
    connection: Connection,
    manager: ZwlrGammaControlManagerV1,
    /// Held rather than taken from a dispatch callback: a temperature arrives over a channel, and creating a
    /// control needs a queue handle at exactly that moment.
    qh: QueueHandle<Gamma>,
    outputs: HashMap<u32, wl_output::WlOutput>,
    /// Keyed by the control's own protocol id, since that is what its events arrive against.
    controls: HashMap<u32, Control>,
    /// The temperature asked for, held so an output plugged in later is warmed to match the ones already on.
    wanted: Option<u32>,
}

impl Gamma {
    fn apply(&mut self, request: Request) {
        match request {
            Request::Warm(kelvin) => {
                self.wanted = Some(kelvin);
                for output in self.outputs.values().cloned().collect::<Vec<_>>() {
                    self.control_for(&output);
                }
                let ids: Vec<u32> = self.controls.keys().copied().collect();
                for id in ids {
                    self.send_ramp(id);
                }
            }
            // Destroying the control is what restores the screen; there is no neutral ramp to send.
            Request::Neutral => {
                self.wanted = None;
                for (_, held) in self.controls.drain() {
                    held.control.destroy();
                }
            }
        }
        let _ = self.connection.flush();
    }

    /// Takes gamma control of `output` unless this already holds one for it.
    fn control_for(&mut self, output: &wl_output::WlOutput) {
        let id = output.id().protocol_id();
        if self.controls.values().any(|held| held.output == id) {
            return;
        }
        let control = self.manager.get_gamma_control(output, &self.qh, id);
        self.controls.insert(
            control.id().protocol_id(),
            Control {
                control,
                output: id,
                size: None,
            },
        );
    }

    /// Sends the ramp for one control, if the compositor has said how long it wants it. A control created a
    /// moment ago has not, and its `gamma_size` is what comes back to do this.
    fn send_ramp(&self, id: u32) {
        let Some(held) = self.controls.get(&id) else {
            return;
        };
        let (Some(size), Some(kelvin)) = (held.size, self.wanted) else {
            return;
        };
        let Some(fd) = ramp_fd(size, kelvin) else {
            tracing::warn!("could not build a gamma ramp");
            return;
        };
        held.control.set_gamma(fd.as_fd());
    }
}

/// The white point of a black body at `kelvin`, as red, green and blue multipliers in 0..=1.
///
/// Tanner Helland's approximation, which is what every night-light implementation uses and is accurate enough
/// that the error is invisible next to the effect. The alternative is a Planckian locus and a colour-space
/// conversion for a difference nobody looking at a warm screen can see.
fn white_point(kelvin: u32) -> (f64, f64, f64) {
    let t = f64::from(kelvin) / 100.0;
    let channel = |v: f64| (v / 255.0).clamp(0.0, 1.0);
    if t <= 66.0 {
        let green = 99.4708025861 * t.ln() - 161.1195681661;
        let blue = if kelvin <= 1900 {
            0.0
        } else {
            138.5177312231 * (t - 10.0).ln() - 305.0447927307
        };
        (1.0, channel(green), channel(blue))
    } else {
        let red = 329.698727446 * (t - 60.0).powf(-0.1332047592);
        let green = 288.1221695283 * (t - 60.0).powf(-0.0755148492);
        (channel(red), channel(green), 1.0)
    }
}

/// The three ramps the protocol wants, back to back: `size` reds, then greens, then blues, as native-endian
/// `u16`. A linear ramp scaled by the white point, which is what makes the correction a tint rather than a
/// change of contrast.
fn ramp(size: u32, kelvin: u32) -> Vec<u8> {
    let (red, green, blue) = white_point(kelvin);
    let last = f64::from(size.saturating_sub(1)).max(1.0);
    let mut table = Vec::with_capacity(size as usize * 3 * 2);
    for channel in [red, green, blue] {
        for step in 0..size {
            let value = (f64::from(step) / last * channel * f64::from(u16::MAX)).round();
            let value = value.clamp(0.0, f64::from(u16::MAX)) as u16;
            table.extend_from_slice(&value.to_ne_bytes());
        }
    }
    table
}

/// The ramp in a pipe, ready to hand to the compositor.
///
/// Written before the read end is sent rather than after: the table is a few kilobytes against a pipe buffer of
/// sixty-four, so it lands in the buffer without anything reading yet — and writing after the request would
/// mean racing a compositor that may already be blocked reading.
fn ramp_fd(size: u32, kelvin: u32) -> Option<OwnedFd> {
    let (read, write) = std::io::pipe().ok()?;
    let mut write = std::fs::File::from(OwnedFd::from(write));
    write.write_all(&ramp(size, kelvin)).ok()?;
    write.flush().ok()?;
    // Dropped here or the compositor never sees the end of the table.
    drop(write);
    Some(OwnedFd::from(read))
}

impl Dispatch<ZwlrGammaControlV1, u32> for Gamma {
    fn event(
        state: &mut Self,
        proxy: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        _: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = proxy.id().protocol_id();
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => {
                if let Some(held) = state.controls.get_mut(&id) {
                    held.size = Some(size);
                }
                state.send_ramp(id);
                let _ = state.connection.flush();
            }
            // Another client owns this output's gamma. Saying so and leaving it alone is the whole handling:
            // the protocol grants control to one client, and fighting for it would flicker.
            zwlr_gamma_control_v1::Event::Failed => {
                tracing::warn!("another client already controls this output's gamma");
                if let Some(held) = state.controls.remove(&id) {
                    held.control.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for Gamma {
    fn event(
        _: &mut Self,
        _: &ZwlrGammaControlManagerV1,
        _: <ZwlrGammaControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Gamma {
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

/// A monitor plugged in while the tint is on has to be warmed too, or one screen stays blue.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Gamma {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => {
                let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, ());
                state.outputs.insert(name, output);
                if state.wanted.is_some() {
                    let outputs: Vec<wl_output::WlOutput> =
                        state.outputs.values().cloned().collect();
                    for output in outputs {
                        state.control_for(&output);
                    }
                }
            }
            wl_registry::Event::GlobalRemove { name } => {
                state.outputs.remove(&name);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direction the whole feature rests on: warmer means less blue, and the neutral point means no tint.
    #[test]
    fn a_warmer_temperature_takes_blue_away_and_leaves_red_alone() {
        let (warm_r, warm_g, warm_b) = white_point(2500);
        let (_, mid_g, mid_b) = white_point(4000);
        let (neutral_r, _, neutral_b) = white_point(NEUTRAL_TEMPERATURE);

        assert_eq!(warm_r, 1.0, "red is never reduced below the neutral point");
        assert!(warm_b < mid_b, "2500K is bluer than it should be: {warm_b}");
        assert!(warm_g < mid_g);
        assert!(
            neutral_b > 0.98 && neutral_r > 0.98,
            "6500K has to come out as very nearly no tint at all, got r={neutral_r} b={neutral_b}"
        );
    }

    #[test]
    fn every_channel_stays_inside_the_representable_range() {
        for kelvin in [
            MIN_TEMPERATURE,
            1500,
            1900,
            2000,
            NEUTRAL_TEMPERATURE,
            8000,
            MAX_TEMPERATURE,
        ] {
            let (r, g, b) = white_point(kelvin);
            for (name, value) in [("red", r), ("green", g), ("blue", b)] {
                assert!(
                    (0.0..=1.0).contains(&value),
                    "{name} at {kelvin}K is {value}"
                );
            }
        }
    }

    /// The wire format, which the compositor reads without negotiating: three ramps back to back, `size`
    /// native-endian `u16` each. Getting the length wrong is a protocol error, not a wrong colour.
    #[test]
    fn the_table_is_three_ramps_of_native_endian_words() {
        let size = 256;
        let table = ramp(size, 3000);
        assert_eq!(table.len(), size as usize * 3 * 2);

        let word = |index: usize| u16::from_ne_bytes([table[index * 2], table[index * 2 + 1]]);
        assert_eq!(word(0), 0, "every ramp starts at black");
        assert_eq!(
            word(size as usize - 1),
            u16::MAX,
            "red is untouched at 3000K, so its ramp reaches full scale"
        );
        // The blue ramp's top is the white point's blue, which at 3000K is well under full scale.
        let blue_top = word(size as usize * 3 - 1);
        assert!(blue_top < u16::MAX / 2, "blue at 3000K is {blue_top}");
    }

    #[test]
    fn a_ramp_is_monotonic_so_the_tint_never_inverts_a_gradient() {
        let table = ramp(64, 4000);
        for channel in 0..3 {
            let base = channel * 64;
            for step in 1..64 {
                let previous = u16::from_ne_bytes([
                    table[(base + step - 1) * 2],
                    table[(base + step - 1) * 2 + 1],
                ]);
                let current =
                    u16::from_ne_bytes([table[(base + step) * 2], table[(base + step) * 2 + 1]]);
                assert!(current >= previous, "channel {channel} dips at {step}");
            }
        }
    }

    #[test]
    fn a_one_entry_ramp_does_not_divide_by_zero() {
        assert_eq!(ramp(1, 3000).len(), 6);
    }

    /// The half of "nothing runs unless something is asking for it" that a lazy start does not give: a producer
    /// nobody needs has to *stop*, and the slot it held has to be free for the next caller to start a fresh one.
    ///
    /// One test rather than three because all three phases move the same two statics, and split across
    /// `cargo test`'s threads they would take turns wrecking each other's world.
    #[test]
    fn the_producer_lives_exactly_as_long_as_the_tint() {
        let install = || {
            let (requests, channel) = channel::<Request>();
            *REQUESTS.lock().unwrap() = Some(requests);
            channel
        };

        // Nothing is tinted: the producer goes, and the slot is free before its thread returns — otherwise the
        // next `warm` would talk to a corpse.
        let _channel = install();
        *APPLIED.lock().unwrap() = None;
        assert!(retire(), "nothing is tinted, so nothing needs the producer");
        assert!(REQUESTS.lock().unwrap().is_none(), "the slot stayed taken");

        // The race the lock exists for: a `warm` between the neutral and the retirement has already recorded
        // its intent and queued its request, so retiring would leave the night light asked for and never
        // applied, with nothing running left to notice.
        let _channel = install();
        *APPLIED.lock().unwrap() = Some(3000);
        assert!(!retire(), "a tint was asked for, so the producer stays");
        assert!(REQUESTS.lock().unwrap().is_some(), "the slot was given up");

        // And turning off a light that is not on opens nothing to say so with.
        forget();
        assert!(neutral());
        assert!(
            REQUESTS.lock().unwrap().is_none(),
            "answering 'nothing to restore' started a producer to answer it with"
        );
    }

    /// The half no fixture can prove: that the compositor accepts the table and holds the tint.
    ///
    /// `HOGAR_SHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland gamma -- --nocapture --test-threads=1`
    ///
    /// **It warms the screen for a second and puts it back.** There is no reading to check instead — the
    /// protocol has no "what is the gamma" request, by design, so the only evidence it worked is that the
    /// compositor did not answer `failed` and the screen went warm.
    #[test]
    fn the_compositor_takes_a_ramp_and_gives_the_screen_back() {
        use std::time::Duration;

        if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_err() {
            eprintln!("set HOGAR_SHELL_WAYLAND_LIVE to tint the real screen; skipping");
            return;
        }
        assert_eq!(
            gamma_supported(),
            Some(true),
            "this compositor does not implement wlr-gamma-control"
        );

        assert!(warm(2500), "the request could not be sent");
        assert_eq!(current(), Some(2500));
        // Long enough for the compositor to answer `gamma_size` and take the table, and to see it happen.
        std::thread::sleep(Duration::from_millis(800));

        assert!(neutral(), "the screen could not be given back");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(current(), None);
        assert!(
            REQUESTS.lock().unwrap().is_none(),
            "the thread holding the tint outlived the tint"
        );

        // And a second tint after the first has been given up gets a producer of its own, which is the half of
        // retiring that a released slot alone does not prove.
        assert!(warm(2500), "a fresh producer could not be started");
        std::thread::sleep(Duration::from_millis(800));
        assert_eq!(current(), Some(2500));
        assert!(neutral());
    }
}
