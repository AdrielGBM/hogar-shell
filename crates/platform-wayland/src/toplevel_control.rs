//! Windows over `zwlr-foreign-toplevel-management-v1`: which one has focus, where it is, and acting on it.
//!
//! The other half of a window list, and the only portable one. `ext-foreign-toplevel-list-v1` (`toplevels.rs`) enumerates windows and gives each a stable identifier; it says nothing about which is focused, minimised or fullscreen, reports no output, and offers no way to raise or close anything. This protocol answers all of that and carries no identifier at all.
//!
//! **The two do not join.** A handle here and a handle there describe the same window and share nothing a client could match on — not an id, not a serial, nothing but a title and an app id that any two windows of the same application have in common. So a reading is taken from one protocol or the other in whole, never assembled from both: "the focused window" comes from here, "the window to capture" from there.
//!
//! **What it cannot say either.** No geometry, no workspace, no process id. A window's position and size are deliberately absent — `set_rectangle` sends a rectangle *to* the compositor, for the animation a minimise comes out of, and there is no reverse. Anything needing those stays on a compositor's own IPC.
//!
//! **It speaks over the shell's own connection whenever one is running.** `set_rectangle` names a surface, and a surface can only be named on the connection it was created on, so a watcher on a connection of its own could never tell the compositor where a window minimises to. It keeps its own queue and thread on that connection, and moves onto it when the shell's connection comes up after it started; a process with no shell running (a one-shot CLI) opens a connection of its own.
//!
//! **On a connection of its own the watcher stops when the last registration is retired**, dropping the connection with it, and the next [`watch`] starts a fresh one — the shell's rule that nothing is resident unless something is asking for it. Bounded by the next event, as a polling producer is bounded by its next turn: until the compositor says something the thread is asleep in `poll`, resident but not running.
//!
//! **On the shell's connection it stays for as long as that connection does.** Starting binds a registry and a seat there, and neither can be handed back: a registry has no destructor, and the compositor goes on sending it every global that comes and goes, queued for a reader that is gone; a seat older than version 5 cannot be released. A watcher started and retired on every reader would leave one of each behind every time.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::{
    Channel, Event as ChannelEvent, Sender, channel,
};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;

use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_output, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use crate::interest::Interest;
use crate::platform::SurfaceRef;

use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

/// The global a compositor advertises when it can be told to act on a window.
pub const TOPLEVEL_MANAGER_INTERFACE: &str = "zwlr_foreign_toplevel_manager_v1";

/// The version that added fullscreen, both as a state and as a request. Below it a caller asking for one is told so rather than being silently ignored.
const FULLSCREEN_SINCE: u32 = 2;

/// The `wl_output` version that names an output.
const OUTPUT_NAME_SINCE: u32 = 4;

const SEAT_RELEASE_SINCE: u32 = 5;

const STATE_MAXIMIZED: u32 = 0;
const STATE_MINIMIZED: u32 = 1;
const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

/// Names one window for as long as it is open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ManagedToplevelId(u32);

impl ManagedToplevelId {
    /// The raw token, for a caller that has to key something on a window's identity — a list row, a stored preference — and wants a number rather than a `Debug` rendering it would then depend on.
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Rebuilds an id from [`ManagedToplevelId::raw`].
    ///
    /// Mostly for tests, which cannot otherwise produce two windows that differ only in identity — the case a window list has to survive, since two windows of one application share a title far more often than they share nothing. Fabricating one is safe: an id the compositor never issued matches no window, so every action against it is a no-op rather than the wrong window being acted on.
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

/// A window, as the compositor's management protocol describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedToplevel {
    pub id: ManagedToplevelId,
    pub title: String,
    /// The application's own id — what `class` is called everywhere except Hyprland.
    pub app_id: String,
    /// The outputs the window is visible on. More than one when it straddles a boundary, none while the compositor has not placed it or its `wl_output` predates version 4.
    pub outputs: Vec<String>,
    /// This is the focused window. The reading `ext-foreign-toplevel-list-v1` cannot produce at all.
    pub activated: bool,
    pub minimized: bool,
    pub maximized: bool,
    /// Reported only by a compositor implementing version 2 or above; false on version 1 whether or not the window is actually fullscreen.
    pub fullscreen: bool,
}

type Handler = Box<dyn FnMut(&[ManagedToplevel]) + Send>;

/// One reader of the list, and the claim that says whether it still wants to be one.
struct Registration {
    interest: Interest,
    handler: Handler,
}

static HANDLERS: Mutex<Vec<Registration>> = Mutex::new(Vec::new());
static LATEST: Mutex<Vec<ManagedToplevel>> = Mutex::new(Vec::new());
/// The live watcher, and `None` whenever none is running — which is what lets a later [`watch`] start a fresh thread rather than register with one that has already gone.
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// Set when starting finds no compositor or no management protocol, so a caller on one that cannot answer does not open a connection on every `watch` — no watcher is left behind to say it already failed.
static UNSUPPORTED: AtomicBool = AtomicBool::new(false);
static GENERATIONS: AtomicU64 = AtomicU64::new(0);

/// A watcher thread as the rest of the process reaches it. The generation is what a thread ending checks before it gives the slot up, so one that was replaced cannot take its successor down with it.
struct Running {
    requests: Sender<Request>,
    generation: u64,
    on_shell: bool,
}

enum Request {
    Focus(ManagedToplevelId),
    Close(ManagedToplevelId),
    Fullscreen(ManagedToplevelId, bool),
    Minimized(ManagedToplevelId, bool),
    Maximized(ManagedToplevelId, bool),
    Rectangle(ManagedToplevelId, wl_surface::WlSurface, Option<Area>),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Area {
    pub fn covering(rect: telar::Rect) -> Option<Self> {
        let (left, top) = (rect.x.floor(), rect.y.floor());
        let (right, bottom) = ((rect.x + rect.width).ceil(), (rect.y + rect.height).ceil());
        let area = Self {
            x: left as i32,
            y: top as i32,
            width: (right - left) as i32,
            height: (bottom - top) as i32,
        };
        (area.width > 0 && area.height > 0).then_some(area)
    }
}

/// Whether the compositor can be told to act on a window, asked over a connection of its own so it answers outside a running shell. `None` means no compositor could be reached.
pub fn toplevel_control_supported() -> Option<bool> {
    crate::globals::advertises(TOPLEVEL_MANAGER_INTERFACE)
}

/// Registers `on_change` for the window list, starting the watcher on first use and keeping it for as long as `interest` is alive.
///
/// Returns false when the compositor does not implement the protocol, in which case `on_change` is never called. A handler registered after the watcher is running is handed the current list immediately.
///
/// The channel slot is held across both the start and the registration, and taken again by [`retire`]: that overlap is what stops a `watch` landing on a watcher already on its way out and never being called.
pub fn watch(
    interest: &Interest,
    on_change: impl FnMut(&[ManagedToplevel]) + Send + 'static,
) -> bool {
    let mut handler: Handler = Box::new(on_change);
    let mut slot = RUNNING.lock().unwrap();
    if !ensure_running(&mut slot) {
        return false;
    }
    // Held from reading the list to joining the readers: a publish in between would otherwise reach neither the read nor the handler.
    let mut handlers = HANDLERS.lock().unwrap();
    let latest = LATEST.lock().unwrap().clone();
    if !latest.is_empty() {
        handler(&latest);
    }
    handlers.push(Registration {
        interest: interest.clone(),
        handler,
    });
    true
}

/// The last list published, without waiting for a window to open, close or take focus.
pub fn current() -> Vec<ManagedToplevel> {
    LATEST.lock().unwrap().clone()
}

/// The focused window, when the compositor reports one. `None` on an empty workspace, and while a layer surface this shell owns holds the keyboard.
pub fn focused() -> Option<ManagedToplevel> {
    current().into_iter().find(|window| window.activated)
}

/// Raises and focuses a window.
pub fn focus(id: ManagedToplevelId) -> bool {
    send(Request::Focus(id))
}

/// Asks a window to close — the same request its own close button makes, so an application with unsaved work gets to put up its dialog rather than being killed.
pub fn close(id: ManagedToplevelId) -> bool {
    send(Request::Close(id))
}

pub fn set_fullscreen(id: ManagedToplevelId, fullscreen: bool) -> bool {
    send(Request::Fullscreen(id, fullscreen))
}

pub fn set_minimized(id: ManagedToplevelId, minimized: bool) -> bool {
    send(Request::Minimized(id, minimized))
}

pub fn set_maximized(id: ManagedToplevelId, maximized: bool) -> bool {
    send(Request::Maximized(id, maximized))
}

/// Tells the compositor where on `surface` the window is represented, which it may use as the place a minimise flies to; `None` withdraws a rectangle `surface` set, and leaves one another surface set since.
pub fn set_rectangle(id: ManagedToplevelId, surface: &SurfaceRef, area: Option<Area>) -> bool {
    let sent = send(Request::Rectangle(id, surface.0.clone(), area));
    if !sent && area.is_some() {
        tracing::warn!("no window watcher is running, so a minimise target was not sent");
    }
    sent
}

/// Whether the request could be handed to the watcher — not whether the compositor honoured it. The protocol answers an action by publishing a new state, so a caller wanting to know watches for it.
fn send(request: Request) -> bool {
    let slot = RUNNING.lock().unwrap();
    slot.as_ref()
        .is_some_and(|running| running.requests.send(request).is_ok())
}

/// Whether a watcher is running, starting one if none is.
///
/// Takes the slot the caller already holds rather than locking again, because [`watch`] has to keep that lock across registering — see there.
fn ensure_running(slot: &mut Option<Running>) -> bool {
    if slot.is_some() {
        return true;
    }
    if UNSUPPORTED.load(Ordering::Relaxed) {
        return false;
    }
    match start() {
        Some(running) => {
            *slot = Some(running);
            true
        }
        None => {
            UNSUPPORTED.store(true, Ordering::Relaxed);
            false
        }
    }
}

/// What becomes of the running watcher when the shell's connection comes or goes.
#[derive(Debug, PartialEq, Eq)]
enum Handover {
    Keep,
    Stop,
    Restart,
}

/// `running_on_shell` is `None` with no watcher running, and otherwise whether it speaks over the shell's connection. A watcher already where `shell` says the shell is stays; one that is not moves there while somebody is listening, and otherwise just stops, since the next [`watch`] starts in the right place.
fn handover(running_on_shell: Option<bool>, shell: bool, listening: bool) -> Handover {
    match running_on_shell {
        None => Handover::Keep,
        Some(on_shell) if on_shell == shell => Handover::Keep,
        Some(_) if listening => Handover::Restart,
        Some(_) => Handover::Stop,
    }
}

/// Moves the watcher onto the connection the shell speaks over now, called as that connection comes and goes. A watcher started before the shell was up is on a connection of its own, which can never name a surface of the shell's, so every minimise target it was handed would be dropped for as long as it ran.
pub(crate) fn follow_shell_connection() {
    let shell = crate::platform::shell_connection().is_some();
    let listening = anyone_listening();
    let mut slot = RUNNING.lock().unwrap();
    let running_on_shell = slot.as_ref().map(|running| running.on_shell);
    match handover(running_on_shell, shell, listening) {
        Handover::Keep => {}
        Handover::Stop => stop(&mut slot),
        Handover::Restart => {
            stop(&mut slot);
            if !ensure_running(&mut slot) {
                HANDLERS.lock().unwrap().clear();
            }
        }
    }
}

/// Tells the running watcher to hand back what it bound and end, and forgets the list it published: its ids name windows on its own connection, which another connection numbers differently.
fn stop(slot: &mut Option<Running>) {
    if let Some(running) = slot.take() {
        let _ = running.requests.send(Request::Stop);
    }
    LATEST.lock().unwrap().clear();
}

/// Whether the watcher `generation`, just woken, should end: only one on a connection of its own, once nobody is listening — see the module's notes on the shell's connection.
fn done(generation: u64, on_shell: bool) -> bool {
    !on_shell && !anyone_listening() && retire(generation)
}

/// Drops the registrations whose owner has retired, and says whether any are left.
fn anyone_listening() -> bool {
    let mut handlers = HANDLERS.lock().unwrap();
    handlers.retain(|registration| registration.interest.alive());
    !handlers.is_empty()
}

/// Gives up the slot of the watcher `generation`, for a thread about to return. `false` is a `watch` having landed since the last registration went: it is already in the list, and retiring now would leave it waiting on nothing. A slot already holding another watcher is left to it.
fn retire(generation: u64) -> bool {
    let mut slot = RUNNING.lock().unwrap();
    if !HANDLERS.lock().unwrap().is_empty() {
        return false;
    }
    if holds(&slot, generation) {
        *slot = None;
    }
    true
}

/// Gives the slot of the watcher `generation` up whatever is registered, for a watcher whose connection has failed under it: leaving the sender behind would have every later `focus` or `close` report success into a channel nobody reads. A watcher that was stopped and replaced leaves its successor's slot and readers alone.
fn forget(generation: u64) {
    let mut slot = RUNNING.lock().unwrap();
    if holds(&slot, generation) {
        *slot = None;
        HANDLERS.lock().unwrap().clear();
    }
}

fn holds(slot: &Option<Running>, generation: u64) -> bool {
    slot.as_ref()
        .is_some_and(|running| running.generation == generation)
}

/// Connects and binds here rather than on the watcher thread, so the answer to "can this compositor be told to act on a window" is known by the time [`watch`] returns.
fn start() -> Option<Running> {
    let shell = crate::platform::shell_connection();
    let on_shell = shell.is_some();
    let connection = match shell {
        Some(shell) => shell,
        None => Connection::connect_to_env().ok()?,
    };
    let (globals, queue) = registry_queue_init::<Watcher>(&connection).ok()?;
    let qh = queue.handle();
    let manager = match globals.bind::<ZwlrForeignToplevelManagerV1, _, _>(&qh, 1..=3, ()) {
        Ok(manager) => manager,
        Err(e) => {
            tracing::debug!("no wlr-foreign-toplevel-management: {e}");
            return None;
        }
    };
    // Focusing a window is a request against a seat, so a compositor with no seat can list windows and not raise them. That is a working watcher, not a reason to have none.
    let seat = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=9, ()).ok();
    if seat.is_none() {
        tracing::warn!("no wl_seat: windows can be listed and closed but not focused");
    }

    let mut bound = HashMap::new();
    globals.contents().with_list(|list| {
        for global in list {
            if global.interface == "wl_output" && global.version >= OUTPUT_NAME_SINCE {
                let output: wl_output::WlOutput =
                    globals
                        .registry()
                        .bind(global.name, OUTPUT_NAME_SINCE, &qh, ());
                bound.insert(global.name, output);
            }
        }
    });

    let (requests, channel) = channel();
    let generation = GENERATIONS.fetch_add(1, Ordering::Relaxed) + 1;
    std::thread::Builder::new()
        .name("hogar-shell-wlr-toplevels".to_string())
        .spawn(move || {
            run(
                Seed {
                    manager,
                    seat,
                    bound,
                    generation,
                    on_shell,
                },
                connection,
                queue,
                channel,
            )
        })
        .ok()?;
    Some(Running {
        requests,
        generation,
        on_shell,
    })
}

/// What the watcher needs that can cross a thread boundary. The loop handle cannot — it holds an `Rc` — and it does not exist until the loop does, so the watcher itself is assembled on the far side.
struct Seed {
    manager: ZwlrForeignToplevelManagerV1,
    seat: Option<wl_seat::WlSeat>,
    bound: HashMap<u32, wl_output::WlOutput>,
    generation: u64,
    on_shell: bool,
}

fn run(
    seed: Seed,
    connection: Connection,
    mut queue: EventQueue<Watcher>,
    requests: Channel<Request>,
) {
    let (generation, on_shell) = (seed.generation, seed.on_shell);
    let mut watcher = Watcher {
        connection: connection.clone(),
        seat: seed.seat,
        bound: seed.bound,
        state: State::default(),
        loop_handle: None,
        pending: None,
        rectangles: HashMap::new(),
        manager: Some(seed.manager),
        finished: false,
        warned_foreign_surface: false,
    };
    // The whole list as it stands, published even when it is empty: a watcher that replaced another has to take the old list's place at once, and ids from another connection name nothing here.
    if queue.roundtrip(&mut watcher).is_err() {
        watcher.release();
        return forget(generation);
    }
    watcher.publish();
    let Ok(mut event_loop) = EventLoop::<Watcher>::try_new() else {
        watcher.release();
        return forget(generation);
    };
    let handle = event_loop.handle();
    watcher.loop_handle = Some(handle.clone());
    if WaylandSource::new(connection, queue)
        .insert(handle.clone())
        .is_err()
    {
        watcher.release();
        return forget(generation);
    }
    let registered = handle.insert_source(requests, |event, _, watcher: &mut Watcher| {
        if let ChannelEvent::Msg(request) = event {
            watcher.apply(request);
        }
    });
    if registered.is_err() {
        watcher.release();
        return forget(generation);
    }

    while !watcher.finished {
        if event_loop.dispatch(None, &mut watcher).is_err() {
            break;
        }
        // Asked after a dispatch rather than after a publish: a registration is retired by whoever made it, which is not something this thread is told about, so the only sound moment to look is every time it wakes.
        if done(generation, on_shell) {
            watcher.release();
            return;
        }
    }
    watcher.release();
    forget(generation);
}

#[derive(Default)]
struct Entry {
    handle: Option<ZwlrForeignToplevelHandleV1>,
    title: String,
    app_id: String,
    outputs: Vec<u32>,
    states: Vec<u32>,
}

/// Everything the events accumulate, with no protocol object of its own — which is what lets the reading be checked without a compositor.
#[derive(Default)]
struct State {
    names: HashMap<u32, String>,
    windows: HashMap<u32, Entry>,
    /// Announcement order, the only order this protocol offers.
    order: Vec<u32>,
}

/// A focus request that has been sent and not yet taken effect, with the tries it has left.
struct Pending {
    target: u32,
    left: u8,
}

struct Watcher {
    connection: Connection,
    seat: Option<wl_seat::WlSeat>,
    bound: HashMap<u32, wl_output::WlOutput>,
    state: State,
    /// Filled in once the loop exists, which is what lets a focus request arm a retry.
    loop_handle: Option<LoopHandle<'static, Watcher>>,
    pending: Option<Pending>,
    /// The last rectangle sent for each window and the surface that asked for it: the protocol keeps only the last one, so a withdrawal from any other surface must leave it standing.
    rectangles: HashMap<u32, (ObjectId, Area)>,
    manager: Option<ZwlrForeignToplevelManagerV1>,
    finished: bool,
    warned_foreign_surface: bool,
}

impl State {
    fn add(&mut self, key: u32, handle: ZwlrForeignToplevelHandleV1) {
        if self
            .windows
            .insert(
                key,
                Entry {
                    handle: Some(handle),
                    ..Entry::default()
                },
            )
            .is_none()
        {
            self.order.push(key);
        }
    }

    fn remove(&mut self, key: u32) {
        self.windows.remove(&key);
        self.order.retain(|open| *open != key);
    }

    fn snapshot(&self) -> Vec<ManagedToplevel> {
        self.order
            .iter()
            .filter_map(|key| {
                let entry = self.windows.get(key)?;
                Some(ManagedToplevel {
                    id: ManagedToplevelId(*key),
                    title: entry.title.clone(),
                    app_id: entry.app_id.clone(),
                    outputs: entry
                        .outputs
                        .iter()
                        .filter_map(|output| self.names.get(output).cloned())
                        .collect(),
                    activated: entry.states.contains(&STATE_ACTIVATED),
                    minimized: entry.states.contains(&STATE_MINIMIZED),
                    maximized: entry.states.contains(&STATE_MAXIMIZED),
                    fullscreen: entry.states.contains(&STATE_FULLSCREEN),
                })
            })
            .collect()
    }
}

impl Watcher {
    fn apply(&mut self, request: Request) {
        let key = match request {
            Request::Stop => {
                self.finished = true;
                return;
            }
            Request::Focus(ManagedToplevelId(key))
            | Request::Close(ManagedToplevelId(key))
            | Request::Fullscreen(ManagedToplevelId(key), _)
            | Request::Minimized(ManagedToplevelId(key), _)
            | Request::Maximized(ManagedToplevelId(key), _)
            | Request::Rectangle(ManagedToplevelId(key), ..) => key,
        };
        let Some(handle) = self
            .state
            .windows
            .get(&key)
            .and_then(|entry| entry.handle.as_ref())
        else {
            return;
        };
        match request {
            Request::Focus(_) => match &self.seat {
                Some(seat) => {
                    handle.activate(seat);
                    self.await_focus(key);
                }
                None => tracing::warn!("cannot focus a window without a seat"),
            },
            Request::Close(_) => handle.close(),
            Request::Fullscreen(_, on) if handle.version() >= FULLSCREEN_SINCE => {
                if on {
                    handle.set_fullscreen(None);
                } else {
                    handle.unset_fullscreen();
                }
            }
            Request::Fullscreen(..) => tracing::warn!(
                "this compositor's wlr-foreign-toplevel-management is version {}; it has no fullscreen request",
                handle.version()
            ),
            Request::Minimized(_, true) => handle.set_minimized(),
            Request::Minimized(_, false) => handle.unset_minimized(),
            Request::Maximized(_, true) => handle.set_maximized(),
            Request::Maximized(_, false) => handle.unset_maximized(),
            Request::Rectangle(_, surface, area) => {
                let handle = handle.clone();
                self.rectangle(key, &handle, &surface, area);
            }
            Request::Stop => {}
        }
        // A request made outside the loop's own dispatch sits in the outgoing buffer until something flushes it.
        let _ = self.connection.flush();
    }

    fn rectangle(
        &mut self,
        key: u32,
        handle: &ZwlrForeignToplevelHandleV1,
        surface: &wl_surface::WlSurface,
        area: Option<Area>,
    ) {
        let on_this_connection = surface
            .backend()
            .upgrade()
            .is_some_and(|backend| backend == self.connection.backend());
        if !on_this_connection {
            if !std::mem::replace(&mut self.warned_foreign_surface, true) {
                tracing::warn!(
                    "minimise targets are being dropped: they name a surface this window watcher's connection cannot name"
                );
            }
            return;
        }
        let asker = surface.id();
        match area {
            Some(area) => {
                if self.rectangles.get(&key) != Some(&(asker.clone(), area)) {
                    handle.set_rectangle(surface, area.x, area.y, area.width, area.height);
                    self.rectangles.insert(key, (asker, area));
                }
            }
            None => {
                if self
                    .rectangles
                    .get(&key)
                    .is_some_and(|(set_by, _)| *set_by == asker)
                {
                    handle.set_rectangle(surface, 0, 0, 0, 0);
                    self.rectangles.remove(&key);
                }
            }
        }
    }

    /// Hands back everything bound on the connection. On the shell's connection nothing else would: the objects outlive this thread's queue for as long as the connection does.
    fn release(&mut self) {
        if let Some(manager) = self.manager.take() {
            manager.stop();
        }
        for entry in self.state.windows.values_mut() {
            if let Some(handle) = entry.handle.take() {
                handle.destroy();
            }
        }
        for (_, output) in self.bound.drain() {
            output.release();
        }
        if let Some(seat) = self.seat.take()
            && seat.version() >= SEAT_RELEASE_SINCE
        {
            seat.release();
        }
        self.rectangles.clear();
        let _ = self.connection.flush();
    }

    /// Watches for a focus request to take effect, and asks again if it did not.
    ///
    /// **A compositor ignores `activate` while another surface holds the seat's keyboard.** Measured against Hyprland 0.56.1: with a layer surface up at `KeyboardInteractivity::Exclusive` the request changes nothing at all, and the same request lands the moment that surface is gone. That is the whole reason a window switcher living in a layer surface did nothing — and it cannot be fixed by ordering alone, because the surface is torn down by the driver thread on its next turn while this is sent from the watcher thread, and nothing orders the two.
    ///
    /// So the request is repeated, briefly, until the compositor acts on it or the window stops existing. The deadline is short enough that a user who changed their mind and clicked elsewhere is not fought with.
    fn await_focus(&mut self, target: u32) {
        const TRIES: u8 = 8;
        self.pending = Some(Pending {
            target,
            left: TRIES,
        });
        self.arm_retry();
    }

    fn arm_retry(&mut self) {
        const RETRY: Duration = Duration::from_millis(70);
        let Some(handle) = self.loop_handle.clone() else {
            return;
        };
        let _ = handle.insert_source(
            Timer::from_duration(RETRY),
            |_, _, watcher: &mut Watcher| {
                watcher.retry_focus();
                TimeoutAction::Drop
            },
        );
    }

    fn retry_focus(&mut self) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        let target = pending.target;
        let landed = self
            .state
            .windows
            .get(&target)
            .is_some_and(|entry| entry.states.contains(&STATE_ACTIVATED));
        // Gone, or focused: either way there is nothing left to ask for.
        if landed || !self.state.windows.contains_key(&target) {
            self.pending = None;
            return;
        }
        pending.left -= 1;
        if pending.left == 0 {
            tracing::warn!("the compositor never acted on a request to focus a window");
            self.pending = None;
            return;
        }
        if let (Some(seat), Some(entry)) = (self.seat.clone(), self.state.windows.get(&target))
            && let Some(handle) = entry.handle.as_ref()
        {
            handle.activate(&seat);
            let _ = self.connection.flush();
        }
        self.arm_retry();
    }

    /// Publishes the whole list. Each window batches its own changes behind a `done`, so a title retyped a keystroke at a time is one publish per commit rather than one per event.
    fn publish(&self) {
        let snapshot = self.state.snapshot();
        *LATEST.lock().unwrap() = snapshot.clone();
        let mut handlers = HANDLERS.lock().unwrap();
        handlers.retain_mut(|registration| {
            if !registration.interest.alive() {
                return false;
            }
            (registration.handler)(&snapshot);
            true
        });
    }
}

/// The state arrives as a flat array of native-endian `uint32`, one per state that is set.
fn states(bytes: &[u8]) -> Vec<u32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_ne_bytes(*c))
        .collect()
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Watcher {
    wayland_client::event_created_child!(Watcher, ZwlrForeignToplevelManagerV1, [
        zwlr_foreign_toplevel_manager_v1::EVT_TOPLEVEL_OPCODE => (ZwlrForeignToplevelHandleV1, ()),
    ]);

    fn event(
        state: &mut Self,
        _: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                state.state.add(toplevel.id().protocol_id(), toplevel)
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => state.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Watcher {
    fn event(
        state: &mut Self,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = proxy.id().protocol_id();
        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                state.state.windows.entry(key).or_default().title = title
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                state.state.windows.entry(key).or_default().app_id = app_id
            }
            zwlr_foreign_toplevel_handle_v1::Event::OutputEnter { output } => state
                .state
                .windows
                .entry(key)
                .or_default()
                .outputs
                .push(output.id().protocol_id()),
            zwlr_foreign_toplevel_handle_v1::Event::OutputLeave { output } => {
                let gone = output.id().protocol_id();
                state
                    .state
                    .windows
                    .entry(key)
                    .or_default()
                    .outputs
                    .retain(|output| *output != gone);
            }
            // The whole set every time, so a state that stopped being reported is one that was unset.
            zwlr_foreign_toplevel_handle_v1::Event::State { state: raw } => {
                state.state.windows.entry(key).or_default().states = states(&raw)
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => state.publish(),
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                state.state.remove(key);
                state.rectangles.remove(&key);
                proxy.destroy();
                state.publish();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Watcher {
    fn event(
        state: &mut Self,
        proxy: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.state.names.insert(proxy.id().protocol_id(), name);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Watcher {
    fn event(
        _: &mut Self,
        _: &wl_seat::WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// A monitor plugged in after the watcher started still has to be nameable, or every window on it reports no output at all.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Watcher {
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
            } if interface == "wl_output" && version >= OUTPUT_NAME_SINCE => {
                let output: wl_output::WlOutput = registry.bind(name, OUTPUT_NAME_SINCE, qh, ());
                state.bound.insert(name, output);
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(output) = state.bound.remove(&name) {
                    state.state.names.remove(&output.id().protocol_id());
                    output.release();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(generation: u64, on_shell: bool) -> (Running, Channel<Request>) {
        let (requests, channel) = channel::<Request>();
        let running = Running {
            requests,
            generation,
            on_shell,
        };
        (running, channel)
    }

    fn listening() -> Interest {
        let interest = Interest::new();
        HANDLERS.lock().unwrap().push(Registration {
            interest: interest.clone(),
            handler: Box::new(|_| {}),
        });
        interest
    }

    /// The half of "nothing runs unless something is asking for it" that a lazy start does not give: a watcher on a connection of its own has to *stop* when the last registration is retired, and give its slot back so the next [`watch`] starts a fresh one rather than registering with a thread on its way out. One on the shell's connection stays, since every start there leaves a registry behind; and a watcher that was replaced never takes its successor's slot or readers down with it as it ends.
    ///
    /// One test rather than several because they all move the same statics, and split across `cargo test`'s threads they would take turns wrecking each other's world.
    #[test]
    fn the_watcher_lives_exactly_as_long_as_its_registrations_or_its_shell() {
        let (own, _own_channel) = running(1, false);
        *RUNNING.lock().unwrap() = Some(own);
        let interest = listening();
        assert!(anyone_listening(), "a live registration is listening");
        assert!(
            !done(1, false),
            "something is registered, so the watcher stays"
        );

        // Retired by whoever registered, and dropped without ever being called again — which is the whole reason the answer lives beside the handler instead of in what it returns.
        interest.retire();
        assert!(!anyone_listening(), "a retired registration was kept");
        assert!(done(1, false), "nothing is registered, so nothing needs it");
        assert!(RUNNING.lock().unwrap().is_none(), "the slot stayed taken");

        let (shell, _shell_channel) = running(2, true);
        *RUNNING.lock().unwrap() = Some(shell);
        assert!(
            !done(2, true),
            "on the shell's connection the watcher stays with nobody listening"
        );
        assert!(holds(&RUNNING.lock().unwrap(), 2));

        let (successor, _successor_channel) = running(3, true);
        *RUNNING.lock().unwrap() = Some(successor);
        let interest = listening();
        forget(2);
        assert!(
            holds(&RUNNING.lock().unwrap(), 3),
            "a replaced watcher ending leaves its successor's slot"
        );
        assert!(anyone_listening(), "and its successor's readers");
        interest.retire();
        assert!(done(2, false), "the replaced watcher still ends");
        assert!(holds(&RUNNING.lock().unwrap(), 3));

        forget(3);
        assert!(RUNNING.lock().unwrap().is_none());
    }

    /// A watcher started before the shell's connection came up is on one of its own, which can never name a shell surface, so it moves across as soon as the shell's connection appears — and back off it once it is gone — while anybody is listening.
    #[test]
    fn the_watcher_follows_the_shells_connection() {
        assert_eq!(handover(None, true, true), Handover::Keep, "none running");
        assert_eq!(handover(Some(true), true, true), Handover::Keep);
        assert_eq!(handover(Some(false), false, true), Handover::Keep);
        assert_eq!(
            handover(Some(false), true, true),
            Handover::Restart,
            "started before the shell, moved onto its connection"
        );
        assert_eq!(
            handover(Some(true), false, true),
            Handover::Restart,
            "the shell's connection gone, its readers kept"
        );
        assert_eq!(
            handover(Some(false), true, false),
            Handover::Stop,
            "nobody to move across for: the next watch starts on the shell's connection"
        );
    }

    fn open_windows() -> State {
        let mut state = State::default();
        state.names.insert(3, "eDP-1".to_string());
        for (key, app_id, title, states) in [
            (30, "kitty", "nvim", vec![STATE_ACTIVATED]),
            (31, "code", "README.md", vec![STATE_MAXIMIZED]),
            (32, "helium", "Docs", vec![STATE_MINIMIZED]),
        ] {
            state.add_for_test(key);
            let entry = state.windows.get_mut(&key).unwrap();
            entry.app_id = app_id.to_string();
            entry.title = title.to_string();
            entry.outputs = vec![3];
            entry.states = states;
        }
        state
    }

    impl State {
        /// [`State::add`] without a protocol object, which a fixture cannot make.
        fn add_for_test(&mut self, key: u32) {
            if self.windows.insert(key, Entry::default()).is_none() {
                self.order.push(key);
            }
        }
    }

    #[test]
    fn a_minimise_target_covers_every_pixel_its_rect_touches_and_an_empty_one_is_none() {
        assert_eq!(
            Area::covering(telar::Rect::new(10.4, 2.6, 30.2, 20.0)),
            Some(Area {
                x: 10,
                y: 2,
                width: 31,
                height: 21,
            })
        );
        assert_eq!(Area::covering(telar::Rect::new(5.0, 5.0, 0.0, 12.0)), None);
    }

    #[test]
    fn states_are_read_as_native_endian_words() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&STATE_ACTIVATED.to_ne_bytes());
        bytes.extend_from_slice(&STATE_MAXIMIZED.to_ne_bytes());
        assert_eq!(states(&bytes), vec![STATE_ACTIVATED, STATE_MAXIMIZED]);
        assert_eq!(states(&[]), Vec::<u32>::new());
    }

    /// The reading this protocol exists for, and the one `ext-foreign-toplevel-list-v1` cannot produce.
    #[test]
    fn exactly_one_window_is_the_focused_one() {
        let windows = open_windows().snapshot();
        let focused: Vec<&str> = windows
            .iter()
            .filter(|w| w.activated)
            .map(|w| w.app_id.as_str())
            .collect();
        assert_eq!(focused, vec!["kitty"]);
        assert!(windows.iter().any(|w| w.minimized && w.app_id == "helium"));
        assert!(windows.iter().any(|w| w.maximized && w.app_id == "code"));
        assert!(
            windows.iter().all(|w| !w.fullscreen),
            "a state the compositor did not report is unset, not unknown"
        );
        assert!(windows.iter().all(|w| w.outputs == vec!["eDP-1"]));
    }

    /// The `state` event carries the whole set every time, so unsetting one is that value no longer arriving — a handler that merged rather than replaced would leave a window minimised for ever.
    #[test]
    fn a_state_that_stops_being_reported_is_unset() {
        let mut state = open_windows();
        state.windows.get_mut(&32).unwrap().states = Vec::new();
        let restored = state
            .snapshot()
            .into_iter()
            .find(|w| w.app_id == "helium")
            .unwrap();
        assert!(!restored.minimized);
    }

    #[test]
    fn a_closed_window_leaves_the_others_where_they_were() {
        let mut state = open_windows();
        state.remove(31);
        assert_eq!(
            state
                .snapshot()
                .iter()
                .map(|w| w.app_id.as_str())
                .collect::<Vec<_>>(),
            vec!["kitty", "helium"]
        );
    }
}
