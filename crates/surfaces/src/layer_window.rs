//! The window of one layer on one output: what is drawn in it, whether it is on screen at all, and what it asks of the compositor.
//!
//! One fullscreen layer-shell window per `(output, layer)`, anchored to all four edges and ignoring every exclusive zone, so **every window on an output shares one coordinate space**: a rect laid out in the top window is the same rect in the overlay one. [`LayerWindows`] owns them, keyed so that a layout change reaches the window that is already up rather than reopening it — the same identity rule the surface reconcile has always had, now over four windows a screen instead of one a role.
//!
//! **A window is on screen only while its layer has something to show**, and how it goes away depends on what it costs to leave it there. Background, desktop and top windows are opened with their output and merely hidden, so coming back is a buffer-less commit rather than a cold map (F-6.11 measured that at ~20 ms to first frame). The overlay window is the exception and is closed outright, because on Hyprland the *surface* is what costs the output direct scanout, from `get_layer_surface` to destroy, whether or not it was ever mapped — see [`WhileEmpty`], which is where that is written down. [`Presence`] holds the rule; the layout and whatever [`Hold`]s a window decide the answer.
//!
//! What a bar, a grid or a stack looks like is not here. [`Areas`] is the seam, and it is one call per area per build returning one node — a builder is never asked where it put the area, because where an area ends up is a question layout answers and the window reads off the node afterwards. The host knows which areas live on which layer, whether that layer is on screen, and what it asks of the compositor; it knows nothing about what any of it draws.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use telar::{
    App, Color, Component, Container, LayoutError, LayoutItem, LayoutStyle, ReactiveList,
    ReadSignal, Rect, RwSignal, ScopedTheme, SizeDimension, WindowConfig, WindowRoot, box_item,
    effect, on_cleanup, provide_theme, reset_layout_runtime, set_context, signal, track_layout,
};

use config::theme::NordTheme;
use config::{Config, Edge, LiveConfig};
use layout::{
    AreaId, Backdrop, LayerKind, Resolved, ResolvedArea, ResolvedAreaKind, ResolvedLayer, Within,
};
use platform_wayland::{
    KeyboardInteractivity, KeyboardMode, Layer, LayerWindowHandle, background_effect_supported,
    open_layer_window,
};

/// A window's identity across a reload: which screen it is on and which layer it is, and nothing else. Two windows with the same key are the same window before and after any edit.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey {
    /// The connector name, or `None` where the compositor chooses the screen.
    pub output: Option<String>,
    pub layer: LayerKind,
}

/// One output's resolved arrangement, and what its windows draw against.
#[derive(Clone, Copy)]
pub struct LayerPlan<'a> {
    /// The connector name to open on, or `None` to let the compositor choose — which is what a session with no named output gets.
    pub output: Option<&'a str>,
    /// The global config merged with this monitor's override, which is what the window's theme and every module's behaviour resolve against.
    pub config: &'a Arc<Config>,
    pub resolved: &'a Resolved,
    /// What this output's reserving areas take off each edge. Planned once for the whole output, because the strips that commit it are planned from the same number and two derivations of it would be two answers to what a window may have.
    pub reserved: Reserved,
    /// This monitor's logical size. Every window on it is the whole output, so it is also every window's size, and it is what turns a fractional rect into pixels.
    pub size: (f32, f32),
}

/// The four edges an output's reserving areas have taken, in logical pixels.
///
/// It is gathered across *every* layer, because reservation is an output-level fact: a bar in the top window and a reserving dock on the desktop both take space from the same screen, and neither can see the other. That is exactly why an area cannot work this out for itself and the host hands it down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reserved {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Reserved {
    /// What `resolved` takes off each edge of the output it describes.
    ///
    /// The config is here for the one part of the answer the model cannot give: a bar floats at its gap from its edge, and a bar that floats reserves that air too, or a window would tile under it. The gap is the bar's own, except that `[shape] frame` takes it away, so it is resolved where the config is.
    ///
    /// Only an output-level area may reserve, so the bars read here are the ones every workspace on the screen shares.
    pub fn of(resolved: &Resolved, config: &Config) -> Self {
        let air = |edge: Edge| {
            resolved
                .areas()
                .filter_map(|(_, area)| match area.kind {
                    ResolvedAreaKind::Bar {
                        edge: on, shape, ..
                    } if on == edge && area.reserve => {
                        Some(config.gap_of(&crate::bar::bar_shape(config, shape)) as f32)
                    }
                    _ => None,
                })
                .fold(0.0, f32::max)
        };
        let on = |edge: Edge| {
            let depth = resolved.reserved(edge);
            match depth > 0.0 {
                true => depth + air(edge),
                false => 0.0,
            }
        };
        Self {
            top: on(Edge::Top),
            right: on(Edge::Right),
            bottom: on(Edge::Bottom),
            left: on(Edge::Left),
        }
    }

    pub fn on(&self, edge: Edge) -> f32 {
        match edge {
            Edge::Top => self.top,
            Edge::Right => self.right,
            Edge::Bottom => self.bottom,
            Edge::Left => self.left,
        }
    }

    /// The box an area measures against: the whole output, or what the reserving areas left of it.
    pub fn box_of(&self, within: Within, size: (f32, f32)) -> Rect {
        match within {
            Within::Output => Rect::new(0.0, 0.0, size.0, size.1),
            Within::Usable => Rect::new(
                self.left,
                self.top,
                (size.0 - self.left - self.right).max(0.0),
                (size.1 - self.top - self.bottom).max(0.0),
            ),
        }
    }
}

/// What a reconcile does to the windows that stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    /// The config changed, which every area is built against: every window that survives builds all of its areas again.
    Rebuild,
    /// Only the arrangement may have changed — a layout edit, a monitor plugged in: each window builds again the areas whose own arrangement, box or screen changed and keeps every other node as it is, so an edit repaints what it touched and a region's picture is not cut off mid-fade by an edit elsewhere.
    Changed,
}

/// What a reconcile did, for the log — and for a test that cares that a layout change reached the windows already up instead of reopening them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    pub opened: usize,
    pub closed: usize,
    /// How many windows built every one of their areas again.
    pub rebuilt: usize,
    /// How many windows are on screen once the reconcile is done, which is the count lazy mapping is about.
    pub mapped: usize,
}

/// The live layer windows, keyed so a reload finds the one it is about.
pub struct LayerWindows {
    areas: Rc<dyn Areas>,
    live: Vec<(WindowKey, Window)>,
    holder: Holder,
}

impl LayerWindows {
    /// A host whose windows build their areas with `areas`.
    pub fn new(areas: Rc<dyn Areas>) -> Self {
        Self {
            areas,
            live: Vec::new(),
            holder: Holder::default(),
        }
    }

    pub fn holder(&self) -> Holder {
        self.holder.clone()
    }

    /// Brings the windows in line with `plans`: hands each one its layer's new arrangement, rebuilds it where the content changed, opens the windows a newly plugged monitor needs and closes the ones an unplugged monitor left behind.
    ///
    /// Closing runs first, so an output that went away gives its surfaces up before anything else is measured against the screen they were on.
    pub fn reconcile(&mut self, plans: &[LayerPlan<'_>], content: Content) -> Reconciled {
        let wanted: HashSet<WindowKey> = plans
            .iter()
            .flat_map(|plan| {
                LayerKind::SESSION.map(|layer| WindowKey {
                    output: plan.output.map(str::to_owned),
                    layer,
                })
            })
            .collect();
        let before = self.live.len();
        self.live.retain(|(key, _)| wanted.contains(key));

        let mut done = Reconciled {
            closed: before - self.live.len(),
            ..Reconciled::default()
        };
        for plan in plans {
            for layer in LayerKind::SESSION {
                let key = WindowKey {
                    output: plan.output.map(str::to_owned),
                    layer,
                };
                let resolved = WindowAreas::of(plan.resolved, layer);
                match self.index_of(&key) {
                    Some(index) => self.live[index].1.adopt(resolved, plan, content, &mut done),
                    None => self.open(key, resolved, plan, &mut done),
                }
            }
        }
        done.mapped = self
            .live
            .iter()
            .filter(|(_, window)| window.presence.is_mapped())
            .count();
        self.holder.set(
            self.live
                .iter()
                .map(|(key, window)| LiveWindow {
                    key: key.clone(),
                    presence: Rc::downgrade(&window.presence),
                    demands: Rc::downgrade(&window.demands),
                    generation: window.generation.clone(),
                })
                .collect(),
        );
        crate::transient::prune(
            &self
                .live
                .iter()
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>(),
        );
        tracing::debug!(
            opened = done.opened,
            closed = done.closed,
            rebuilt = done.rebuilt,
            mapped = done.mapped,
            live = self.live.len(),
            "layer windows reconciled"
        );
        done
    }

    /// Keeps the window of `layer` on `output` on screen for as long as the returned hold lives, whatever its layout says — an unanchored transient, a stack card, an edit-mode host.
    ///
    /// The holder decides when the window may go, so a launcher gives its hold up once its exit transition has run rather than when it was asked to close. `None` where that window is not open, which is every layer on a screen the compositor has not reported.
    pub fn hold(&self, output: Option<&str>, layer: LayerKind) -> Option<Hold> {
        self.window(output, layer)
            .map(|window| Hold::new(&window.presence))
    }

    /// Whether that window is on screen — what the host last asked the compositor for, which is the whole of what lazy mapping decides.
    pub fn is_mapped(&self, output: Option<&str>, layer: LayerKind) -> bool {
        self.window(output, layer)
            .is_some_and(|window| window.presence.is_mapped())
    }

    /// Whether that window's surface exists at all. For the overlay layer this is the question worth asking, because the surface costs its output direct scanout from the moment it is created (see [`WhileEmpty`]).
    pub fn is_open(&self, output: Option<&str>, layer: LayerKind) -> bool {
        self.window(output, layer)
            .is_some_and(|window| window.presence.is_open())
    }

    /// What that window's mounted nodes currently ask of the compositor.
    pub fn demands(&self, output: Option<&str>, layer: LayerKind) -> Option<Rc<Demands>> {
        self.window(output, layer)
            .map(|window| Rc::clone(&window.demands))
    }

    /// Whether that window's own areas are set aside for an edit mode drawing them elsewhere (see [`Concealment`]).
    pub fn is_concealed(&self, output: Option<&str>, layer: LayerKind) -> bool {
        self.window(output, layer)
            .is_some_and(|window| window.generation.is_concealed())
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    fn window(&self, output: Option<&str>, layer: LayerKind) -> Option<&Window> {
        let key = WindowKey {
            output: output.map(str::to_owned),
            layer,
        };
        self.index_of(&key).map(|index| &self.live[index].1)
    }

    fn index_of(&self, key: &WindowKey) -> Option<usize> {
        self.live.iter().position(|(live, _)| live == key)
    }

    fn open(
        &mut self,
        key: WindowKey,
        resolved: WindowAreas,
        plan: &LayerPlan<'_>,
        done: &mut Reconciled,
    ) {
        let Some((wlr, namespace)) = window_layer(key.layer) else {
            return;
        };
        let layer = LiveLayer::new(resolved);
        let config = LiveConfig::new(Arc::clone(plan.config));
        let demands = Rc::new(Demands::new(wlr));
        let screen = Rc::new(Cell::new(Screen {
            size: plan.size,
            reserved: plan.reserved,
        }));
        let generation = Generation::default();
        let shown = ScreenFeed::default();
        let mapped = MappedFeed::default();
        let surface = {
            let kind = key.layer;
            let output = key.output.clone();
            let layer = layer.clone();
            let config = config.clone();
            let demands = Rc::clone(&demands);
            let screen = Rc::clone(&screen);
            let generation = generation.clone();
            let shown = shown.clone();
            let mapped = mapped.clone();
            let areas = Rc::clone(&self.areas);
            move || {
                let on = demands.layer();
                let handle = Rc::new(open_layer_window(
                    output.clone(),
                    on,
                    namespace,
                    LayerApp {
                        kind,
                        output: output.clone(),
                        layer: layer.clone(),
                        config: config.clone(),
                        demands: Rc::clone(&demands),
                        screen: Rc::clone(&screen),
                        generation: generation.clone(),
                        shown: shown.clone(),
                        mapped: mapped.clone(),
                        areas: Rc::clone(&areas),
                    },
                ));
                demands.rebind(Rc::downgrade(&handle), on);
                handle
            }
        };
        let presence = Presence::new(while_empty(key.layer), Box::new(surface), mapped);
        presence.set_draws(window_draws(&layer.get()));
        done.opened += 1;
        self.live.push((
            key,
            Window {
                layer,
                config,
                demands,
                screen,
                generation,
                shown,
                presence,
            },
        ));
    }
}

/// One live window and everything that outlives any one build of it — and, for a layer whose empty window is closed rather than hidden, everything that outlives the surface itself.
struct Window {
    layer: LiveLayer,
    config: LiveConfig,
    demands: Rc<Demands>,
    /// Shared with the window: this output's size and reserved edges, both of which change under a window that stays and neither of which one layer can work out on its own.
    screen: Rc<Cell<Screen>>,
    generation: Generation,
    shown: ScreenFeed,
    presence: Rc<Presence>,
}

impl Window {
    fn adopt(
        &self,
        resolved: WindowAreas,
        plan: &LayerPlan<'_>,
        content: Content,
        done: &mut Reconciled,
    ) {
        self.layer.set(resolved);
        self.config.set(Arc::clone(plan.config));
        self.screen.set(Screen {
            size: plan.size,
            reserved: plan.reserved,
        });
        self.shown.set(self.screen.get());
        match content {
            Content::Rebuild => {
                self.generation.bump();
                done.rebuilt += 1;
            }
            Content::Changed => self.generation.look_again(),
        }
        self.presence.set_draws(window_draws(&self.layer.get()));
    }
}

/// Closing the host closes its windows, even where a [`Hold`] outlives it and keeps the [`Presence`] alive.
impl Drop for Window {
    fn drop(&mut self) {
        self.presence.shut();
    }
}

/// The arrangement a live window builds from.
///
/// A shared cell rather than a plain value, for the reason [`LiveConfig`] is one: the window outlives any one layout, and after an edit it is the same window with something else to show. The reconcile writes, the window's next build reads.
#[derive(Clone)]
pub struct LiveLayer(Rc<RefCell<Rc<WindowAreas>>>);

impl LiveLayer {
    pub fn new(layer: WindowAreas) -> Self {
        Self(Rc::new(RefCell::new(Rc::new(layer))))
    }

    pub fn get(&self) -> Rc<WindowAreas> {
        Rc::clone(&self.0.borrow())
    }

    pub fn set(&self, layer: WindowAreas) {
        *self.0.borrow_mut() = Rc::new(layer);
    }

    /// Whether two handles are the same cell — which is what "the same window" means across a reload.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// How a layer's window is put away once there is nothing in it.
///
/// **This is not a preference, it is a compositor fact.** On Hyprland a layer surface joins its monitor's per-layer list in `CLayerSurface::create`, which runs on `get_layer_surface` — before any buffer, before any map (`src/desktop/view/LayerSurface.cpp:57`) — and leaves it only in the destructor (`:100`). The direct-scanout check is `!m_layerSurfaceLayers[OVERLAY].empty()` (`src/output/Monitor.cpp`), the bare emptiness of that list, with no mapped or alpha test — unlike the `TOP` list right below it, which is walked surface by surface and only counts one whose fade alpha is non-zero.
///
/// So an **overlay** surface costs its output direct scanout from creation to destruction, and `set_mapped(false)` does not give it back: only closing does. Every other layer is safe to leave open and merely hide, and is, because reopening costs a cold map — 8–9 ms of commit and a ~20 ms first frame (F-6.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WhileEmpty {
    /// Hidden with `set_mapped(false)`: the surface stays, so coming back is a buffer-less commit and the next configure.
    Hide,
    /// Closed outright, because the surface itself is what costs something.
    Close,
}

fn while_empty(layer: LayerKind) -> WhileEmpty {
    match layer {
        LayerKind::Overlay => WhileEmpty::Close,
        _ => WhileEmpty::Hide,
    }
}

/// Whether a window is on screen, and why.
///
/// Two independent reasons, because they change on their own terms: the layout resolves something visible on that layer, or something holds the window open that no layout mentions. Either way the compositor is asked only when the answer between them changes, and what it is asked for is [`WhileEmpty`]'s — a redundant map is a frame nobody wanted, and a redundant overlay surface is an output that cannot scan out.
pub struct Presence {
    empty: WhileEmpty,
    /// The live surface. `None` only for a [`WhileEmpty::Close`] layer with nothing in it, which is the overlay window's resting state.
    window: RefCell<Option<Rc<LayerWindowHandle>>>,
    /// Opens the surface again over the cells it never gave up, so a reopened window is the same window: the same arrangement, the same config and the same demands, rebuilt.
    surface: Box<dyn Fn() -> Rc<LayerWindowHandle>>,
    draws: Cell<bool>,
    holds: Cell<usize>,
    /// Whether the compositor has the window on screen. A layer surface is created wanted, so a hidden layer starts true and its first settle is what takes it back off.
    on_screen: Cell<bool>,
    /// Set when the host lets the window go, so a [`Hold`] that outlives it cannot open a surface nothing owns.
    shut: Cell<bool>,
    /// What the window's tree reads [`Presence::on_screen`] through.
    mapped: MappedFeed,
}

impl Presence {
    fn new(
        empty: WhileEmpty,
        surface: Box<dyn Fn() -> Rc<LayerWindowHandle>>,
        mapped: MappedFeed,
    ) -> Rc<Self> {
        let on_screen = empty == WhileEmpty::Hide;
        mapped.set(on_screen);
        let window = on_screen.then(&surface);
        Rc::new(Self {
            empty,
            on_screen: Cell::new(on_screen),
            window: RefCell::new(window),
            surface,
            draws: Cell::new(false),
            holds: Cell::new(0),
            shut: Cell::new(false),
            mapped,
        })
    }

    pub fn is_mapped(&self) -> bool {
        self.on_screen.get()
    }

    /// Whether the surface exists at all, which for the overlay layer is the question that matters rather than whether it is mapped.
    pub fn is_open(&self) -> bool {
        self.window.borrow().is_some()
    }

    fn set_draws(&self, draws: bool) {
        self.draws.set(draws);
        self.settle();
    }

    fn shut(&self) {
        self.shut.set(true);
        self.on_screen.set(false);
        self.mapped.set(false);
        let closing = self.window.borrow_mut().take();
        drop(closing);
    }

    fn settle(&self) {
        if self.shut.get() {
            return;
        }
        let wanted = self.draws.get() || self.holds.get() > 0;
        if wanted == self.on_screen.get() {
            return;
        }
        self.on_screen.set(wanted);
        self.mapped.set(wanted);
        match (self.empty, wanted) {
            (WhileEmpty::Close, true) => *self.window.borrow_mut() = Some((self.surface)()),
            (WhileEmpty::Close, false) => {
                let closing = self.window.borrow_mut().take();
                drop(closing);
            }
            (WhileEmpty::Hide, _) => {
                if let Some(window) = self.window.borrow().as_ref() {
                    window.set_mapped(wanted);
                }
            }
        }
    }
}

/// A reason a window is on screen that its layout does not give.
///
/// Dropping it is what unmaps the window, so whatever plays an exit transition holds on until the exit has run rather than until it was asked to close.
pub struct Hold(Rc<Presence>);

impl Hold {
    fn new(presence: &Rc<Presence>) -> Self {
        presence.holds.set(presence.holds.get() + 1);
        presence.settle();
        Self(Rc::clone(presence))
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.0.holds.set(self.0.holds.get().saturating_sub(1));
        self.0.settle();
    }
}

/// The live windows as the last reconcile left them, for what reaches a window by its key without owning the host: the transient registry and an edit mode.
#[derive(Clone, Default)]
pub struct Holder(Rc<RefCell<Vec<LiveWindow>>>);

struct LiveWindow {
    key: WindowKey,
    presence: Weak<Presence>,
    demands: Weak<Demands>,
    generation: Generation,
}

impl Holder {
    fn set(&self, live: Vec<LiveWindow>) {
        *self.0.borrow_mut() = live;
    }

    pub fn hold(&self, key: &WindowKey) -> Option<Hold> {
        let presence = self
            .0
            .borrow()
            .iter()
            .find(|live| live.key == *key)
            .and_then(|live| live.presence.upgrade())?;
        (!presence.shut.get()).then(|| Hold::new(&presence))
    }

    /// What that window's mounted nodes ask of the compositor, and where anything else asks it for more.
    pub fn demands(&self, key: &WindowKey) -> Option<Rc<Demands>> {
        self.0
            .borrow()
            .iter()
            .find(|live| live.key == *key)
            .and_then(|live| live.demands.upgrade())
    }

    /// Sets that window's own areas aside for as long as the token lives. `None` where the window is not open.
    pub fn conceal(&self, key: &WindowKey) -> Option<Concealment> {
        self.0
            .borrow()
            .iter()
            .find(|live| live.key == *key && live.presence.strong_count() > 0)
            .map(|live| Concealment::new(&live.generation))
    }

    pub fn keys(&self) -> Vec<WindowKey> {
        self.0
            .borrow()
            .iter()
            .map(|live| live.key.clone())
            .collect()
    }
}

/// The screen a live window covers, as a signal its transients place themselves by: a layout edit that moves a reserving bar changes the box a drawer is kept inside without rebuilding it.
#[derive(Clone, Default)]
struct ScreenFeed(Rc<Cell<Option<RwSignal<Screen>>>>);

impl ScreenFeed {
    fn set(&self, screen: Screen) {
        if let Some(signal) = self.0.get().filter(RwSignal::is_alive) {
            signal.set(screen);
        }
    }

    fn attach(&self, signal: RwSignal<Screen>) {
        self.0.set(Some(signal));
    }
}

/// Whether a window is on screen, as a signal its tree reads: what an expression drawn in it subscribes by, since a hidden window keeps its tree (F-2.16) and tree disposal alone would never stop one. The host writes it as the window is mapped and hidden; a tree built before the host first says reads it as on screen.
#[derive(Clone)]
pub(crate) struct MappedFeed {
    now: Rc<Cell<bool>>,
    signal: Rc<Cell<Option<RwSignal<bool>>>>,
}

impl Default for MappedFeed {
    fn default() -> Self {
        Self {
            now: Rc::new(Cell::new(true)),
            signal: Rc::default(),
        }
    }
}

impl MappedFeed {
    fn set(&self, mapped: bool) {
        self.now.set(mapped);
        if let Some(signal) = self.signal.get().filter(RwSignal::is_alive)
            && signal.peek() != mapped
        {
            signal.set(mapped);
        }
    }

    fn attach(&self) -> ReadSignal<bool> {
        let mapped = signal(self.now.get());
        self.signal.set(Some(mapped));
        mapped.read_only()
    }
}

/// Which build of its areas a window is on, and whether that build draws them at all. Bumping it rebuilds every area and nothing else, so a config edit leaves the transients above them — and whatever the user is doing in one — exactly as they were; looking again rebuilds only the areas whose arrangement changed, which is what a layout edit does.
#[derive(Clone, Default)]
struct Generation {
    signal: Rc<Cell<Option<RwSignal<Builds>>>>,
    /// How many [`Concealment`]s set this window's areas aside.
    concealed: Rc<Cell<usize>>,
}

/// What a window's areas were last asked to do: `build` moves when every one of them builds again, `look` when the arrangement they are drawn from may have changed under them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Builds {
    build: u64,
    look: u64,
}

impl Generation {
    fn bump(&self) {
        self.touch(|builds| builds.build = builds.build.wrapping_add(1));
    }

    fn look_again(&self) {
        self.touch(|builds| builds.look = builds.look.wrapping_add(1));
    }

    fn touch(&self, change: impl FnOnce(&mut Builds)) {
        if let Some(signal) = self.signal.get().filter(RwSignal::is_alive) {
            signal.update(change);
        }
    }

    fn attach(&self, signal: RwSignal<Builds>) {
        self.signal.set(Some(signal));
    }

    fn is_concealed(&self) -> bool {
        self.concealed.get() > 0
    }
}

/// A window's own areas set aside for as long as this lives: an edit mode on a compositor that cannot raise the window draws the same areas in the overlay window instead (TA-4, R-6), and the window underneath must not draw them a second time. The window keeps its surface, its mapped state and its transients; only its areas are rebuilt, empty now and again once the last token goes.
pub struct Concealment(Generation);

impl Concealment {
    fn new(generation: &Generation) -> Self {
        generation.concealed.set(generation.concealed.get() + 1);
        generation.bump();
        Self(generation.clone())
    }
}

impl Drop for Concealment {
    fn drop(&mut self) {
        let concealed = &self.0.concealed;
        concealed.set(concealed.get().saturating_sub(1));
        self.0.bump();
    }
}

/// What one window's content asks of the compositor, as against what its layout puts in it.
///
/// Every answer is renegotiated the moment it changes rather than read once per build, because each is about what is mounted *now*: a `wl_surface` has exactly one keyboard interactivity and one layer, and the launcher that wants the keyboard opens and closes without the window it lives in being rebuilt. A demand is a token, so a node takes its own back by dropping one and disturbs nothing else's.
pub struct Demands {
    window: RefCell<Weak<LayerWindowHandle>>,
    /// The layer the window is opened on and goes back to once nothing raises it.
    home: Layer,
    keyboard: RefCell<BTreeMap<u64, KeyboardMode>>,
    raises: RefCell<BTreeMap<u64, Layer>>,
    next: Cell<u64>,
    /// What the compositor was last told. A layer window is created taking no keyboard at all, on the layer it was opened on and with no blur region, so none is pushed until something asks for more.
    sent_keyboard: Cell<KeyboardMode>,
    sent_layer: Cell<Layer>,
    sent_blur: RefCell<Vec<Rect>>,
}

impl Demands {
    pub fn new(home: Layer) -> Self {
        Self {
            window: RefCell::new(Weak::new()),
            home,
            keyboard: RefCell::new(BTreeMap::new()),
            raises: RefCell::new(BTreeMap::new()),
            next: Cell::new(0),
            sent_keyboard: Cell::new(KeyboardMode::None),
            sent_layer: Cell::new(home),
            sent_blur: RefCell::new(Vec::new()),
        }
    }

    /// Asks the window to take the keyboard at least this way for as long as the token lives. What the window negotiates is the maximum of every live demand: `None` < `OnDemand` < `Exclusive`.
    pub fn keyboard(self: &Rc<Self>, want: KeyboardMode) -> Demand {
        let id = self.token();
        self.keyboard.borrow_mut().insert(id, want);
        self.settle_keyboard();
        Demand {
            demands: Rc::clone(self),
            id,
        }
    }

    /// The interactivity the window currently negotiates.
    pub fn keyboard_wanted(&self) -> KeyboardMode {
        self.keyboard
            .borrow()
            .values()
            .copied()
            .max_by_key(|mode| keyboard_rank(*mode))
            .unwrap_or(KeyboardMode::None)
    }

    /// Moves the window up to `to` for as long as the token lives — an edit mode lifting the desktop above the user's windows (TA-4). The window sits on the highest of its own layer and every live raise, so a raise below where it already is changes nothing, and the last token dropped puts it back where it was opened.
    ///
    /// Asked for whether or not the compositor can do it: below `zwlr_layer_shell_v1` version 2 the request is dropped and logged, and the window stays put. [`platform_wayland::layer_restack_supported`] is the question to ask first, for a caller with another way to show the window's content (R-6).
    pub fn raise(self: &Rc<Self>, to: Layer) -> Demand {
        let id = self.token();
        self.raises.borrow_mut().insert(id, to);
        self.settle_layer();
        Demand {
            demands: Rc::clone(self),
            id,
        }
    }

    /// The layer the window is asked to be on: its own, or the highest a live [`raise`](Self::raise) asks for.
    pub fn layer(&self) -> Layer {
        self.raises
            .borrow()
            .values()
            .copied()
            .chain([self.home])
            .max_by_key(|layer| layer_rank(*layer))
            .unwrap_or(self.home)
    }

    /// What the compositor is currently asked to blur behind.
    pub fn blur_region(&self) -> Vec<Rect> {
        self.sent_blur.borrow().clone()
    }

    fn token(&self) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }

    /// Points the demands at a surface opened on `layer`, which for the overlay layer happens again every time its window is reopened.
    ///
    /// What was already pushed is forgotten with the surface that held it: a new one is created taking no keyboard and with no blur region, so both have to be asked for again rather than diffed against what a surface that no longer exists was told.
    fn rebind(&self, window: Weak<LayerWindowHandle>, layer: Layer) {
        *self.window.borrow_mut() = window;
        self.sent_keyboard.set(KeyboardMode::None);
        self.sent_layer.set(layer);
        self.sent_blur.borrow_mut().clear();
        self.settle_keyboard();
        self.settle_layer();
    }

    fn withdraw(&self, id: u64) {
        self.keyboard.borrow_mut().remove(&id);
        self.raises.borrow_mut().remove(&id);
        self.settle_keyboard();
        self.settle_layer();
    }

    fn settle_keyboard(&self) {
        let wanted = self.keyboard_wanted();
        if wanted == self.sent_keyboard.get() {
            return;
        }
        self.sent_keyboard.set(wanted);
        if let Some(window) = self.window.borrow().upgrade() {
            window.set_keyboard(interactivity(wanted));
        }
    }

    fn settle_layer(&self) {
        let wanted = self.layer();
        if wanted == self.sent_layer.get() {
            return;
        }
        self.sent_layer.set(wanted);
        if let Some(window) = self.window.borrow().upgrade() {
            window.set_layer(wanted);
        }
    }

    /// Replaces what the areas of this window's layer ask to have blurred behind them, and pushes it only where it differs from what the compositor already has — the protocol's region is double-buffered state, and re-sending the same one is a commit for nothing.
    ///
    /// Rectangles, not rounded rectangles: `ext-background-effect-v1` takes a `wl_region`, so an area's corner radius reaches the paint but not the blur behind it.
    fn set_area_blur(&self, rects: Vec<Rect>) {
        if *self.sent_blur.borrow() == rects {
            return;
        }
        *self.sent_blur.borrow_mut() = rects.clone();
        if let Some(window) = self.window.borrow().upgrade() {
            window.set_blur_region(rects);
        }
    }
}

/// One node's standing request of its window, withdrawn when this is dropped — which is what makes the window's keyboard and layer the maximum of what is mounted now rather than of everything that ever asked.
pub struct Demand {
    demands: Rc<Demands>,
    id: u64,
}

impl Drop for Demand {
    fn drop(&mut self) {
        self.demands.withdraw(self.id);
    }
}

/// `None` < `OnDemand` < `Exclusive`: asking for more than is needed is not free, since a surface holding the keyboard takes it from the focused window.
fn keyboard_rank(mode: KeyboardMode) -> u8 {
    match mode {
        KeyboardMode::None => 0,
        KeyboardMode::OnDemand => 1,
        KeyboardMode::Exclusive => 2,
    }
}

/// Bottom to top, in the order the compositor stacks them; the background, and any layer this build does not know, at the bottom.
fn layer_rank(layer: Layer) -> u8 {
    match layer {
        Layer::Bottom => 1,
        Layer::Top => 2,
        Layer::Overlay => 3,
        _ => 0,
    }
}

fn interactivity(mode: KeyboardMode) -> KeyboardInteractivity {
    match mode {
        KeyboardMode::None => KeyboardInteractivity::None,
        KeyboardMode::OnDemand => KeyboardInteractivity::OnDemand,
        KeyboardMode::Exclusive => KeyboardInteractivity::Exclusive,
    }
}

/// The layer-shell layer and namespace one model layer's window opens on, or `None` for the lock layer, which is the same model on a different protocol role and has no layer window at all (TA-8).
///
/// `desktop` is layer-shell's `bottom`, and is not named after it: `hogar-shell-bottom` would read as "the bottom bar" to a user writing their own compositor rules (DEC-14).
fn window_layer(kind: LayerKind) -> Option<(Layer, &'static str)> {
    let layer = match kind {
        LayerKind::Background => Layer::Background,
        LayerKind::Desktop => Layer::Bottom,
        LayerKind::Top => Layer::Top,
        LayerKind::Overlay => Layer::Overlay,
        LayerKind::Lock => return None,
    };
    Some((layer, kind.namespace()?))
}

/// Whether an area puts anything on the screen, which is the question the whole of lazy mapping turns on.
///
/// Paint is always something: a wallpaper region and a texture are their own content. Everything else draws what is placed in it, plus whatever its own style fills — so a bar with no modules and no fill of its own is a strip of nothing, and does not earn its layer a mapped window.
///
/// An area with a `visible` expression counts whatever the expression says now: an expression is read only while its window is on screen, so a window taken off screen because one turned false would never hear it turn true again. While false the area draws nothing and takes no input inside a window that stays up.
pub fn area_draws(area: &ResolvedArea) -> bool {
    !area.kind.holds_instances()
        || area.style.fill.is_some()
        || area.groups.iter().any(|group| !group.children.is_empty())
}

/// Whether a window has anything to show, which is whether it is on screen at all.
///
/// An area above fullscreen holds the overlay window open for as long as it exists, drawing or not (DEC-17): it is the one kind of overlay content the user asked to keep, and the scanout it costs is what `layout check` warns about.
pub fn window_draws(window: &WindowAreas) -> bool {
    window
        .areas
        .iter()
        .any(|(_, area)| area.above_fullscreen || area_draws(area))
}

/// The window an area is drawn in: its own layer's, except that an area above fullscreen is drawn in the overlay window, the one layer a fullscreen window leaves on screen (DEC-17, F-6.2). The lock layer has no window, and is above everything already.
pub fn window_of(home: LayerKind, area: &ResolvedArea) -> LayerKind {
    match area.above_fullscreen && home != LayerKind::Lock {
        true => LayerKind::Overlay,
        false => home,
    }
}

/// What one window draws, each area with the layer it was written on.
///
/// For every window but the overlay one that is its own layer's areas less those lifted above fullscreen. The overlay window draws those lifted areas first, bottom layer first, and its own layer's over them, so a notification still lands on top of a bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowAreas {
    pub areas: Vec<(LayerKind, ResolvedArea)>,
}

impl WindowAreas {
    pub fn of(resolved: &Resolved, window: LayerKind) -> Self {
        let areas = LayerKind::SESSION
            .into_iter()
            .filter_map(|home| Some((home, resolved.layer(home)?)))
            .flat_map(|(home, layer)| layer.areas.iter().map(move |area| (home, area)))
            .filter(|(home, area)| window_of(*home, area) == window)
            .map(|(home, area)| (home, area.clone()))
            .collect();
        Self { areas }
    }

    /// One layer's areas, all of them written on it.
    pub fn home(layer: LayerKind, areas: ResolvedLayer) -> Self {
        Self {
            areas: areas.areas.into_iter().map(|area| (layer, area)).collect(),
        }
    }
}

/// What turns one resolved area into the node its layer's window mounts.
///
/// The node places itself: every window on an output is the whole output and shares one coordinate space (TA-1), so an area is positioned absolutely within it rather than flowed, and the order areas are built in is their z-order within the layer.
///
/// A builder is not asked where it put the area. Where it ended up is a question layout answers, and the window reads it off the node afterwards for the one thing that needs it — see [`blur_of`]. A builder that had to report its own rectangle would be restating something it often cannot know: a bar running [`Extent::Fill`](layout::Extent::Fill) is as long as its neighbours leave it.
pub trait Areas {
    fn build(&self, area: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError>;
}

/// Who blurs what is behind an area styled `backdrop = "blur"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blur {
    /// What is behind it is drawn by the same surface — the pictures of the background layer, the lock screen's own background — so the shell blurs it itself, and it works on every compositor.
    InSurface,
    /// What is behind it is other surfaces — application windows, the layers below — which only the compositor can blur, through `ext-background-effect-v1`. Without the protocol the area draws translucent and unblurred, and `layout check` and its popover say so (F-10.49).
    Compositor,
}

/// Who blurs behind `area`, written on the layer `home`, or `None` where it asks for no blur.
///
/// A window tracks the laid-out rect of the areas the compositor blurs behind and no others: the blur region is the only thing it needs an area's geometry *for*, since everything else about where an area sits is settled by the node placing itself. Rectangle, not rounded rectangle: `ext-background-effect-v1` takes a `wl_region`, so at the corners of a rounded translucent area the blur runs past the paint. Nothing is under the background layer's window, and the lock surfaces get no effect object at all (F-2.12), so an area on either blurs what its own surface drew beneath it instead.
pub fn blur_of(home: LayerKind, area: &ResolvedArea) -> Option<Blur> {
    (area.style.backdrop == Some(Backdrop::Blur)).then_some(match home {
        LayerKind::Background | LayerKind::Lock => Blur::InSurface,
        LayerKind::Desktop | LayerKind::Top | LayerKind::Overlay => Blur::Compositor,
    })
}

/// Everything one area's builder is handed.
pub struct AreaContext<'a> {
    pub area: &'a ResolvedArea,
    /// Which layer's window the area is being built into. An area does not carry its own layer, because which layer it is on is a property of where it was written.
    pub layer: LayerKind,
    /// The layer the area was written on: `layer` itself, except for an area above fullscreen, which is written on its own layer and drawn in the overlay window.
    pub home: LayerKind,
    pub output: Option<&'a str>,
    /// The global config merged with this monitor's override: behaviour, theme and module defaults, which stay in `config.toml` while placement moves to the layout.
    pub config: &'a Arc<Config>,
    pub theme: NordTheme,
    /// The box this area's geometry is measured in, in the window's coordinate space, with [`layout::Within`] already applied — so a fractional [`layout::Rect`] is a fraction of *this*, and `area.within` is not a builder's to read.
    pub bounds: Rect,
    /// What the reserving areas of this output take off each edge. A bar running [`Extent::Fill`](layout::Extent) is as long as its neighbours leave it, and its neighbours are on layers this window cannot see.
    pub reserved: Reserved,
    /// Whether the compositor will actually blur behind an area styled `backdrop = "blur"`. Live state rather than a bind check — the capability can be withdrawn and come back — so an area styled to blur where this is false draws translucent and unblurred instead of pretending.
    pub blur_available: bool,
    /// What the area may ask of the window it is in beyond drawing: the keyboard, for as long as it holds the token back.
    pub demands: &'a Rc<Demands>,
    /// The whole output, for the few things that are about the screen rather than about the area — a wipe transition's travel, for one.
    pub output_size: (f32, f32),
}

/// The builder every window uses until the real ones are wired in. It draws nothing, so a window built from it maps and unmaps exactly as its layout says and shows an empty screen while it is up.
pub struct NoAreas;

impl Areas for NoAreas {
    fn build(&self, _: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
        Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?))
    }
}

/// What a node inside a layer window can read about the window it is in, without being handed it down the tree.
#[derive(Clone)]
pub struct LayerWindowContext {
    pub layer: LayerKind,
    pub output: Option<String>,
    pub demands: Rc<Demands>,
    /// Whether the window is on screen now, read reactively: what an expression drawn in it is gated on.
    pub mapped: ReadSignal<bool>,
}

impl LayerWindowContext {
    /// The window the node being built is in; `None` outside one.
    pub fn current() -> Option<Self> {
        telar::context::<Self>()
    }
}

/// The app one layer window draws: its layer's areas built into one tree over a cell the host writes a new arrangement into, so a layout change is a rebuild rather than a new window.
struct LayerApp {
    kind: LayerKind,
    output: Option<String>,
    layer: LiveLayer,
    config: LiveConfig,
    demands: Rc<Demands>,
    /// This output's reserved edges and logical size, shared with the host because both are facts about the whole screen that one layer cannot see, and both change under a window that stays.
    screen: Rc<Cell<Screen>>,
    generation: Generation,
    shown: ScreenFeed,
    mapped: MappedFeed,
    areas: Rc<dyn Areas>,
}

/// What a window needs to know about the output it covers in order to place the areas on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Screen {
    pub(crate) size: (f32, f32),
    pub(crate) reserved: Reserved,
}

impl LayerApp {
    /// Builds one area of the window, registering its box with the window's blur region while it lives if it asks the compositor to blur behind it.
    fn build_area(
        &self,
        blurring: RwSignal<Vec<(u64, RwSignal<Rect>)>>,
    ) -> impl Fn(Drawn) -> Result<Box<dyn LayoutItem>, LayoutError> + 'static {
        let (kind, output) = (self.kind, self.output.clone());
        let demands = Rc::clone(&self.demands);
        let areas = Rc::clone(&self.areas);
        let tokens = Rc::new(Cell::new(0u64));
        move |drawn| {
            let building = Building {
                window: kind,
                output: output.as_deref(),
                config: &drawn.config,
                theme: drawn.theme,
                size: drawn.screen.size,
                reserved: drawn.screen.reserved,
                demands: &demands,
            };
            let Some(BuiltArea { node, blurred }) =
                build_area(areas.as_ref(), drawn.home, &drawn.area, &building)
            else {
                return Container::new(LayoutStyle::new(), Vec::new()).map(box_item);
            };
            if let Some(rect) = blurred {
                let token = tokens.get().wrapping_add(1);
                tokens.set(token);
                blurring.update(|held| held.push((token, rect)));
                on_cleanup(move || {
                    if blurring.is_alive() {
                        blurring.update(|held| held.retain(|(own, _)| *own != token));
                    }
                });
            }
            Ok(node)
        }
    }

    /// What the window draws now — its reconciled areas, or a preview's in their place — one entry per area, keyed by the build it belongs to and a version that moves only when that area, or the screen it is placed on, changes: an edit or a preview rebuilds the areas it touched and keeps every other node, and a config edit or a config preview, which every area is built against, rebuilds them all.
    fn drawing(
        &self,
        generation: RwSignal<Builds>,
        theme: ScopedTheme,
    ) -> impl Fn() -> Vec<Drawn> + 'static {
        let layer = self.layer.clone();
        let config = self.config.clone();
        let screen = Rc::clone(&self.screen);
        let concealment = self.generation.clone();
        let key = WindowKey {
            output: self.output.clone(),
            layer: self.kind,
        };
        let seen = RefCell::new(Seen::default());
        move || {
            let build = generation.get().build;
            let drawn = previewed(&key).unwrap_or_else(|| layer.get());
            let (reconfigured, config) = crate::reconcile::shown_config(&config.get());
            let mut seen = seen.borrow_mut();
            if seen.build != Some(build) || seen.reconfigured != reconfigured {
                let resolved = config.resolve_theme();
                theme.set(resolved);
                services::locale::attach(config.language());
                // Kept across a config preview, whose areas are of the same build: a version handed out again would name a node already built.
                let next = seen.next;
                *seen = Seen {
                    build: Some(build),
                    reconfigured,
                    theme: Some(resolved),
                    next,
                    ..Seen::default()
                };
            }
            if concealment.is_concealed() {
                return Vec::new();
            }
            let screen = screen.get();
            let theme = seen.theme.unwrap_or_else(|| config.resolve_theme());
            let mut areas = HashMap::with_capacity(drawn.areas.len());
            let list = drawn
                .areas
                .iter()
                .map(|(home, area)| {
                    let at = (*home, area.id.clone());
                    let version = match seen.areas.get(&at) {
                        Some((version, was, placed)) if was == area && *placed == screen => {
                            *version
                        }
                        Some((version, was, placed))
                            if *placed == screen
                                && crate::area::moves_only(was, area)
                                && crate::area::move_cells(
                                    key.output.as_deref(),
                                    key.layer,
                                    *home,
                                    area,
                                ) =>
                        {
                            *version
                        }
                        _ => {
                            seen.next = seen.next.wrapping_add(1);
                            seen.next
                        }
                    };
                    areas.insert(at, (version, area.clone(), screen));
                    Drawn {
                        build,
                        version,
                        home: *home,
                        area: area.clone(),
                        screen,
                        config: Arc::clone(&config),
                        theme,
                    }
                })
                .collect();
            seen.areas = areas;
            list
        }
    }
}

/// What [`LayerApp::drawing`] remembers of the areas it last handed out: the build they belong to, the config preview and theme they were drawn with, and each area's version with what it was drawn from.
#[derive(Default)]
struct Seen {
    build: Option<u64>,
    reconfigured: Option<u64>,
    theme: Option<NordTheme>,
    next: u64,
    areas: HashMap<(LayerKind, AreaId), (u64, ResolvedArea, Screen)>,
}

/// One area of one build of a window: see [`LayerApp::drawing`].
#[derive(Clone)]
struct Drawn {
    build: u64,
    version: u64,
    home: LayerKind,
    area: ResolvedArea,
    screen: Screen,
    config: Arc<Config>,
    theme: NordTheme,
}

impl Drawn {
    fn key(&self) -> (u64, LayerKind, AreaId, u64) {
        (self.build, self.home, self.area.id.clone(), self.version)
    }
}

/// What a preview draws in the window `key`, while one is showing.
fn previewed(key: &WindowKey) -> Option<Rc<WindowAreas>> {
    let desktops = crate::reconcile::previewing()?;
    let desktop = desktops
        .iter()
        .find(|desktop| desktop.output == key.output)?;
    Some(Rc::new(WindowAreas::of(&desktop.resolved, key.layer)))
}

/// The window a set of areas is being built into, and what about its screen they are built against.
pub struct Building<'a> {
    pub window: LayerKind,
    pub output: Option<&'a str>,
    pub config: &'a Arc<Config>,
    pub theme: NordTheme,
    pub size: (f32, f32),
    pub reserved: Reserved,
    pub demands: &'a Rc<Demands>,
}

/// One node per area of `drawn`, in z-order. An area that fails to build is logged and left out, so the rest still draw.
///
/// What an edit mode builds a layer it cannot raise with, into the overlay window instead (TA-4, R-6): the builder a layer window builds each of its own areas with, so one renderer, whichever window the areas land in. Nothing is asked of the compositor's blur: the region belongs to the window drawing these.
pub fn build_window_areas(
    areas: &dyn Areas,
    drawn: &WindowAreas,
    building: &Building<'_>,
) -> Vec<Box<dyn LayoutItem>> {
    drawn
        .areas
        .iter()
        .filter_map(|(home, area)| build_area(areas, *home, area, building))
        .map(|built| built.node)
        .collect()
}

/// The node `area`, written on `home`, draws in the window being built, and the tracked box of it when it asks the compositor to blur what is behind it; `None`, logged, where it fails to build.
fn build_area(
    areas: &dyn Areas,
    home: LayerKind,
    area: &ResolvedArea,
    building: &Building<'_>,
) -> Option<BuiltArea> {
    // Its own owner, so the chrome an area provides — the global one here, a bar's own shape inside it — reaches only that area.
    let _area = telar::owner_scope();
    ui::chrome::Chrome::global(
        Arc::clone(building.config),
        building.output.map(str::to_string),
    )
    .provide();
    let built = areas.build(&AreaContext {
        area,
        layer: building.window,
        home,
        output: building.output,
        config: building.config,
        theme: building.theme,
        bounds: building.reserved.box_of(area.within, building.size),
        reserved: building.reserved,
        blur_available: background_effect_supported(),
        demands: building.demands,
        output_size: building.size,
    });
    let node = match built {
        Ok(node) => node,
        Err(error) => {
            tracing::error!(
                area = %area.id,
                layer = %building.window,
                "the area failed to build: {error}"
            );
            return None;
        }
    };
    let blurred = (blur_of(home, area) == Some(Blur::Compositor))
        .then(|| track_layout(node.layout_node()))
        .flatten();
    Some(BuiltArea { node, blurred })
}

/// One area built into a window: its node, and the tracked box of it where it asks the compositor to blur what is behind it.
struct BuiltArea {
    node: Box<dyn LayoutItem>,
    blurred: Option<RwSignal<Rect>>,
}

/// Keeps the window's blur region in step with where layout actually put the areas that ask the compositor to blur.
///
/// An effect rather than a value read during a build, because at build time nothing has been laid out yet and every rectangle is still zero. Each area adds its box as it builds and takes it back as it goes, so the region follows an edit that rebuilt one area and kept the rest. The region lands one frame behind the layout that moved it — the same frame the input region already costs, and for the same reason.
///
/// A zero-area rectangle is dropped rather than sent: it adds nothing to a `wl_region`, and before the first layout it is what every area reports.
fn watch_blur(demands: Rc<Demands>, blurring: RwSignal<Vec<(u64, RwSignal<Rect>)>>) {
    effect(move || {
        let rects = blurring
            .get()
            .iter()
            .map(|(_, area)| area.get())
            .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
            .collect();
        demands.set_area_blur(rects);
    });
}

impl App for LayerApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        let theme = ScopedTheme::new(config.resolve_theme());
        services::locale::attach(config.language());
        set_context(LayerWindowContext {
            layer: self.kind,
            output: self.output.clone(),
            demands: Rc::clone(&self.demands),
            mapped: self.mapped.attach(),
        });

        let generation = signal(Builds::default());
        self.generation.attach(generation);
        let blurring = signal(Vec::new());
        watch_blur(Rc::clone(&self.demands), blurring);
        let build = self.build_area(blurring);
        let drawing = self.drawing(generation, theme);
        let screen = signal(self.screen.get());
        self.shown.attach(screen);
        let key = WindowKey {
            output: self.output.clone(),
            layer: self.kind,
        };
        let frame = crate::transient::Frame {
            screen: screen.read_only(),
            demands: Rc::clone(&self.demands),
        };
        let content = provide_theme(theme, move || {
            let areas =
                ReactiveList::with_style(whole_window(), drawing, Drawn::key, build)?.as_row();
            let transients =
                ui::chrome::or_empty("transient layer", crate::transient::layer(key, frame));
            Container::new(whole_window(), vec![Box::new(areas), transients]).map(box_item)
        })
        .map(box_item);
        let root = ui::chrome::or_empty(self.kind.as_str(), content);
        Box::new(WindowRoot::new(Box::new(crate::menu::Pointed::new(root))))
    }

    /// Nothing behind the areas: a layer window covers its whole output, and whatever an area does not paint is whatever is under the window.
    fn clear_color(&self) -> Option<Color> {
        None
    }

    fn window_config(&self) -> Option<WindowConfig> {
        Some(WindowConfig {
            is_transparent: true,
            ..WindowConfig::default()
        })
    }
}

fn whole_window() -> LayoutStyle {
    LayoutStyle::new()
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Arc, Mutex};

    use platform_headless::{HeadlessPlatform, SurfaceFrameSink};
    use telar::{
        AppConfig, AppPathsProvider, NoPaths, StyledContainer, SurfaceId, run_multi_with_platform,
        run_with_platform,
    };

    use config::Edge;
    use layout::{
        AreaId, BarShape, Expr, Extent, GroupId, GroupKind, InstanceId, Representation,
        ResolvedAreaKind, ResolvedGroup, ResolvedInstance, Style, Zone,
    };
    use telar::set_theme;
    use ui::scale::paint;

    use super::*;

    const SCREEN: &str = "DP-1";

    fn config() -> Arc<Config> {
        Arc::new(Config::default())
    }

    fn plan<'a>(config: &'a Arc<Config>, resolved: &'a Resolved) -> LayerPlan<'a> {
        LayerPlan {
            output: Some(SCREEN),
            config,
            resolved,
            reserved: Reserved::of(resolved, config),
            size: (1920.0, 1080.0),
        }
    }

    fn resolved(layers: &[(LayerKind, ResolvedLayer)]) -> Resolved {
        Resolved::of(SCREEN, layers.iter().cloned())
    }

    fn layer(areas: Vec<ResolvedArea>) -> ResolvedLayer {
        ResolvedLayer { areas }
    }

    /// What a top window draws when `areas` are all its layer has.
    fn drawn(areas: Vec<ResolvedArea>) -> WindowAreas {
        WindowAreas::home(LayerKind::Top, layer(areas))
    }

    /// A screen the size of the headless surface, so an area filling its bounds fills the frame.
    fn screen() -> Screen {
        Screen {
            size: (SIDE as f32, SIDE as f32),
            reserved: Reserved::default(),
        }
    }

    fn bar(id: &str, modules: &[&str]) -> ResolvedArea {
        ResolvedArea {
            id: AreaId::new(id),
            kind: ResolvedAreaKind::Bar {
                edge: Edge::Top,
                thickness: 34.0,
                length: Extent::Fill,
                offset: 0.0,
                shape: BarShape::default(),
                autohide: None,
            },
            reserve: true,
            above_fullscreen: false,
            within: layout::Within::Output,
            style: Style::default(),
            visible: None,
            actions: Default::default(),
            groups: vec![ResolvedGroup {
                id: GroupId::new("start"),
                kind: GroupKind::Zone { zone: Zone::Start },
                arrange: None,
                cols: layout::Arrange::TRACKS,
                rows: layout::Arrange::TRACKS,
                gap: None,
                repeat: None,
                komponent: None,
                style: Style::default(),
                children: modules.iter().map(instance).collect(),
            }],
        }
    }

    fn instance(module: &&str) -> ResolvedInstance {
        ResolvedInstance {
            id: InstanceId::new(*module),
            module: (*module).to_string(),
            representation: Representation::Chip,
            options: toml::Table::new(),
            bindings: BTreeMap::new(),
            style: Style::default(),
            placement: None,
            actions: BTreeMap::new(),
        }
    }

    fn wallpaper() -> ResolvedArea {
        ResolvedArea {
            id: AreaId::new("wall"),
            kind: ResolvedAreaKind::WallpaperRegion {
                rect: layout::Rect::default(),
                source: "~/pictures/wall.png".into(),
                fit: layout::Fit::Cover,
                transition: layout::Transition::Fade,
            },
            reserve: false,
            above_fullscreen: false,
            within: layout::Within::Output,
            style: Style::default(),
            visible: None,
            actions: Default::default(),
            groups: Vec::new(),
        }
    }

    /// The starter arrangement: one top bar with modules on it and nothing anywhere else, which is what the built-in layout resolves to.
    fn only_bars() -> Resolved {
        resolved(&[(LayerKind::Top, layer(vec![bar("bar-top", &["clock"])]))])
    }

    fn host() -> LayerWindows {
        LayerWindows::new(Rc::new(NoAreas))
    }

    /// T-4.1's acceptance, first half. Every session layer is tracked from the moment its output is, so a hold or a layout change reaches it; only the layer that resolves something visible is ever on screen.
    #[test]
    fn with_only_bars_configured_only_the_top_window_is_on_screen() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();

        let done = windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);

        assert_eq!(done.opened, 4, "one window per session layer");
        assert_eq!(done.mapped, 1);
        assert!(windows.is_mapped(Some(SCREEN), LayerKind::Top));
        for quiet in [
            LayerKind::Background,
            LayerKind::Desktop,
            LayerKind::Overlay,
        ] {
            assert!(
                !windows.is_mapped(Some(SCREEN), quiet),
                "{quiet} resolves nothing, so its window must not cost the compositor a buffer"
            );
        }
    }

    /// An overlay *surface* is what costs the output direct scanout — it joins the monitor's overlay list at `get_layer_surface` and leaves it only when destroyed — so an empty overlay window is not hidden, it is not created. Every other layer keeps its surface, because reopening one costs a cold map.
    #[test]
    fn an_empty_overlay_window_has_no_surface_at_all_while_the_others_keep_theirs() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);

        assert!(
            !windows.is_open(Some(SCREEN), LayerKind::Overlay),
            "creating the surface is what costs scanout, so an empty overlay window must not exist"
        );
        for kept in [LayerKind::Background, LayerKind::Desktop, LayerKind::Top] {
            assert!(
                windows.is_open(Some(SCREEN), kept),
                "{kept} is hidden rather than closed, so its surface survives being empty"
            );
        }
    }

    /// An overlay area in the layout opens the surface, and taking it away closes it again.
    #[test]
    fn an_overlay_area_opens_the_surface_and_losing_it_closes_it() {
        let config = config();
        let mut windows = host();
        let with_hud = resolved(&[
            (LayerKind::Top, layer(vec![bar("bar-top", &["clock"])])),
            (LayerKind::Overlay, layer(vec![bar("hud", &["stack"])])),
        ]);
        windows.reconcile(&[plan(&config, &with_hud)], Content::Rebuild);
        assert!(windows.is_open(Some(SCREEN), LayerKind::Overlay));

        let bars_only = only_bars();
        windows.reconcile(&[plan(&config, &bars_only)], Content::Rebuild);
        assert!(
            !windows.is_open(Some(SCREEN), LayerKind::Overlay),
            "the output must be able to scan out again once nothing is in the overlay"
        );
    }

    /// T-4.1's acceptance, second half: the launcher. It is in no layout, so it maps the overlay window by holding it — and the hold is what the exit transition outlives, which is why letting go is the caller's to time.
    #[test]
    fn a_hold_maps_the_overlay_window_and_letting_go_takes_it_off_screen() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);

        let launcher = windows
            .hold(Some(SCREEN), LayerKind::Overlay)
            .expect("a hold reaches the overlay window even before it has a surface");
        assert!(
            windows.is_mapped(Some(SCREEN), LayerKind::Overlay),
            "an unanchored transient is why the overlay window exists at all"
        );
        assert!(windows.is_open(Some(SCREEN), LayerKind::Overlay));

        drop(launcher);
        assert!(!windows.is_mapped(Some(SCREEN), LayerKind::Overlay));
        assert!(
            !windows.is_open(Some(SCREEN), LayerKind::Overlay),
            "the surface has to go, not just its buffer: the output cannot scan out while it exists"
        );
    }

    /// Two things holding one window is not two mappings, and the first to let go must not take the screen from the second.
    #[test]
    fn a_window_stays_on_screen_until_the_last_hold_lets_go() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);

        let card = windows
            .hold(Some(SCREEN), LayerKind::Overlay)
            .expect("open");
        let menu = windows
            .hold(Some(SCREEN), LayerKind::Overlay)
            .expect("open");
        drop(card);
        assert!(windows.is_mapped(Some(SCREEN), LayerKind::Overlay));
        drop(menu);
        assert!(!windows.is_mapped(Some(SCREEN), LayerKind::Overlay));
    }

    /// A layout that grows an overlay area maps the window that was already up, rather than one opened for it.
    #[test]
    fn a_layer_that_gains_an_area_maps_the_window_already_open_for_it() {
        let config = config();
        let mut windows = host();
        let before = only_bars();
        windows.reconcile(&[plan(&config, &before)], Content::Rebuild);

        let after = resolved(&[
            (LayerKind::Top, layer(vec![bar("bar-top", &["clock"])])),
            (LayerKind::Overlay, layer(vec![bar("hud", &["stack"])])),
        ]);
        let done = windows.reconcile(&[plan(&config, &after)], Content::Rebuild);

        assert_eq!(done.opened, 0, "the windows were already up");
        assert_eq!(
            done.rebuilt, 4,
            "a layout edit is a rebuild of every window"
        );
        assert_eq!(done.mapped, 2);
        assert!(windows.is_mapped(Some(SCREEN), LayerKind::Overlay));
    }

    /// The identity rule: a reload reaches the window that is up. Reopening one would lose its tree, its per-instance state and a frame.
    #[test]
    fn a_reload_reuses_every_window_and_opens_none() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);

        let cells: Vec<LiveLayer> = windows
            .live
            .iter()
            .map(|(_, window)| window.layer.clone())
            .collect();

        let done = windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);
        assert_eq!(done.opened, 0);
        assert_eq!(done.closed, 0);
        assert_eq!(windows.len(), 4);
        for (before, (_, after)) in cells.iter().zip(&windows.live) {
            assert!(
                before.ptr_eq(&after.layer),
                "the reload wrote into the cell the window was already building from"
            );
        }
    }

    /// A monitor plugged in must not rebuild the screens that were already there: it would throw their state away to redraw exactly what was on them.
    #[test]
    fn an_output_arriving_opens_its_own_windows_and_rebuilds_nobody_elses() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);

        let second = Resolved::of(
            "HDMI-A-1",
            [(LayerKind::Top, layer(vec![bar("bar-top", &["clock"])]))],
        );
        let done = windows.reconcile(
            &[
                plan(&config, &resolved),
                LayerPlan {
                    output: Some("HDMI-A-1"),
                    config: &config,
                    resolved: &second,
                    reserved: Reserved::of(&second, &config),
                    size: (2560.0, 1440.0),
                },
            ],
            Content::Changed,
        );

        assert_eq!(done.opened, 4);
        assert_eq!(done.rebuilt, 0);
        assert_eq!(windows.len(), 8);
        assert!(windows.is_mapped(Some("HDMI-A-1"), LayerKind::Top));
    }

    #[test]
    fn an_output_going_away_takes_its_windows_with_it() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);

        let done = windows.reconcile(&[], Content::Changed);
        assert_eq!(done.closed, 4);
        assert!(windows.is_empty());
        assert!(
            windows.hold(Some(SCREEN), LayerKind::Top).is_none(),
            "nothing can hold a window on a screen that is gone"
        );
    }

    fn empty_grid() -> ResolvedArea {
        ResolvedArea {
            id: AreaId::new("desktop"),
            kind: ResolvedAreaKind::Grid {
                rect: layout::Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: layout::Anchor::TopLeft,
            },
            reserve: false,
            above_fullscreen: false,
            within: layout::Within::Usable,
            style: Style::default(),
            visible: None,
            actions: Default::default(),
            groups: Vec::new(),
        }
    }

    /// Desktop mode on a desktop with nothing placed yet: the grid draws nothing, so its window is off screen until the edit mode holds it — and the edit mode reaches it by key, through the holder, since it does not own the host.
    #[test]
    fn a_hold_puts_an_empty_desktop_on_screen_and_letting_go_hides_it_again() {
        let config = config();
        let resolved = resolved(&[
            (LayerKind::Top, layer(vec![bar("bar-top", &["clock"])])),
            (LayerKind::Desktop, layer(vec![empty_grid()])),
        ]);
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);
        assert!(!windows.is_mapped(Some(SCREEN), LayerKind::Desktop));

        let key = WindowKey {
            output: Some(SCREEN.into()),
            layer: LayerKind::Desktop,
        };
        let editing = windows
            .holder()
            .hold(&key)
            .expect("the desktop window is up");
        assert!(windows.is_mapped(Some(SCREEN), LayerKind::Desktop));
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);
        assert!(
            windows.is_mapped(Some(SCREEN), LayerKind::Desktop),
            "an edit committed mid-mode rebuilds the areas and leaves the hold alone"
        );

        drop(editing);
        assert!(!windows.is_mapped(Some(SCREEN), LayerKind::Desktop));
        assert!(
            windows.is_open(Some(SCREEN), LayerKind::Desktop),
            "hidden, not closed: the desktop keeps its surface"
        );
    }

    /// T-6.1's state-restore criterion at the host: an edit mode raises the window it edits above the user's windows, holds it on screen and takes the keyboard in the overlay window, and letting all three go leaves every window as it was.
    #[test]
    fn raising_a_window_and_letting_go_puts_its_layer_keyboard_and_mapping_back() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);
        let state = |windows: &LayerWindows| -> Vec<(Layer, KeyboardMode, bool)> {
            LayerKind::SESSION
                .iter()
                .map(|&layer| {
                    let demands = windows.demands(Some(SCREEN), layer).expect("open");
                    (
                        demands.layer(),
                        demands.keyboard_wanted(),
                        windows.is_mapped(Some(SCREEN), layer),
                    )
                })
                .collect()
        };
        let before = state(&windows);

        for edited in [LayerKind::Background, LayerKind::Desktop] {
            let key = WindowKey {
                output: Some(SCREEN.into()),
                layer: edited,
            };
            let demands = windows.holder().demands(&key).expect("open");
            let host = windows
                .holder()
                .demands(&WindowKey {
                    output: Some(SCREEN.into()),
                    layer: LayerKind::Overlay,
                })
                .expect("the overlay window is tracked before it has a surface");
            let held = windows.holder().hold(&key).expect("open");
            let raised = demands.raise(Layer::Overlay);
            let keyboard = host.keyboard(KeyboardMode::Exclusive);
            assert_eq!(demands.layer(), Layer::Overlay);
            assert!(windows.is_mapped(Some(SCREEN), edited));

            windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);
            assert_eq!(
                demands.layer(),
                Layer::Overlay,
                "an edit committed mid-mode leaves the window where the mode put it"
            );

            drop((raised, keyboard, held));
            assert_eq!(state(&windows), before, "after editing {edited}");
        }
    }

    /// A raise is a floor, not a move: asking a window to be lower than it is changes nothing, and two raises settle on the higher until both are gone.
    #[test]
    fn a_window_sits_on_the_highest_of_its_own_layer_and_every_raise() {
        let demands = Rc::new(Demands::new(Layer::Top));
        let lower = demands.raise(Layer::Bottom);
        assert_eq!(demands.layer(), Layer::Top);
        let higher = demands.raise(Layer::Overlay);
        drop(lower);
        assert_eq!(demands.layer(), Layer::Overlay);
        drop(higher);
        assert_eq!(demands.layer(), Layer::Top);
    }

    #[test]
    fn a_bar_with_no_modules_and_no_fill_of_its_own_draws_nothing() {
        assert!(!area_draws(&bar("bar-top", &[])));
        assert!(area_draws(&bar("bar-top", &["clock"])));
    }

    /// An empty bar the user gave a colour is a stripe they asked for, and a wallpaper region holds no instances at all but is the one thing on its layer.
    #[test]
    fn an_area_that_is_its_own_paint_draws_whether_or_not_anything_is_placed_in_it() {
        let mut stripe = bar("bar-top", &[]);
        stripe.style.fill = Some("surface".into());
        assert!(area_draws(&stripe));
        assert!(area_draws(&wallpaper()));
    }

    /// An area its expression hides keeps its window on screen: the expression is read only while the window is mapped, so taking the window away for a false one would leave nothing to hear it turn true. What it hides is the area's paint and input, inside the window (`expressions_tests`).
    #[test]
    fn an_area_hidden_by_an_expression_keeps_its_window_on_screen_to_hear_it_change() {
        let mut conditional = bar("bar-top", &["clock"]);
        conditional.visible = Some(layout::ResolvedExpr {
            expr: Expr("$media.playing".into()),
            origin: layout::Origin::Level(layout::Level {
                layout: layout::LayoutId::new("conditional"),
                output: layout::OutputMatch::default(),
                workspace: None,
            }),
            within: None,
        });
        assert!(area_draws(&conditional));
        assert!(window_draws(&drawn(vec![conditional])));
    }

    #[test]
    fn a_layer_draws_when_any_one_of_its_areas_does() {
        assert!(!window_draws(&WindowAreas::default()));
        assert!(!window_draws(&drawn(vec![bar("a", &[]), bar("b", &[])])));
        assert!(window_draws(&drawn(vec![
            bar("a", &[]),
            bar("b", &["clock"])
        ])));
    }

    /// A `wl_surface` has one keyboard interactivity, so what the window negotiates is the maximum of what is mounted in it — and it must come back down when the thing that wanted it goes.
    #[test]
    fn the_keyboard_a_window_takes_is_the_maximum_of_what_is_mounted_in_it() {
        let config = config();
        let resolved = only_bars();
        let mut windows = host();
        windows.reconcile(&[plan(&config, &resolved)], Content::Rebuild);
        let demands = windows
            .demands(Some(SCREEN), LayerKind::Overlay)
            .expect("the overlay window is open");
        assert_eq!(demands.keyboard_wanted(), KeyboardMode::None);

        let notes = demands.keyboard(KeyboardMode::OnDemand);
        assert_eq!(demands.keyboard_wanted(), KeyboardMode::OnDemand);

        let picker = demands.keyboard(KeyboardMode::Exclusive);
        assert_eq!(demands.keyboard_wanted(), KeyboardMode::Exclusive);

        drop(picker);
        assert_eq!(
            demands.keyboard_wanted(),
            KeyboardMode::OnDemand,
            "the panel that is still up keeps its own demand"
        );
        drop(notes);
        assert_eq!(demands.keyboard_wanted(), KeyboardMode::None);
    }

    /// A window's blur region is asked for again only where it differs, because the region is double-buffered state that costs a commit.
    #[test]
    fn a_blur_region_is_pushed_only_where_it_differs_from_what_the_compositor_has() {
        let demands = Demands::new(Layer::Top);
        let strip = vec![Rect::new(0.0, 0.0, 1920.0, 34.0)];
        assert!(demands.blur_region().is_empty());

        demands.set_area_blur(strip.clone());
        assert_eq!(demands.blur_region(), strip);
        demands.set_area_blur(strip.clone());
        assert_eq!(demands.blur_region(), strip);

        demands.set_area_blur(Vec::new());
        assert!(
            demands.blur_region().is_empty(),
            "asking for no blur is still asking, and is how the effect is removed"
        );
    }

    /// The window resolves [`Within`], not the builders, so the two boxes cannot drift apart between one area kind and the next.
    ///
    /// This is the regression the field exists to prevent: a wallpaper belongs under the bar and measures against the whole output, while a clock at the bottom right means the bottom right of what the bar left. Measured against the output it would sit underneath one.
    #[test]
    fn the_host_insets_the_usable_box_by_what_the_reserving_areas_took() {
        let reserved = Reserved::of(&only_bars(), &config());
        assert_eq!(reserved.top, 34.0, "the top bar reserves its thickness");
        assert_eq!(
            (reserved.left, reserved.right, reserved.bottom),
            (0.0, 0.0, 0.0),
            "and nothing reserves any other edge"
        );

        let size = (1920.0, 1080.0);
        assert_eq!(
            reserved.box_of(Within::Output, size),
            Rect::new(0.0, 0.0, 1920.0, 1080.0)
        );
        assert_eq!(
            reserved.box_of(Within::Usable, size),
            Rect::new(0.0, 34.0, 1920.0, 1046.0)
        );
    }

    /// Reservation is gathered across every layer, which is exactly what one window cannot see for itself: a reserving dock on the desktop shortens a bar in the top window.
    #[test]
    fn what_is_reserved_is_gathered_across_the_layers_a_window_cannot_see() {
        let mut dock = bar("dock", &["visualiser"]);
        dock.kind = ResolvedAreaKind::Dock {
            edge: Edge::Left,
            thickness: 60.0,
        };
        let both = resolved(&[
            (LayerKind::Top, layer(vec![bar("bar-top", &["clock"])])),
            (LayerKind::Desktop, layer(vec![dock])),
        ]);

        let reserved = Reserved::of(&both, &config());
        assert_eq!(reserved.top, 34.0);
        assert_eq!(
            reserved.left, 60.0,
            "the desktop layer's dock takes its edge from the same screen the top bar is on"
        );
    }

    /// Only an area that asked to blur is blurred, so nobody has to wonder why the window does not track every area's rect; the compositor blurs behind the layers application windows sit among, and an area of the background or the lock screen blurs the pictures its own surface drew under it, which is all there is behind it.
    #[test]
    fn only_an_area_styled_to_blur_its_backdrop_is_blurred_and_by_whoever_draws_what_is_behind_it()
    {
        assert_eq!(blur_of(LayerKind::Top, &bar("bar-top", &["clock"])), None);

        let mut blurring = bar("bar-top", &["clock"]);
        blurring.style.backdrop = Some(Backdrop::Blur);
        for layer in [LayerKind::Desktop, LayerKind::Top, LayerKind::Overlay] {
            assert_eq!(blur_of(layer, &blurring), Some(Blur::Compositor), "{layer}");
        }
        for layer in [LayerKind::Background, LayerKind::Lock] {
            assert_eq!(blur_of(layer, &blurring), Some(Blur::InSurface), "{layer}");
        }

        let mut plain = bar("bar-top", &["clock"]);
        plain.style.backdrop = Some(Backdrop::None);
        assert_eq!(
            blur_of(LayerKind::Top, &plain),
            None,
            "saying `backdrop = \"none\"` out loud is still not asking for blur"
        );
    }

    /// The lock layer is the same model on `ext-session-lock-v1`, so it has no layer-shell window and no namespace to open one under.
    #[test]
    fn every_session_layer_has_a_namespace_and_the_lock_layer_has_none() {
        for kind in LayerKind::SESSION {
            let (_, namespace) = window_layer(kind).expect("a session layer opens a window");
            assert_eq!(namespace, format!("hogar-shell-{kind}"));
        }
        assert!(window_layer(LayerKind::Lock).is_none());
    }

    /// `desktop` is layer-shell's `bottom` — the one place the model's name and the protocol's differ, and the whole of DEC-14's rename.
    #[test]
    fn the_desktop_layer_is_layer_shells_bottom() {
        assert_eq!(
            window_layer(LayerKind::Desktop).map(|(l, _)| l),
            Some(Layer::Bottom)
        );
        assert_eq!(
            window_layer(LayerKind::Top).map(|(l, _)| l),
            Some(Layer::Top)
        );
    }

    struct Counting(Rc<Cell<u32>>);

    impl Areas for Counting {
        fn build(&self, _: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
            self.0.set(self.0.get() + 1);
            Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?))
        }
    }

    /// The standing rule that hot reload is non-destructive: an edit builds the window's areas again and leaves a transient open in it — and whatever the user was doing there — as it was.
    #[test]
    fn a_layout_edit_rebuilds_the_areas_and_leaves_an_open_transient_alone() {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        crate::transient::close_all();
        let areas = Rc::new(Cell::new(0));
        let app = LayerApp {
            kind: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn(vec![bar("bar-top", &["clock"])])),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(Layer::Top)),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Counting(Rc::clone(&areas))),
        };
        let content = Rc::new(Cell::new(0));
        let counted = Rc::clone(&content);
        crate::transient::open(crate::transient::Spec::new(
            "probe",
            crate::transient::Place::Beside(crate::transient::Anchor {
                output: Some(SCREEN.to_string()),
                layer: LayerKind::Top,
                edge: Edge::Top,
                rect: Rect::new(10.0, 0.0, 20.0, 20.0),
                chrome: ui::chrome::Chrome::global(config(), None),
                gap: 8.0,
            }),
            Rc::new(move |_: &ui::chrome::Chrome| {
                counted.set(counted.get() + 1);
                Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?) as _)
            }),
        ));

        let _root = app.root();
        assert_eq!((areas.get(), content.get()), (1, 1));

        app.generation.bump();
        assert_eq!(areas.get(), 2, "the edit reached the areas");
        assert_eq!(
            content.get(),
            1,
            "the transient kept the tree it had: a rebuild would put the caret back at the start of whatever was being typed"
        );
        crate::transient::close_all();
    }

    struct Naming(Rc<RefCell<Vec<String>>>);

    impl Areas for Naming {
        fn build(&self, context: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
            self.0.borrow_mut().push(context.area.id.to_string());
            Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?))
        }
    }

    /// A layout edit builds again the areas it changed and keeps every other node — a region's picture mid-fade, a card being read — while a config edit, which every area is built against, builds them all, and a screen that changed size moves them all.
    #[test]
    fn an_arrangement_change_rebuilds_only_the_areas_it_touched() {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        let built = Rc::new(RefCell::new(Vec::new()));
        let app = LayerApp {
            kind: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn(vec![
                bar("bar-top", &["clock"]),
                bar("bar-second", &["notes"]),
            ])),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(Layer::Top)),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Naming(Rc::clone(&built))),
        };
        let _root = app.root();
        let taken = || std::mem::take(&mut *built.borrow_mut());
        assert_eq!(taken(), ["bar-top", "bar-second"]);

        let mut thicker = bar("bar-second", &["notes"]);
        if let ResolvedAreaKind::Bar { thickness, .. } = &mut thicker.kind {
            *thickness = 50.0;
        }
        app.layer
            .set(drawn(vec![bar("bar-top", &["clock"]), thicker.clone()]));
        app.generation.look_again();
        assert_eq!(taken(), ["bar-second"], "only what the edit changed");

        app.generation.look_again();
        assert!(taken().is_empty(), "nothing changed, nothing is built");

        app.generation.bump();
        assert_eq!(
            taken(),
            ["bar-top", "bar-second"],
            "a config edit builds all"
        );

        app.screen.set(Screen {
            size: (SIDE as f32 * 2.0, SIDE as f32),
            reserved: Reserved::default(),
        });
        app.generation.look_again();
        assert_eq!(
            taken(),
            ["bar-top", "bar-second"],
            "a screen of another size places every area again"
        );
    }

    /// F-10.42 for komponents: a komponent file that changes changes the arrangement of the areas whose groups draw it and of no other, so a reload builds those again and keeps every other node.
    #[test]
    fn a_changed_komponent_rebuilds_only_the_areas_that_draw_it() {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        let stored: layout::Layout = toml::from_str(
            r#"
            id = "mine"
            [[outputs]]
            match = "*"
            [[outputs.layers.top.areas]]
            id = "bar-top"
            kind = "bar"
            edge = "top"
            thickness = 32
            [[outputs.layers.top.areas.groups]]
            id = "start"
            place = "zone"
            zone = "start"
            komponent = "pill"
            [[outputs.layers.top.areas]]
            id = "bar-second"
            kind = "bar"
            edge = "bottom"
            thickness = 32
            [[outputs.layers.top.areas.groups]]
            id = "start"
            place = "zone"
            zone = "start"
            [[outputs.layers.top.areas.groups.children]]
            id = "notes"
            module = "notes"
            "#,
        )
        .expect("the layout parses");
        let pill = |format: &str| -> layout::Komponent {
            toml::from_str(&format!(
                "[[children]]\nid = \"clock\"\nmodule = \"clock\"\noptions = {{ format = \"{format}\" }}\n"
            ))
            .expect("the komponent parses")
        };
        let drawn_with = |format: &str| {
            let library = layout::Library::default().with_komponent("pill", pill(format));
            let (resolved, report) = layout::resolve(&stored, &library, SCREEN, None);
            assert!(report.is_clean(), "{}", report.render());
            WindowAreas::of(&resolved, LayerKind::Top)
        };
        let built = Rc::new(RefCell::new(Vec::new()));
        let app = LayerApp {
            kind: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn_with("%H")),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(Layer::Top)),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Naming(Rc::clone(&built))),
        };
        let _root = app.root();
        let taken = || std::mem::take(&mut *built.borrow_mut());
        assert_eq!(taken(), ["bar-top", "bar-second"]);

        app.layer.set(drawn_with("%H:%M"));
        app.generation.look_again();
        assert_eq!(taken(), ["bar-top"], "only the bar whose group draws it");

        app.layer.set(drawn_with("%H:%M"));
        app.generation.look_again();
        assert!(
            taken().is_empty(),
            "the same komponent again builds nothing"
        );
    }

    fn picture(dir: &std::path::Path, name: &str, pixel: [u8; 4]) -> String {
        std::fs::create_dir_all(dir).expect("a scratch directory");
        let path = dir.join(name);
        image::RgbaImage::from_pixel(2, 2, image::Rgba(pixel))
            .save(&path)
            .expect("a picture on disk");
        path.display().to_string()
    }

    fn half(id: &str, x: f32, source: &str) -> ResolvedArea {
        ResolvedArea {
            id: AreaId::new(id),
            kind: ResolvedAreaKind::WallpaperRegion {
                rect: layout::Rect {
                    x,
                    y: 0.0,
                    w: 0.5,
                    h: 1.0,
                },
                source: source.to_string(),
                fit: layout::Fit::Cover,
                transition: layout::Transition::Fade,
            },
            reserve: false,
            above_fullscreen: false,
            within: Within::Output,
            style: Style::default(),
            visible: None,
            groups: Vec::new(),
            actions: BTreeMap::new(),
        }
    }

    fn images(commands: &[telar::DrawCommand]) -> HashSet<u64> {
        commands
            .iter()
            .filter_map(|command| match command {
                telar::DrawCommand::Image { data, .. } if data.width > 1 => Some(data.id),
                _ => None,
            })
            .collect()
    }

    /// T-7.1's acceptance, through the window's own build: `wallpaper set --region left` is a layout edit that changes the left region alone, so the window builds that region again — fading from the picture it showed to the new one — and keeps the right region's node, its picture and whatever fade it is in; the frame after the edit repaints nothing outside the left region's box (F-5.1, F-5.2).
    #[test]
    fn a_new_picture_for_one_region_repaints_that_region_and_nothing_beside_it() {
        const WIDE: u32 = 400;
        const HIGH: u32 = 200;
        telar::reset_layout_runtime();
        let config = Arc::new(Config::starter());
        set_theme(config.resolve_theme());
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("layer-window-regions");
        let (red, blue, green) = (
            picture(&dir, "red.png", [255, 0, 0, 255]),
            picture(&dir, "blue.png", [0, 0, 255, 255]),
            picture(&dir, "green.png", [0, 255, 0, 255]),
        );
        let window = |left: &str| {
            WindowAreas::home(
                LayerKind::Background,
                layer(vec![half("left", 0.0, left), half("right", 0.5, &blue)]),
            )
        };
        let app = LayerApp {
            kind: LayerKind::Background,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(window(&red)),
            config: LiveConfig::new(Arc::clone(&config)),
            demands: Rc::new(Demands::new(Layer::Background)),
            screen: Rc::new(Cell::new(Screen {
                size: (WIDE as f32, HIGH as f32),
                reserved: Reserved::default(),
            })),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(crate::area::ShellAreas),
        };
        let tree = telar::testing::mount(app.root(), WIDE, HIGH);
        let frame = || {
            telar::relayout_if_dirty();
            tree.commands().to_vec()
        };
        let before = frame();
        assert_eq!(images(&before).len(), 2, "one picture each");

        app.layer.set(window(&green));
        app.generation.look_again();
        let after = frame();
        assert_eq!(
            images(&after).len(),
            3,
            "the left region holds the picture it fades from beside the one it fades to, and the right one still its own"
        );
        let damaged = telar::testing::damage(&after, &before).expect("a change a region bounds");
        let left = Rect::new(0.0, 0.0, WIDE as f32 / 2.0, HIGH as f32);
        let inside = |rect: &Rect| {
            rect.x >= left.x
                && rect.y >= left.y
                && rect.x + rect.width <= left.x + left.width
                && rect.y + rect.height <= left.y + left.height
        };
        assert!(
            damaged
                .iter()
                .any(|rect| rect.width > 0.0 && rect.height > 0.0),
            "the left region is repainted: {damaged:?}"
        );
        assert!(
            damaged.iter().all(inside),
            "and nothing of the right one is: {damaged:?}"
        );

        app.generation.look_again();
        assert_eq!(
            telar::testing::damage(&frame(), &after),
            Some(Vec::new()),
            "an arrangement that did not change repaints nothing"
        );
    }

    fn counting_app(kind: LayerKind, resolved: &Resolved, built: &Rc<Cell<u32>>) -> LayerApp {
        LayerApp {
            kind,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(WindowAreas::of(resolved, kind)),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(window_layer(kind).expect("a session layer").0)),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Counting(Rc::clone(built))),
        }
    }

    /// An edit's preview reaches the windows whose areas it changes and no others, and ending it puts back exactly what was reconciled — the same arrangement, not a copy of it — rebuilding only what the preview had touched.
    #[test]
    fn a_preview_rebuilds_only_the_windows_it_changes_and_ending_it_puts_back_what_was_reconciled()
    {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        let known = layout::Library::default();
        let stored = layout::built_in();
        let (resolved, _) = layout::resolve(&stored, &known, SCREEN, None);
        crate::reconcile::publish(&[crate::reconcile::Desktop {
            output: Some(SCREEN.to_string()),
            config: config(),
            resolved: resolved.clone(),
            reserved: Reserved::of(&resolved, &config()),
            size: (1920.0, 1080.0),
        }]);
        let reconciled = crate::reconcile::desktops();
        let (top, wallpaper) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let _top = counting_app(LayerKind::Top, &resolved, &top).root();
        let _wallpaper = counting_app(LayerKind::Background, &resolved, &wallpaper).root();
        assert_eq!((top.get(), wallpaper.get()), (1, 1));

        crate::reconcile::preview(&stored, &known);
        assert!(
            crate::reconcile::previewing().is_none(),
            "a draft that resolves to what is on screen shows nothing new"
        );

        let mut draft = stored.clone();
        layout::ops::apply(
            &mut draft,
            &layout::LayoutOp::MoveInstance {
                from: layout::Spot {
                    site: layout::Site::everywhere(LayerKind::Top),
                    area: AreaId::new("bar-top"),
                    group: GroupId::new("center"),
                },
                to: layout::Spot {
                    site: layout::Site::everywhere(LayerKind::Top),
                    area: AreaId::new("bar-top"),
                    group: GroupId::new("end"),
                },
                id: InstanceId::new("clock"),
                index: 0,
            },
        )
        .expect("the clock moves");
        crate::reconcile::preview(&draft, &known);
        assert_eq!(
            (top.get(), wallpaper.get()),
            (2, 1),
            "the bar is drawn from the draft and the wallpaper is left alone"
        );
        let shown = crate::reconcile::desktops();
        assert!(!Rc::ptr_eq(&shown, &reconciled));
        assert_eq!(shown[0].reserved, reconciled[0].reserved);

        crate::reconcile::preview(&draft, &known);
        assert_eq!(top.get(), 2, "the same draft again rebuilds nothing");

        crate::reconcile::end_preview();
        assert_eq!((top.get(), wallpaper.get()), (3, 1));
        assert!(Rc::ptr_eq(&crate::reconcile::desktops(), &reconciled));
    }

    struct Radii(Rc<RefCell<Vec<f32>>>);

    impl Areas for Radii {
        fn build(&self, area: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
            assert_eq!(area.theme.radius, area.config.resolve_theme().radius);
            self.0.borrow_mut().push(area.theme.radius);
            Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?))
        }
    }

    /// A config preview builds every area of the window again with the previewed config and its theme, and ending it builds them again with the reconciled one.
    #[test]
    fn a_config_preview_rebuilds_every_area_with_it_and_ending_it_puts_the_reconciled_one_back() {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        let (resolved, _) = layout::resolve(
            &layout::built_in(),
            &layout::Library::default(),
            SCREEN,
            None,
        );
        let built = Rc::new(RefCell::new(Vec::new()));
        let app = LayerApp {
            areas: Rc::new(Radii(Rc::clone(&built))),
            ..counting_app(LayerKind::Top, &resolved, &Rc::new(Cell::new(0)))
        };
        let _root = app.root();
        let was = config().resolve_theme().radius;
        let areas = built.borrow().len();
        assert!(areas > 0);
        assert!(built.borrow().iter().all(|radius| *radius == was));

        crate::reconcile::preview_config(|config| {
            let mut config = config.clone();
            config.theme.radius = Some(21);
            config
        });
        assert_eq!(built.borrow().len(), 2 * areas);
        assert!(built.borrow()[areas..].iter().all(|radius| *radius == 21.0));

        crate::reconcile::end_config_preview();
        assert_eq!(built.borrow().len(), 3 * areas);
        assert!(
            built.borrow()[2 * areas..]
                .iter()
                .all(|radius| *radius == was)
        );
    }

    /// TA-4's fallback draws a layer's areas in the overlay window, so the window they came from must stop drawing them for exactly as long as that lasts — and must go back to drawing them without anyone rebuilding it by hand.
    #[test]
    fn a_concealed_window_builds_none_of_its_areas_until_the_last_token_goes() {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        let areas = Rc::new(Cell::new(0));
        let app = LayerApp {
            kind: LayerKind::Desktop,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn(vec![bar("bar-top", &["clock"])])),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(Layer::Bottom)),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Counting(Rc::clone(&areas))),
        };
        let _root = app.root();
        assert_eq!(areas.get(), 1);

        let first = Concealment::new(&app.generation);
        let second = Concealment::new(&app.generation);
        assert_eq!(areas.get(), 1, "concealed, the rebuild draws no area");
        drop(first);
        assert_eq!(areas.get(), 1, "still concealed while one token lives");
        drop(second);
        assert_eq!(
            areas.get(),
            2,
            "the last token going builds the areas again"
        );
    }

    thread_local! {
        static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn reads_its_options(host: &ui::host::Host) -> ui::descriptor::Built {
        let format = host.options::<config::ClockConfig>().date_format;
        SEEN.with(|seen| seen.borrow_mut().push(format));
        Ok(Box::new(Container::new(LayoutStyle::new(), Vec::new())?))
    }

    const READER: &[ui::descriptor::ModuleDescriptor] = &[ui::descriptor::ModuleDescriptor {
        id: "reader",
        name: "Reader",
        icon: "circle",
        category: ui::descriptor::Category::Info,
        options: &[ui::descriptor::OptionsType::of::<config::ClockConfig>()],
        representations: ui::descriptor::Representations {
            chip: Some(ui::descriptor::ChipDef::new(
                reads_its_options,
                ui::descriptor::Input::ReadOnly,
            )),
            ..ui::descriptor::Representations::NONE
        },
        actions: &[],
        sources: &[],
    }];

    /// A layout edit that changes one instance's options — `layout set reader options.date_format …`, or any commit — reaches the module: the window rebuilds its areas and the instance draws with the value its entry now says.
    #[test]
    fn an_edit_to_an_instance_s_options_re_renders_it_with_the_new_value() {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        ui::descriptor::install(READER);
        SEEN.with(|seen| seen.borrow_mut().clear());
        let placed = |format: Option<&str>| {
            let mut area = bar("bar-top", &["reader"]);
            if let Some(format) = format {
                area.groups[0].children[0].options.insert(
                    "date_format".to_string(),
                    toml::Value::String(format.to_string()),
                );
            }
            drawn(vec![area])
        };
        let app = LayerApp {
            kind: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(placed(None)),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(Layer::Top)),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(crate::area::ShellAreas),
        };
        let _root = app.root();
        let section = config::ClockConfig::default().date_format;

        app.layer.set(placed(Some("%A")));
        app.generation.bump();
        app.layer.set(placed(None));
        app.generation.bump();
        assert_eq!(
            SEEN.with(|seen| seen.borrow().clone()),
            [section.clone(), "%A".to_string(), section],
            "built from the section, then the instance's own value, then the section again once the option is gone"
        );
    }

    struct Pressable(Rc<Cell<u32>>);

    impl Areas for Pressable {
        fn build(&self, _: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
            let pressed = Rc::clone(&self.0);
            Ok(box_item(
                StyledContainer::new(
                    super::whole_window(),
                    paint::md(Color::from_rgb_u8(40, 40, 40)),
                    Vec::new(),
                )?
                .input_opaque()
                .on_press(move || pressed.set(pressed.get() + 1)),
            ))
        }
    }

    /// The transient layer covers the whole window above the areas, and dispatch stops at the first sibling that covers a point — so a layer that took the pointer would leave every area under it dead to presses and hover, open transient or not.
    #[test]
    fn a_press_beside_an_open_transient_reaches_the_area_under_the_transient_layer() {
        use telar::{Event, PointerButton, PointerSource};

        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        crate::transient::close_all();
        let pressed = Rc::new(Cell::new(0));
        let app = LayerApp {
            kind: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn(vec![bar("bar-top", &["clock"])])),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(Layer::Top)),
            screen: Rc::new(Cell::new(Screen {
                size: (400.0, 300.0),
                reserved: Reserved::default(),
            })),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Pressable(Rc::clone(&pressed))),
        };
        crate::transient::open(crate::transient::Spec::new(
            "card",
            crate::transient::Place::Beside(crate::transient::Anchor {
                output: Some(SCREEN.to_string()),
                layer: LayerKind::Top,
                edge: Edge::Top,
                rect: Rect::new(10.0, 0.0, 20.0, 20.0),
                chrome: ui::chrome::Chrome::global(config(), None),
                gap: 8.0,
            }),
            Rc::new(|_: &ui::chrome::Chrome| {
                Ok(box_item(StyledContainer::new(
                    LayoutStyle::new().width(40.0).height(40.0),
                    paint::md(Color::from_rgb_u8(200, 40, 40)),
                    Vec::new(),
                )?))
            }),
        ));

        let mut root = app.root();
        root.on_event(&Event::WindowResized {
            width: 400,
            height: 300,
        });
        let press = |x: f64, y: f64| Event::PointerPressed {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        };
        let release = |x: f64, y: f64| Event::PointerReleased {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        };
        root.on_event(&press(300.0, 250.0));
        root.on_event(&release(300.0, 250.0));
        assert_eq!(
            pressed.get(),
            1,
            "a press where no transient draws belongs to the area under the transient layer"
        );
        crate::transient::close_all();
    }

    /// One window per layer per output is the whole surface model: a transient is a node in one of them, so nothing outside this host and the reservation strips may open a layer surface of its own.
    #[test]
    fn only_layer_windows_and_reservation_strips_open_layer_surfaces() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the workspace root is two levels above this crate")
            .to_path_buf();
        let allowed = [
            (
                concat!("open_", "layer_window("),
                "crates/surfaces/src/layer_window.rs",
            ),
            (concat!("open_", "layer_window("), "apps/spike/"),
            (
                concat!("open_", "reservation("),
                "crates/surfaces/src/reconcile.rs",
            ),
        ];
        let mut offenders = Vec::new();
        let mut stack = vec![root.join("crates"), root.join("apps")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = dir.read_dir() else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().and_then(|n| n.to_str()) != Some(".telar") {
                        stack.push(path);
                    }
                    continue;
                }
                let relative = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                let is_source = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e == "rs" || e == "rsx");
                if !is_source || relative.starts_with("crates/platform-wayland/") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (call, _) in allowed {
                    let at_home = allowed
                        .iter()
                        .any(|(allowed, home)| *allowed == call && relative.starts_with(home));
                    if text.contains(call)
                        && !at_home
                        && !offenders.contains(&format!("{relative}: {call}"))
                    {
                        offenders.push(format!("{relative}: {call}"));
                    }
                }
                let opened = concat!("open_", "surface(");
                if text.contains(opened) {
                    offenders.push(format!("{relative}: {opened}"));
                }
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "a layer surface opened outside the layer-window host and the reservation strips: {offenders:#?}"
        );
    }

    /// An area builder that fills its window, so a captured frame says whether the window mounted anything at all.
    struct Solid(Color);

    impl Areas for Solid {
        fn build(&self, _: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
            Ok(box_item(StyledContainer::new(
                super::whole_window(),
                paint::md(self.0),
                Vec::new(),
            )?))
        }
    }

    const SIDE: u32 = 32;

    fn app(kind: LayerKind, areas: Vec<ResolvedArea>) -> LayerApp {
        LayerApp {
            kind,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn(areas)),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(Layer::Top)),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Solid(Color::from_rgb_u8(40, 200, 40))),
        }
    }

    /// The measurement path, end to end: the host asks no builder where its area went. It reads the rect off the laid-out node and pushes that as the blur region, one frame behind the layout that settled it.
    #[test]
    fn a_blurring_area_reaches_the_blur_region_from_its_own_laid_out_rect() {
        let mut blurring = bar("bar-top", &["clock"]);
        blurring.style.backdrop = Some(Backdrop::Blur);

        let demands = Rc::new(Demands::new(Layer::Top));
        let app = LayerApp {
            kind: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn(vec![blurring])),
            config: LiveConfig::new(config()),
            demands: Rc::clone(&demands),
            screen: Rc::new(Cell::new(screen())),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Solid(Color::from_rgb_u8(40, 200, 40))),
        };

        run_with_platform::<_, _, ()>(
            HeadlessPlatform::new(SIDE, SIDE).with_frames(3),
            AppConfig::from(WindowConfig {
                width: SIDE,
                height: SIDE,
                is_transparent: true,
                ..WindowConfig::default()
            }),
            Arc::new(NoPaths) as Arc<dyn AppPathsProvider>,
            app,
            "hogar-shell-layer-blur-test",
        )
        .expect("headless run");

        assert_eq!(
            demands.blur_region(),
            vec![Rect::new(0.0, 0.0, SIDE as f32, SIDE as f32)],
            "the area fills its window, so that is what the compositor is asked to blur behind"
        );
    }

    /// Four layer windows on one shared runtime, each with its own tree: the layer that resolves an area paints, and the three that resolve nothing mount an empty root and paint nothing.
    ///
    /// This is as far as a headless run reaches. Whether a window is *mapped* is layer-shell state the wayland driver owns, and there is no compositor here, so the mapping half of T-4.1's acceptance is asserted through [`LayerWindows::is_mapped`] above instead.
    #[test]
    fn each_layer_window_renders_its_own_areas_and_an_empty_layer_paints_nothing() {
        let surfaces: Vec<(SurfaceId, AppConfig)> = LayerKind::SESSION
            .iter()
            .enumerate()
            .map(|(index, _)| {
                (
                    SurfaceId(index as u64),
                    AppConfig::from(WindowConfig {
                        width: SIDE,
                        height: SIDE,
                        is_transparent: true,
                        ..WindowConfig::default()
                    }),
                )
            })
            .collect();

        let frames: SurfaceFrameSink = Arc::new(Mutex::new(HashMap::new()));
        let platform = HeadlessPlatform::new(SIDE, SIDE)
            .with_frames(2)
            .capture_surfaces_into(frames.clone());

        run_multi_with_platform(
            platform,
            surfaces,
            |_| Arc::new(NoPaths) as Arc<dyn AppPathsProvider>,
            |id| {
                let kind = LayerKind::SESSION[id.0 as usize];
                let areas = if kind == LayerKind::Top {
                    vec![bar("bar-top", &["clock"])]
                } else {
                    Vec::new()
                };
                app(kind, areas)
            },
            "hogar-shell-layer-windows-test",
        )
        .expect("the run completes");

        let frames = frames.lock().expect("frames");
        for (index, kind) in LayerKind::SESSION.iter().enumerate() {
            let pixels = frames
                .get(&SurfaceId(index as u64))
                .unwrap_or_else(|| panic!("the {kind} window produced no frame"));
            let painted = pixels
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|px| px[3] > 0)
                .count();
            if *kind == LayerKind::Top {
                assert!(
                    painted > 0,
                    "the {kind} window has an area and must paint it"
                );
            } else {
                assert_eq!(
                    painted, 0,
                    "the {kind} window resolves nothing, so its tree paints nothing"
                );
            }
        }
    }

    /// DEC-17: an area above fullscreen is drawn in its output's overlay window, under the overlay layer's own areas, and holds that window open for as long as it exists — drawing or not. Its reservation is still the model's, and taking it away closes the window, since the output cannot scan out while the surface exists.
    #[test]
    fn an_area_above_fullscreen_is_drawn_in_the_overlay_window_and_holds_it_open() {
        let config = config();
        let mut windows = host();
        let mut flagged = bar("bar-top", &["clock"]);
        flagged.above_fullscreen = true;
        let above = resolved(&[
            (LayerKind::Top, layer(vec![flagged.clone()])),
            (LayerKind::Overlay, layer(vec![bar("hud", &["stack"])])),
        ]);
        let ids = |window: LayerKind| -> Vec<(LayerKind, String)> {
            WindowAreas::of(&above, window)
                .areas
                .iter()
                .map(|(home, area)| (*home, area.id.to_string()))
                .collect()
        };
        assert!(ids(LayerKind::Top).is_empty(), "the top window lets it go");
        assert_eq!(
            ids(LayerKind::Overlay),
            [
                (LayerKind::Top, "bar-top".to_string()),
                (LayerKind::Overlay, "hud".to_string())
            ],
            "under the overlay layer's own areas, and still said to be written on top"
        );
        let alone = resolved(&[(LayerKind::Top, layer(vec![flagged.clone()]))]);
        assert_eq!(
            plan(&config, &alone).reserved.top,
            34.0,
            "it reserves what the model says"
        );
        windows.reconcile(&[plan(&config, &alone)], Content::Rebuild);
        assert!(windows.is_open(Some(SCREEN), LayerKind::Overlay));
        assert!(windows.is_mapped(Some(SCREEN), LayerKind::Overlay));
        assert!(
            !windows.is_mapped(Some(SCREEN), LayerKind::Top),
            "the top window has nothing else to show"
        );

        let mut empty = bar("bar-top", &[]);
        empty.above_fullscreen = true;
        let nothing_in_it = resolved(&[(LayerKind::Top, layer(vec![empty]))]);
        windows.reconcile(&[plan(&config, &nothing_in_it)], Content::Rebuild);
        assert!(
            windows.is_open(Some(SCREEN), LayerKind::Overlay),
            "the flag holds the window open whether or not the area draws anything"
        );

        windows.reconcile(&[plan(&config, &only_bars())], Content::Rebuild);
        assert!(
            !windows.is_open(Some(SCREEN), LayerKind::Overlay),
            "once the area is gone the output can scan out again"
        );
        assert!(windows.is_mapped(Some(SCREEN), LayerKind::Top));
    }

    fn dot(_: &ui::host::Host) -> ui::descriptor::Built {
        Ok(Box::new(Container::new(
            LayoutStyle::new().width(20.0).height(20.0),
            Vec::new(),
        )?))
    }

    const fn dot_module(id: &'static str) -> ui::descriptor::ModuleDescriptor {
        ui::descriptor::ModuleDescriptor {
            id,
            name: id,
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: ui::descriptor::Representations {
                chip: Some(ui::descriptor::ChipDef::new(
                    dot,
                    ui::descriptor::Input::ReadOnly,
                )),
                ..ui::descriptor::Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    const DOTS: &[ui::descriptor::ModuleDescriptor] = &[dot_module("dot-a"), dot_module("dot-b")];

    fn bar_on(
        edge: Edge,
        start: &[&str],
        end: &[&str],
        autohide: Option<layout::AutoHide>,
    ) -> ResolvedArea {
        let mut area = bar("bar", start);
        area.kind = ResolvedAreaKind::Bar {
            edge,
            thickness: 34.0,
            length: Extent::Fill,
            offset: 0.0,
            shape: BarShape::default(),
            autohide,
        };
        area.groups.push(ResolvedGroup {
            id: GroupId::new("end"),
            kind: GroupKind::Zone { zone: Zone::End },
            arrange: None,
            cols: layout::Arrange::TRACKS,
            rows: layout::Arrange::TRACKS,
            gap: None,
            repeat: None,
            komponent: None,
            style: Style::default(),
            children: end.iter().map(instance).collect(),
        });
        area
    }

    fn app_of(kind: LayerKind, areas: WindowAreas) -> LayerApp {
        let wlr = window_layer(kind).expect("a session layer").0;
        LayerApp {
            kind,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(areas),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::new(wlr)),
            screen: Rc::new(Cell::new(Screen {
                size: (600.0, 400.0),
                reserved: Reserved::default(),
            })),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(crate::area::ShellAreas),
        }
    }

    fn laid_out(root: &mut Box<dyn Component>) {
        root.on_event(&telar::Event::WindowResized {
            width: 600,
            height: 400,
        });
    }

    /// The registry is what is on screen: after a layout edit an instance is where the edit put it and nowhere else, a group with nothing left in it is gone, and the area is the strip its bar takes — on every edge.
    #[test]
    fn the_registry_follows_a_layout_edit_on_every_edge() {
        for edge in Edge::ALL {
            telar::reset_layout_runtime();
            set_theme(Config::default().resolve_theme());
            ui::descriptor::install(DOTS);
            let scope = telar::owner_scope();
            let owner = scope.id();
            let app = app_of(
                LayerKind::Top,
                WindowAreas::home(
                    LayerKind::Top,
                    layer(vec![bar_on(edge, &["dot-a", "dot-b"], &[], None)]),
                ),
            );
            let mut root = app.root();
            laid_out(&mut root);
            let at = crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar"));
            let (start, end) = (GroupId::new("start"), GroupId::new("end"));
            let (a, b) = (InstanceId::new("dot-a"), InstanceId::new("dot-b"));
            let rect = |node: &crate::rects::Node| crate::rects::rect(node);

            let strip = rect(&at).expect("the bar is registered");
            let across = if edge.is_horizontal() {
                strip.height
            } else {
                strip.width
            };
            assert_eq!(across, 34.0, "{edge:?}: the area is its strip");
            let first = rect(&at.instance(&start, &a)).expect("dot-a");
            let second = rect(&at.instance(&start, &b)).expect("dot-b");
            let group = rect(&at.group(&start)).expect("the start group");
            for chip in [first, second] {
                assert!(
                    group.contains(chip.x + 1.0, chip.y + 1.0)
                        && strip.contains(chip.x + 1.0, chip.y + 1.0),
                    "{edge:?}: {chip:?} is inside its group {group:?} and its bar {strip:?}"
                );
            }
            assert!(
                rect(&at.group(&end)).is_none(),
                "{edge:?}: an empty group has no rect"
            );

            app.layer.set(WindowAreas::home(
                LayerKind::Top,
                layer(vec![bar_on(edge, &["dot-a"], &["dot-b"], None)]),
            ));
            app.generation.bump();
            laid_out(&mut root);
            assert!(
                rect(&at.instance(&start, &b)).is_none(),
                "{edge:?}: not where it was"
            );
            let moved = rect(&at.instance(&end, &b)).expect("dot-b, moved");
            let far = |r: telar::Rect| if edge.is_horizontal() { r.x } else { r.y };
            assert!(
                far(moved)
                    > far(strip)
                        + (if edge.is_horizontal() {
                            strip.width
                        } else {
                            strip.height
                        }) / 2.0,
                "{edge:?}: at the far end of the bar, {moved:?} of {strip:?}"
            );

            app.layer.set(WindowAreas::home(
                LayerKind::Top,
                layer(vec![bar_on(edge, &[], &["dot-b"], None)]),
            ));
            app.generation.bump();
            laid_out(&mut root);
            assert!(
                rect(&at.instance(&start, &a)).is_none(),
                "{edge:?}: removed"
            );
            assert!(
                rect(&at.group(&start)).is_none(),
                "{edge:?}: and its group with it"
            );
            assert!(rect(&at.instance(&end, &b)).is_some());
            drop((root, scope));
            telar::dispose_owner(owner);
        }
    }

    /// A bar above fullscreen still works on every edge, hidden or not: it is built into the overlay window, registered under the layer it was written on, and what its chips open hangs off them in the overlay window, the window they are in (DEC-9).
    #[test]
    fn a_bar_above_fullscreen_is_built_in_the_overlay_window_on_every_edge() {
        for edge in Edge::ALL {
            for autohide in [
                None,
                Some(layout::AutoHide {
                    peek: 2.0,
                    on_hover: true,
                }),
            ] {
                telar::reset_layout_runtime();
                set_theme(Config::default().resolve_theme());
                ui::descriptor::install(DOTS);
                let scope = telar::owner_scope();
                let owner = scope.id();
                let mut flagged = bar_on(edge, &["dot-a"], &[], autohide);
                flagged.above_fullscreen = true;
                let resolved = resolved(&[(LayerKind::Top, layer(vec![flagged]))]);
                let app = app_of(
                    LayerKind::Overlay,
                    WindowAreas::of(&resolved, LayerKind::Overlay),
                );
                let mut root = app.root();
                laid_out(&mut root);

                let at =
                    crate::rects::Node::area(Some(SCREEN), LayerKind::Top, &AreaId::new("bar"));
                let strip =
                    crate::rects::rect(&at).expect("registered under the layer it was written on");
                let across = if edge.is_horizontal() {
                    strip.height
                } else {
                    strip.width
                };
                assert_eq!(across, 34.0, "{edge:?} {autohide:?}");
                let found = crate::transient::chips::find("dot-a", None, Some(SCREEN))
                    .expect("its chip opens things");
                assert_eq!(
                    found.anchor.layer,
                    LayerKind::Overlay,
                    "{edge:?} {autohide:?}: what the chip opens lives in the chip's window"
                );
                assert_eq!(found.anchor.edge, edge);
                drop((root, scope));
                telar::dispose_owner(owner);
            }
        }
    }
}

#[cfg(test)]
mod grid_tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;

    use config::Config;
    use layout::{
        Anchor, AreaId, GroupId, GroupKind, InstanceId, LayerKind, Representation, ResolvedArea,
        ResolvedAreaKind, ResolvedGroup, ResolvedInstance, ResolvedLayer, Style, Within,
    };
    use platform_wayland::Layer;
    use telar::{Component, LayoutError, LayoutItem, set_theme};

    use super::*;

    const SCREEN: &str = "DP-1";

    fn dot(_: &ui::host::Host) -> ui::descriptor::Built {
        Ok(Box::new(telar::Container::new(
            telar::LayoutStyle::new().width(20.0).height(20.0),
            Vec::new(),
        )?))
    }

    const fn dot_module(id: &'static str) -> ui::descriptor::ModuleDescriptor {
        ui::descriptor::ModuleDescriptor {
            id,
            name: id,
            icon: "circle",
            category: ui::descriptor::Category::Info,
            options: &[],
            representations: ui::descriptor::Representations {
                widget: Some(ui::descriptor::WidgetDef {
                    sizes: &ui::host::WidgetSize::ALL,
                    build: dot,
                    input: ui::descriptor::Input::ReadOnly,
                }),
                ..ui::descriptor::Representations::NONE
            },
            actions: &[],
            sources: &[],
        }
    }

    const DOTS: &[ui::descriptor::ModuleDescriptor] = &[dot_module("dot-a"), dot_module("dot-b")];

    fn widget(id: &str, col: u32, row: u32, representation: Representation) -> ResolvedGroup {
        ResolvedGroup {
            id: GroupId::new(id),
            kind: GroupKind::Cell {
                col,
                row,
                col_span: 1,
                row_span: 1,
            },
            arrange: None,
            cols: layout::Arrange::TRACKS,
            rows: layout::Arrange::TRACKS,
            gap: None,
            repeat: None,
            komponent: None,
            style: Style::default(),
            children: vec![ResolvedInstance {
                id: InstanceId::new(id),
                module: id.to_string(),
                representation,
                options: toml::Table::new(),
                bindings: BTreeMap::new(),
                style: Style::default(),
                placement: None,
                actions: BTreeMap::new(),
            }],
        }
    }

    fn grid(groups: Vec<ResolvedGroup>) -> ResolvedArea {
        ResolvedArea {
            id: AreaId::new("widgets"),
            kind: ResolvedAreaKind::Grid {
                rect: layout::Rect::default(),
                cell: 80.0,
                gap: 16.0,
                anchor: Anchor::TopLeft,
            },
            reserve: false,
            above_fullscreen: false,
            within: Within::Output,
            style: Style::default(),
            visible: None,
            groups,
            actions: BTreeMap::new(),
        }
    }

    struct Counted(Rc<Cell<usize>>);

    impl Areas for Counted {
        fn build(&self, context: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError> {
            self.0.set(self.0.get() + 1);
            crate::area::ShellAreas.build(context)
        }
    }

    fn drawn(groups: Vec<ResolvedGroup>) -> WindowAreas {
        WindowAreas::home(
            LayerKind::Desktop,
            ResolvedLayer {
                areas: vec![grid(groups)],
            },
        )
    }

    /// F-5.8: a layout change that only moves a grid's widgets to other cells keeps the grid's nodes and moves them, which is what lets `animate_layout` slide them there; a change to what a group holds builds the grid again.
    #[test]
    fn a_widget_moved_to_another_cell_keeps_its_node_and_moves() {
        telar::reset_layout_runtime();
        set_theme(Config::default().resolve_theme());
        ui::descriptor::install(DOTS);
        let scope = telar::owner_scope();
        let owner = scope.id();
        let builds = Rc::new(Cell::new(0));
        let app = LayerApp {
            kind: LayerKind::Desktop,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(drawn(vec![
                widget("dot-a", 0, 0, Representation::WidgetS),
                widget("dot-b", 2, 0, Representation::WidgetS),
            ])),
            config: LiveConfig::new(Arc::new(Config::default())),
            demands: Rc::new(Demands::new(Layer::Bottom)),
            screen: Rc::new(Cell::new(Screen {
                size: (1200.0, 800.0),
                reserved: Reserved::default(),
            })),
            generation: Generation::default(),
            shown: ScreenFeed::default(),
            mapped: MappedFeed::default(),
            areas: Rc::new(Counted(Rc::clone(&builds))),
        };
        let mut root = app.root();
        let lay_out = |root: &mut Box<dyn Component>| {
            root.on_event(&telar::Event::WindowResized {
                width: 1200,
                height: 800,
            })
        };
        lay_out(&mut root);
        let at =
            crate::rects::Node::area(Some(SCREEN), LayerKind::Desktop, &AreaId::new("widgets"));
        let b = at.group(&GroupId::new("dot-b"));
        let before = crate::rects::rect(&b).expect("dot-b is placed");
        assert_eq!((before.x, before.y), (192.0, 0.0), "two cells of 96 px in");
        assert_eq!(builds.get(), 1);

        app.layer.set(drawn(vec![
            widget("dot-a", 0, 0, Representation::WidgetS),
            widget("dot-b", 4, 2, Representation::WidgetS),
        ]));
        app.generation.look_again();
        lay_out(&mut root);
        assert_eq!(builds.get(), 1, "the grid kept its nodes");
        let after = crate::rects::rect(&b).expect("still placed");
        assert_eq!(
            (after.x, after.y),
            (384.0, 192.0),
            "and dot-b is on its new cells"
        );

        app.layer.set(drawn(vec![
            widget("dot-a", 0, 0, Representation::WidgetM),
            widget("dot-b", 4, 2, Representation::WidgetS),
        ]));
        app.generation.look_again();
        lay_out(&mut root);
        assert_eq!(builds.get(), 2, "a widget of another size is built again");
        drop((root, scope));
        telar::dispose_owner(owner);
    }

    #[test]
    fn only_a_change_of_cells_counts_as_a_move() {
        let was = grid(vec![widget("dot-a", 0, 0, Representation::WidgetS)]);
        let moved = grid(vec![widget("dot-a", 3, 1, Representation::WidgetS)]);
        let resized = grid(vec![widget("dot-a", 0, 0, Representation::WidgetL)]);
        let renamed = grid(vec![widget("dot-b", 0, 0, Representation::WidgetS)]);
        assert!(crate::area::moves_only(&was, &moved));
        assert!(!crate::area::moves_only(&was, &resized));
        assert!(!crate::area::moves_only(&was, &renamed));
        let mut padded = moved.clone();
        padded.style.padding = Some(layout::Sides::all(8.0));
        assert!(!crate::area::moves_only(&was, &padded));
    }
}
