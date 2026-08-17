use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crate::placement::{
    KeyboardMode, SurfaceAlign, SurfaceAnchor, SurfacePlacement, SurfaceRole, SurfaceSize,
};
use telar::{
    AlignItems, App, Color, Component, Edge, Event, EventHandler, Key, ModifiersState,
    MultiSurfacePlatform, NamedKey, PlatformError, PointerButton, PointerSource, ScrollDelta,
    SurfaceContent, SurfaceControl, SurfaceHost, SurfaceId, SurfaceRoot, SurfaceScaffold,
    SurfaceToken, SurfaceTransition, Window, WindowConfig,
    begin_batch, build_surface_handler, end_batch, reset_layout_runtime, set_surface_host,
};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop::channel::{
    Channel, Event as ChannelEvent, Sender as ChannelSender, channel,
};
use smithay_client_toolkit::reexports::calloop::ping::make_ping;
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle, RegistrationToken};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use smithay_client_toolkit::reexports::protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{PointerEvent, PointerEventKind, PointerHandler};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure, SurfaceKind,
};
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, registry_handlers,
};
use wayland_client::backend::ObjectId;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::config::{Anchor, KeyboardInteractivity, Layer, LayerConfig, OutputDescriptor};
use crate::link::{ExitPlan, SurfaceLink, SurfaceUpdate};
use crate::lock::LockSession;
use crate::window::LayerWindow;

/// The type driven every surface handler is boxed to, so one loop holds statically-declared bars and
/// runtime-opened drawers/OSDs in one `Vec` (the blanket `EventHandler for Box<dyn EventHandler>` makes the
/// box callable). All surfaces share this UI thread; isolation is the handler's own `ui_core::Surface`.
pub(crate) type BoxedHandler = Box<dyn EventHandler<LayerWindow>>;

/// The calloop sources (timers, channels) a surface registered while its handler ran, removed together when the
/// surface is torn down. Shared by `Rc` so `with_current` can hand the sink to `interval`/`watch` without
/// borrowing the driver's `SurfaceEntry`.
type SourceSink = Rc<RefCell<Vec<RegistrationToken>>>;

/// Where `on_close` files a surface's exit transition while its handler runs, on the same terms as
/// [`SourceSink`]: shared by `Rc` so the driver can read the plan back without borrowing its entry.
type ExitSink = Rc<RefCell<ExitPlan>>;

thread_local! {
    static LOOP_HANDLE: RefCell<Option<LoopHandle<'static, Driver>>> = const { RefCell::new(None) };
    // The control channel of the surface whose handler is currently running, so `request_close` and `request_geometry` target it (a surface opened by `open_surface` has one; a lock surface does not). Set by the driver around each handler call.
    static CURRENT_LINK: RefCell<Option<Arc<SurfaceLink>>> = const { RefCell::new(None) };
    // Where `interval`/`watch` file their registration tokens while a surface's handler runs, so the driver can
    // drop them with that surface. `None` outside a surface (app-level setup), where sources are process-lived.
    static CURRENT_SOURCES: RefCell<Option<SourceSink>> = const { RefCell::new(None) };
    // Where `on_close` files what the current surface does on its way out. `None` outside a surface.
    static CURRENT_EXIT: RefCell<Option<ExitSink>> = const { RefCell::new(None) };
    // Dynamic surfaces requested via `open_surface` on the UI thread; the driver drains and mounts them.
    static DYN_QUEUE: RefCell<Vec<PendingSurface>> = const { RefCell::new(Vec::new()) };
    // App-level setup to run once on the driver thread after the loop is up (see `run_on_start`).
    static STARTUP: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
    // The driver's live view of the compositor's outputs, so `outputs()` needs no second Wayland connection.
    static OUTPUTS: RefCell<Vec<OutputDescriptor>> = const { RefCell::new(Vec::new()) };
    // Notified when the output set changes once the shell is up, so the app can reconcile its surfaces (hotplug).
    static OUTPUTS_CHANGED: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Files `token` against the surface currently being driven, so its teardown removes the source. Outside a
/// surface the token is dropped: app-level sources (the config watcher) live as long as the process.
fn track_source(token: RegistrationToken) {
    CURRENT_SOURCES.with(|s| {
        if let Some(sink) = s.borrow().as_ref() {
            sink.borrow_mut().push(token);
        }
    });
}

/// Wayland surfaces the driver is holding, refreshed once a turn. An atomic rather than a thread-local because
/// the point is to be readable as a number without being on the driver thread, which is what makes a surface
/// leak observable from a script instead of from `top`.
static LIVE_SURFACES: AtomicUsize = AtomicUsize::new(0);

/// How many surfaces are mapped right now — every bar, panel, drawer and popup the driver holds, not just the
/// ones a user opened. Reported by `shell status`.
pub fn live_surfaces() -> usize {
    LIVE_SURFACES.load(Ordering::Relaxed)
}

/// Registers a closure to run once on the driver thread just after its loop is set up (its `LOOP_HANDLE` and
/// `SurfaceHost` installed), so app-level setup that needs `watch`/`open_surface` — e.g. the notification popup
/// host — runs on the right thread. Call it before `run_multi_with_platform` (same thread as the driver).
pub fn run_on_start(task: impl FnOnce() + 'static) {
    STARTUP.with(|s| s.borrow_mut().push(Box::new(task)));
}

struct PendingSurface {
    config: LayerConfig,
    // `None` for a reservation-only strip (no rsx handler, just its exclusive zone).
    handler: Option<BoxedHandler>,
    link: Arc<SurfaceLink>,
}

/// Runs the handler closure with the current surface's worlds installed: `link` so `request_close` and
/// `request_geometry` reach the right surface, `sources` as the sink `interval`/`watch` file their registration
/// tokens into (so the surface's timers and channels die with it), and `exit` as the one `on_close` files its
/// exit transition into. All three are restored afterwards.
fn with_current<R>(
    link: &Option<Arc<SurfaceLink>>,
    sources: &SourceSink,
    exit: &ExitSink,
    f: impl FnOnce() -> R,
) -> R {
    CURRENT_LINK.with(|c| *c.borrow_mut() = link.clone());
    CURRENT_SOURCES.with(|s| *s.borrow_mut() = Some(Rc::clone(sources)));
    CURRENT_EXIT.with(|e| *e.borrow_mut() = Some(Rc::clone(exit)));
    let result = f();
    CURRENT_LINK.with(|c| *c.borrow_mut() = None);
    CURRENT_SOURCES.with(|s| *s.borrow_mut() = None);
    CURRENT_EXIT.with(|e| *e.borrow_mut() = None);
    result
}

fn with_current_link(read: impl FnOnce(&Arc<SurfaceLink>)) {
    CURRENT_LINK.with(|c| {
        if let Some(link) = c.borrow().as_ref() {
            read(link);
        }
    });
}

/// Asks the *current* surface to close — for a dynamic surface (drawer/OSD), flips its close flag so the driver
/// tears it down on the next loop turn, or plays out the exit transition it registered with [`on_close`] first.
/// No-op on a surface the driver mounted itself (a lock surface), which has no control channel.
pub fn request_close() {
    with_current_link(|link| link.request_close());
}

/// Asks the compositor to renegotiate the *current* surface's size, from inside its own handler — the drag of a
/// float's resize grip, say. Layer-shell sizes are logical pixels; `0` on an axis hands that axis back to the
/// compositor.
pub fn request_size(width: u32, height: u32) {
    with_current_link(|link| link.request_update(SurfaceUpdate::size(width, height)));
}

/// Asks the compositor to move the *current* surface relative to the edges it is anchored to, as
/// `(top, right, bottom, left)` logical pixels. A margin only takes effect on an edge the surface is anchored
/// to, and a negative one pushes it off that edge — which is how an auto-hidden bar leaves only a hover strip
/// on screen.
pub fn request_margin(margin: (i32, i32, i32, i32)) {
    with_current_link(|link| link.request_update(SurfaceUpdate::margin(margin)));
}

/// Registers what the *current* surface does when it is asked to close, and how long the driver keeps it mapped
/// afterwards so that reaction can be seen — an exit transition, in other words.
///
/// Call it while the surface's content is being built, which is the one time the surface is current. More than
/// one caller per surface is expected and additive: the hosted scaffold fades its scrim out while the panel
/// content slides back toward its bar edge, and the driver waits for the longer of the two.
///
/// A zero `linger`, or no registration at all, is the original behaviour — the surface goes on the driver's next
/// loop turn.
pub fn on_close(linger: Duration, react: impl FnOnce() + 'static) {
    CURRENT_EXIT.with(|e| {
        if let Some(sink) = e.borrow().as_ref() {
            sink.borrow_mut().push(linger, Box::new(react));
        }
    });
}

/// Repeats `callback` every `period` on the shared loop. Bound to the surface that registered it: when that
/// surface is torn down (a drawer closing, a bar replaced on config reload) the timer is removed with it, so a
/// reopened panel never stacks a second ticker on the first.
pub fn interval(period: Duration, mut callback: impl FnMut() + 'static) {
    LOOP_HANDLE.with(|h| {
        if let Some(handle) = h.borrow().as_ref() {
            let registered = handle.insert_source(
                Timer::from_duration(period),
                move |_instant, _meta, _state: &mut Driver| {
                    callback();
                    TimeoutAction::ToDuration(period)
                },
            );
            if let Ok(token) = registered {
                track_source(token);
            }
        }
    });
}

/// Runs `callback` once, `delay` from now, on the shared event loop, then drops the timer. Used for an OSD's
/// auto-dismiss. No-op when called outside a surface loop (e.g. a headless test).
pub fn timeout(delay: Duration, callback: impl FnOnce() + 'static) {
    LOOP_HANDLE.with(|h| {
        if let Some(handle) = h.borrow().as_ref() {
            let mut callback = Some(callback);
            let _ = handle.insert_source(
                Timer::from_duration(delay),
                move |_instant, _meta, _state: &mut Driver| {
                    if let Some(cb) = callback.take() {
                        cb();
                    }
                    TimeoutAction::Drop
                },
            );
        }
    });
}

pub struct EventSender<T> {
    channel: ChannelSender<T>,
    receiver: Weak<()>,
}

impl<T> EventSender<T> {
    pub fn send(&self, event: T) -> bool {
        self.channel.send(event).is_ok()
    }

    /// Whether the surface on the other end still exists — asked *without* sending anything.
    ///
    /// A failed `send` answers the same question and was the only way to ask it, which meant a service could
    /// not discover it was unwanted without first doing the work of a reading: the producer that nobody is
    /// listening to is precisely the one that must not take one. The strong half of this handle lives in the
    /// loop source's callback, so removing that source with its surface is what makes this `false`.
    pub fn alive(&self) -> bool {
        self.receiver.strong_count() > 0
    }
}

/// The receiving end of a [`detached`] subscription, held by the caller instead of by a loop source.
pub struct Subscription<T> {
    channel: Channel<T>,
    /// Never read — holding it is the point. The sender's weak twin dies when this drops, which is what makes
    /// `EventSender::alive` answer `false`.
    #[allow(dead_code)]
    receiver: Arc<()>,
}

impl<T> Subscription<T> {
    /// The next value the producer sent, or `None` if it has not sent one yet.
    pub fn try_recv(&self) -> Option<T> {
        self.channel.try_recv().ok()
    }
}

/// A subscription with no surface behind it: the caller holds the receiving end itself, and the producer's
/// sender reports itself dead once that end is dropped. What stands in for a surface where there is no event
/// loop to file one against — a test, or a producer consuming another service.
pub fn detached<T>() -> (EventSender<T>, Subscription<T>) {
    let (tx, rx) = channel::<T>();
    let receiver = Arc::new(());
    let sender = EventSender {
        channel: tx,
        receiver: Arc::downgrade(&receiver),
    };
    (
        sender,
        Subscription {
            channel: rx,
            receiver,
        },
    )
}

/// Runs `producer` on its own thread and delivers what it sends to `on_event` on the loop thread. Bound to the
/// surface that registered it: tearing that surface down removes the channel source, which drops the receiver
/// so the producer's next `send` fails and it winds itself down (every producer here checks that result), and
/// makes [`EventSender::alive`] answer `false` so a producer with nothing left to feed can retire before it
/// takes another reading.
///
/// Returns a handle to the registration for the app-level caller that has to be able to take it back — a watcher
/// installed from the config, which a reload may switch off. A caller inside a surface can ignore it: the
/// surface's own teardown already removes the source.
pub fn watch<T, P, F>(producer: P, mut on_event: F) -> Option<WatchToken>
where
    T: Send + 'static,
    P: FnOnce(EventSender<T>) + Send + 'static,
    F: FnMut(T) + 'static,
{
    LOOP_HANDLE.with(|h| {
        let handle = h.borrow();
        let handle = handle.as_ref()?;
        let (tx, rx) = channel::<T>();
        let receiver = Arc::new(());
        let sender = EventSender {
            channel: tx,
            receiver: Arc::downgrade(&receiver),
        };
        let _ = std::thread::Builder::new()
            .name("hyprshell-watch".to_string())
            .spawn(move || producer(sender));
        let registered = handle.insert_source(rx, move |event, _meta, _state: &mut Driver| {
            // Owned by this callback so it dies with the source: that is what `EventSender::alive` reads.
            let _ = &receiver;
            if let ChannelEvent::Msg(item) = event {
                on_event(item);
            }
        });
        let token = registered.ok()?;
        track_source(token);
        Some(WatchToken(token))
    })
}

/// A [`watch`] registration, so the caller that installed it can take it back.
pub struct WatchToken(RegistrationToken);

/// Removes a [`watch`], dropping the channel that fed it. The producer's next `send` fails and
/// `EventSender::alive` turns false, so the service behind it winds down too — which is the point: switching a
/// watcher off has to stop the thing it started, not just stop listening to it.
///
/// Must run on the driver thread, which is where every `watch` callback and every config reload already runs.
pub fn unwatch(token: WatchToken) {
    LOOP_HANDLE.with(|h| {
        if let Some(handle) = h.borrow().as_ref() {
            handle.remove(token.0);
        }
    });
}

/// Registers the app's reaction to the compositor's output set changing after startup — a monitor plugged in or
/// unplugged — so it can open bars on the new screen and drop the ones on the old. Fires on the driver thread.
pub fn on_outputs_changed(callback: impl Fn() + 'static) {
    OUTPUTS_CHANGED.with(|c| *c.borrow_mut() = Some(Box::new(callback)));
}

/// The compositor's outputs. On the driver thread this reads the live set the driver already tracks; anywhere
/// else (before the loop is up) it falls back to a throwaway connection via [`enumerate_outputs`].
pub fn outputs() -> Vec<OutputDescriptor> {
    let cached = OUTPUTS.with(|o| o.borrow().clone());
    if cached.is_empty() {
        return enumerate_outputs();
    }
    cached
}

#[derive(Default)]
pub struct LayerShellPlatform {
    configs: HashMap<SurfaceId, LayerConfig>,
    shutdown: Option<Arc<AtomicBool>>,
}

impl LayerShellPlatform {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_surface(mut self, id: SurfaceId, config: LayerConfig) -> Self {
        self.configs.insert(id, config);
        self
    }

    /// Shared shutdown flag: flipping tears down all surfaces for config reload.
    pub fn with_shutdown(mut self, flag: Arc<AtomicBool>) -> Self {
        self.shutdown = Some(flag);
        self
    }
}

impl MultiSurfacePlatform for LayerShellPlatform {
    type Window = LayerWindow;

    fn run_surfaces<H, F>(
        self,
        surfaces: Vec<(SurfaceId, WindowConfig)>,
        factory: F,
    ) -> Result<(), PlatformError>
    where
        H: EventHandler<LayerWindow> + 'static,
        F: Fn(SurfaceId) -> H + 'static,
    {
        run_driver(self.configs, self.shutdown, surfaces, factory)
    }
}

/// The shell object a surface is mounted through. Two roles share every other part of the driver — one
/// connection, one seat, one loop, the same rsx handler and the same `LayerWindow` bridging it to wgpu — and
/// differ only in which protocol object carries the surface and how it is configured.
pub(crate) enum Shell {
    Layer(LayerSurface),
    /// A session-lock surface, which owns its `wl_surface` directly (there is no SCTK wrapper for it).
    Lock {
        surface: wl_surface::WlSurface,
        lock: ExtSessionLockSurfaceV1,
    },
}

impl Shell {
    pub(crate) fn wl_surface(&self) -> &wl_surface::WlSurface {
        match self {
            Shell::Layer(layer) => layer.wl_surface(),
            Shell::Lock { surface, .. } => surface,
        }
    }

    fn commit(&self) {
        self.wl_surface().commit();
    }
}

/// A single mounted surface: its shell object, wgpu-bridging window, and (unless it is a reservation-only
/// strip) the rsx handler that renders it. All entries live on one thread and share one Wayland connection.
pub(crate) struct SurfaceEntry {
    pub(crate) shell: Shell,
    wl_id: ObjectId,
    window: Option<LayerWindow>,
    handler: Option<BoxedHandler>,
    // `Some` for a surface opened through a `SurfaceHandle` (its close flag and geometry channel); `None` for one the driver mounted itself — a lock surface — which only goes on the shared shutdown.
    link: Option<Arc<SurfaceLink>>,
    /// Timers and channel sources this surface registered (via `interval`/`watch`), removed from the loop when
    /// it is torn down so a closed drawer stops ticking instead of outliving its own signals.
    sources: SourceSink,
    /// What this surface's content asked to happen on its way out (via `on_close`), and for how long.
    exit: ExitSink,
    /// When a started exit transition runs out and the surface is torn down. `None` until it is asked to close.
    exit_deadline: Option<Instant>,
    /// The layer-shell namespace, so a diagnostic can name which surface an event reached.
    namespace: String,
    reserve_only: bool,
    interactive_input_region: bool,
    /// The scale to render at, in 120ths — `wp_fractional_scale_v1`'s own unit, and the only one that can carry
    /// the 1.25× and 1.5× a compositor rounds to 1 or 2 when it has to answer in whole numbers.
    scale_120: u32,
    /// The pair that makes a fractional scale renderable, and `None` together on a compositor without them: the
    /// viewport maps a device-pixel buffer back onto its logical size, and the scale object is what says which.
    viewport: Option<WpViewport>,
    fractional: Option<WpFractionalScaleV1>,
    logical_size: (u32, u32),
    /// The size or the scale moved and the buffer behind them has not caught up yet. Cleared once per turn by
    /// [`Self::apply_geometry`], which is the only thing that resizes what the renderer draws into.
    geometry_dirty: bool,
    configured: bool,
    resumed: bool,
    closed: bool,
    events: Vec<Event>,
    timeout: Option<Duration>,
    input_region: Vec<(i32, i32, i32, i32)>,
    reservation: Option<(SlotPool, Buffer, (u32, u32))>,
}

impl SurfaceEntry {
    /// A surface the driver mounts with no layer-shell configuration of its own — currently only a lock
    /// surface, whose size, anchoring and input are the compositor's to decide.
    pub(crate) fn new(
        shell: Shell,
        wl_id: ObjectId,
        handler: Option<BoxedHandler>,
        link: Option<Arc<SurfaceLink>>,
        namespace: String,
        scale: i32,
        logical_size: (u32, u32),
    ) -> Self {
        Self {
            shell,
            wl_id,
            window: None,
            handler,
            link,
            sources: SourceSink::default(),
            exit: ExitSink::default(),
            exit_deadline: None,
            namespace,
            reserve_only: false,
            interactive_input_region: false,
            scale_120: scale.max(1) as u32 * 120,
            viewport: None,
            fractional: None,
            logical_size,
            geometry_dirty: false,
            configured: false,
            resumed: false,
            closed: false,
            events: Vec::new(),
            timeout: None,
            input_region: Vec::new(),
            reservation: None,
        }
    }

    /// What the renderer draws at, as a multiplier of the logical size.
    fn scale(&self) -> f64 {
        f64::from(self.scale_120) / 120.0
    }

    /// The logical size in device pixels — the buffer the renderer has to fill.
    fn device_size(&self) -> (u32, u32) {
        (
            device_pixels(self.logical_size.0, self.scale_120),
            device_pixels(self.logical_size.1, self.scale_120),
        )
    }

    /// Tells the compositor how to put this surface's buffer on the screen.
    ///
    /// Two routes, and the fractional one is the reason the pair is bound together: a viewport whose
    /// destination is the *logical* size lets the buffer be any size at all, so 1.5× is a buffer 1.5× the
    /// logical size rather than a 1× buffer the compositor stretches. The protocol asks for a buffer scale of 1
    /// alongside it, since the destination already says everything about the mapping. Without the pair the only
    /// thing that can be said is a whole number, which is what a compositor at 1.5× rounds for us.
    fn map_buffer(&self) {
        let surface = self.shell.wl_surface();
        match &self.viewport {
            Some(viewport) => {
                surface.set_buffer_scale(1);
                let (width, height) = self.logical_size;
                viewport.set_destination(width.max(1) as i32, height.max(1) as i32);
            }
            None => surface.set_buffer_scale((self.scale_120 / 120).max(1) as i32),
        }
    }

    /// Adopts a compositor-decided size, and (once the first configure has been taken) tells the handler to
    /// re-lay-out. The buffer behind it is resized by [`Self::apply_geometry`] rather than here.
    pub(crate) fn apply_configure(&mut self, width: u32, height: u32) {
        self.logical_size = (width, height);
        self.geometry_dirty = true;
        if self.configured {
            self.events.push(Event::WindowResized { width, height });
        }
        self.configured = true;
    }

    /// Takes a new scale, or does nothing if it is the one already in use.
    ///
    /// The resize is pushed as well as the scale: the logical size has not moved, but the buffer behind it has,
    /// and a renderer told only that the scale changed would keep drawing at the old device size.
    fn rescale(&mut self, scale_120: u32) {
        if scale_120 == 0 || scale_120 == self.scale_120 {
            return;
        }
        self.scale_120 = scale_120;
        self.geometry_dirty = true;
        // The preferred scale usually lands before the first configure, where the size is still a placeholder
        // and the window is built from the scale rather than told about it — so there is nothing to tell yet.
        if !self.configured {
            return;
        }
        self.events.push(Event::ScaleFactorChanged {
            scale_factor: self.scale(),
        });
        let (width, height) = self.logical_size;
        self.events.push(Event::WindowResized { width, height });
    }

    /// Hands the size and scale the compositor last asked for to the surface and the renderer, once per turn.
    ///
    /// **One change arrives as two events** — `configure` carries the logical size and `preferred_scale` the
    /// scale — and acting on each as it lands is what makes a scale change expensive out of all proportion to
    /// it: the renderer is handed the new size at the old scale, throws away its swapchain and every texture
    /// sized to the old one to build them again, and is then handed the same size at the new scale and does it
    /// all a second time. Deferring to the turn also collapses a *burst* — a scale flipped back and forth, a
    /// monitor reconfigured — into the one resize its end state deserves, across every surface at once.
    fn apply_geometry(&mut self) {
        if !self.geometry_dirty {
            return;
        }
        self.geometry_dirty = false;
        self.map_buffer();
        let (device_width, device_height) = self.device_size();
        tracing::debug!(
            "{}: scale {}/120, {}×{} logical, {device_width}×{device_height} device",
            self.namespace,
            self.scale_120,
            self.logical_size.0,
            self.logical_size.1
        );
        if let Some(window) = &self.window {
            window.set_size(device_width, device_height);
            window.set_scale_factor(self.scale());
        }
    }

    /// Pushes whatever the surface asked to renegotiate since the last turn to the compositor. Only a layer
    /// surface has state of its own to renegotiate; a lock surface's is the compositor's to decide, which is the
    /// whole point of the protocol.
    ///
    /// A size change comes back as a `configure` and from there as a `WindowResized`, so the content is never
    /// resized by this call directly — it learns its new size the same way it learns about a monitor's.
    fn apply_update(&mut self, change: SurfaceUpdate) {
        let Shell::Layer(layer) = &self.shell else {
            return;
        };
        if let Some((width, height)) = change.size {
            layer.set_size(width, height);
        }
        if let Some((top, right, bottom, left)) = change.margin {
            layer.set_margin(top, right, bottom, left);
        }
        if let Some(zone) = change.exclusive_zone {
            layer.set_exclusive_zone(zone);
        }
        if let Some(anchor) = change.anchor {
            layer.set_anchor(anchor);
        }
        if let Some(shell_layer) = change.layer {
            // Restacking a mapped surface arrived in version 2 of the protocol, and sending a request an
            // object does not implement is a protocol error — which kills the whole connection, not the one
            // surface. On an older compositor the surface keeps the layer it was created on instead.
            match layer.kind() {
                SurfaceKind::Wlr(wlr) if wlr.version() >= 2 => layer.set_layer(shell_layer),
                _ => tracing::warn!(
                    "{}: this compositor's layer-shell cannot restack a mapped surface; \
                     restart for the change to take effect",
                    self.namespace
                ),
            }
        }
        if let Some(keyboard) = change.keyboard_interactivity {
            layer.set_keyboard_interactivity(keyboard);
        }
        self.commit_pending();
    }

    /// Applies what this thread has queued on the surface — or leaves it for the renderer's next frame to carry.
    ///
    /// **A `wl_surface` has one set of pending state and nothing guarding it.** The renderer commits from its
    /// own thread to present, so a commit from here can land between the buffer it attached and the commit it
    /// was about to make — taking its explicit-sync acquire point with no buffer of our own behind it, which
    /// the compositor answers with `wp_linux_drm_syncobj_surface_v1` error 3 and the death of the whole
    /// connection. Asking for a frame instead lets the one thread that owns the surface's buffer carry both,
    /// which costs a frame's delay on an input region or a renegotiated size and nothing else.
    ///
    /// A surface with no renderer — a reservation strip, or one that has not had its first frame — has no such
    /// thread, and commits here.
    fn commit_pending(&self) {
        match &self.window {
            Some(window) => window.request_redraw(),
            None => self.shell.commit(),
        }
    }

    /// Builds this surface's content again on the surface it is already on, and drops everything the outgoing
    /// content had registered against the loop.
    ///
    /// Those registrations are the whole reason a rebuild is more than one call: `interval` and `watch` file
    /// their sources against the surface, and the tree being replaced is about to register its own — so a
    /// rebuild that kept them would leave a clock ticking twice and a service feeding a tree nobody draws. Same
    /// for the exit transition, whose reactions animate widgets that no longer exist.
    fn rebuild(&mut self, window: &LayerWindow, loop_handle: &LoopHandle<'static, Driver>) {
        for token in self.sources.borrow_mut().drain(..) {
            loop_handle.remove(token);
        }
        *self.exit.borrow_mut() = ExitPlan::default();
        let link = self.link.clone();
        let sources = Rc::clone(&self.sources);
        let exit = Rc::clone(&self.exit);
        if let Some(handler) = self.handler.as_mut() {
            with_current(&link, &sources, &exit, || handler.remount(window));
        }
    }

    /// Whether a surface that has been asked to close should be torn down *now*.
    ///
    /// The first call is what starts its exit transition: the reactions its content registered with `on_close`
    /// run, and the surface stays mapped until their linger is up. Without one — nothing registered, or the
    /// user has animation switched off — this answers `true` immediately, which is what the driver did before
    /// any exit transition existed.
    fn exit_elapsed(&mut self) -> bool {
        if let Some(deadline) = self.exit_deadline {
            return Instant::now() >= deadline;
        }
        let plan = std::mem::take(&mut *self.exit.borrow_mut());
        if plan.is_empty() {
            return true;
        }
        self.exit_deadline = Some(Instant::now() + plan.linger());
        let link = self.link.clone();
        let sources = Rc::clone(&self.sources);
        let exit = Rc::clone(&self.exit);
        with_current(&link, &sources, &exit, || plan.run());
        false
    }

    /// How long the driver may sleep while this surface is on its way out. An exit is usually carried by an
    /// animation, which paces the loop on its own — but one that settles early (or never starts, because the
    /// reaction moved nothing) would otherwise let the loop sleep straight past the deadline and leave a
    /// closed surface on screen.
    fn exit_timeout(&self) -> Option<Duration> {
        self.exit_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }
}

/// A logical length in device pixels, at a scale given in 120ths.
///
/// Rounded half away from zero, which is the rule `wp_fractional_scale_v1` states rather than one chosen here:
/// a client that rounds the other way from its compositor hands over a buffer a row short of the destination
/// it declared, and gets it stretched back. The integer arithmetic is the same rule without the float — 60 is
/// half of 120.
fn device_pixels(logical: u32, scale_120: u32) -> u32 {
    (logical.saturating_mul(scale_120).saturating_add(60) / 120).max(1)
}

/// The single-thread driver: one Wayland connection's shared globals (registry/output/seat/shm) plus every
/// live surface. The SCTK delegate handlers route each event to its surface by `wl_surface` id.
pub(crate) struct Driver {
    registry_state: RegistryState,
    pub(crate) output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    modifiers: ModifiersState,
    // The surface currently holding keyboard focus, so key events route to the right handler.
    keyboard_focus: Option<ObjectId>,
    // The surface the pointer is currently over. Enter and leave are edges, not levels, so a surface that
    // rebuilds its content between them has to be told where the pointer already is.
    pointer_focus: Option<ObjectId>,
    pub(crate) surfaces: Vec<SurfaceEntry>,
    /// `None` where the compositor does not implement `ext-session-lock-v1`, which is what makes the shell
    /// refuse to lock rather than draw an overlay it cannot enforce.
    pub(crate) lock_manager: Option<ExtSessionLockManagerV1>,
    pub(crate) lock: Option<LockSession>,
    pub(crate) scaling: Option<Scaling>,
}

/// The two globals a surface needs to render on the device pixel grid, held together because either alone is
/// useless: a preferred scale with no viewport is a number nothing can act on, and a viewport with no scale to
/// put in it is a mapping with nothing to map.
pub(crate) struct Scaling {
    manager: WpFractionalScaleManagerV1,
    viewporter: WpViewporter,
}

/// What the shell can ask about this compositor before it commits to a feature. Read from any thread that has
/// gone through the driver, so a UI handler can grey out "lock" rather than fail on the attempt.
#[derive(Clone, Copy, Default)]
pub(crate) struct DriverFacts {
    pub(crate) lock_supported: bool,
}

thread_local! {
    static FACTS: RefCell<DriverFacts> = const { RefCell::new(DriverFacts { lock_supported: false }) };
}

pub(crate) fn with_driver_facts<R>(read: impl FnOnce(&DriverFacts) -> R) -> R {
    FACTS.with(|facts| read(&facts.borrow()))
}

impl Driver {
    fn entry_mut(&mut self, wl_id: &ObjectId) -> Option<&mut SurfaceEntry> {
        self.surfaces.iter_mut().find(|e| &e.wl_id == wl_id)
    }

    /// Gives a freshly created surface its scale and viewport objects, where the compositor has them.
    ///
    /// Called for every surface this driver mounts, lock surfaces included: a lock screen covers a whole output
    /// with text, which is the last place a shell can afford to hand over a buffer for the compositor to blur.
    pub(crate) fn attach_scaling(&self, entry: &mut SurfaceEntry, qh: &QueueHandle<Driver>) {
        if let Some(scaling) = &self.scaling {
            let surface = entry.shell.wl_surface();
            entry.viewport = Some(scaling.viewporter.get_viewport(surface, qh, ()));
            entry.fractional = Some(scaling.manager.get_fractional_scale(
                surface,
                qh,
                entry.wl_id.clone(),
            ));
        }
        entry.map_buffer();
    }

    /// The layer-shell namespace of the surface an event landed on, for diagnostics. `None` means the event
    /// named a surface this driver does not own.
    fn surface_namespace(&self, wl_id: &ObjectId) -> Option<&str> {
        self.surfaces
            .iter()
            .find(|e| &e.wl_id == wl_id)
            .map(|e| e.namespace.as_str())
    }

    fn descriptors(&mut self) -> Vec<OutputDescriptor> {
        let outputs: Vec<_> = self.output_state.outputs().collect();
        outputs
            .into_iter()
            .filter_map(|o| self.output_state.info(&o))
            .map(|info| OutputDescriptor {
                name: info.name,
                logical_size: info.logical_size,
                position: info.logical_position.unwrap_or(info.location),
                scale: info.scale_factor,
            })
            .collect()
    }

    /// Refreshes the cached output set and, once the shell is up, notifies the app when it actually changed so
    /// it can open bars on a newly connected monitor and drop the ones on a disconnected one.
    fn refresh_outputs(&mut self) {
        let next = self.descriptors();
        let changed = OUTPUTS.with(|o| {
            let mut cache = o.borrow_mut();
            let changed = names(&cache) != names(&next);
            *cache = next;
            changed
        });
        if changed {
            OUTPUTS_CHANGED.with(|c| {
                if let Some(callback) = c.borrow().as_ref() {
                    callback();
                }
            });
        }
    }
}

/// Outputs compared by name: the identity a `LayerConfig` pins a surface to, so a scale or resolution change
/// (which the surface handles through `configure`) doesn't trigger a full surface reconciliation.
fn names(outputs: &[OutputDescriptor]) -> Vec<Option<String>> {
    outputs.iter().map(|o| o.name.clone()).collect()
}

#[allow(clippy::too_many_arguments)]
fn create_surface_entry(
    driver: &mut Driver,
    compositor: &CompositorState,
    layer_shell: &LayerShell,
    qh: &QueueHandle<Driver>,
    config: &LayerConfig,
    handler: Option<BoxedHandler>,
    link: Option<Arc<SurfaceLink>>,
) {
    let output = config.output.as_deref().and_then(|name| {
        driver
            .output_state
            .outputs()
            .find(|o| driver.output_state.info(o).and_then(|i| i.name).as_deref() == Some(name))
    });
    let scale = output
        .as_ref()
        .and_then(|o| driver.output_state.info(o))
        .map(|i| i.scale_factor)
        .unwrap_or(1)
        .max(1);

    let surface = compositor.create_surface(qh);
    let layer = layer_shell.create_layer_surface(
        qh,
        surface,
        config.layer,
        Some(config.namespace.clone()),
        output.as_ref(),
    );
    layer.set_anchor(config.anchor);
    layer.set_size(config.size.0, config.size.1);
    layer.set_exclusive_zone(config.exclusive_zone);
    let (mt, mr, mb, ml) = config.margin;
    layer.set_margin(mt, mr, mb, ml);
    layer.set_keyboard_interactivity(config.keyboard_interactivity);
    // A fully click-through surface, and an interactive-region one before its first frame computes its rects,
    // both start with an empty input region so they never steal clicks from windows beneath.
    if (config.input_transparent || config.interactive_input_region)
        && let Ok(region) = Region::new(compositor)
    {
        layer
            .wl_surface()
            .set_input_region(Some(region.wl_region()));
    }

    let wl_id = layer.wl_surface().id();
    let mut entry = SurfaceEntry::new(
        Shell::Layer(layer),
        wl_id,
        handler,
        link,
        config.namespace.clone(),
        scale,
        (config.size.0.max(1), config.size.1.max(1)),
    );
    entry.reserve_only = config.reserve_only;
    entry.interactive_input_region = config.interactive_input_region;
    // Before the first commit, so the surface is never mapped under a mapping it is about to replace.
    driver.attach_scaling(&mut entry, qh);
    entry.shell.commit();
    driver.surfaces.push(entry);
}

fn run_driver<H, F>(
    configs: HashMap<SurfaceId, LayerConfig>,
    shutdown: Option<Arc<AtomicBool>>,
    surfaces: Vec<(SurfaceId, WindowConfig)>,
    factory: F,
) -> Result<(), PlatformError>
where
    H: EventHandler<LayerWindow> + 'static,
    F: Fn(SurfaceId) -> H + 'static,
{
    let conn = Connection::connect_to_env()
        .map_err(|e| PlatformError(format!("wayland connect failed: {e}")))?;
    let (globals, event_queue) = registry_queue_init::<Driver>(&conn)
        .map_err(|e| PlatformError(format!("registry init failed: {e}")))?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|e| PlatformError(format!("wl_compositor unavailable: {e}")))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|e| PlatformError(format!("zwlr_layer_shell_v1 unavailable: {e}")))?;
    let shm =
        Shm::bind(&globals, &qh).map_err(|e| PlatformError(format!("wl_shm unavailable: {e}")))?;
    // Optional by design: a compositor without either protocol still runs every bar and panel. The features
    // that need them ask first (`lock_supported`, `idle_supported`) rather than failing at the point of use.
    let lock_manager = globals
        .bind::<ExtSessionLockManagerV1, Driver, ()>(&qh, 1..=1, ())
        .inspect_err(|e| tracing::info!("ext-session-lock-v1 unavailable: {e}"))
        .ok();
    let idle_notifier = globals
        .bind::<ExtIdleNotifierV1, Driver, ()>(&qh, 1..=2, ())
        .inspect_err(|e| tracing::info!("ext-idle-notify-v1 unavailable: {e}"))
        .ok();
    // Both or neither, since neither is any use alone. A compositor without them still draws every surface —
    // through the whole-number buffer scale, which is what this replaces.
    let scaling = globals
        .bind::<WpFractionalScaleManagerV1, Driver, ()>(&qh, 1..=1, ())
        .and_then(|manager| {
            let viewporter = globals.bind::<WpViewporter, Driver, ()>(&qh, 1..=1, ())?;
            Ok(Scaling {
                manager,
                viewporter,
            })
        })
        .inspect_err(|e| tracing::info!("fractional scaling unavailable: {e}"))
        .ok();
    FACTS.with(|facts| facts.borrow_mut().lock_supported = lock_manager.is_some());

    let mut driver = Driver {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        keyboard: None,
        pointer: None,
        modifiers: ModifiersState::default(),
        keyboard_focus: None,
        pointer_focus: None,
        surfaces: Vec::new(),
        lock_manager,
        lock: None,
        scaling,
    };

    let mut event_loop: EventLoop<Driver> =
        EventLoop::try_new().map_err(|e| PlatformError(format!("calloop init failed: {e}")))?;
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| PlatformError(format!("wayland source insert failed: {e}")))?;

    // One ping wakes the shared loop; each frame re-drives every live surface (idle ones no-op internally).
    let (ping, ping_source) =
        make_ping().map_err(|e| PlatformError(format!("calloop ping failed: {e}")))?;
    loop_handle
        .insert_source(ping_source, |_, _, _: &mut Driver| {})
        .map_err(|e| PlatformError(format!("ping source insert failed: {e}")))?;

    LOOP_HANDLE.with(|h| *h.borrow_mut() = Some(loop_handle.clone()));
    set_surface_host(LayerShellSurfaceHost);

    // Prime the registry so outputs are known before matching `config.output` on surface creation.
    for _ in 0..3 {
        if event_loop
            .dispatch(Duration::from_millis(40), &mut driver)
            .is_err()
        {
            return Ok(());
        }
    }

    // After priming, since an idle notification is taken out against a seat and the seat only exists once the
    // registry has been round-tripped. Absent either half, idle timers report themselves as unsupported.
    if let (Some(notifier), Some(seat)) = (idle_notifier, driver.seat_state.seats().next()) {
        crate::idle::install(notifier, seat, qh.clone());
    }

    for (id, _window_config) in surfaces {
        let config = configs.get(&id).cloned().unwrap_or_default();
        let handler: Option<BoxedHandler> = if config.reserve_only {
            None
        } else {
            Some(Box::new(factory(id)))
        };
        create_surface_entry(
            &mut driver,
            &compositor,
            &layer_shell,
            &qh,
            &config,
            handler,
            None,
        );
    }

    // App-level setup that needs the driver thread (LOOP_HANDLE + SurfaceHost now installed): the popup host.
    for task in STARTUP.with(|s| std::mem::take(&mut *s.borrow_mut())) {
        task();
    }

    let mut next_timeout: Option<Duration> = Some(Duration::ZERO);
    loop {
        // Before the surface pass, so a lock taken during the last dispatch has its surfaces mounted — and an
        // unlock has them torn down — in this same turn rather than one frame late.
        crate::lock::poll(&mut driver, &compositor, &qh, &conn, &loop_handle);

        // Mount any dynamic surfaces requested since the last turn (drawers/OSDs opened via `open_surface`).
        let pending: Vec<PendingSurface> = DYN_QUEUE.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for p in pending {
            create_surface_entry(
                &mut driver,
                &compositor,
                &layer_shell,
                &qh,
                &p.config,
                p.handler,
                Some(p.link),
            );
        }

        // Bracket the dispatch in a reactive batch so signal writes from Wayland/calloop callbacks (an icon
        // download landing, a service update) are deferred and flushed once here, not synchronously mid-callback
        // — which under M3's shared runtime would re-enter a callback still holding a RefCell borrow. This
        // mirrors the winit runner bracketing each dispatch with the handler's new_events/about_to_wait.
        begin_batch();
        let dispatched = event_loop.dispatch(next_timeout, &mut driver);
        end_batch();
        if dispatched.is_err() {
            break;
        }
        if shutdown.as_ref().is_some_and(|f| f.load(Ordering::Relaxed)) {
            break;
        }

        let mut min_timeout: Option<Duration> = None;
        let mut remove: Vec<usize> = Vec::new();
        let display_ptr = NonNull::new(conn.backend().display_ptr() as *mut c_void);
        let Driver {
            surfaces,
            shm: shm_state,
            pointer_focus,
            ..
        } = &mut driver;
        for (index, entry) in surfaces.iter_mut().enumerate() {
            // The compositor closing the surface leaves nothing to play an exit onto, so that path skips the transition entirely: as far as the compositor is concerned the surface is already gone.
            if entry.closed {
                remove.push(index);
                continue;
            }
            if entry.link.as_ref().is_some_and(|link| link.is_closing()) && entry.exit_elapsed() {
                remove.push(index);
                continue;
            }
            if let Some(change) = entry.link.as_ref().and_then(|link| link.take_update()) {
                entry.apply_update(change);
            }
            if !entry.configured {
                continue;
            }
            entry.apply_geometry();
            if entry.reserve_only {
                commit_reservation(shm_state, entry);
                continue;
            }
            if entry.window.is_none() {
                let Some(display_ptr) = display_ptr else {
                    tracing::error!("null wayland display pointer (system backend missing?)");
                    remove.push(index);
                    continue;
                };
                let Some(surface_ptr) =
                    NonNull::new(entry.shell.wl_surface().id().as_ptr() as *mut c_void)
                else {
                    remove.push(index);
                    continue;
                };
                let (device_width, device_height) = entry.device_size();
                let ping = ping.clone();
                entry.window = Some(LayerWindow::new(
                    surface_ptr,
                    display_ptr,
                    device_width,
                    device_height,
                    entry.scale(),
                    move || ping.ping(),
                ));
            }
            let window = entry.window.clone().expect("window built above");

            // Taken whether or not it can be acted on: a rebuild asked for before the surface had ever been
            // mounted *is* the mount below, whose first build already reads whatever the request was about.
            let rebuild = entry.link.as_ref().is_some_and(|link| link.take_rebuild());

            if !entry.resumed {
                let link = entry.link.clone();
                let sources = Rc::clone(&entry.sources);
                let exit = Rc::clone(&entry.exit);
                let ok = with_current(&link, &sources, &exit, || {
                    let handler = entry
                        .handler
                        .as_mut()
                        .expect("rendering surface has a handler");
                    handler.new_events();
                    let resumed = handler.on_resume(&window);
                    handler.about_to_wait();
                    resumed
                });
                if !ok {
                    tracing::error!("layer surface on_resume failed (renderer init)");
                    remove.push(index);
                    continue;
                }
                entry.resumed = true;
            } else if rebuild {
                entry.rebuild(&window, &loop_handle);
                // The pointer does not enter a surface twice, so a rebuilt surface under it would otherwise
                // never hear that it is hovered — an auto-hidden bar rebuilt while it was out would slide away
                // under the cursor and stay there until the pointer left and came back.
                if pointer_focus.as_ref() == Some(&entry.wl_id) {
                    entry.events.push(Event::CursorEntered);
                }
            }

            let link = entry.link.clone();
            let sources = Rc::clone(&entry.sources);
            let exit = Rc::clone(&entry.exit);
            let events: Vec<Event> = entry.events.drain(..).collect();
            entry.timeout = with_current(&link, &sources, &exit, || {
                let handler = entry
                    .handler
                    .as_mut()
                    .expect("rendering surface has a handler");
                handler.new_events();
                for event in events {
                    handler.on_event(event, &window);
                }
                handler.on_redraw(&window);
                handler.about_to_wait()
            });
            if entry.interactive_input_region {
                let rects = entry
                    .handler
                    .as_ref()
                    .map(|handler| handler.interactive_rects())
                    .unwrap_or_default();
                if update_input_region(
                    &compositor,
                    entry.shell.wl_surface(),
                    &entry.namespace,
                    rects,
                    &mut entry.input_region,
                ) {
                    entry.commit_pending();
                }
            }
            min_timeout = merge_timeout(min_timeout, entry.timeout);
            min_timeout = merge_timeout(min_timeout, entry.exit_timeout());
        }

        for index in remove.into_iter().rev() {
            let entry = driver.surfaces.remove(index);
            tear_down(entry, &loop_handle);
        }

        LIVE_SURFACES.store(driver.surfaces.len(), Ordering::Relaxed);
        next_timeout = min_timeout;
    }

    for entry in driver.surfaces.drain(..) {
        tear_down(entry, &loop_handle);
    }
    Ok(())
}

/// Suspends a surface's handler and then removes every loop source it registered, so its timers and watch
/// channels stop with it. Dropping the channel receivers also ends the producer threads feeding them.
pub(crate) fn tear_down(mut entry: SurfaceEntry, loop_handle: &LoopHandle<'static, Driver>) {
    if let Some(mut handler) = entry.handler.take() {
        let sources = Rc::clone(&entry.sources);
        let exit = Rc::clone(&entry.exit);
        with_current(&entry.link, &sources, &exit, || handler.on_suspend());
    }
    for token in entry.sources.borrow_mut().drain(..) {
        loop_handle.remove(token);
    }
    // Both hang off the wl_surface and neither is freed by dropping its handle, so they go before it does:
    // every request on a viewport whose surface is gone is a protocol error, which kills the connection rather
    // than the surface.
    if let Some(viewport) = entry.viewport.take() {
        viewport.destroy();
    }
    if let Some(fractional) = entry.fractional.take() {
        fractional.destroy();
    }
    // A layer surface is destroyed by dropping SCTK's wrapper; a lock surface has no wrapper, so its two
    // protocol objects are released here — the role object first, as the protocol's ordering requires.
    if let Shell::Lock { surface, lock } = &entry.shell {
        lock.destroy();
        surface.destroy();
    }
}

fn merge_timeout(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// (Re)commits a fully-transparent shm buffer sized to the reservation strip so its exclusive_zone takes hold;
/// only rebuilds when the pixel size changed.
fn commit_reservation(shm: &Shm, entry: &mut SurfaceEntry) {
    let (w, h) = entry.device_size();
    if entry
        .reservation
        .as_ref()
        .is_some_and(|(_, _, size)| *size == (w, h))
    {
        return;
    }
    let stride = w as i32 * 4;
    let len = (h as usize) * (stride as usize);
    let mut pool = match SlotPool::new(len.max(1), shm) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("reservation surface: shm pool failed: {e}");
            return;
        }
    };
    let buffer = match pool.create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888) {
        Ok((buffer, _canvas)) => buffer,
        Err(e) => {
            tracing::error!("reservation surface: shm buffer failed: {e}");
            return;
        }
    };
    let surface = entry.shell.wl_surface();
    if buffer.attach_to(surface).is_ok() {
        surface.damage_buffer(0, 0, w as i32, h as i32);
        entry.shell.commit();
        entry.reservation = Some((pool, buffer, (w, h)));
    }
}

/// Rebuilds the surface's input region from its handler's pointer targets — the laid-out interactive widgets,
/// in logical surface coordinates — committing only when the set changed (`last` is the previously applied set,
/// sorted so a reordered read isn't mistaken for a change). An empty set yields an empty region, i.e. fully
/// click-through, so an overlay with no interactive content never blocks the windows beneath.
///
/// The rects come from the handler rather than from the global `telar::interactive_rects`, and that is the whole
/// correctness of this function: the registry is one of the handler's *per-surface* worlds, live only inside
/// its own calls. Read from out here — after the handler has returned — the ambient world answers, and it is
/// always empty, so every surface using this was click-through everywhere.
///
/// Returns whether the region moved, since the surface it was set on still has to be committed — which is the
/// caller's to do, and not from this thread while a renderer owns the surface (see
/// [`SurfaceEntry::commit_pending`]).
fn update_input_region(
    compositor: &CompositorState,
    surface: &wl_surface::WlSurface,
    namespace: &str,
    rects: Vec<telar::Rect>,
    last: &mut Vec<(i32, i32, i32, i32)>,
) -> bool {
    let mut rects: Vec<(i32, i32, i32, i32)> = rects
        .into_iter()
        .map(|r| {
            let x = r.x.floor() as i32;
            let y = r.y.floor() as i32;
            let right = (r.x + r.width).ceil() as i32;
            let bottom = (r.y + r.height).ceil() as i32;
            (x, y, right - x, bottom - y)
        })
        .collect();
    rects.sort_unstable();
    if rects == *last {
        return false;
    }
    // What distinguishes "the compositor is not delivering to us" from "we told it not to": zero rects means no pointer input at all.
    tracing::debug!(
        "input region for {namespace}: {} rect(s) {rects:?}",
        rects.len()
    );
    let Ok(region) = Region::new(compositor) else {
        return false;
    };
    for (x, y, w, h) in &rects {
        region.add(*x, *y, *w, *h);
    }
    surface.set_input_region(Some(region.wl_region()));
    *last = rects;
    true
}

pub(crate) struct NoPaths;
impl telar::AppPathsProvider for NoPaths {
    fn config_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn data_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn cache_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// A live dynamically-opened surface. Dropping it — or calling [`close`](Self::close) — asks the driver to tear it down.
pub struct SurfaceHandle {
    link: Arc<SurfaceLink>,
}

impl SurfaceHandle {
    /// Asks the surface to close. Returns immediately; the driver tears it down on its next loop turn, or once
    /// the exit transition the surface registered with [`on_close`] has played out. Deliberately non-blocking so
    /// a UI event handler can close a drawer without stalling.
    pub fn close(&self) {
        self.link.request_close();
    }

    /// Whether this surface has been asked to close (by `close`, drop, or the surface closing itself via `request_close`). Lets the owner reconcile its own toggle state after a self-close.
    pub fn is_closing(&self) -> bool {
        self.link.is_closing()
    }

    /// Builds the surface's content again, in place: the window, its renderer and its position are kept, and
    /// only the tree is built anew — from whatever the app reads now.
    ///
    /// This is how a surface follows something that changed underneath it (a config file the user edited)
    /// without being replaced by a new one. Non-blocking, and coalesced: several requests between two loop
    /// turns are one rebuild.
    pub fn rebuild(&self) {
        self.link.request_rebuild();
    }

    /// Renegotiates any part of the surface's layer-shell state in one commit — the shape a caller
    /// reconciling a surface against a changed configuration wants, rather than one request per field.
    pub fn update(&self, change: SurfaceUpdate) {
        self.link.request_update(change);
    }

    /// Asks the compositor to renegotiate the surface's size. `0` on an axis hands that axis back to it.
    pub fn set_size(&self, width: u32, height: u32) {
        self.link.request_update(SurfaceUpdate::size(width, height));
    }

    /// Moves the surface relative to the edges it is anchored to, as `(top, right, bottom, left)` logical
    /// pixels. Only an edge the surface is anchored to honours its margin; a negative value pushes it off that
    /// edge, which is how an auto-hidden bar keeps a hover strip on screen and nothing else.
    pub fn set_margin(&self, margin: (i32, i32, i32, i32)) {
        self.link.request_update(SurfaceUpdate::margin(margin));
    }

    /// Changes how much of the screen the surface reserves for itself. `0` reserves nothing (windows tile under
    /// it), `-1` opts out of every other surface's reservation, and a positive value is a logical-pixel strip.
    pub fn set_exclusive_zone(&self, zone: i32) {
        self.link
            .request_update(SurfaceUpdate::exclusive_zone(zone));
    }
}

impl Drop for SurfaceHandle {
    fn drop(&mut self) {
        self.link.request_close();
    }
}

impl SurfaceControl for SurfaceHandle {
    fn close(&self) {
        SurfaceHandle::close(self);
    }
    fn is_closing(&self) -> bool {
        SurfaceHandle::is_closing(self)
    }
    fn rebuild(&self) {
        SurfaceHandle::rebuild(self);
    }
}

/// Opens a new layer-shell surface at runtime. Builds the handler on the UI thread and enqueues it for the
/// driver to mount on its next loop turn — no new thread, so it shares the one reactive runtime (M3).
pub fn open_surface<A: App + 'static>(spec: LayerConfig, app: A) -> SurfaceHandle {
    let link = Arc::new(SurfaceLink::default());
    let handler = build_surface_handler::<LayerWindow, A>(app, Box::new(NoPaths), "hyprshell");
    DYN_QUEUE.with(|q| {
        q.borrow_mut().push(PendingSurface {
            config: spec,
            handler: Some(handler),
            link: Arc::clone(&link),
        })
    });
    SurfaceHandle { link }
}

/// Opens a reservation-only strip (no rsx content — just its exclusive zone, an invisible transparent buffer),
/// closeable like any dynamic surface. Used to reserve bar space so the strip and the visible bar are
/// independent surfaces (see the bar/reservation split), reconcilable on config reload without a full teardown.
pub fn open_reservation(spec: LayerConfig) -> SurfaceHandle {
    let link = Arc::new(SurfaceLink::default());
    DYN_QUEUE.with(|q| {
        q.borrow_mut().push(PendingSurface {
            config: spec,
            handler: None,
            link: Arc::clone(&link),
        })
    });
    SurfaceHandle { link }
}

/// Maps rsx's backend-agnostic [`SurfaceAnchor`] to layer-shell edge flags. `Center` anchors to no edge, so
/// the compositor centres the surface.
fn anchor_flags(anchor: SurfaceAnchor) -> Anchor {
    match anchor {
        SurfaceAnchor::Top => Anchor::TOP,
        SurfaceAnchor::Bottom => Anchor::BOTTOM,
        SurfaceAnchor::Left => Anchor::LEFT,
        SurfaceAnchor::Right => Anchor::RIGHT,
        SurfaceAnchor::Center => Anchor::empty(),
    }
}

/// Derives the layer-shell surface config from a [`SurfacePlacement`]. A placement needing a scaffold
/// (scrim or outside-dismiss) becomes a full-screen surface the `SurfaceScaffold` positions its panel
/// within; a directly-anchored one is sized and anchored by the compositor.
fn layer_config_for(placement: &SurfacePlacement) -> LayerConfig {
    let namespace = match placement.role {
        SurfaceRole::Drawer => "hyprshell-drawer",
        SurfaceRole::Popup => "hyprshell-popup",
        SurfaceRole::Osd => "hyprshell-osd",
        SurfaceRole::Float => "hyprshell-float",
        SurfaceRole::Overlay => "hyprshell-overlay",
    }
    .to_string();
    // `exclusive` is an input grab, not merely a keyboard one: while such a surface is up the compositor stops
    // delivering pointer events to every other surface, the shell's own bar included. Only a surface selecting
    // part of the screen wants that — see `Placement::modal` for the launcher, which does not.
    let keyboard_interactivity = match placement.keyboard {
        KeyboardMode::None => KeyboardInteractivity::None,
        KeyboardMode::OnDemand => KeyboardInteractivity::OnDemand,
        KeyboardMode::Exclusive => KeyboardInteractivity::Exclusive,
    };
    if placement.needs_scaffold() {
        LayerConfig {
            output: placement.output.clone(),
            layer: Layer::Overlay,
            anchor: Anchor::TOP
                .union(Anchor::BOTTOM)
                .union(Anchor::LEFT)
                .union(Anchor::RIGHT),
            exclusive_zone: 0,
            size: (0, 0),
            margin: (0, 0, 0, 0),
            keyboard_interactivity,
            namespace,
            reserve_only: false,
            input_transparent: false,
            interactive_input_region: false,
        }
    } else {
        let size = match placement.size {
            SurfaceSize::Fixed(w, h) => (w, h),
            SurfaceSize::Auto => (0, 0),
        };
        LayerConfig {
            output: placement.output.clone(),
            layer: Layer::Overlay,
            anchor: anchor_flags(placement.anchor),
            exclusive_zone: 0,
            size,
            margin: placement.margin,
            keyboard_interactivity,
            namespace,
            reserve_only: false,
            input_transparent: placement.input_transparent,
            interactive_input_region: false,
        }
    }
}

/// The internal rsx app for a hosted secondary surface: builds the content, wraps it in the placement's
/// scaffold (scrim + outside-dismiss) or a plain full-surface root, and arms an auto-dismiss timer when the
/// placement asks for one. The auto-dismiss captures this surface's own close flag directly, so it fires
/// regardless of which surface is current when the timer elapses.
///
/// What is held here rather than built per call is what belongs to the *surface* rather than to one build of
/// its content, and it is what makes a rebuild a rebuild: the app outlives the tree, so an entrance played
/// once stays played and a dismissal armed once stays armed.
struct HostedSurfaceApp {
    placement: SurfacePlacement,
    content: SurfaceContent,
    link: Arc<SurfaceLink>,
    transition: RefCell<Option<SurfaceTransition>>,
    dismiss_armed: std::cell::Cell<bool>,
}

impl App for HostedSurfaceApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let content = (self.content)();
        // Armed once for the surface, not once per build: a rebuilt OSD still goes away when it was always
        // going to, rather than starting its countdown over — or running two.
        if let Some(delay) = self.placement.timeout
            && !self.dismiss_armed.replace(true)
        {
            let link = Arc::clone(&self.link);
            timeout(delay, move || link.request_close());
        }
        // One transition drives both halves: it runs to 1 as the surface opens, and `on_close` runs it back to 0 while the driver holds the surface mapped for exactly as long as that takes.
        //
        // Kept across rebuilds, because rebuilding content is not a second arrival: a fresh transition would
        // replay the slide-in, so every edit to the config would look like the panel closing and opening.
        let transition = self
            .transition
            .borrow_mut()
            .get_or_insert_with(SurfaceTransition::enter)
            .clone();
        on_close(transition.duration(), {
            let transition = transition.clone();
            move || transition.leave()
        });
        if self.placement.needs_scaffold() {
            let dismiss: Option<Rc<dyn Fn()>> = self.placement.dismiss_on_outside.then(|| {
                let link = Arc::clone(&self.link);
                Rc::new(move || link.request_close()) as Rc<dyn Fn()>
            });
            Box::new(
                SurfaceScaffold::new(
                    scaffold_edge(self.placement.anchor),
                    scaffold_align(self.placement.align),
                    self.placement.margin,
                    self.placement.scrim.then_some(telar::DEFAULT_SCRIM),
                    dismiss,
                    content,
                )
                .expect("surface scaffold build failed")
                .animate(transition),
            )
        } else {
            Box::new(
                SurfaceRoot::new(content)
                    .expect("surface root build failed")
                    .animate(transition),
            )
        }
    }

    fn window_config(&self) -> Option<WindowConfig> {
        // The scaffold paints its own scrim; the surface itself stays transparent so the compositor blends it.
        Some(WindowConfig {
            is_transparent: true,
            ..WindowConfig::default()
        })
    }

    fn clear_color(&self) -> Option<Color> {
        None
    }
}

/// Installed once so the shell's rsx world can open drawers/OSDs/popups via `telar::open_surface`.
struct LayerShellSurfaceHost;

/// Lowers this backend's own anchor/align onto what the framework scaffold takes.
fn scaffold_edge(anchor: SurfaceAnchor) -> Edge {
    match anchor {
        SurfaceAnchor::Top => Edge::Top,
        SurfaceAnchor::Bottom => Edge::Bottom,
        SurfaceAnchor::Left => Edge::Left,
        SurfaceAnchor::Right => Edge::Right,
        SurfaceAnchor::Center => Edge::Center,
    }
}

fn scaffold_align(align: SurfaceAlign) -> AlignItems {
    match align {
        SurfaceAlign::Start => AlignItems::START,
        SurfaceAlign::Center => AlignItems::CENTER,
        SurfaceAlign::End => AlignItems::END,
    }
}

impl SurfaceHost<SurfacePlacement> for LayerShellSurfaceHost {
    fn open(&self, placement: SurfacePlacement, content: SurfaceContent) -> SurfaceToken {
        let config = layer_config_for(&placement);
        let link = Arc::new(SurfaceLink::default());
        let app = HostedSurfaceApp {
            placement,
            content,
            link: Arc::clone(&link),
            transition: RefCell::new(None),
            dismiss_armed: std::cell::Cell::new(false),
        };
        let handler = build_surface_handler::<LayerWindow, _>(app, Box::new(NoPaths), "hyprshell");
        DYN_QUEUE.with(|q| {
            q.borrow_mut().push(PendingSurface {
                config,
                handler: Some(handler),
                link: Arc::clone(&link),
            })
        });
        SurfaceToken::new(Box::new(SurfaceHandle { link }))
    }
}

fn map_button(code: u32) -> Option<PointerButton> {
    // Codes from linux/input-event-codes.h — not immediately obvious why these specific hex values.
    match code {
        0x110 => Some(PointerButton::Primary),
        0x111 => Some(PointerButton::Secondary),
        0x112 => Some(PointerButton::Auxiliary),
        _ => None,
    }
}

fn map_key(event: &KeyEvent) -> Option<Key> {
    // Editing keys carry a control-char `utf8` (or none), so they must be resolved from the keysym — the
    // printable `utf8` path below drops them.
    if let Some(named) = named_from_keysym(event.keysym) {
        return Some(Key::Named(named));
    }
    // Printable characters: take the resolved UTF-8, dropping any remaining control char.
    let ch = event.utf8.as_deref()?.chars().next()?;
    if ch.is_control() {
        return None;
    }
    Some(Key::Char(ch))
}

/// Maps an xkb keysym to the editing/navigation [`NamedKey`] it represents, or `None` for keys that carry
/// their own printable character. Mirrors platform-winit's named-key mapping, over xkb keysyms.
fn named_from_keysym(keysym: Keysym) -> Option<NamedKey> {
    match keysym {
        Keysym::Return | Keysym::KP_Enter => Some(NamedKey::Enter),
        Keysym::BackSpace => Some(NamedKey::Backspace),
        Keysym::Escape => Some(NamedKey::Escape),
        Keysym::Tab | Keysym::ISO_Left_Tab => Some(NamedKey::Tab),
        Keysym::Delete => Some(NamedKey::Delete),
        Keysym::Left => Some(NamedKey::ArrowLeft),
        Keysym::Right => Some(NamedKey::ArrowRight),
        Keysym::Up => Some(NamedKey::ArrowUp),
        Keysym::Down => Some(NamedKey::ArrowDown),
        Keysym::Home => Some(NamedKey::Home),
        Keysym::End => Some(NamedKey::End),
        Keysym::Page_Up => Some(NamedKey::PageUp),
        Keysym::Page_Down => Some(NamedKey::PageDown),
        Keysym::Insert => Some(NamedKey::Insert),
        _ => None,
    }
}

impl CompositorHandler for Driver {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let id = surface.id();
        let Some(entry) = self.entry_mut(&id) else {
            return;
        };
        // The whole-number scale keeps arriving alongside the fractional one and says less: a 1.5× output
        // announces 2 here. Taking it would resize the buffer to something the viewport then squeezes.
        if entry.fractional.is_some() {
            return;
        }
        entry.rescale(new_factor.max(1) as u32 * 120);
    }
    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
    }
    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

/// The event this whole pair exists for: the scale the compositor actually wants, in 120ths.
///
/// It arrives before the first configure and again whenever the surface moves to an output at another scale,
/// so it is also what a surface dragged between a 1× and a 1.5× monitor redraws on.
impl Dispatch<WpFractionalScaleV1, ObjectId> for Driver {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        wl_id: &ObjectId,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };
        if let Some(entry) = state.entry_mut(wl_id) {
            entry.rescale(scale);
        }
    }
}

// The other three carry no events at all: two are factories and a viewport is written to and never read.
delegate_noop!(Driver: ignore WpFractionalScaleManagerV1);
delegate_noop!(Driver: ignore WpViewporter);
delegate_noop!(Driver: ignore WpViewport);

impl OutputHandler for Driver {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh_outputs();
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh_outputs();
    }
    fn output_destroyed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        // Before the refresh, so a monitor unplugged and plugged back in gets a fresh lock surface instead of
        // being skipped as one this session already covered.
        crate::lock::forget_output(self, &output);
        self.refresh_outputs();
    }
}

impl LayerShellHandler for Driver {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        let id = layer.wl_surface().id();
        if let Some(entry) = self.entry_mut(&id) {
            entry.closed = true;
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let id = layer.wl_surface().id();
        let Some(entry) = self.entry_mut(&id) else {
            return;
        };
        // configure sizes are LOGICAL. `0` on an axis means the compositor left it to us — keep the last value.
        let (mut lw, mut lh) = configure.new_size;
        if lw == 0 {
            lw = entry.logical_size.0.max(1);
        }
        if lh == 0 {
            lh = entry.logical_size.1.max(1);
        }
        entry.apply_configure(lw, lh);
    }
}

impl SeatHandler for Driver {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(kb) = self.keyboard.take()
        {
            kb.release();
        }
        if capability == Capability::Pointer
            && let Some(ptr) = self.pointer.take()
        {
            ptr.release();
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for Driver {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.keyboard_focus = Some(surface.id());
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if self.keyboard_focus.as_ref() == Some(&surface.id()) {
            self.keyboard_focus = None;
        }
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        let modifiers = self.modifiers;
        if let (Some(key), Some(id)) = (map_key(&event), self.keyboard_focus.clone())
            && let Some(entry) = self.entry_mut(&id)
        {
            entry.events.push(Event::KeyPressed { key, modifiers });
        }
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        let modifiers = self.modifiers;
        if let (Some(key), Some(id)) = (map_key(&event), self.keyboard_focus.clone())
            && let Some(entry) = self.entry_mut(&id)
        {
            entry.events.push(Event::KeyReleased { key, modifiers });
        }
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        let modifiers = self.modifiers;
        if let (Some(key), Some(id)) = (map_key(&event), self.keyboard_focus.clone())
            && let Some(entry) = self.entry_mut(&id)
        {
            entry.events.push(Event::KeyPressed { key, modifiers });
        }
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = ModifiersState {
            is_shift: modifiers.shift,
            is_ctrl: modifiers.ctrl,
            is_alt: modifiers.alt,
            is_meta: modifiers.logo,
        };
    }
}

impl PointerHandler for Driver {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let id = event.surface.id();
            let (x, y) = event.position;
            let telar_event = match event.kind {
                // An enter carries the pointer's position and a widget resolves its hover from a move, so delivering it as bare "the cursor is over this surface" leaves a pointer that arrives and stops hovering nothing. `CursorEntered` is still emitted first, for whatever tracks the surface rather than the widget.
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    Event::PointerMoved {
                        x,
                        y,
                        source: PointerSource::Mouse,
                    }
                }
                PointerEventKind::Leave { .. } => Event::CursorLeft,
                PointerEventKind::Press { button, .. } => {
                    let Some(button) = map_button(button) else {
                        continue;
                    };
                    // Whether a press reached a surface at all is the one thing that distinguishes "our input
                    // region is wrong" from "the compositor acted on an event it also delivered to us". Logged
                    // at debug so `RUST_LOG=platform_wayland=debug` can answer it without a custom build.
                    tracing::debug!(
                        "pointer press {button:?} at ({x:.0},{y:.0}) delivered to surface {:?}",
                        self.surface_namespace(&id)
                    );
                    Event::PointerPressed {
                        x,
                        y,
                        button,
                        source: PointerSource::Mouse,
                    }
                }
                PointerEventKind::Release { button, .. } => {
                    let Some(button) = map_button(button) else {
                        continue;
                    };
                    Event::PointerReleased {
                        x,
                        y,
                        button,
                        source: PointerSource::Mouse,
                    }
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => Event::Scrolled {
                    // Wayland axis is positive down/right; negate to the winit convention the shared scroll area expects (`offset -= delta`) so the view tracks the gesture.
                    delta: ScrollDelta::Pixels {
                        x: -(horizontal.absolute as f32),
                        y: -(vertical.absolute as f32),
                    },
                    // The wheel belongs to whatever is under it, so the scroll area needs where it happened.
                    x,
                    y,
                },
            };
            match event.kind {
                PointerEventKind::Enter { .. } => self.pointer_focus = Some(id.clone()),
                PointerEventKind::Leave { .. } if self.pointer_focus.as_ref() == Some(&id) => {
                    self.pointer_focus = None
                }
                _ => {}
            }
            if let Some(entry) = self.entry_mut(&id) {
                if matches!(event.kind, PointerEventKind::Enter { .. }) {
                    entry.events.push(Event::CursorEntered);
                }
                entry.events.push(telar_event);
            }
        }
    }
}

impl ProvidesRegistryState for Driver {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl ShmHandler for Driver {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(Driver);
delegate_output!(Driver);
delegate_layer!(Driver);
delegate_seat!(Driver);
delegate_keyboard!(Driver);
delegate_pointer!(Driver);
delegate_shm!(Driver);
delegate_registry!(Driver);

struct OutputEnumState {
    registry_state: RegistryState,
    output_state: OutputState,
}

impl OutputHandler for OutputEnumState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for OutputEnumState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_output!(OutputEnumState);
delegate_registry!(OutputEnumState);

pub fn enumerate_outputs() -> Vec<OutputDescriptor> {
    let Ok(conn) = Connection::connect_to_env() else {
        return Vec::new();
    };
    let Ok((globals, mut event_queue)) = registry_queue_init::<OutputEnumState>(&conn) else {
        return Vec::new();
    };
    let qh = event_queue.handle();
    let mut state = OutputEnumState {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
    };
    for _ in 0..2 {
        if event_queue.roundtrip(&mut state).is_err() {
            break;
        }
    }
    state
        .output_state
        .outputs()
        .filter_map(|o| state.output_state.info(&o))
        .map(|info| OutputDescriptor {
            name: info.name,
            logical_size: info.logical_size,
            position: info.logical_position.unwrap_or(info.location),
            scale: info.scale_factor,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `on_close`, `request_close` and `request_size` all name "the current surface", and outside one there is
    /// no such thing. A silent no-op is the right answer — a preview render, a unit test and app-level setup all
    /// run this code with no surface installed — but it has to be a no-op rather than a panic, and an exit
    /// registered outside a surface must not leak into whichever surface runs next.
    #[test]
    fn asking_the_current_surface_for_anything_outside_one_does_nothing() {
        request_close();
        request_size(400, 300);
        request_margin((0, 0, 0, 0));
        on_close(Duration::from_millis(200), || {
            panic!("an exit registered outside a surface has nothing to belong to")
        });

        let exit = ExitSink::default();
        with_current(&None, &SourceSink::default(), &exit, || {});
        assert!(
            exit.borrow().is_empty(),
            "the reaction registered before any surface was current must not be picked up by one"
        );
    }

    /// Both halves of a hosted surface's exit — the scaffold fading its scrim, the panel content sliding back
    /// toward its bar edge — register against the same surface, and the driver has to wait for the longer.
    #[test]
    fn every_exit_a_surface_registers_reaches_its_plan() {
        let fired = Rc::new(RefCell::new(Vec::new()));
        let exit = ExitSink::default();
        with_current(&None, &SourceSink::default(), &exit, || {
            for (name, ms) in [("scaffold", 200u64), ("panel", 320)] {
                let fired = Rc::clone(&fired);
                on_close(Duration::from_millis(ms), move || {
                    fired.borrow_mut().push(name)
                });
            }
        });

        let plan = std::mem::take(&mut *exit.borrow_mut());
        assert_eq!(plan.linger(), Duration::from_millis(320));
        plan.run();
        assert_eq!(*fired.borrow(), vec!["scaffold", "panel"]);
    }

    /// The whole point of the pair: the scales a whole number cannot say.
    #[test]
    fn a_fractional_scale_reaches_the_buffer_it_asks_for() {
        assert_eq!(device_pixels(1000, 120), 1000, "1×");
        assert_eq!(device_pixels(1000, 180), 1500, "1.5×");
        assert_eq!(device_pixels(1000, 150), 1250, "1.25×");
        assert_eq!(device_pixels(1000, 240), 2000, "2×");
        // What the integer buffer scale did instead, and the reason this exists: the same 1.5× output rounds
        // to 2 there, so the buffer was a third larger than the screen and the compositor scaled it back down.
        assert_ne!(device_pixels(1000, 180), device_pixels(1000, 240));
    }

    #[test]
    fn a_size_that_does_not_land_on_a_pixel_rounds_the_way_the_compositor_does() {
        // Half away from zero, per the protocol. Rounding down here would leave the last row of the surface
        // outside the buffer that has to fill it.
        assert_eq!(device_pixels(31, 180), 47, "46.5 rounds up");
        assert_eq!(device_pixels(33, 180), 50, "49.5 rounds up");
        assert_eq!(device_pixels(7, 150), 9, "8.75 rounds up");
        assert_eq!(device_pixels(9, 150), 11, "11.25 rounds down");
        // A surface can be configured to nothing on an axis it does not own; a zero-sized buffer is not a
        // buffer, and every renderer behind this asks for at least one pixel.
        assert_eq!(device_pixels(0, 180), 1);
    }

    /// Whether this compositor can be asked for a fractional scale at all — the one half of this a unit test
    /// cannot answer, since the fallback is silent by design and looks like success from inside.
    /// `HYPRSHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland advertises_fractional -- --nocapture`
    #[test]
    fn advertises_fractional_scaling() {
        if std::env::var("HYPRSHELL_WAYLAND_LIVE").is_err() {
            eprintln!("set HYPRSHELL_WAYLAND_LIVE to ask the real compositor; skipping");
            return;
        }
        let interfaces = ["wp_fractional_scale_manager_v1", "wp_viewporter"];
        for interface in interfaces {
            println!("{interface}: {:?}", crate::advertises(interface));
        }
        assert_eq!(
            crate::advertises_all(&interfaces),
            Some(true),
            "this compositor cannot be asked for a fractional scale; surfaces fall back to whole numbers"
        );
    }

    #[test]
    fn editing_keysyms_map_to_named_keys() {
        assert_eq!(named_from_keysym(Keysym::Return), Some(NamedKey::Enter));
        assert_eq!(named_from_keysym(Keysym::KP_Enter), Some(NamedKey::Enter));
        assert_eq!(
            named_from_keysym(Keysym::BackSpace),
            Some(NamedKey::Backspace)
        );
        assert_eq!(named_from_keysym(Keysym::Escape), Some(NamedKey::Escape));
        assert_eq!(named_from_keysym(Keysym::Tab), Some(NamedKey::Tab));
        assert_eq!(named_from_keysym(Keysym::Left), Some(NamedKey::ArrowLeft));
        assert_eq!(named_from_keysym(Keysym::Right), Some(NamedKey::ArrowRight));
    }
}
