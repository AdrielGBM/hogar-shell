use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use telar::{
    App, Cursor, Event, EventHandler, Key, LocalApp, ModifiersState, MultiSurfacePlatform, NamedKey,
    PlatformError, PointerButton, PointerSource, ScrollDelta, SurfaceId, Window, WindowConfig,
    begin_batch, build_surface_handler, end_batch,
};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, Region};
use smithay_client_toolkit::globals::ProvidesBoundGlobal;
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
use smithay_client_toolkit::reexports::protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
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
use wayland_client::protocol::{
    wl_buffer, wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::{
    self, ExtBackgroundEffectManagerV1,
};
use wayland_protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    self, WpCursorShapeDeviceV1,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::config::{Layer, LayerConfig, OutputDescriptor};
use crate::layer_window::{LayerWindowHandle, Mapping, Transition};
use crate::link::{SurfaceLink, SurfaceUpdate};
use crate::lock::LockSession;
use crate::lock_notify::CompositorLock;
use crate::window::LayerWindow;

/// The type driven every surface handler is boxed to, so one loop holds every layer window, reservation strip and lock surface in one `Vec` (the blanket `EventHandler for Box<dyn EventHandler>` makes the box callable). All surfaces share this UI thread; isolation is the handler's own `ui_core::Surface`.
pub(crate) type BoxedHandler = Box<dyn EventHandler<LayerWindow>>;

/// The calloop sources (timers, channels) a surface registered while its handler ran, removed together when the surface is torn down. Shared by `Rc` so `with_current` can hand the sink to `interval`/`watch` without borrowing the driver's `SurfaceEntry`.
type SourceSink = Rc<RefCell<Vec<RegistrationToken>>>;

thread_local! {
    static LOOP_HANDLE: RefCell<Option<LoopHandle<'static, Driver>>> = const { RefCell::new(None) };
    // Where `interval`/`watch` file their registration tokens while a surface's handler runs, so the driver can drop them with that surface. `None` outside a surface (app-level setup), where sources are process-lived.
    static CURRENT_SOURCES: RefCell<Option<SourceSink>> = const { RefCell::new(None) };
    // Surfaces opened on the UI thread (layer windows and reservation strips); the driver drains and mounts them.
    static DYN_QUEUE: Pending = const { Pending(RefCell::new(Vec::new())) };
    // App-level setup to run once on the driver thread after the loop is up (see `run_on_start`).
    static STARTUP: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
    // The driver's live view of the compositor's outputs, so `outputs()` needs no second Wayland connection.
    static OUTPUTS: RefCell<Vec<OutputDescriptor>> = const { RefCell::new(Vec::new()) };
    // Notified when the output set changes once the shell is up, so the app can reconcile its surfaces (hotplug).
    static OUTPUTS_CHANGED: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    // The seat pointer's `wp-cursor-shape-v1` device — `None` before a pointer exists or on a compositor without the global. Set by `SeatHandler` and read by `request_cursor_shape`, which reaches the driver thread only through `Window::set_cursor`'s bare `&self`.
    static CURSOR_SHAPE_DEVICE: RefCell<Option<WpCursorShapeDeviceV1>> = const { RefCell::new(None) };
    // The serial of the pointer's last `enter`, which `set_shape` must echo back or be ignored (the protocol's own rule, not a guess this crate makes).
    static LAST_POINTER_SERIAL: Cell<u32> = const { Cell::new(0) };
    // Set while `app_watch` registers, so a source meant to outlive whatever build asked for it is not tied to that build's owner.
    static APP_LEVEL: Cell<bool> = const { Cell::new(false) };
}

/// Files `token` against the surface currently being driven, so its teardown removes the source, and against the reactive owner building it, so a part of the tree rebuilt inside a surface that stays takes its sources with it. Outside a surface the token is dropped: app-level sources (the config watcher) live as long as the process.
fn track_source(token: RegistrationToken) {
    let sink = CURRENT_SOURCES.with(|s| s.borrow().clone());
    if let Some(sink) = &sink {
        sink.borrow_mut().push(token);
    }
    if APP_LEVEL.with(Cell::get) {
        return;
    }
    // Tied to the owner whether or not a surface pass is running: a row built in the loop's closing batch flush registers outside one, and its sources must still go when it does. Tokens are versioned, so removing one the surface already removed is harmless.
    let filed = sink.as_ref().map(Rc::downgrade);
    telar::on_cleanup(move || {
        if let Some(filed) = filed.and_then(|filed| filed.upgrade()) {
            filed.borrow_mut().retain(|kept| *kept != token);
        }
        LOOP_HANDLE.with(|h| {
            if let Some(handle) = h.borrow().as_ref() {
                handle.remove(token);
            }
        });
    });
}

/// Wayland surfaces the driver is holding, refreshed once a turn. An atomic rather than a thread-local because the point is to be readable as a number without being on the driver thread, which is what makes a surface leak observable from a script instead of from `top`.
static LIVE_SURFACES: AtomicUsize = AtomicUsize::new(0);

/// How many surfaces the driver holds right now, mapped or not — every layer window, reservation strip and lock surface. Reported by `shell status`.
pub fn live_surfaces() -> usize {
    LIVE_SURFACES.load(Ordering::Relaxed)
}

/// Registers a closure to run once on the driver thread just after its loop is set up (its `LOOP_HANDLE` installed), so app-level setup that needs `watch` or opens a window runs on the right thread. Call it before `run_multi_with_platform` (same thread as the driver).
pub fn run_on_start(task: impl FnOnce() + 'static) {
    STARTUP.with(|s| s.borrow_mut().push(Box::new(task)));
}

struct PendingSurface {
    config: LayerConfig,
    // `None` for a reservation-only strip (no rsx handler, just its exclusive zone).
    handler: Option<BoxedHandler>,
    link: Arc<SurfaceLink>,
}

/// The surfaces opened and not mounted yet.
///
/// Its own drop runs only as the thread ends, when thread-locals are torn down in an order nothing may rely on: a handler still waiting here was built against telar state that can be gone by then, and dropping it would reach for that state. So what is left is let go of without being dropped — it never reached the compositor, and the thread is ending (F-9).
struct Pending(RefCell<Vec<PendingSurface>>);

impl Drop for Pending {
    fn drop(&mut self) {
        std::mem::forget(std::mem::take(self.0.get_mut()));
    }
}

/// Runs the handler closure with the current surface's sink installed, which is where `interval`/`watch` file their registration tokens so the surface's timers and channels die with it. Restored afterwards.
fn with_current<R>(sources: &SourceSink, f: impl FnOnce() -> R) -> R {
    CURRENT_SOURCES.with(|s| *s.borrow_mut() = Some(Rc::clone(sources)));
    let result = f();
    CURRENT_SOURCES.with(|s| *s.borrow_mut() = None);
    result
}

/// Starts a surface's renderer, inside one reactive batch and with the surface's sink installed: the first resume mounts the tree, and a later one presents the tree the suspend kept. `false` means the renderer could not be built.
fn resume_handler<W: Window>(
    handler: &mut dyn EventHandler<W>,
    window: &W,
    sources: &SourceSink,
) -> bool {
    with_current(sources, || {
        handler.new_events();
        let resumed = handler.on_resume(window);
        handler.about_to_wait();
        resumed
    })
}

/// Stops a surface's renderer, joining its thread, and keeps the handler with its app and tree.
fn suspend_handler<W: Window>(handler: &mut dyn EventHandler<W>, sources: &SourceSink) {
    with_current(sources, || handler.on_suspend());
}

/// Repeats `callback` every `period` on the shared loop. Bound to the surface that registered it: when that surface is torn down, or the reactive owner that registered it is disposed, the timer is removed with it, so a rebuilt tree never stacks a second ticker on the first.
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

/// Runs `callback` once, `delay` from now, on the shared event loop, then drops the timer. Used for an OSD's auto-dismiss. No-op when called outside a surface loop (e.g. a headless test).
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

impl<T> Clone for EventSender<T> {
    fn clone(&self) -> Self {
        Self {
            channel: self.channel.clone(),
            receiver: Weak::clone(&self.receiver),
        }
    }
}

impl<T> EventSender<T> {
    pub fn send(&self, event: T) -> bool {
        self.channel.send(event).is_ok()
    }

    /// Whether the surface on the other end still exists — asked *without* sending anything.
    ///
    /// A failed `send` answers the same question and was the only way to ask it, which meant a service could not discover it was unwanted without first doing the work of a reading: the producer that nobody is listening to is precisely the one that must not take one. The strong half of this handle lives in the loop source's callback, so removing that source with its surface is what makes this `false`.
    pub fn alive(&self) -> bool {
        self.receiver.strong_count() > 0
    }
}

/// The receiving end of a [`detached`] subscription, held by the caller instead of by a loop source.
pub struct Subscription<T> {
    channel: Channel<T>,
    /// Never read — holding it is the point. The sender's weak twin dies when this drops, which is what makes `EventSender::alive` answer `false`.
    #[allow(dead_code)]
    receiver: Arc<()>,
}

impl<T> Subscription<T> {
    /// The next value the producer sent, or `None` if it has not sent one yet.
    pub fn try_recv(&self) -> Option<T> {
        self.channel.try_recv().ok()
    }
}

/// A subscription with no surface behind it: the caller holds the receiving end itself, and the producer's sender reports itself dead once that end is dropped. What stands in for a surface where there is no event loop to file one against — a test, or a producer consuming another service.
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

/// Runs `producer` on its own thread and delivers what it sends to `on_event` on the loop thread. Bound to the surface that registered it: tearing that surface down removes the channel source, which drops the receiver so the producer's next `send` fails and it winds itself down (every producer here checks that result), and makes [`EventSender::alive`] answer `false` so a producer with nothing left to feed can retire before it takes another reading.
///
/// Returns a handle to the registration for the app-level caller that has to be able to take it back — a watcher installed from the config, which a reload may switch off. A caller inside a surface can ignore it: the surface's own teardown already removes the source.
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
            .name("hogar-shell-watch".to_string())
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

/// A [`watch`] that belongs to the process rather than to whichever surface's handler is running when it registers: a store every surface shares starts its worker on the first request, which is somebody's build, and bound to that surface the worker would die with it.
pub fn app_watch<T, P, F>(producer: P, on_event: F) -> Option<WatchToken>
where
    T: Send + 'static,
    P: FnOnce(EventSender<T>) + Send + 'static,
    F: FnMut(T) + 'static,
{
    let surface = CURRENT_SOURCES.with(|s| s.borrow_mut().take());
    let lived = APP_LEVEL.with(|app| app.replace(true));
    let token = watch(producer, on_event);
    APP_LEVEL.with(|app| app.set(lived));
    CURRENT_SOURCES.with(|s| *s.borrow_mut() = surface);
    token
}

/// A [`watch`] registration, so the caller that installed it can take it back.
pub struct WatchToken(RegistrationToken);

/// Removes a [`watch`], dropping the channel that fed it. The producer's next `send` fails and `EventSender::alive` turns false, so the service behind it winds down too — which is the point: switching a watcher off has to stop the thing it started, not just stop listening to it.
///
/// Must run on the driver thread, which is where every `watch` callback and every config reload already runs.
pub fn unwatch(token: WatchToken) {
    LOOP_HANDLE.with(|h| {
        if let Some(handle) = h.borrow().as_ref() {
            handle.remove(token.0);
        }
    });
}

/// Registers the app's reaction to the compositor's output set changing after startup — a monitor plugged in or unplugged — so it can open bars on the new screen and drop the ones on the old. Fires on the driver thread.
pub fn on_outputs_changed(callback: impl Fn() + 'static) {
    OUTPUTS_CHANGED.with(|c| *c.borrow_mut() = Some(Box::new(callback)));
}

/// The compositor's outputs. On the driver thread this reads the live set the driver already tracks; anywhere else (before the loop is up) it falls back to a throwaway connection via [`enumerate_outputs`].
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

/// The shell object a surface is mounted through. Two roles share every other part of the driver — one connection, one seat, one loop, the same rsx handler and the same `LayerWindow` bridging it to wgpu — and differ only in which protocol object carries the surface and how it is configured.
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

/// A single mounted surface: its shell object, wgpu-bridging window, and (unless it is a reservation-only strip) the rsx handler that renders it. All entries live on one thread and share one Wayland connection.
pub(crate) struct SurfaceEntry {
    pub(crate) shell: Shell,
    wl_id: ObjectId,
    window: Option<LayerWindow>,
    handler: Option<BoxedHandler>,
    // `Some` for a surface opened through a `SurfaceHandle` (its close flag and geometry channel); `None` for one the driver mounted itself — a lock surface — which only goes on the shared shutdown.
    link: Option<Arc<SurfaceLink>>,
    /// Timers and channel sources this surface registered (via `interval`/`watch`), removed from the loop when it is torn down so a closed surface stops ticking instead of outliving its own signals.
    sources: SourceSink,
    /// The layer-shell namespace, so a diagnostic can name which surface an event reached.
    namespace: String,
    reserve_only: bool,
    /// Whether the input region is carved from what the content draws: a layer window's only input policy. A reservation strip keeps an empty region and a lock surface has none to carve.
    drawn_input: bool,
    /// Whether the owner wants the surface on screen, and whether a configure may be presented on.
    mapping: Mapping,
    /// The layer-shell state this surface last asked for, which re-arming after an unmap asks for again. `None` for a lock surface, which has none.
    layer_state: Option<LayerConfig>,
    /// The scale to render at, in 120ths — `wp_fractional_scale_v1`'s own unit, and the only one that can carry the 1.25× and 1.5× a compositor rounds to 1 or 2 when it has to answer in whole numbers.
    scale_120: u32,
    /// The pair that makes a fractional scale renderable, and `None` together on a compositor without them: the viewport maps a device-pixel buffer back onto its logical size, and the scale object is what says which.
    viewport: Option<WpViewport>,
    fractional: Option<WpFractionalScaleV1>,
    /// This surface's handle on `ext-background-effect-v1`, `None` on a compositor without the global. Exactly one per `wl_surface` — a second is the `background_effect_exists` error — so it is created with the surface and destroyed with it rather than on demand.
    background_effect: Option<ExtBackgroundEffectSurfaceV1>,
    logical_size: (u32, u32),
    /// The size or the scale moved and the buffer behind them has not caught up yet. Cleared once per turn by [`Self::apply_geometry`], which is the only thing that resizes what the renderer draws into.
    geometry_dirty: bool,
    configured: bool,
    resumed: bool,
    /// Whether the handler has built its tree. Set by the first resume and never cleared, since a suspend keeps the tree.
    mounted: bool,
    closed: bool,
    events: Vec<Event>,
    timeout: Option<Duration>,
    input_region: Vec<(i32, i32, i32, i32)>,
    /// The blur region last applied, sorted, so a surface asking for the same one every frame commits once.
    blur_region: Vec<(i32, i32, i32, i32)>,
    reservation: Option<Reservation>,
    /// The size the reservation strip's buffer was last committed at — logical for the single-pixel route, whose viewport destination is the thing that moves, and device for the shm one, whose buffer is what has to be reallocated.
    reservation_size: (u32, u32),
}

/// The transparent buffer a reservation strip is mapped with.
///
/// A strip paints nothing — it exists to hold an exclusive zone — but **an unmapped layer surface reserves nothing and a `wl_surface` with no buffer is never mapped**, which is the only reason it needs a buffer at all.
enum Reservation {
    /// One 1×1 transparent pixel from `wp-single-pixel-buffer-v1`, stretched over the strip by the `wp_viewport` the surface already has. It allocates nothing, and it never has to be rebuilt: the buffer stays 1×1 however large the strip grows.
    SinglePixel(wl_buffer::WlBuffer),
    /// The fallback where either half of that pair is missing: a strip-sized shm mapping. Neither object is ever read again — they are held to stay alive, because dropping the `Buffer` destroys the `wl_buffer` the compositor is showing and dropping the pool unmaps the memory behind it.
    #[expect(
        dead_code,
        reason = "held to keep the mapping the compositor is reading alive"
    )]
    Shm(SlotPool, Buffer),
}

impl SurfaceEntry {
    /// A surface the driver mounts with no layer-shell configuration of its own — currently only a lock surface, whose size, anchoring and input are the compositor's to decide.
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
            namespace,
            reserve_only: false,
            drawn_input: false,
            mapping: Mapping::default(),
            layer_state: None,
            scale_120: scale.max(1) as u32 * 120,
            viewport: None,
            fractional: None,
            background_effect: None,
            logical_size,
            geometry_dirty: false,
            configured: false,
            resumed: false,
            mounted: false,
            closed: false,
            events: Vec::new(),
            timeout: None,
            input_region: Vec::new(),
            blur_region: Vec::new(),
            reservation: None,
            reservation_size: (0, 0),
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
    /// Two routes, and the fractional one is the reason the pair is bound together: a viewport whose destination is the *logical* size lets the buffer be any size at all, so 1.5× is a buffer 1.5× the logical size rather than a 1× buffer the compositor stretches. The protocol asks for a buffer scale of 1 alongside it, since the destination already says everything about the mapping. Without the pair the only thing that can be said is a whole number, which is what a compositor at 1.5× rounds for us.
    fn map_buffer(&self) {
        let surface = self.shell.wl_surface();
        match &self.viewport {
            Some(viewport) => {
                surface.set_buffer_scale(1);
                let (width, height) = viewport_destination(self.logical_size);
                viewport.set_destination(width, height);
            }
            None => surface.set_buffer_scale((self.scale_120 / 120).max(1) as i32),
        }
    }

    /// Adopts a compositor-decided size and tells the handler to re-lay-out; ignored if the surface isn't mapped yet, since a configure reaching it before it re-armed answers a mapping that no longer exists.
    pub(crate) fn apply_configure(&mut self, width: u32, height: u32) {
        self.logical_size = (width, height);
        self.geometry_dirty = true;
        if !self.mapping.accepts_configure() {
            return;
        }
        if self.configured {
            self.events.push(Event::WindowResized { width, height });
        }
        self.configured = true;
    }

    /// Takes a new scale, or does nothing if it is the one already in use.
    ///
    /// The resize is pushed as well as the scale: the logical size has not moved, but the buffer behind it has, and a renderer told only that the scale changed would keep drawing at the old device size.
    fn rescale(&mut self, scale_120: u32) {
        if scale_120 == 0 || scale_120 == self.scale_120 {
            return;
        }
        self.scale_120 = scale_120;
        self.geometry_dirty = true;
        // The preferred scale usually lands before the first configure, where the size is still a placeholder and the window is built from the scale rather than told about it — so there is nothing to tell yet.
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
    /// **One change arrives as two events** — `configure` carries the logical size and `preferred_scale` the scale — and acting on each as it lands is what makes a scale change expensive out of all proportion to it: the renderer is handed the new size at the old scale, throws away its swapchain and every texture sized to the old one to build them again, and is then handed the same size at the new scale and does it all a second time. Deferring to the turn also collapses a *burst* — a scale flipped back and forth, a monitor reconfigured — into the one resize its end state deserves, across every surface at once.
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

    /// Whether its link holds something the next pass would act on. A rebuild asked of a hidden surface is not one: it waits for the surface to be shown.
    fn has_request(&self) -> bool {
        let Some(link) = &self.link else {
            return false;
        };
        let driven = self.configured && self.mapping.wanted();
        link.is_closing() || link.has_update() || (driven && self.mounted && link.wants_rebuild())
    }

    /// Pushes whatever the surface asked for since the last turn to the compositor; mapping is applied last, since both of its transitions commit on the spot and carry whatever the fields before it queued.
    fn apply_update(&mut self, change: SurfaceUpdate, compositor: &CompositorState) {
        let mut moved = false;
        if let Shell::Layer(layer) = &self.shell {
            // Every layer-shell field is a value the compositor is simply told, so asking at all is a change worth a commit. A blur region is the exception and diffs itself.
            moved = change.renegotiates();
            if !push_layer_state(layer, &change) {
                tracing::warn!(
                    "{}: this compositor's layer-shell cannot restack a mapped surface; \
                     restart for the change to take effect",
                    self.namespace
                );
            }
            if let Some(state) = &mut self.layer_state {
                state.absorb(&change);
            }
        }
        if let Some(rects) = change.blur_region {
            moved |= self.set_blur_region(compositor, rects);
        }
        let presented = self.resumed || self.reservation.is_some();
        let transition = change.mapped.map_or(Transition::None, |wanted| {
            self.mapping.set(wanted, presented)
        });
        match transition {
            Transition::Release => self.release(),
            Transition::Rearm => self.rearm(),
            Transition::None if moved => self.commit_pending(),
            Transition::None => {}
        }
    }

    /// Takes the surface off screen: the renderer first, joining its thread, so no frame of its own can attach a buffer after the null one — that would be a buffer on an unconfigured surface, which the compositor answers by killing the connection.
    fn release(&mut self) {
        if self.resumed
            && let Some(handler) = self.handler.as_mut()
        {
            suspend_handler(handler.as_mut(), &self.sources);
        }
        self.resumed = false;
        self.configured = false;
        self.events.clear();
        self.timeout = None;
        self.shell.wl_surface().attach(None, 0, 0);
        self.shell.commit();
        if let Some(Reservation::SinglePixel(buffer)) = self.reservation.take() {
            buffer.destroy();
        }
        self.reservation_size = (0, 0);
    }

    /// Puts a released surface back in line for the screen: its layer-shell state asked for again — the protocol returns an unmapped layer surface to the state it had right after `get_layer_surface` — and a commit without a buffer, which the compositor answers with the configure the surface presents on.
    fn rearm(&self) {
        if let (Shell::Layer(layer), Some(state)) = (&self.shell, &self.layer_state) {
            // An older layer-shell that cannot restack also never moved the surface off the layer it was created on, which is the one right after `get_layer_surface`.
            push_layer_state(layer, &state.as_update());
        }
        self.shell.commit();
    }

    /// Asks the compositor to blur what is behind `rects` — in logical surface coordinates, clipped by the compositor to the surface — and reports whether the region actually moved.
    ///
    /// It reports rather than commits because `set_blur_region` is **double-buffered**: it lands on the surface's next `wl_surface.commit`, which belongs to whoever owns the surface's buffer (see [`Self::commit_pending`]). The `wl_region` is destroyed the moment this returns, which the protocol explicitly allows — the region has copy semantics — and an empty set is sent as the NULL region that *removes* the effect rather than as a region of no area.
    ///
    /// A surface on a compositor without the global has no effect object and answers `false`: nothing is asked for, nothing is committed, and nothing is logged above debug.
    fn set_blur_region(&mut self, compositor: &CompositorState, rects: Vec<telar::Rect>) -> bool {
        let Some(effect) = &self.background_effect else {
            return false;
        };
        let Some(rects) = region_change(rects, &self.blur_region) else {
            return false;
        };
        tracing::debug!(
            "blur region for {}: {} rect(s) {rects:?}",
            self.namespace,
            rects.len()
        );
        if rects.is_empty() {
            effect.set_blur_region(None);
        } else {
            let Ok(region) = Region::new(compositor) else {
                return false;
            };
            for (x, y, w, h) in &rects {
                region.add(*x, *y, *w, *h);
            }
            effect.set_blur_region(Some(region.wl_region()));
        }
        self.blur_region = rects;
        true
    }

    /// Applies what this thread has queued on the surface — or leaves it for the renderer's next frame to carry.
    ///
    /// **A `wl_surface` has one set of pending state and nothing guarding it.** The renderer commits from its own thread to present, so a commit from here can land between the buffer it attached and the commit it was about to make — taking its explicit-sync acquire point with no buffer of our own behind it, which the compositor answers with `wp_linux_drm_syncobj_surface_v1` error 3 and the death of the whole connection. Asking for a frame instead lets the one thread that owns the surface's buffer carry both, which costs a frame's delay on an input region or a renegotiated size and nothing else.
    ///
    /// A surface with no renderer — a reservation strip, or one that has not had its first frame — has no such thread, and commits here.
    fn commit_pending(&mut self) {
        match (&self.window, self.handler.as_mut()) {
            (Some(window), Some(handler)) => {
                handler.owe_presentation();
                window.request_redraw();
            }
            _ => self.shell.commit(),
        }
    }

    /// Builds this surface's content again on the surface it is already on, and drops everything the outgoing content had registered against the loop.
    ///
    /// Those registrations are the whole reason a rebuild is more than one call: `interval` and `watch` file their sources against the surface, and the tree being replaced is about to register its own — so a rebuild that kept them would leave a clock ticking twice and a service feeding a tree nobody draws.
    fn rebuild(&mut self, window: &LayerWindow, loop_handle: &LoopHandle<'static, Driver>) {
        for token in self.sources.borrow_mut().drain(..) {
            loop_handle.remove(token);
        }
        let sources = Rc::clone(&self.sources);
        if let Some(handler) = self.handler.as_mut() {
            with_current(&sources, || handler.remount(window));
        }
    }
}

/// Sends every layer-shell field `change` names. `false` when it asked for a layer this compositor cannot restack to: that arrived in version 2 of the protocol, and a request an object does not implement is a protocol error that kills the whole connection, so on an older compositor the surface keeps the layer it was created on.
fn push_layer_state(layer: &LayerSurface, change: &SurfaceUpdate) -> bool {
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
    if let Some(keyboard) = change.keyboard_interactivity {
        layer.set_keyboard_interactivity(keyboard);
    }
    let Some(shell_layer) = change.layer else {
        return true;
    };
    match layer.kind() {
        SurfaceKind::Wlr(wlr) if wlr.version() >= 2 => {
            layer.set_layer(shell_layer);
            true
        }
        _ => false,
    }
}

/// A logical length in device pixels, at a scale given in 120ths.
///
/// Rounded half away from zero, which is the rule `wp_fractional_scale_v1` states rather than one chosen here: a client that rounds the other way from its compositor hands over a buffer a row short of the destination it declared, and gets it stretched back. The integer arithmetic is the same rule without the float — 60 is half of 120.
fn device_pixels(logical: u32, scale_120: u32) -> u32 {
    (logical.saturating_mul(scale_120).saturating_add(60) / 120).max(1)
}

/// Where a `wp_viewport` puts a surface's buffer: its logical size, which is the mapping the compositor configured, whatever size the buffer behind it happens to be.
///
/// That indifference to the buffer is what lets a reservation strip be a single pixel stretched over its whole edge as easily as it lets a 1.5× surface be a buffer half again as large as its logical size. Each axis is floored at one because a surface is created with a zero on the axis the compositor fills and is driven for a turn before its first `configure` says what that is — and `set_destination` answers a zero with `wp_viewport`'s `bad_value` error, which kills the connection rather than the surface.
fn viewport_destination(logical_size: (u32, u32)) -> (i32, i32) {
    (logical_size.0.max(1) as i32, logical_size.1.max(1) as i32)
}

/// The single-thread driver: one Wayland connection's shared globals (registry/output/seat/shm) plus every live surface. The SCTK delegate handlers route each event to its surface by `wl_surface` id.
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
    // The surface the pointer is currently over. Enter and leave are edges, not levels, so a surface that rebuilds its content between them has to be told where the pointer already is.
    pointer_focus: Option<ObjectId>,
    pub(crate) surfaces: Vec<SurfaceEntry>,
    /// `None` where the compositor does not implement `ext-session-lock-v1`, which is what makes the shell refuse to lock rather than draw an overlay it cannot enforce.
    pub(crate) lock_manager: Option<ExtSessionLockManagerV1>,
    pub(crate) lock: Option<LockSession>,
    pub(crate) scaling: Option<Scaling>,
    /// The blur factory, `None` where the compositor does not implement `ext-background-effect-v1`. Whether it will actually blur is a separate question the manager answers with its `capabilities` event and keeps answering; see [`background_effect_supported`].
    pub(crate) background_effect: Option<ExtBackgroundEffectManagerV1>,
    /// The 1×1-buffer factory a reservation strip is mapped with instead of a strip-sized shm pool. `None` keeps [`commit_reservation`]'s shm path in service.
    single_pixel: Option<WpSinglePixelBufferManagerV1>,
    /// The factory behind [`CURSOR_SHAPE_DEVICE`], `None` on a compositor without `wp-cursor-shape-v1`. Kept here only to mint the device once a pointer capability arrives (`SeatHandler::new_capability`) — nothing else asks it for anything.
    cursor_shape_manager: Option<WpCursorShapeManagerV1>,
}

/// The two globals a surface needs to render on the device pixel grid, held together because either alone is useless: a preferred scale with no viewport is a number nothing can act on, and a viewport with no scale to put in it is a mapping with nothing to map.
pub(crate) struct Scaling {
    manager: WpFractionalScaleManagerV1,
    viewporter: WpViewporter,
}

/// What the shell can ask about this compositor before it commits to a feature, and what the compositor says about the session. Read from any thread that has gone through the driver, so a UI handler can grey out "lock" rather than fail on the attempt.
#[derive(Clone, Copy, Default)]
pub(crate) struct DriverFacts {
    pub(crate) lock_supported: bool,
    /// Whether the bound `zwlr_layer_shell_v1` is version 2 or later, the first that can move a mapped surface to another layer. Settled at bind time.
    pub(crate) layer_restack_supported: bool,
    /// Unlike the one above, this is not settled at bind time: the blur capability arrives as an event and can be withdrawn, so this is rewritten every time the manager says so.
    pub(crate) background_effect_supported: bool,
    /// The compositor's word on whether the session is locked, whoever locked it — live like the blur capability: settled at driver init by the notifier's first read, then rewritten by every `locked` and `unlocked` the live notification receives. [`CompositorLock::CannotTell`] until then, and for good where the compositor has no `hyprland-lock-notify-v1`.
    pub(crate) compositor_lock: CompositorLock,
}

thread_local! {
    static FACTS: RefCell<DriverFacts> = const {
        RefCell::new(DriverFacts {
            lock_supported: false,
            layer_restack_supported: false,
            background_effect_supported: false,
            compositor_lock: CompositorLock::CannotTell,
        })
    };
}

pub(crate) fn with_driver_facts<R>(read: impl FnOnce(&DriverFacts) -> R) -> R {
    FACTS.with(|facts| read(&facts.borrow()))
}

pub(crate) fn update_driver_facts(write: impl FnOnce(&mut DriverFacts)) {
    FACTS.with(|facts| write(&mut facts.borrow_mut()));
}

/// Whether this compositor will actually blur behind a surface right now: `ext-background-effect-v1` is bound *and* its `blur` capability is currently set.
///
/// Both halves matter and the second is live state rather than a one-time answer. The manager sends `capabilities` when the global is bound and again whenever they change, and the protocol is explicit that a capability which goes away **stops being applied even to a surface that already set a region** — so a compositor that turns blur off mid-session turns this false, and a shell that read it once at startup would keep asking for an effect nothing applies.
///
/// Like [`lock_supported`](crate::lock_supported) and [`idle_supported`](crate::idle_supported), this reads driver state, so outside a running event loop it answers false because there is no driver rather than because the compositor lacks the protocol. Those are different answers: `advertises("ext_background_effect_manager_v1")` is the one to ask from a bare CLI process, and it reports only the global — no registry read can see a capability, which is the half this function exists to add.
pub fn background_effect_supported() -> bool {
    with_driver_facts(|facts| facts.background_effect_supported)
}

/// Whether a layer window can be moved to another layer while it is up: `zwlr_layer_shell_v1` version 2 or later. Where it cannot, [`LayerWindowHandle::set_layer`] is dropped with a log line and the window stays on the layer it was opened on. Like [`background_effect_supported`], this reads driver state, so outside a running event loop it answers false.
pub fn layer_restack_supported() -> bool {
    with_driver_facts(|facts| facts.layer_restack_supported)
}

impl Driver {
    fn entry_mut(&mut self, wl_id: &ObjectId) -> Option<&mut SurfaceEntry> {
        self.surfaces.iter_mut().find(|e| &e.wl_id == wl_id)
    }

    /// Gives a freshly created surface its scale and viewport objects, where the compositor has them.
    ///
    /// Called for every surface this driver mounts, lock surfaces included: a lock screen covers a whole output with text, which is the last place a shell can afford to hand over a buffer for the compositor to blur.
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

    /// Gives a freshly created surface its blur handle, where the compositor has the global.
    ///
    /// Created with the surface rather than on the first blur request, because the protocol allows exactly one per `wl_surface` and answers a second with `background_effect_exists` — a connection-killing error for what would otherwise be an ordinary race between two things asking the same surface to blur. It costs one inert object per surface: the initial blur region is empty, so a surface that never asks for one is a surface the compositor does nothing to.
    ///
    /// Attached whether or not the `blur` capability is currently set, for the same reason [`background_effect_supported`] has to be re-read rather than cached: the capability can come back, and a surface created while it was off must be able to blur when it does.
    pub(crate) fn attach_background_effect(
        &self,
        entry: &mut SurfaceEntry,
        qh: &QueueHandle<Driver>,
    ) {
        if let Some(manager) = &self.background_effect {
            entry.background_effect =
                Some(manager.get_background_effect(entry.shell.wl_surface(), qh, ()));
        }
    }

    /// The layer-shell namespace of the surface an event landed on, for diagnostics. `None` means the event named a surface this driver does not own.
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

    /// Refreshes the cached output set and, once the shell is up, notifies the app when it actually changed so it can open bars on a newly connected monitor and drop the ones on a disconnected one.
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

/// Outputs compared by name: the identity a `LayerConfig` pins a surface to, so a scale or resolution change (which the surface handles through `configure`) doesn't trigger a full surface reconciliation.
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
    let drawn_input = handler.is_some();
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
    // Every surface starts with an empty input region: a reservation strip keeps it, and a window carving its region from its content has not computed one before its first frame, so neither steals a click from what is beneath.
    if let Ok(region) = Region::new(compositor) {
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
    entry.drawn_input = drawn_input;
    entry.layer_state = Some(config.clone());
    // Before the first commit, so the surface is never mapped under a mapping it is about to replace.
    driver.attach_scaling(&mut entry, qh);
    driver.attach_background_effect(&mut entry, qh);
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
    // Optional by design: a compositor without either protocol still runs every bar and panel. The features that need them ask first (`lock_supported`, `idle_supported`) rather than failing at the point of use.
    let lock_manager = globals
        .bind::<ExtSessionLockManagerV1, Driver, ()>(&qh, 1..=1, ())
        .inspect_err(|e| tracing::info!("ext-session-lock-v1 unavailable: {e}"))
        .ok();
    // Optional too, and read here rather than on first use: its answer has to be settled before the startup tasks run, and settling it takes a roundtrip, which cannot be made from inside the loop's own dispatch.
    crate::lock_notify::bind(&globals, &conn, &qh);
    let idle_notifier = globals
        .bind::<ExtIdleNotifierV1, Driver, ()>(&qh, 1..=2, ())
        .inspect_err(|e| tracing::info!("ext-idle-notify-v1 unavailable: {e}"))
        .ok();
    // Both or neither, since neither is any use alone. A compositor without them still draws every surface — through the whole-number buffer scale, which is what this replaces.
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
    // Optional, and the one whose absence has a user-visible substitute: without it a translucent surface is only blurred if the user wrote a per-namespace blur rule into their compositor's own config.
    let background_effect = globals
        .bind::<ExtBackgroundEffectManagerV1, Driver, ()>(&qh, 1..=1, ())
        .inspect_err(|e| tracing::info!("ext-background-effect-v1 unavailable: {e}"))
        .ok();
    // Optional, and invisible when absent: a reservation strip falls back to the shm pool this replaces, which costs memory and changes nothing about the zone it holds.
    let single_pixel = globals
        .bind::<WpSinglePixelBufferManagerV1, Driver, ()>(&qh, 1..=1, ())
        .inspect_err(|e| tracing::info!("wp-single-pixel-buffer-v1 unavailable: {e}"))
        .ok();
    // Optional, and the request is simply dropped without it: a handle still takes the drag or resize, and the pointer keeps whatever image it entered with (T-2.3, F-2.12).
    let cursor_shape_manager = globals
        .bind::<WpCursorShapeManagerV1, Driver, ()>(&qh, 1..=1, ())
        .inspect_err(|e| tracing::info!("wp-cursor-shape-v1 unavailable: {e}"))
        .ok();
    let layer_restack_supported =
        ProvidesBoundGlobal::<ZwlrLayerShellV1, 1>::bound_global(&layer_shell)
            .is_ok_and(|shell| shell.version() >= 2);
    if !layer_restack_supported {
        tracing::info!(
            "zwlr_layer_shell_v1 is older than version 2, so a layer window cannot move to another layer"
        );
    }
    FACTS.with(|facts| {
        let mut facts = facts.borrow_mut();
        facts.lock_supported = lock_manager.is_some();
        facts.layer_restack_supported = layer_restack_supported;
    });

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
        background_effect,
        single_pixel,
        cursor_shape_manager,
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

    // Prime the registry so outputs are known before matching `config.output` on surface creation.
    for _ in 0..3 {
        if event_loop
            .dispatch(Duration::from_millis(40), &mut driver)
            .is_err()
        {
            return Ok(());
        }
    }

    // After priming, since an idle notification is taken out against a seat and the seat only exists once the registry has been round-tripped. Absent either half, idle timers report themselves as unsupported.
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

    // App-level setup that needs the driver thread, now that LOOP_HANDLE is installed.
    for task in STARTUP.with(|s| std::mem::take(&mut *s.borrow_mut())) {
        task();
    }

    let mut next_timeout: Option<Duration> = Some(Duration::ZERO);
    loop {
        // Before the surface pass, so a lock taken during the last dispatch has its surfaces mounted — and an unlock has them torn down — in this same turn rather than one frame late.
        crate::lock::poll(&mut driver, &compositor, &qh, &conn, &loop_handle);

        // Mount the layer windows and reservation strips opened since the last turn.
        let pending: Vec<PendingSurface> =
            DYN_QUEUE.with(|q| std::mem::take(&mut *q.0.borrow_mut()));
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

        // Bracket the dispatch in a reactive batch so signal writes from Wayland/calloop callbacks (an icon download landing, a service update) are deferred and flushed once here, not synchronously mid-callback — which under M3's shared runtime would re-enter a callback still holding a RefCell borrow. This mirrors the winit runner bracketing each dispatch with the handler's new_events/about_to_wait.
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
            single_pixel,
            ..
        } = &mut driver;
        let single_pixel = single_pixel.as_ref();
        for (index, entry) in surfaces.iter_mut().enumerate() {
            if entry.closed || entry.link.as_ref().is_some_and(|link| link.is_closing()) {
                remove.push(index);
                continue;
            }
            if let Some(change) = entry.link.as_ref().and_then(|link| link.take_update()) {
                entry.apply_update(change, &compositor);
            }
            // A hidden surface is not driven at all: its renderer is suspended, so nothing would present, and a dirty tree reporting a frame deadline would spin the loop at the frame rate for a window nobody can see.
            if !entry.configured || !entry.mapping.wanted() {
                continue;
            }
            entry.apply_geometry();
            if entry.reserve_only {
                commit_reservation(shm_state, single_pixel, &qh, entry);
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

            // Taken whether or not it can be acted on: a rebuild asked for before the surface had ever been mounted *is* the mount below, whose first build already reads whatever the request was about. One asked for while a mounted surface was hidden is not: the resume shows the tree the suspend kept.
            let rebuild =
                entry.link.as_ref().is_some_and(|link| link.take_rebuild()) && entry.mounted;

            if !entry.resumed {
                let sources = Rc::clone(&entry.sources);
                let handler = entry
                    .handler
                    .as_mut()
                    .expect("rendering surface has a handler");
                let ok = resume_handler(handler.as_mut(), &window, &sources);
                if !ok {
                    tracing::error!("layer surface on_resume failed (renderer init)");
                    remove.push(index);
                    continue;
                }
                entry.resumed = true;
                entry.mounted = true;
            }
            if rebuild {
                entry.rebuild(&window, &loop_handle);
                // The pointer does not enter a surface twice, so a rebuilt surface under it would otherwise never hear that it is hovered — an auto-hidden bar rebuilt while it was out would slide away under the cursor and stay there until the pointer left and came back.
                if pointer_focus.as_ref() == Some(&entry.wl_id) {
                    entry.events.push(Event::CursorEntered);
                }
            }

            let sources = Rc::clone(&entry.sources);
            let events = std::mem::take(&mut entry.events);
            entry.timeout = with_current(&sources, || {
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
            if entry.drawn_input {
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
        }

        for index in remove.into_iter().rev() {
            let entry = driver.surfaces.remove(index);
            tear_down(entry, &loop_handle);
        }

        LIVE_SURFACES.store(driver.surfaces.len(), Ordering::Relaxed);
        // A surface visited early in the pass can be asked for something by one visited after it — an edit mode closing in the overlay window lowers the desktop window it raised — and nothing else wakes the loop for it.
        let asked = DYN_QUEUE.with(|q| !q.0.borrow().is_empty())
            || driver.surfaces.iter().any(SurfaceEntry::has_request);
        next_timeout = match asked {
            true => Some(Duration::ZERO),
            false => min_timeout,
        };
    }

    for entry in driver.surfaces.drain(..) {
        tear_down(entry, &loop_handle);
    }
    Ok(())
}

/// Suspends a surface's handler and then removes every loop source it registered, so its timers and watch channels stop with it. Dropping the channel receivers also ends the producer threads feeding them.
pub(crate) fn tear_down(mut entry: SurfaceEntry, loop_handle: &LoopHandle<'static, Driver>) {
    if let Some(mut handler) = entry.handler.take() {
        let sources = Rc::clone(&entry.sources);
        with_current(&sources, || handler.on_suspend());
    }
    for token in entry.sources.borrow_mut().drain(..) {
        loop_handle.remove(token);
    }
    // Both hang off the wl_surface and neither is freed by dropping its handle, so they go before it does: every request on a viewport whose surface is gone is a protocol error, which kills the connection rather than the surface.
    if let Some(viewport) = entry.viewport.take() {
        viewport.destroy();
    }
    if let Some(fractional) = entry.fractional.take() {
        fractional.destroy();
    }
    // Same rule, same reason: `set_blur_region` on an effect object whose surface is gone is the `surface_destroyed` error, so the object goes first. Its own destruction removes the effect region on the next commit, which there will not be — the surface is going with it.
    if let Some(effect) = entry.background_effect.take() {
        effect.destroy();
    }
    // A single-pixel buffer is this crate's own protocol object, unlike SCTK's shm `Buffer`, which destroys itself with its pool as the entry drops. It hangs off no surface, so there is no ordering to respect.
    if let Some(Reservation::SinglePixel(buffer)) = &entry.reservation {
        buffer.destroy();
    }
    // A layer surface is destroyed by dropping SCTK's wrapper; a lock surface has no wrapper, so its two protocol objects are released here — the role object first, as the protocol's ordering requires.
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

/// Maps a reservation strip with a fully transparent buffer so the compositor honours the exclusive zone the strip was created to hold, and re-commits only when the strip's size moved.
///
/// A strip paints nothing, and the only reason it needs a buffer is that **an unmapped layer surface reserves nothing and a `wl_surface` with no buffer is never mapped**. `wp-single-pixel-buffer-v1` is the route that says exactly that and no more; the shm route below it is what a compositor missing either half of the pair still maps a strip with, which is a fallback rather than dead code.
fn commit_reservation(
    shm: &Shm,
    single_pixel: Option<&WpSinglePixelBufferManagerV1>,
    qh: &QueueHandle<Driver>,
    entry: &mut SurfaceEntry,
) {
    // Both halves or neither: a 1×1 buffer with no viewport to stretch it is a 1×1 surface, so a compositor carrying the factory without the `wp_viewporter` that comes with `Scaling` takes the shm path too.
    match single_pixel.filter(|_| entry.viewport.is_some()) {
        Some(manager) => commit_single_pixel_reservation(manager, qh, entry),
        None => commit_shm_reservation(shm, entry),
    }
}

/// Attaches one transparent pixel and lets the strip's own `wp_viewport` stretch it over the whole surface, which costs a protocol object and no memory at all.
///
/// The buffer is never rebuilt, because it is 1×1 however large the strip is: what follows a resize is the viewport destination [`SurfaceEntry::map_buffer`] has already set, so a strip whose size moved costs a commit rather than an allocation. That the stretch is a stretch changes nothing about what the strip reserves either — an exclusive zone is independent of the surface's size, and a 1 px strip asking for 32 reserves 32 (measured on Hyprland 0.56.2).
fn commit_single_pixel_reservation(
    manager: &WpSinglePixelBufferManagerV1,
    qh: &QueueHandle<Driver>,
    entry: &mut SurfaceEntry,
) {
    let size = entry.logical_size;
    if entry.reservation.is_some() && entry.reservation_size == size {
        return;
    }
    if entry.reservation.is_none() {
        // The protocol's values are premultiplied, so every channel of an invisible pixel has to be zero — a colour above zero at zero alpha is not a premultiplied pixel at all.
        let buffer = manager.create_u32_rgba_buffer(0, 0, 0, 0, qh, ());
        let surface = entry.shell.wl_surface();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        entry.reservation = Some(Reservation::SinglePixel(buffer));
    }
    entry.reservation_size = size;
    entry.shell.commit();
}

/// Allocates a strip-sized transparent shm buffer, for a compositor that cannot be handed a single pixel. Only rebuilt when the device size changed, since here the buffer itself is what the size describes.
fn commit_shm_reservation(shm: &Shm, entry: &mut SurfaceEntry) {
    let (w, h) = entry.device_size();
    if entry.reservation.is_some() && entry.reservation_size == (w, h) {
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
        entry.reservation = Some(Reservation::Shm(pool, buffer));
        entry.reservation_size = (w, h);
    }
}

/// Rebuilds the surface's input region from its handler's pointer targets — the laid-out interactive widgets, in logical surface coordinates — committing only when the set changed (`last` is the previously applied set, sorted so a reordered read isn't mistaken for a change). An empty set yields an empty region, i.e. fully click-through, so an overlay with no interactive content never blocks the windows beneath.
///
/// The rects come from the handler rather than from the global `telar::interactive_rects`, and that is the whole correctness of this function: the registry is one of the handler's *per-surface* worlds, live only inside its own calls. Read from out here — after the handler has returned — the ambient world answers, and it is always empty, so every surface using this was click-through everywhere.
///
/// Returns whether the region moved, since the surface it was set on still has to be committed — which is the caller's to do, and not from this thread while a renderer owns the surface (see [`SurfaceEntry::commit_pending`]).
fn update_input_region(
    compositor: &CompositorState,
    surface: &wl_surface::WlSurface,
    namespace: &str,
    rects: Vec<telar::Rect>,
    last: &mut Vec<(i32, i32, i32, i32)>,
) -> bool {
    let Some(rects) = region_change(rects, last) else {
        return false;
    };
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

/// The integer surface-local rects a `wl_region` would be built from, or `None` when they are the set already applied.
///
/// Both region requests in this crate — the input region and the blur region — are double-buffered state that costs a commit to land, so a surface whose content asks for the same region every frame would commit every frame; this is what makes it commit once. The comparison is against the *sorted* set, since the layout the rects come from is free to enumerate the same widgets in another order, and a reordered read is not a change.
///
/// Rounded **outward**: a widget laid out on a half pixel has the whole of itself inside the region rather than a row of it outside, which for input means the edge of a button still takes a click and for blur means the edge of a card is still blurred.
fn region_change(
    rects: Vec<telar::Rect>,
    last: &[(i32, i32, i32, i32)],
) -> Option<Vec<(i32, i32, i32, i32)>> {
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
    (rects != last).then_some(rects)
}

/// A live reservation strip. Dropping it asks the driver to tear it down on its next loop turn.
pub struct SurfaceHandle {
    link: Arc<SurfaceLink>,
}

impl SurfaceHandle {
    /// Renegotiates any part of the strip's layer-shell state in one commit — the shape a caller reconciling a strip against a changed arrangement wants, rather than one request per field.
    pub fn update(&self, change: SurfaceUpdate) {
        self.link.request_update(change);
    }
}

impl Drop for SurfaceHandle {
    fn drop(&mut self) {
        self.link.request_close();
    }
}

/// Opens the window of one layer on one output: the whole output, at the compositor's size, ignoring every exclusive zone, drawn by `app`; call [`LayerWindowHandle::set_mapped`] before the driver's next turn to open it hidden, at no cost beyond the layer surface itself.
pub fn open_layer_window<A: App + 'static>(
    output: Option<String>,
    layer: Layer,
    namespace: impl Into<String>,
    app: A,
) -> LayerWindowHandle {
    let link = Arc::new(SurfaceLink::default());
    let handler = build_surface_handler::<LayerWindow, _>(
        LocalApp(app),
        Arc::new(telar::NoPaths),
        "hogar-shell",
        surface_fonts(),
    );
    DYN_QUEUE.with(|q| {
        q.0.borrow_mut().push(PendingSurface {
            config: LayerConfig::whole_output(output, layer, namespace.into()),
            handler: Some(handler),
            link: Arc::clone(&link),
        })
    });
    LayerWindowHandle::new(link)
}

/// Opens a reservation-only strip (no rsx content — just its exclusive zone, an invisible transparent buffer), dropped to close. The strip and the window that draws what reserves are independent surfaces, so a strip is reconciled on its own without a teardown.
pub fn open_reservation(spec: LayerConfig) -> SurfaceHandle {
    let link = Arc::new(SurfaceLink::default());
    DYN_QUEUE.with(|q| {
        q.0.borrow_mut().push(PendingSurface {
            config: spec,
            handler: None,
            link: Arc::clone(&link),
        })
    });
    SurfaceHandle { link }
}

thread_local! {
    /// What every surface this shell opens shapes its text in.
    ///
    /// Telar's process-wide font family is gone — a family belongs to a surface's own configuration — but
    /// this crate is below `config` and cannot read the live one. So the shell sets it here, wherever it sets its theme, and every
    /// surface opened afterwards carries it. A *shell's* default rather than a framework's, which is the
    /// difference that matters: it is one application deciding for its own windows.
    static SURFACE_FONTS: std::cell::RefCell<telar::AppConfig> =
        std::cell::RefCell::new(telar::AppConfig::default());
}

/// Sets what the surfaces opened from here on shape their text in. Called at startup and on every config
/// reload, so a reopened bar follows a font change.
pub fn set_surface_fonts(fonts: telar::AppConfig) {
    SURFACE_FONTS.with(|f| *f.borrow_mut() = fonts);
}

fn surface_fonts() -> telar::AppConfig {
    SURFACE_FONTS.with(|f| f.borrow().clone())
}

pub(crate) fn surface_fonts_for_lock() -> telar::AppConfig {
    surface_fonts()
}

/// Maps a box's resolved [`Cursor`] (F-5.10 picks one winner per surface) onto the shape `wp-cursor-shape-v1` shows for it. Every variant has a namesake in the protocol's `shape` enum, including the resize and grab shapes an edit-mode handle needs (T-2.3): `EwResize`/`NsResize` for a straight edge, `NwseResize`/`NeswResize` for a corner, `Move` for a handle that moves freely in both axes.
fn cursor_shape(cursor: Cursor) -> wp_cursor_shape_device_v1::Shape {
    use wp_cursor_shape_device_v1::Shape;
    match cursor {
        Cursor::Default => Shape::Default,
        Cursor::Pointer => Shape::Pointer,
        Cursor::Crosshair => Shape::Crosshair,
        Cursor::Grab => Shape::Grab,
        Cursor::Grabbing => Shape::Grabbing,
        Cursor::ColResize => Shape::ColResize,
        Cursor::RowResize => Shape::RowResize,
        Cursor::EwResize => Shape::EwResize,
        Cursor::NsResize => Shape::NsResize,
        Cursor::NwseResize => Shape::NwseResize,
        Cursor::NeswResize => Shape::NeswResize,
        Cursor::Move => Shape::Move,
        Cursor::Text => Shape::Text,
        Cursor::NotAllowed => Shape::NotAllowed,
        Cursor::Wait => Shape::Wait,
    }
}

/// Whether [`request_cursor_shape`] has already logged that it has nowhere to send a shape. Set once per process: the condition (no protocol, or no pointer yet) does not change from one hover to the next, so repeating the line would only bury whatever else is logged at info level.
static CURSOR_SHAPE_UNAVAILABLE_LOGGED: AtomicBool = AtomicBool::new(false);

/// Asks the seat's pointer to show `cursor`'s shape, through `wp-cursor-shape-v1` (T-2.3). The only caller is [`LayerWindow::set_cursor`](crate::window::LayerWindow::set_cursor), which a box's `.cursor(…)` drives; a `Window` trait method gets nothing but `&self`, so [`CURSOR_SHAPE_DEVICE`] and [`LAST_POINTER_SERIAL`] are how this reaches back into driver state.
///
/// A no-op, logged once, on a compositor without the global or before a pointer capability has arrived: a handle still takes the drag or resize, and the pointer keeps whatever image it already had (F-2.12).
pub(crate) fn request_cursor_shape(cursor: Cursor) {
    let shape = cursor_shape(cursor);
    let sent = CURSOR_SHAPE_DEVICE.with(|device| {
        device
            .borrow()
            .as_ref()
            .map(|device| device.set_shape(LAST_POINTER_SERIAL.with(Cell::get), shape))
    });
    if sent.is_some() {
        return;
    }
    if !CURSOR_SHAPE_UNAVAILABLE_LOGGED.swap(true, Ordering::Relaxed) {
        tracing::info!(
            "wp-cursor-shape-v1 unavailable: handles still work, but the pointer image will not change"
        );
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
    key_of(event.keysym, event.utf8.as_deref())
}

fn key_of(keysym: Keysym, utf8: Option<&str>) -> Option<Key> {
    // Editing keys carry a control-char `utf8` (or none), so they must be resolved from the keysym — the printable `utf8` path below drops them.
    if let Some(named) = named_from_keysym(keysym) {
        return Some(Key::Named(named));
    }
    let ch = utf8?.chars().next()?;
    // xkb turns a letter held with Ctrl into a control char (Ctrl+Z is U+001A); the chord's key is the keysym's own character, which is what `Key::Char` means on every other backend.
    let ch = match ch.is_control() {
        true => keysym.key_char().filter(|ch| !ch.is_control())?,
        false => ch,
    };
    Some(Key::Char(ch))
}

/// Maps an xkb keysym to the editing/navigation [`NamedKey`] it represents, or `None` for keys that carry their own printable character. Mirrors platform-winit's named-key mapping, over xkb keysyms.
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
        Keysym::Menu => Some(NamedKey::ContextMenu),
        Keysym::F1 => Some(NamedKey::F1),
        Keysym::F2 => Some(NamedKey::F2),
        Keysym::F3 => Some(NamedKey::F3),
        Keysym::F4 => Some(NamedKey::F4),
        Keysym::F5 => Some(NamedKey::F5),
        Keysym::F6 => Some(NamedKey::F6),
        Keysym::F7 => Some(NamedKey::F7),
        Keysym::F8 => Some(NamedKey::F8),
        Keysym::F9 => Some(NamedKey::F9),
        Keysym::F10 => Some(NamedKey::F10),
        Keysym::F11 => Some(NamedKey::F11),
        Keysym::F12 => Some(NamedKey::F12),
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
        // The whole-number scale keeps arriving alongside the fractional one and says less: a 1.5× output announces 2 here. Taking it would resize the buffer to something the viewport then squeezes.
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
/// It arrives before the first configure and again whenever the surface moves to an output at another scale, so it is also what a surface dragged between a 1× and a 1.5× monitor redraws on.
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

/// The blur manager's one event, and it is live state rather than an answer: the protocol sends it when the global is bound *and again whenever the capabilities change*, and says that a capability which goes away stops being applied even to a surface that already set a region. So this is what [`background_effect_supported`] reads, and reading it once at startup would be reading a fact that can expire.
impl Dispatch<ExtBackgroundEffectManagerV1, ()> for Driver {
    fn event(
        _state: &mut Self,
        _proxy: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let ext_background_effect_manager_v1::Event::Capabilities { flags } = event else {
            return;
        };
        let blur = grants_blur(flags);
        tracing::debug!(
            "ext-background-effect-v1: blur {}",
            if blur { "granted" } else { "withdrawn" }
        );
        FACTS.with(|facts| facts.borrow_mut().background_effect_supported = blur);
    }
}

/// Whether a `capabilities` bitfield grants blur.
///
/// Read as bits rather than through the generated enum on purpose. `capability` is a bitfield, and a later version that adds a second effect will send both bits at once — which `Capability::from_bits` refuses *whole*, handing back an `Unknown` that carries the blur bit this build does understand. Matching on the enum alone would take a compositor that gained an effect for one that lost blur.
fn grants_blur(flags: WEnum<ext_background_effect_manager_v1::Capability>) -> bool {
    let bits = match flags {
        WEnum::Value(capabilities) => capabilities.bits(),
        WEnum::Unknown(bits) => bits,
    };
    bits & ext_background_effect_manager_v1::Capability::Blur.bits() != 0
}

// The per-surface effect object is written to and never read; a single-pixel buffer's only event is the `release` of a buffer that is never reused.
delegate_noop!(Driver: ignore ExtBackgroundEffectSurfaceV1);
delegate_noop!(Driver: ignore WpSinglePixelBufferManagerV1);
delegate_noop!(Driver: ignore wl_buffer::WlBuffer);
// Neither sends an event: the manager only mints devices, and a device is written to (`set_shape`) and never read.
delegate_noop!(Driver: ignore WpCursorShapeManagerV1);
delegate_noop!(Driver: ignore WpCursorShapeDeviceV1);

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
        // Before the refresh, so a monitor unplugged and plugged back in gets a fresh lock surface instead of being skipped as one this session already covered.
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
            // One device per pointer, minted with it rather than on the first `set_shape`: the manager has no per-pointer uniqueness rule to violate, but there is still only ever one pointer to speak for.
            if let (Some(manager), Some(pointer)) = (&self.cursor_shape_manager, &self.pointer) {
                let device = manager.get_pointer(pointer, qh, ());
                CURSOR_SHAPE_DEVICE.with(|d| *d.borrow_mut() = Some(device));
            }
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
            if let Some(device) = CURSOR_SHAPE_DEVICE.with(|d| d.borrow_mut().take()) {
                device.destroy();
            }
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
                    // Whether a press reached a surface at all is the one thing that distinguishes "our input region is wrong" from "the compositor acted on an event it also delivered to us". Logged at debug so `RUST_LOG=platform_wayland=debug` can answer it without a custom build.
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
                PointerEventKind::Enter { serial } => {
                    self.pointer_focus = Some(id.clone());
                    // `set_shape` must echo the latest `enter` serial or the compositor ignores it — kept beside the device rather than on the entry, since the request names no surface.
                    LAST_POINTER_SERIAL.with(|s| s.set(serial));
                }
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

    /// A headless window that counts the frames asked of it, so a test can tell a hidden surface that stays quiet from one that asks to be drawn.
    #[derive(Clone)]
    struct CountingWindow {
        inner: platform_headless::HeadlessWindow,
        redraws: Arc<AtomicUsize>,
    }

    type BoxedCountingHandler = Box<dyn EventHandler<CountingWindow>>;

    impl CountingWindow {
        fn new(width: u32, height: u32) -> Self {
            Self {
                inner: platform_headless::HeadlessWindow::new(width, height),
                redraws: Arc::default(),
            }
        }

        fn redraws(&self) -> usize {
            self.redraws.load(Ordering::Relaxed)
        }
    }

    impl raw_window_handle::HasWindowHandle for CountingWindow {
        fn window_handle(
            &self,
        ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
            self.inner.window_handle()
        }
    }

    impl raw_window_handle::HasDisplayHandle for CountingWindow {
        fn display_handle(
            &self,
        ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
            self.inner.display_handle()
        }
    }

    impl Window for CountingWindow {
        fn width(&self) -> u32 {
            self.inner.width()
        }

        fn height(&self) -> u32 {
            self.inner.height()
        }

        fn request_redraw(&self) {
            self.redraws.fetch_add(1, Ordering::Relaxed);
        }

        fn scale_factor(&self) -> f64 {
            self.inner.scale_factor()
        }

        fn is_offscreen(&self) -> bool {
            true
        }
    }

    /// A layer window's app whose tree owns what the app never sees — a signal made in `root`, a node and a ticker on the loop — and reports each of them, so a kept tree can be told from a rebuilt one.
    #[derive(Default)]
    struct Tracked {
        roots: Rc<std::cell::Cell<u32>>,
        local: Rc<std::cell::Cell<Option<telar::RwSignal<u32>>>>,
        node: Rc<std::cell::Cell<Option<telar::NodeId>>>,
        tree: Rc<RefCell<std::rc::Weak<()>>>,
    }

    impl App for Tracked {
        fn root(&self) -> Box<dyn telar::Component> {
            use telar::LayoutItem;

            telar::reset_layout_runtime();
            self.roots.set(self.roots.get() + 1);
            let local = telar::signal(0u32);
            self.local.set(Some(local));
            interval(Duration::from_secs(3600), || {});
            let alive = Rc::new(());
            *self.tree.borrow_mut() = Rc::downgrade(&alive);
            let tint = telar::Rectangle::new(
                telar::LayoutStyle::new().width(40.0).height(40.0),
                move || {
                    let _tree = &alive;
                    telar::RectStyle::filled(
                        telar::Color::rgba(local.get() as f32 / 10.0, 0.0, 0.0, 1.0),
                        0.0,
                    )
                },
            )
            .expect("a rectangle lays out");
            self.node.set(Some(tint.layout_node()));
            Box::new(telar::WindowRoot::new(telar::box_item(tint)))
        }

        fn window_config(&self) -> Option<WindowConfig> {
            Some(WindowConfig {
                is_transparent: true,
                ..WindowConfig::default()
            })
        }
    }

    /// Hiding a layer window and showing it again runs exactly the handler calls the driver makes — suspend to hide, resume to show — on one handler, against a real telar app over a headless window: the handler and the tree survive — no second build, the tree's own signal keeps its value, its node keeps its identity, its ticker stays registered once — while the renderer is gone for as long as the window is hidden and nothing asks the hidden window for a frame.
    #[test]
    fn a_hidden_and_shown_layer_window_keeps_its_handler_and_tree() {
        use smithay_client_toolkit::reexports::calloop::EventLoop;

        let event_loop: EventLoop<'static, Driver> =
            EventLoop::try_new().expect("an event loop needs no compositor");
        LOOP_HANDLE.with(|h| *h.borrow_mut() = Some(event_loop.handle()));

        let app = Tracked::default();
        let (roots, local, node, tree) = (
            Rc::clone(&app.roots),
            Rc::clone(&app.local),
            Rc::clone(&app.node),
            Rc::clone(&app.tree),
        );
        let window = CountingWindow::new(320, 200);
        let mut handler = build_surface_handler::<CountingWindow, _>(
            LocalApp(app),
            Arc::new(telar::NoPaths),
            "hogar-shell-test",
            telar::AppConfig::default(),
        );
        let sources = SourceSink::default();
        let draw = |handler: &mut BoxedCountingHandler| {
            with_current(&sources, || {
                handler.new_events();
                handler.on_redraw(&window);
                handler.about_to_wait();
            });
        };

        assert!(resume_handler(handler.as_mut(), &window, &sources));
        draw(&mut handler);
        assert!(
            handler.last_frame_rgba().is_some(),
            "precondition: a shown window has a renderer"
        );
        let first_node = node.get().expect("the tree placed its node");
        let first_tree = tree.borrow().clone();
        let first_local = local.get().expect("the tree made its signal");
        let redraws_when_shown = window.redraws();
        begin_batch();
        first_local.set(3);
        end_batch();
        assert!(
            window.redraws() > redraws_when_shown,
            "precondition: the same change on a shown window asks for a frame"
        );
        assert_eq!(roots.get(), 1);
        assert_eq!(sources.borrow().len(), 1, "the tree's ticker");

        suspend_handler(handler.as_mut(), &sources);
        assert!(
            handler.last_frame_rgba().is_none(),
            "a hidden window holds no renderer, and with it nothing it drew into"
        );
        let redraws_when_hidden = window.redraws();
        begin_batch();
        first_local.set(7);
        end_batch();
        assert_eq!(
            window.redraws(),
            redraws_when_hidden,
            "the tree changing while hidden asks the hidden window for no frame"
        );

        assert!(resume_handler(handler.as_mut(), &window, &sources));
        assert_eq!(roots.get(), 1, "showing the window built no second tree");
        assert_eq!(
            local.get().map(|signal| signal.peek()),
            Some(7),
            "the tree's own signal kept what was written to it while hidden"
        );
        assert_eq!(
            node.get(),
            Some(first_node),
            "the tree's node kept its identity"
        );
        assert!(
            first_tree.upgrade().is_some(),
            "the tree shown is the one built first, not a copy of it"
        );
        assert_eq!(
            sources.borrow().len(),
            1,
            "the ticker stayed registered once, neither dropped nor doubled"
        );

        draw(&mut handler);
        assert!(
            handler.last_frame_rgba().is_some(),
            "the renderer came back with the window, on the same handler"
        );

        LOOP_HANDLE.with(|h| *h.borrow_mut() = None);
    }

    /// The whole point of the pair: the scales a whole number cannot say.
    #[test]
    fn a_fractional_scale_reaches_the_buffer_it_asks_for() {
        assert_eq!(device_pixels(1000, 120), 1000, "1×");
        assert_eq!(device_pixels(1000, 180), 1500, "1.5×");
        assert_eq!(device_pixels(1000, 150), 1250, "1.25×");
        assert_eq!(device_pixels(1000, 240), 2000, "2×");
        // What the integer buffer scale did instead, and the reason this exists: the same 1.5× output rounds to 2 there, so the buffer was a third larger than the screen and the compositor scaled it back down.
        assert_ne!(device_pixels(1000, 180), device_pixels(1000, 240));
    }

    #[test]
    fn a_size_that_does_not_land_on_a_pixel_rounds_the_way_the_compositor_does() {
        // Half away from zero, per the protocol. Rounding down here would leave the last row of the surface outside the buffer that has to fill it.
        assert_eq!(device_pixels(31, 180), 47, "46.5 rounds up");
        assert_eq!(device_pixels(33, 180), 50, "49.5 rounds up");
        assert_eq!(device_pixels(7, 150), 9, "8.75 rounds up");
        assert_eq!(device_pixels(9, 150), 11, "11.25 rounds down");
        // A surface can be configured to nothing on an axis it does not own; a zero-sized buffer is not a buffer, and every renderer behind this asks for at least one pixel.
        assert_eq!(device_pixels(0, 180), 1);
    }

    /// A reservation strip's buffer is 1×1 whatever the strip covers, so the destination its `wp_viewport` is given has to be the whole strip: the output's length on the axis it spans, its own thickness on the axis it holds. Which axis is which is the only thing the edge changes.
    #[test]
    fn a_strips_viewport_destination_covers_its_whole_edge() {
        assert_eq!(viewport_destination((2560, 32)), (2560, 32), "top");
        assert_eq!(viewport_destination((2560, 48)), (2560, 48), "bottom");
        assert_eq!(viewport_destination((32, 1440)), (32, 1440), "left");
        assert_eq!(viewport_destination((48, 1440)), (48, 1440), "right");
        // A strip is created with a zero on the axis the compositor fills, and is driven for a turn before the first configure says what that is. A zero destination is `wp_viewport`'s `bad_value`, which kills the connection rather than the surface, so the floor is the protocol's requirement and not a convenience.
        assert_eq!(
            viewport_destination((0, 32)),
            (1, 32),
            "a horizontal strip before its first configure"
        );
        assert_eq!(
            viewport_destination((32, 0)),
            (32, 1),
            "a vertical strip before its first configure"
        );
    }

    /// The whole of "only on change", for both of this crate's region requests: each is double-buffered state that costs a commit to land, so content asking for the same region every frame must commit once.
    #[test]
    fn a_region_asked_for_twice_is_one_commit() {
        let asked = vec![
            telar::Rect::new(8.0, 4.0, 120.0, 32.0),
            telar::Rect::new(200.0, 4.0, 64.0, 32.0),
        ];
        let applied = region_change(asked.clone(), &[]).expect("nothing has been applied yet");
        assert_eq!(applied, vec![(8, 4, 120, 32), (200, 4, 64, 32)]);
        assert!(
            region_change(asked, &applied).is_none(),
            "the same rects again must not cost a second commit"
        );

        // The same region enumerated the other way round. A layout pass is free to walk its widgets in any order, and sorting is what keeps a reordered read from looking like a change.
        let reordered = vec![
            telar::Rect::new(200.0, 4.0, 64.0, 32.0),
            telar::Rect::new(8.0, 4.0, 120.0, 32.0),
        ];
        assert!(region_change(reordered, &applied).is_none());

        // One rect a pixel to the right is a change, and so is giving the region up — which for blur is the NULL region that removes the effect rather than a region of no area.
        let moved = vec![
            telar::Rect::new(9.0, 4.0, 120.0, 32.0),
            telar::Rect::new(200.0, 4.0, 64.0, 32.0),
        ];
        assert!(region_change(moved, &applied).is_some());
        assert_eq!(region_change(Vec::new(), &applied), Some(Vec::new()));
        assert!(
            region_change(Vec::new(), &[]).is_none(),
            "and a surface that never had a region does not commit to say so again"
        );
    }

    /// Rounded outward, so a card laid out on a half pixel has the whole of itself blurred rather than a row of it left sharp — and the same rule keeps the edge of a button taking clicks.
    #[test]
    fn a_region_rect_on_a_half_pixel_rounds_outward() {
        let applied = region_change(vec![telar::Rect::new(8.5, 4.25, 120.5, 32.5)], &[])
            .expect("nothing has been applied yet");
        assert_eq!(applied, vec![(8, 4, 121, 33)]);
    }

    /// A compositor that *gains* an effect must not read as one that lost blur, which is what taking the bitfield through the generated enum alone would do.
    #[test]
    fn a_capability_bitfield_this_build_only_half_knows_still_grants_blur() {
        use ext_background_effect_manager_v1::Capability;

        assert!(grants_blur(WEnum::Value(Capability::Blur)));
        assert!(
            !grants_blur(WEnum::Value(Capability::empty())),
            "a bound manager that grants nothing is not a manager that blurs"
        );
        // Blur alongside an effect added in a later version: `from_bits` refuses the pair whole, so it arrives as the raw bits.
        assert!(grants_blur(WEnum::Unknown(0b11)));
        assert!(
            !grants_blur(WEnum::Unknown(0b10)),
            "an effect this build does not know is not blur"
        );
    }

    /// Ctrl+Z reaches a key handler as `z` with Ctrl held, not as nothing: the control char xkb hands for the chord is answered from the keysym, and a key with no text at all stays silent.
    #[test]
    fn a_ctrl_chord_arrives_as_the_character_it_was_typed_on() {
        assert_eq!(key_of(Keysym::z, Some("\u{1a}")), Some(Key::Char('z')));
        assert_eq!(key_of(Keysym::Z, Some("\u{1a}")), Some(Key::Char('Z')));
        assert_eq!(key_of(Keysym::y, Some("y")), Some(Key::Char('y')));
        assert_eq!(key_of(Keysym::Shift_L, None), None);
    }

    /// The two ways a keyboard opens a context menu — the menu key, and Shift+F10 — arrive as keys rather than as nothing.
    #[test]
    fn the_keys_that_open_a_context_menu_are_named() {
        assert_eq!(
            key_of(Keysym::Menu, None),
            Some(Key::Named(NamedKey::ContextMenu))
        );
        assert_eq!(key_of(Keysym::F10, None), Some(Key::Named(NamedKey::F10)));
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

    /// The resize and grab shapes an edit-mode handle needs (T-2.3) map onto the protocol's own names for them, with nothing lost or substituted in translation.
    #[test]
    fn every_cursor_maps_to_its_namesake_shape() {
        use wp_cursor_shape_device_v1::Shape;

        assert_eq!(cursor_shape(Cursor::Default), Shape::Default);
        assert_eq!(cursor_shape(Cursor::Pointer), Shape::Pointer);
        assert_eq!(cursor_shape(Cursor::Crosshair), Shape::Crosshair);
        assert_eq!(cursor_shape(Cursor::Grab), Shape::Grab);
        assert_eq!(cursor_shape(Cursor::Grabbing), Shape::Grabbing);
        assert_eq!(cursor_shape(Cursor::ColResize), Shape::ColResize);
        assert_eq!(cursor_shape(Cursor::RowResize), Shape::RowResize);
        assert_eq!(cursor_shape(Cursor::EwResize), Shape::EwResize);
        assert_eq!(cursor_shape(Cursor::NsResize), Shape::NsResize);
        assert_eq!(cursor_shape(Cursor::NwseResize), Shape::NwseResize);
        assert_eq!(cursor_shape(Cursor::NeswResize), Shape::NeswResize);
        assert_eq!(cursor_shape(Cursor::Move), Shape::Move);
        assert_eq!(cursor_shape(Cursor::Text), Shape::Text);
        assert_eq!(cursor_shape(Cursor::NotAllowed), Shape::NotAllowed);
        assert_eq!(cursor_shape(Cursor::Wait), Shape::Wait);
    }

    /// Without a device — no protocol, or no pointer yet — the request is dropped rather than panicking, and [`request_cursor_shape`] does not need a live driver to be called this way.
    #[test]
    fn requesting_a_shape_with_no_device_is_a_silent_no_op() {
        CURSOR_SHAPE_DEVICE.with(|d| assert!(d.borrow().is_none()));
        request_cursor_shape(Cursor::Grab);
    }
}
