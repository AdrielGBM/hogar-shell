//! The window of one layer on one output: what is drawn in it, whether it is on screen at all, and what it asks of the compositor.
//!
//! One fullscreen layer-shell window per `(output, layer)`, anchored to all four edges and ignoring every exclusive zone, so **every window on an output shares one coordinate space**: a rect laid out in the top window is the same rect in the overlay one. [`LayerWindows`] owns them, keyed so that a layout change reaches the window that is already up rather than reopening it — the same identity rule the surface reconcile has always had, now over four windows a screen instead of one a role.
//!
//! **A window is on screen only while its layer has something to show**, and how it goes away depends on what it costs to leave it there. Background, desktop and top windows are opened with their output and merely hidden, so coming back is a buffer-less commit rather than a cold map (F-6.11 measured that at ~20 ms to first frame). The overlay window is the exception and is closed outright, because on Hyprland the *surface* is what costs the output direct scanout, from `get_layer_surface` to destroy, whether or not it was ever mapped — see [`WhileEmpty`], which is where that is written down. [`Presence`] holds the rule; the layout and whatever [`Hold`]s a window decide the answer.
//!
//! What a bar, a grid or a stack looks like is not here. [`Areas`] is the seam, and it is one call per area per build returning one node — a builder is never asked where it put the area, because where an area ends up is a question layout answers and the window reads off the node afterwards. The host knows which areas live on which layer, whether that layer is on screen, and what it asks of the compositor; it knows nothing about what any of it draws.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use telar::{
    App, Color, Component, Container, LayoutError, LayoutItem, LayoutStyle, Rect, SizeDimension,
    WindowConfig, WindowRoot, box_item, effect, reset_layout_runtime, set_context, set_theme,
    track_layout,
};

use config::theme::NordTheme;
use config::{Config, Edge, LiveConfig};
use layout::{Backdrop, LayerKind, Resolved, ResolvedArea, ResolvedLayer, Within};
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
    /// This monitor's logical size. Every window on it is the whole output, so it is also every window's size, and it is what turns a fractional rect into pixels.
    pub size: (f32, f32),
}

/// The four edges an output's reserving areas have taken, in logical pixels.
///
/// It is summed across *every* layer, because reservation is an output-level fact: a bar in the top window and a reserving dock on the desktop both take space from the same screen, and neither can see the other. That is exactly why an area cannot work this out for itself and the host hands it down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reserved {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Reserved {
    pub fn of(resolved: &Resolved) -> Self {
        Self {
            top: resolved.reserved(Edge::Top),
            right: resolved.reserved(Edge::Right),
            bottom: resolved.reserved(Edge::Bottom),
            left: resolved.reserved(Edge::Left),
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
///
/// A layout or config edit changes what they draw, so every window that survives it builds again. A monitor being plugged in does not: rebuilding the other screens' windows for it would throw away their state to redraw exactly what was already there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    Rebuild,
    Keep,
}

/// What a reconcile did, for the log — and for a test that cares that a layout change reached the windows already up instead of reopening them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    pub opened: usize,
    pub closed: usize,
    pub rebuilt: usize,
    /// How many windows are on screen once the reconcile is done, which is the count lazy mapping is about.
    pub mapped: usize,
}

/// The live layer windows, keyed so a reload finds the one it is about.
pub struct LayerWindows {
    areas: Rc<dyn Areas>,
    live: Vec<(WindowKey, Window)>,
}

impl LayerWindows {
    /// A host whose windows build their areas with `areas`.
    pub fn new(areas: Rc<dyn Areas>) -> Self {
        Self {
            areas,
            live: Vec::new(),
        }
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
                let resolved = plan.resolved.layer(layer).cloned().unwrap_or_default();
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
        resolved: ResolvedLayer,
        plan: &LayerPlan<'_>,
        done: &mut Reconciled,
    ) {
        let Some((wlr, namespace)) = window_layer(key.layer) else {
            return;
        };
        let layer = LiveLayer::new(resolved);
        let config = LiveConfig::new(Arc::clone(plan.config));
        let demands = Rc::new(Demands::default());
        let screen = Rc::new(Cell::new(Screen {
            size: plan.size,
            reserved: Reserved::of(plan.resolved),
        }));
        let surface = {
            let kind = key.layer;
            let output = key.output.clone();
            let layer = layer.clone();
            let config = config.clone();
            let demands = Rc::clone(&demands);
            let screen = Rc::clone(&screen);
            let areas = Rc::clone(&self.areas);
            move || {
                let handle = Rc::new(open_layer_window(
                    output.clone(),
                    wlr,
                    namespace,
                    LayerApp {
                        kind,
                        output: output.clone(),
                        layer: layer.clone(),
                        config: config.clone(),
                        demands: Rc::clone(&demands),
                        screen: Rc::clone(&screen),
                        areas: Rc::clone(&areas),
                    },
                ));
                demands.rebind(Rc::downgrade(&handle));
                handle
            }
        };
        let presence = Presence::new(while_empty(key.layer), Box::new(surface));
        presence.set_draws(layer_draws(&layer.get()));
        done.opened += 1;
        self.live.push((
            key,
            Window {
                layer,
                config,
                demands,
                screen,
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
    presence: Rc<Presence>,
}

impl Window {
    fn adopt(
        &self,
        resolved: ResolvedLayer,
        plan: &LayerPlan<'_>,
        content: Content,
        done: &mut Reconciled,
    ) {
        self.layer.set(resolved);
        self.config.set(Arc::clone(plan.config));
        self.screen.set(Screen {
            size: plan.size,
            reserved: Reserved::of(plan.resolved),
        });
        if content == Content::Rebuild {
            self.presence.rebuild();
            done.rebuilt += 1;
        }
        self.presence.set_draws(layer_draws(&self.layer.get()));
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
pub struct LiveLayer(Rc<RefCell<Rc<ResolvedLayer>>>);

impl LiveLayer {
    pub fn new(layer: ResolvedLayer) -> Self {
        Self(Rc::new(RefCell::new(Rc::new(layer))))
    }

    pub fn get(&self) -> Rc<ResolvedLayer> {
        Rc::clone(&self.0.borrow())
    }

    pub fn set(&self, layer: ResolvedLayer) {
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
}

impl Presence {
    fn new(empty: WhileEmpty, surface: Box<dyn Fn() -> Rc<LayerWindowHandle>>) -> Rc<Self> {
        let window = match empty {
            WhileEmpty::Hide => Some(surface()),
            WhileEmpty::Close => None,
        };
        Rc::new(Self {
            empty,
            on_screen: Cell::new(window.is_some()),
            window: RefCell::new(window),
            surface,
            draws: Cell::new(false),
            holds: Cell::new(0),
            shut: Cell::new(false),
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

    /// A rebuild asked for while the window is closed is dropped, not queued: the surface that reopens builds from the cells as it mounts, so it is already what the rebuild was about.
    fn rebuild(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            window.rebuild();
        }
    }

    fn shut(&self) {
        self.shut.set(true);
        self.on_screen.set(false);
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

/// What one window's content asks of the compositor, as against what its layout puts in it.
///
/// Both answers are renegotiated the moment they change rather than read once per build, because both are about what is mounted *now*: a `wl_surface` has exactly one keyboard interactivity, and the launcher that wants the keyboard opens and closes without the window it lives in being rebuilt. A demand is a token, so a node takes its own back by dropping one and disturbs nothing else's.
#[derive(Default)]
pub struct Demands {
    window: RefCell<Weak<LayerWindowHandle>>,
    keyboard: RefCell<BTreeMap<u64, KeyboardMode>>,
    next: Cell<u64>,
    /// What the compositor was last told. A layer window is created taking no keyboard at all and with no blur region, so neither is pushed until something asks for more.
    sent_keyboard: Cell<KeyboardMode>,
    sent_blur: RefCell<Vec<Rect>>,
}

impl Demands {
    /// Asks the window to take the keyboard at least this way for as long as the token lives. What the window negotiates is the maximum of every live demand: `None` < `OnDemand` < `Exclusive`.
    pub fn keyboard(self: &Rc<Self>, want: KeyboardMode) -> Demand {
        let id = self.next.get();
        self.next.set(id + 1);
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

    /// What the compositor is currently asked to blur behind.
    pub fn blur_region(&self) -> Vec<Rect> {
        self.sent_blur.borrow().clone()
    }

    /// Points the demands at a surface, which for the overlay layer happens again every time its window is reopened.
    ///
    /// What was already pushed is forgotten with the surface that held it: a new one is created taking no keyboard and with no blur region, so both have to be asked for again rather than diffed against what a surface that no longer exists was told.
    fn rebind(&self, window: Weak<LayerWindowHandle>) {
        *self.window.borrow_mut() = window;
        self.sent_keyboard.set(KeyboardMode::None);
        self.sent_blur.borrow_mut().clear();
        self.settle_keyboard();
    }

    fn withdraw(&self, id: u64) {
        self.keyboard.borrow_mut().remove(&id);
        self.settle_keyboard();
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

/// One node's standing request of its window, withdrawn when this is dropped — which is what makes the window's keyboard the maximum of what is mounted now rather than of everything that ever asked.
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
/// An area with a `visible` expression counts as visible. The evaluator arrives with the data sprint, and until it does, hiding what the user placed because the shell cannot yet read the condition is the worse of the two wrong answers.
pub fn area_draws(area: &ResolvedArea) -> bool {
    !area.kind.holds_instances()
        || area.style.fill.is_some()
        || area.groups.iter().any(|group| !group.children.is_empty())
}

/// Whether a layer has anything to show, which is whether its window is on screen at all.
pub fn layer_draws(layer: &ResolvedLayer) -> bool {
    layer.areas.iter().any(area_draws)
}

/// What turns one resolved area into the node its layer's window mounts.
///
/// The node places itself: every window on an output is the whole output and shares one coordinate space (TA-1), so an area is positioned absolutely within it rather than flowed, and the order areas are built in is their z-order within the layer.
///
/// A builder is not asked where it put the area. Where it ended up is a question layout answers, and the window reads it off the node afterwards for the one thing that needs it — see [`blurs`]. A builder that had to report its own rectangle would be restating something it often cannot know: a bar running [`Extent::Fill`](layout::Extent::Fill) is as long as its neighbours leave it.
pub trait Areas {
    fn build(&self, area: &AreaContext<'_>) -> Result<Box<dyn LayoutItem>, LayoutError>;
}

/// Whether an area asks the compositor to blur what is behind it, and is therefore one of the few whose rectangle the window measures.
///
/// The host tracks the laid-out rect of these areas and no others. Measuring every area would be bookkeeping for a question nobody asks: the blur region is the only thing a window needs an area's geometry *for*, since everything else about where an area sits is settled by the node placing itself.
///
/// Rectangle, not rounded rectangle: `ext-background-effect-v1` takes a `wl_region`, which is rectangles added and subtracted, so an area's corner radius reaches its paint and not the blur behind it. At the corners of a rounded translucent area the blur runs past the paint.
fn blurs(area: &ResolvedArea) -> bool {
    area.style.backdrop == Some(Backdrop::Blur)
}

/// Everything one area's builder is handed.
pub struct AreaContext<'a> {
    pub area: &'a ResolvedArea,
    /// Which layer's window the area is being built into. An area does not carry its own layer, because which layer it is on is a property of where it was written.
    pub layer: LayerKind,
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
    areas: Rc<dyn Areas>,
}

/// What a window needs to know about the output it covers in order to place the areas on it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Screen {
    size: (f32, f32),
    reserved: Reserved,
}

impl LayerApp {
    /// Keeps the window's blur region in step with where layout actually put the areas that asked to blur.
    ///
    /// An effect rather than a value read during the build, because at build time nothing has been laid out yet and every rectangle is still zero. It belongs to the owner of the build that registered it, so a rebuild disposes it and the next one takes over with whatever areas blur now. The region therefore lands one frame behind the layout that moved it — the same frame the input region already costs, and for the same reason.
    ///
    /// A zero-area rectangle is dropped rather than sent: it adds nothing to a `wl_region`, and before the first layout it is what every area reports.
    fn watch_blur(&self, areas: Vec<telar::RwSignal<Rect>>) {
        let demands = Rc::clone(&self.demands);
        effect(move || {
            let rects = areas
                .iter()
                .map(|area| area.get())
                .filter(|rect| rect.width > 0.0 && rect.height > 0.0)
                .collect();
            demands.set_area_blur(rects);
        });
    }
}

impl App for LayerApp {
    fn root(&self) -> Box<dyn Component> {
        reset_layout_runtime();
        let config = self.config.get();
        let theme = config.resolve_theme();
        set_theme(theme);
        services::locale::attach(config.language());
        set_context(LayerWindowContext {
            layer: self.kind,
            output: self.output.clone(),
            demands: Rc::clone(&self.demands),
        });

        let layer = self.layer.get();
        let screen = self.screen.get();
        let blur_available = background_effect_supported();
        let mut nodes = Vec::with_capacity(layer.areas.len());
        let mut blurring = Vec::new();
        for area in &layer.areas {
            let built = self.areas.build(&AreaContext {
                area,
                layer: self.kind,
                output: self.output.as_deref(),
                config: &config,
                theme,
                bounds: screen.reserved.box_of(area.within, screen.size),
                reserved: screen.reserved,
                blur_available,
                demands: &self.demands,
                output_size: screen.size,
            });
            let node = match built {
                Ok(node) => node,
                Err(error) => {
                    tracing::error!(
                        area = %area.id,
                        layer = %self.kind,
                        "the area failed to build: {error}"
                    );
                    continue;
                }
            };
            if blurs(area)
                && let Some(rect) = track_layout(node.layout_node())
            {
                blurring.push(rect);
            }
            nodes.push(node);
        }
        self.watch_blur(blurring);

        let root = ui::panel::or_empty(
            self.kind.as_str(),
            Container::new(whole_window(), nodes).map(box_item),
        );
        Box::new(WindowRoot::new(root))
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
        AreaId, AreaStyle, BarShape, Expr, Extent, GroupId, GroupKind, InstanceId, Representation,
        ResolvedAreaKind, ResolvedGroup, ResolvedInstance, Zone,
    };
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
            size: (1920.0, 1080.0),
        }
    }

    fn resolved(layers: &[(LayerKind, ResolvedLayer)]) -> Resolved {
        Resolved {
            output: SCREEN.to_string(),
            workspace: None,
            layers: layers.iter().cloned().collect(),
        }
    }

    fn layer(areas: Vec<ResolvedArea>) -> ResolvedLayer {
        ResolvedLayer { areas }
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
            style: AreaStyle::default(),
            visible: None,
            groups: vec![ResolvedGroup {
                id: GroupId::new("start"),
                kind: GroupKind::Zone { zone: Zone::Start },
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
            style: AreaStyle::default(),
            visible: None,
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

        let second = Resolved {
            output: "HDMI-A-1".to_string(),
            ..only_bars()
        };
        let done = windows.reconcile(
            &[
                plan(&config, &resolved),
                LayerPlan {
                    output: Some("HDMI-A-1"),
                    config: &config,
                    resolved: &second,
                    size: (2560.0, 1440.0),
                },
            ],
            Content::Keep,
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

        let done = windows.reconcile(&[], Content::Keep);
        assert_eq!(done.closed, 4);
        assert!(windows.is_empty());
        assert!(
            windows.hold(Some(SCREEN), LayerKind::Top).is_none(),
            "nothing can hold a window on a screen that is gone"
        );
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

    /// Until the expression evaluator lands, an area conditioned on one counts as visible: hiding what the user placed because the shell cannot read the condition is the worse of the two wrong answers.
    #[test]
    fn an_area_hidden_by_an_expression_still_counts_as_visible_for_now() {
        let mut conditional = bar("bar-top", &["clock"]);
        conditional.visible = Some(Expr("gaming".into()));
        assert!(area_draws(&conditional));
    }

    #[test]
    fn a_layer_draws_when_any_one_of_its_areas_does() {
        assert!(!layer_draws(&ResolvedLayer::default()));
        assert!(!layer_draws(&layer(vec![bar("a", &[]), bar("b", &[])])));
        assert!(layer_draws(&layer(vec![
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
        let demands = Demands::default();
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
        let reserved = Reserved::of(&only_bars());
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

    /// Reservation is summed across every layer, which is exactly what one window cannot see for itself: a reserving dock on the desktop shortens a bar in the top window.
    #[test]
    fn what_is_reserved_is_summed_across_the_layers_a_window_cannot_see() {
        let mut dock = bar("dock", &["visualiser"]);
        dock.kind = ResolvedAreaKind::Dock {
            edge: Edge::Left,
            thickness: 60.0,
        };
        let both = resolved(&[
            (LayerKind::Top, layer(vec![bar("bar-top", &["clock"])])),
            (LayerKind::Desktop, layer(vec![dock])),
        ]);

        let reserved = Reserved::of(&both);
        assert_eq!(reserved.top, 34.0);
        assert_eq!(
            reserved.left, 60.0,
            "the desktop layer's dock takes its edge from the same screen the top bar is on"
        );
    }

    /// Only an area that asked to blur is measured, so nobody has to wonder why the window does not track every area's rect.
    #[test]
    fn only_an_area_styled_to_blur_its_backdrop_is_measured() {
        assert!(!blurs(&bar("bar-top", &["clock"])));

        let mut blurring = bar("bar-top", &["clock"]);
        blurring.style.backdrop = Some(Backdrop::Blur);
        assert!(blurs(&blurring));

        let mut plain = bar("bar-top", &["clock"]);
        plain.style.backdrop = Some(Backdrop::None);
        assert!(
            !blurs(&plain),
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
            layer: LiveLayer::new(layer(areas)),
            config: LiveConfig::new(config()),
            demands: Rc::new(Demands::default()),
            screen: Rc::new(Cell::new(screen())),
            areas: Rc::new(Solid(Color::from_rgb_u8(40, 200, 40))),
        }
    }

    /// The measurement path, end to end: the host asks no builder where its area went. It reads the rect off the laid-out node and pushes that as the blur region, one frame behind the layout that settled it.
    #[test]
    fn a_blurring_area_reaches_the_blur_region_from_its_own_laid_out_rect() {
        let mut blurring = bar("bar-top", &["clock"]);
        blurring.style.backdrop = Some(Backdrop::Blur);

        let demands = Rc::new(Demands::default());
        let app = LayerApp {
            kind: LayerKind::Top,
            output: Some(SCREEN.to_string()),
            layer: LiveLayer::new(layer(vec![blurring])),
            config: LiveConfig::new(config()),
            demands: Rc::clone(&demands),
            screen: Rc::new(Cell::new(screen())),
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
}
