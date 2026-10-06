//! Everything the shell opens over its layout, as a node inside a layer window rather than a surface of its own (DEC-9).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use telar::{
    Color, Container, DismissRegistration, LayoutItem, LayoutStyle, ReactiveList, ReadSignal, Rect,
    RectStyle, RwSignal, SizeDimension, StyledContainer, Transition, box_item, exits_in_flight,
    on_cleanup, signal,
};

use config::fingerprint::{Fingerprint, Reload, Stamp};
use config::{Config, Edge, ResolvedShape};
use layout::LayerKind;
use platform_wayland::{KeyboardMode, timeout};
use ui::chrome::Chrome;
use ui::descriptor::Built;

use crate::layer_window::{Concealment, Demand, Demands, Hold, Holder, Screen, WindowKey};

pub const DEFAULT_GAP: f32 = 8.0;

#[derive(Clone)]
pub struct Anchor {
    pub output: Option<String>,
    pub layer: LayerKind,
    pub edge: Edge,
    pub rect: Rect,
    /// The anchor's own area's chrome, so a drawer rounds and breathes like the bar it hangs off.
    pub chrome: Chrome,
    pub gap: f32,
}

#[derive(Clone)]
pub enum Place {
    Beside(Anchor),
    /// The whole window its anchor is drawn in, for content that works on the anchored thing itself as well as beside it: a popover and the handles it puts on the item it customizes.
    Over(Anchor),
    Centred,
    /// Pinned to `anchor` of the box `within` names, moved by `offset` and kept inside it: the launcher where a stack says it opens.
    Pinned {
        within: layout::Within,
        anchor: layout::Anchor,
        offset: layout::Offset,
    },
    Docked {
        edge: Edge,
        thickness: f32,
    },
    Whole,
}

impl Place {
    fn anchor(&self) -> Option<&Anchor> {
        match self {
            Place::Beside(anchor) | Place::Over(anchor) => Some(anchor),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    None,
    Fade,
    Slide(Edge),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// One at a time, and a standing window closes it: a drawer claims the whole output to hear a press outside it, so a window opened under one would be dismissed by the first press near it.
    Drawer,
    Standing,
    Free,
}

pub type Content = Rc<dyn Fn(&Chrome) -> Built>;

#[derive(Clone)]
pub struct Spec {
    pub id: String,
    pub slot: Slot,
    pub place: Place,
    /// The screen an unanchored transient opens on; `None` is the focused one.
    pub output: Option<String>,
    pub keyboard: KeyboardMode,
    /// The window claims the whole output while it is up, which is how a press outside reaches the shell at all.
    pub dismiss_on_outside: bool,
    pub motion: Motion,
    pub content: Content,
    pub on_close: Option<Rc<dyn Fn()>>,
}

impl Spec {
    pub fn new(id: impl Into<String>, place: Place, content: Content) -> Self {
        Self {
            id: id.into(),
            slot: Slot::Free,
            place,
            output: None,
            keyboard: KeyboardMode::None,
            dismiss_on_outside: false,
            motion: Motion::None,
            content,
            on_close: None,
        }
    }

    pub fn slot(mut self, slot: Slot) -> Self {
        self.slot = slot;
        self
    }

    pub fn output(mut self, output: Option<String>) -> Self {
        self.output = output;
        self
    }

    pub fn keyboard(mut self, keyboard: KeyboardMode) -> Self {
        self.keyboard = keyboard;
        self
    }

    pub fn dismiss_on_outside(mut self) -> Self {
        self.dismiss_on_outside = true;
        self
    }

    pub fn motion(mut self, motion: Motion) -> Self {
        self.motion = motion;
        self
    }

    pub fn on_close(mut self, on_close: impl Fn() + 'static) -> Self {
        self.on_close = Some(Rc::new(on_close));
        self
    }
}

pub struct Entry {
    spec: Spec,
    serial: u64,
    window: WindowKey,
    closing: Cell<bool>,
    /// Created by the row, and flipped by the registry from outside the tree.
    shown: RefCell<Option<RwSignal<bool>>>,
    builds: RefCell<Option<RwSignal<u64>>>,
    stamp: RefCell<Stamp>,
    rebuilt: Cell<u64>,
    /// Given up the moment the transient closes rather than when its exit ends, so Esc and the keyboard go back at once.
    claims: RefCell<Option<(Demand, DismissRegistration)>>,
    hold: RefCell<Option<Hold>>,
}

impl Entry {
    pub fn id(&self) -> &str {
        &self.spec.id
    }

    fn is_open(&self) -> bool {
        !self.closing.get()
    }
}

#[derive(Default)]
struct Registry {
    entries: Vec<Rc<Entry>>,
    sinks: Vec<(WindowKey, RwSignal<Vec<Rc<Entry>>>)>,
    /// How many exits each window reported in flight when it last looked, which is what says whether a closing transient can go at once.
    exits: Vec<(WindowKey, usize)>,
    holder: Option<Holder>,
    draining: Vec<(WindowKey, Hold, std::time::Instant)>,
    serial: u64,
}

type Hidden = dyn Fn(Option<&str>, LayerKind) -> bool;

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
    static HIDDEN: RefCell<Option<Box<Hidden>>> = const { RefCell::new(None) };
}

pub fn install(holder: Holder) {
    REGISTRY.with(|registry| registry.borrow_mut().holder = Some(holder));
}

/// How the registry learns that a layer is out of sight on an output, so what hangs off it opens in the overlay window instead.
pub fn set_hidden_layers(hidden: impl Fn(Option<&str>, LayerKind) -> bool + 'static) {
    HIDDEN.with(|slot| *slot.borrow_mut() = Some(Box::new(hidden)));
}

fn layer_hidden(output: Option<&str>, layer: LayerKind) -> bool {
    HIDDEN.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|hidden| hidden(output, layer))
    })
}

pub fn focused_output() -> Option<String> {
    let dir = services::hyprland::socket_dir()?;
    services::hyprland::focused_monitor(&dir)
}

fn route(spec: &Spec, keys: &[WindowKey]) -> WindowKey {
    if let Some(anchor) = spec.place.anchor() {
        let layer = match layer_hidden(anchor.output.as_deref(), anchor.layer) {
            true => LayerKind::Overlay,
            false => anchor.layer,
        };
        return WindowKey {
            output: anchor.output.clone(),
            layer,
        };
    }
    let wanted = spec.output.clone().or_else(focused_output);
    let known = |output: &Option<String>| keys.iter().any(|key| key.output == *output);
    let output = match known(&wanted) {
        true => wanted,
        false => keys
            .iter()
            .find(|key| key.layer == LayerKind::Overlay)
            .map(|key| key.output.clone())
            .unwrap_or(wanted),
    };
    WindowKey {
        output,
        layer: LayerKind::Overlay,
    }
}

pub fn hold(output: Option<&str>, layer: LayerKind) -> Option<Hold> {
    let key = WindowKey {
        output: output.map(str::to_string),
        layer,
    };
    REGISTRY.with(|registry| registry.borrow().holder.as_ref()?.hold(&key))
}

/// What the window of `layer` on `output` asks of the compositor, for whatever asks it for more without being built into it: an edit mode raising the window it edits, or taking the keyboard for it.
pub fn demands(output: Option<&str>, layer: LayerKind) -> Option<Rc<Demands>> {
    let key = WindowKey {
        output: output.map(str::to_string),
        layer,
    };
    REGISTRY.with(|registry| registry.borrow().holder.as_ref()?.demands(&key))
}

/// Sets the areas of the window of `layer` on `output` aside for as long as the token lives: an edit mode drawing them in the overlay window instead, where the compositor cannot raise that window (TA-4, R-6). `None` where that window is not open.
pub fn conceal(output: Option<&str>, layer: LayerKind) -> Option<Concealment> {
    let key = WindowKey {
        output: output.map(str::to_string),
        layer,
    };
    REGISTRY.with(|registry| registry.borrow().holder.as_ref()?.conceal(&key))
}

/// Waiting out `exit` before looking is what makes the answer independent of which of two listeners to the same source ran first: the one dropping the hold, or the one starting the exit.
pub fn release(hold: Hold, output: Option<&str>, layer: LayerKind, exit: std::time::Duration) {
    let window = WindowKey {
        output: output.map(str::to_string),
        layer,
    };
    let due = std::time::Instant::now() + exit;
    REGISTRY.with(|registry| {
        registry
            .borrow_mut()
            .draining
            .push((window.clone(), hold, due))
    });
    timeout(exit, move || settle(&window));
}

pub fn open(spec: Spec) {
    if matches!(spec.slot, Slot::Drawer | Slot::Standing) {
        close_drawer();
    }
    close(&spec.id);
    let (entry, publish) = REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let keys = registry
            .holder
            .as_ref()
            .map(Holder::keys)
            .unwrap_or_default();
        let window = route(&spec, &keys);
        let hold = registry
            .holder
            .as_ref()
            .and_then(|holder| holder.hold(&window));
        registry.serial += 1;
        let entry = Rc::new(Entry {
            serial: registry.serial,
            window: window.clone(),
            closing: Cell::new(false),
            shown: RefCell::new(None),
            builds: RefCell::new(None),
            stamp: RefCell::new(Stamp::default()),
            rebuilt: Cell::new(0),
            hold: RefCell::new(hold),
            claims: RefCell::new(None),
            spec,
        });
        registry.entries.push(Rc::clone(&entry));
        (entry, registry.publication(&window))
    });
    tracing::debug!(id = entry.id(), window = ?entry.window, "transient opened");
    deliver(publish);
}

pub fn toggle(spec: Spec) {
    if is_open(&spec.id) {
        close(&spec.id);
        return;
    }
    open(spec);
}

pub fn is_open(id: &str) -> bool {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .any(|entry| entry.id() == id && entry.is_open())
    })
}

/// Whether an open transient other than `except` takes the keyboard: what the shortcuts of the one that is `except` stand aside for, since the keys are that other one's while it is up.
pub fn takes_keyboard_besides(except: &str) -> bool {
    REGISTRY.with(|registry| {
        registry.borrow().entries.iter().any(|entry| {
            entry.is_open() && entry.id() != except && entry.spec.keyboard != KeyboardMode::None
        })
    })
}

/// The window the open transient `id` is drawn in: its anchor's, or an overlay window.
pub fn drawn_in(id: &str) -> Option<WindowKey> {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .find(|entry| entry.id() == id && entry.is_open())
            .map(|entry| entry.window.clone())
    })
}

pub fn drawer_is_open(id: &str) -> bool {
    drawer().as_deref() == Some(id)
}

fn drawer() -> Option<String> {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .find(|entry| entry.spec.slot == Slot::Drawer && entry.is_open())
            .map(|entry| entry.id().to_string())
    })
}

pub fn close_drawer() {
    if let Some(id) = drawer() {
        close(&id);
    }
}

pub fn open_ids() -> Vec<String> {
    let mut ids: Vec<String> = REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .filter(|entry| entry.is_open())
            .map(|entry| entry.id().to_string())
            .collect()
    });
    ids.sort();
    ids
}

pub fn close(id: &str) {
    let closing: Vec<Rc<Entry>> = REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .filter(|entry| entry.id() == id && entry.is_open())
            .cloned()
            .collect()
    });
    for entry in closing {
        entry.closing.set(true);
        let claims = entry.claims.borrow_mut().take();
        drop(claims);
        if let Some(on_close) = entry.spec.on_close.clone() {
            on_close();
        }
        let shown = *entry.shown.borrow();
        match shown.filter(RwSignal::is_alive) {
            Some(shown) => {
                shown.set(false);
                let window = entry.window.clone();
                timeout(std::time::Duration::ZERO, move || settle(&window));
            }
            None => forget(&entry),
        }
    }
}

pub fn close_all() {
    let entries = REGISTRY.with(|registry| std::mem::take(&mut registry.borrow_mut().entries));
    for entry in &entries {
        entry.closing.set(true);
        entry.hold.borrow_mut().take();
    }
    let publish = REGISTRY.with(|registry| registry.borrow().publish_all());
    deliver(publish);
}

/// Forgets every transient routed to a window that is gone — an output unplugged under it — so it neither keeps its slot nor holds a window nothing draws.
pub(crate) fn prune(live: &[WindowKey]) {
    let orphans: Vec<Rc<Entry>> = REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .filter(|entry| !live.contains(&entry.window))
            .cloned()
            .collect()
    });
    for entry in orphans {
        entry.closing.set(true);
        let claims = entry.claims.borrow_mut().take();
        drop(claims);
        forget(&entry);
    }
}

/// A stamp of content, not of authorship: the settings window must not be rebuilt by the save it just made, or the caret jumps back to the start of the field being typed into.
pub fn stamp(id: &str, content: Fingerprint) {
    REGISTRY.with(|registry| {
        for entry in registry
            .borrow()
            .entries
            .iter()
            .filter(|entry| entry.id() == id && entry.is_open())
        {
            entry.stamp.borrow_mut().record(content.clone());
        }
    });
}

pub fn rebuild_all(content: &Fingerprint, reload: Reload) {
    let entries: Vec<Rc<Entry>> = REGISTRY.with(|registry| registry.borrow().entries.clone());
    for entry in entries.iter().filter(|entry| entry.is_open()) {
        let needs = entry.stamp.borrow().needs(reload, content);
        entry.stamp.borrow_mut().record(content.clone());
        if !needs {
            continue;
        }
        entry.rebuilt.set(entry.rebuilt.get() + 1);
        let builds = *entry.builds.borrow();
        if let Some(builds) = builds.filter(RwSignal::is_alive) {
            builds.update(|n| *n = n.wrapping_add(1));
        }
    }
}

pub fn rebuilds(id: &str) -> Option<u64> {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .find(|entry| entry.id() == id && entry.is_open())
            .map(|entry| entry.rebuilt.get())
    })
}

fn settle(window: &WindowKey) {
    let exits = REGISTRY.with(|registry| {
        registry
            .borrow()
            .exits
            .iter()
            .find(|(key, _)| key == window)
            .map_or(0, |(_, exits)| *exits)
    });
    if exits > 0 {
        return;
    }
    let drained: Vec<Hold> = REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let (done, kept) = std::mem::take(&mut registry.draining)
            .into_iter()
            .partition(|(key, _, due)| key == window && *due <= std::time::Instant::now());
        registry.draining = kept;
        done.into_iter().map(|(_, hold, _)| hold).collect()
    });
    drop(drained);
    let closed: Vec<Rc<Entry>> = REGISTRY.with(|registry| {
        registry
            .borrow()
            .entries
            .iter()
            .filter(|entry| entry.window == *window && !entry.is_open())
            .cloned()
            .collect()
    });
    for entry in closed {
        forget(&entry);
    }
}

fn forget(entry: &Rc<Entry>) {
    let publish = REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        registry.entries.retain(|held| !Rc::ptr_eq(held, entry));
        registry.publication(&entry.window)
    });
    entry.hold.borrow_mut().take();
    deliver(publish);
}

type Publication = Vec<(RwSignal<Vec<Rc<Entry>>>, Vec<Rc<Entry>>)>;

impl Registry {
    fn routed_to(&self, window: &WindowKey) -> Vec<Rc<Entry>> {
        self.entries
            .iter()
            .filter(|entry| entry.window == *window)
            .cloned()
            .collect()
    }

    fn publication(&mut self, window: &WindowKey) -> Publication {
        self.sinks.retain(|(_, sink)| sink.is_alive());
        self.sinks
            .iter()
            .filter(|(key, _)| key == window)
            .map(|(_, sink)| (*sink, self.routed_to(window)))
            .collect()
    }

    fn publish_all(&self) -> Publication {
        self.sinks
            .iter()
            .filter(|(_, sink)| sink.is_alive())
            .map(|(key, sink)| (*sink, self.routed_to(key)))
            .collect()
    }
}

/// Writes outside the registry's borrow: a write runs effects, and an effect may open or close a transient.
fn deliver(publish: Publication) {
    for (sink, entries) in publish {
        sink.set(entries);
    }
}

#[derive(Clone)]
pub(crate) struct Frame {
    pub(crate) screen: ReadSignal<Screen>,
    pub(crate) demands: Rc<Demands>,
}

pub(crate) fn layer(key: WindowKey, frame: Frame) -> Built {
    let entries = signal(REGISTRY.with(|registry| registry.borrow().routed_to(&key)));
    REGISTRY.with(|registry| registry.borrow_mut().sinks.push((key.clone(), entries)));

    let exits = exits_in_flight();
    let watched = key.clone();
    telar::effect(move || {
        let in_flight = exits.get();
        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            match registry.exits.iter_mut().find(|(key, _)| *key == watched) {
                Some((_, held)) => *held = in_flight,
                None => registry.exits.push((watched.clone(), in_flight)),
            }
        });
        if in_flight == 0 {
            let window = watched.clone();
            timeout(std::time::Duration::ZERO, move || settle(&window));
        }
    });

    let list = ReactiveList::with_style(
        whole(),
        move || entries.get(),
        |entry: &Rc<Entry>| entry.serial,
        move |entry: Rc<Entry>| row(entry, frame.clone()),
    )?;
    Ok(Box::new(passthrough(whole(), vec![Box::new(list)])?))
}

/// A box that paints nothing and takes nothing from the pointer, so what is drawn under it still answers where nothing in it does. Every full-window box in this layer is one: dispatch stops at the first sibling that covers a point, wanted or not.
fn passthrough(
    style: LayoutStyle,
    children: Vec<Box<dyn LayoutItem>>,
) -> Result<StyledContainer, telar::LayoutError> {
    Ok(StyledContainer::new(style, |_| RectStyle::default(), children)?.input_transparent())
}

fn whole() -> LayoutStyle {
    LayoutStyle::new()
        .absolute()
        .inset_start(0.0)
        .inset_top(0.0)
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

fn row(entry: Rc<Entry>, frame: Frame) -> Built {
    let shown = signal(false);
    let builds = signal(0u64);
    *entry.shown.borrow_mut() = Some(shown);
    *entry.builds.borrow_mut() = Some(builds);

    let config = config::config_for(entry.window.output.as_deref());
    let transition = transition(entry.spec.motion, &config);
    let built = Rc::clone(&entry);
    let presence = telar::Presence::with_style(
        whole(),
        move || shown.get(),
        transition,
        move || placed(&built, &frame, builds),
    )?;
    if entry.is_open() {
        shown.set(true);
    }
    Ok(Box::new(presence))
}

fn transition(motion: Motion, config: &Config) -> Transition {
    let tween = config.animation.panel_tween();
    match motion {
        Motion::None => Transition::fade(telar::motion::tween(
            std::time::Duration::ZERO,
            telar::motion::Easing::Linear,
        )),
        Motion::Fade => Transition::fade(tween),
        Motion::Slide(edge) => Transition::slide(slide_from(edge), 24.0, tween),
    }
}

fn slide_from(edge: Edge) -> telar::Edge {
    match edge {
        Edge::Top => telar::Edge::Top,
        Edge::Bottom => telar::Edge::Bottom,
        Edge::Left => telar::Edge::Left,
        Edge::Right => telar::Edge::Right,
    }
}

fn placed(entry: &Rc<Entry>, frame: &Frame, builds: RwSignal<u64>) -> Built {
    let spec = &entry.spec;
    let id = spec.id.clone();
    *entry.claims.borrow_mut() = Some((
        frame.demands.keyboard(spec.keyboard),
        DismissRegistration::new(Rc::new(move || close(&id))),
    ));
    let claimed = Rc::clone(entry);
    on_cleanup(move || drop(claimed.claims.borrow_mut().take()));

    let mut children: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(2);
    if spec.dismiss_on_outside {
        let id = spec.id.clone();
        children.push(Box::new(
            StyledContainer::new(
                whole(),
                |_| RectStyle::filled(Color::TRANSPARENT, 0.0),
                Vec::new(),
            )?
            .input_opaque()
            .on_press(move || close(&id)),
        ));
    }
    let content = content(entry, builds, fill_of(&spec.place))?;
    children.push(position(&spec.place, content, frame.screen)?);
    Ok(Box::new(passthrough(whole(), children)?))
}

/// A whole-screen transient is given the screen to lay out in, so what it sizes as a share of its parent is a share of the screen; every other place sizes to what it holds.
fn fill_of(place: &Place) -> LayoutStyle {
    match place {
        Place::Whole => whole(),
        _ => LayoutStyle::new(),
    }
}

fn content(entry: &Rc<Entry>, builds: RwSignal<u64>, style: LayoutStyle) -> Built {
    let built = Rc::clone(entry);
    let list = ReactiveList::with_style(
        style,
        move || vec![builds.get()],
        |build: &u64| *build,
        move |_| {
            let chrome = chrome_of(&built);
            chrome.provide();
            (built.spec.content)(&chrome)
        },
    )?;
    Ok(Box::new(list))
}

fn chrome_of(entry: &Entry) -> Chrome {
    let output = entry.window.output.clone();
    let config = config::config_for(output.as_deref());
    match entry.spec.place.anchor() {
        Some(anchor) => Chrome::new(config, anchor.chrome.shape, output),
        None => Chrome::global(config, output),
    }
}

fn position(place: &Place, content: Box<dyn LayoutItem>, screen: ReadSignal<Screen>) -> Built {
    let usable = move || {
        let screen = screen.get();
        screen.reserved.box_of(layout::Within::Usable, screen.size)
    };
    match place.clone() {
        Place::Beside(anchor) => {
            let style = LayoutStyle::new()
                .absolute()
                .inset_start(0.0)
                .inset_top(0.0);
            Ok(Box::new(
                StyledContainer::new(style, |_| RectStyle::default(), vec![content])?
                    .with_transform(move |laid| {
                        let (x, y) = beside(&anchor, (laid.width, laid.height), usable());
                        Some([1.0, 0.0, 0.0, 1.0, x - laid.x, y - laid.y])
                    }),
            ))
        }
        Place::Pinned {
            within,
            anchor,
            offset,
        } => {
            let bounds = move || {
                let screen = screen.get();
                screen.reserved.box_of(within, screen.size)
            };
            Ok(Box::new(
                StyledContainer::new(
                    LayoutStyle::new()
                        .absolute()
                        .inset_start(0.0)
                        .inset_top(0.0),
                    |_| RectStyle::default(),
                    vec![content],
                )?
                .with_transform(move |laid| {
                    let at =
                        crate::pinned::pinned(bounds(), anchor, (laid.width, laid.height), offset);
                    Some([1.0, 0.0, 0.0, 1.0, at.x - laid.x, at.y - laid.y])
                }),
            ))
        }
        Place::Centred => Ok(Box::new(
            passthrough(LayoutStyle::new(), vec![content])?.styled_by(move || {
                at(usable())
                    .flex_column()
                    .justify_content(telar::JustifyContent::CENTER)
                    .align_items(telar::AlignItems::CENTER)
            }),
        )),
        Place::Docked { edge, thickness } => Ok(Box::new(
            passthrough(
                LayoutStyle::new(),
                vec![box_item(Container::new(
                    LayoutStyle::new()
                        .width(SizeDimension::Percent(1.0))
                        .height(SizeDimension::Percent(1.0)),
                    vec![content],
                )?)],
            )?
            .styled_by(move || {
                at(docked(edge, thickness, usable(), DEFAULT_GAP))
                    .flex_column()
                    .align_items(telar::AlignItems::STRETCH)
            }),
        )),
        Place::Over(_) | Place::Whole => Ok(Box::new(passthrough(whole(), vec![content])?)),
    }
}

fn at(rect: Rect) -> LayoutStyle {
    LayoutStyle::new()
        .absolute()
        .inset_start(rect.x)
        .inset_top(rect.y)
        .width(rect.width)
        .height(rect.height)
}

/// A vertical anchor lines up by its top rather than its centre because a transient beside a vertical bar is as tall as its content, and what the eye follows from a chip in a column is the row it is on.
pub fn beside(anchor: &Anchor, panel: (f32, f32), usable: Rect) -> (f32, f32) {
    beside_chip(anchor.edge, anchor.gap, anchor.rect, panel, usable)
}

pub(crate) fn kept_inside(at: f32, start: f32, length: f32, extent: f32, gap: f32) -> f32 {
    let far = (start + length - extent - gap).max(start + gap);
    at.clamp(start + gap, far)
}

pub(crate) fn beside_chip(
    edge: Edge,
    gap: f32,
    chip: Rect,
    panel: (f32, f32),
    usable: Rect,
) -> (f32, f32) {
    let (width, height) = panel;
    let across = || {
        kept_inside(
            chip.x + chip.width / 2.0 - width / 2.0,
            usable.x,
            usable.width,
            width,
            gap,
        )
    };
    let down = || kept_inside(chip.y, usable.y, usable.height, height, gap);
    match edge {
        Edge::Top => (across(), chip.y + chip.height + gap),
        Edge::Bottom => (across(), chip.y - gap - height),
        Edge::Left => (chip.x + chip.width + gap, down()),
        Edge::Right => (chip.x - gap - width, down()),
    }
}

pub fn docked(edge: Edge, thickness: f32, usable: Rect, gap: f32) -> Rect {
    let inner = Rect::new(
        usable.x + gap,
        usable.y + gap,
        (usable.width - 2.0 * gap).max(0.0),
        (usable.height - 2.0 * gap).max(0.0),
    );
    let thickness = match edge.is_vertical() {
        true => thickness.min(inner.width),
        false => thickness.min(inner.height),
    };
    match edge {
        Edge::Top => Rect::new(inner.x, inner.y, inner.width, thickness),
        Edge::Bottom => Rect::new(
            inner.x,
            inner.y + inner.height - thickness,
            inner.width,
            thickness,
        ),
        Edge::Left => Rect::new(inner.x, inner.y, thickness, inner.height),
        Edge::Right => Rect::new(
            inner.x + inner.width - thickness,
            inner.y,
            thickness,
            inner.height,
        ),
    }
}

pub fn standoff(config: &Config, shape: &ResolvedShape) -> f32 {
    match config.shape.frame || shape.gap == 0 {
        true => DEFAULT_GAP,
        false => shape.gap as f32,
    }
}

pub mod chips {
    use super::*;

    #[derive(Clone)]
    pub struct Site {
        pub output: Option<String>,
        pub layer: LayerKind,
        pub edge: Edge,
        pub chrome: Chrome,
        pub gap: f32,
    }

    impl Site {
        /// Where a chip built under `host` is, read while it is being built: the window it is in is the one being built.
        pub fn of_host(host: &ui::host::Host) -> Option<Self> {
            let edge = host.axis?;
            let config = host.config();
            Some(Self {
                output: host.output.clone(),
                layer: crate::layer_window::LayerWindowContext::current()
                    .map_or(LayerKind::Top, |window| window.layer),
                edge,
                chrome: Chrome::new(
                    std::sync::Arc::clone(config),
                    host.shape,
                    host.output.clone(),
                ),
                gap: standoff(config, &host.shape),
            })
        }

        pub fn anchor(&self, rect: Rect) -> Anchor {
            Anchor {
                output: self.output.clone(),
                layer: self.layer,
                edge: self.edge,
                rect,
                chrome: self.chrome.clone(),
                gap: self.gap,
            }
        }
    }

    /// A chip of the module asked for: where to hang what it opens, and the instance that opens it.
    pub struct Found {
        pub anchor: Anchor,
        pub instance: ui::host::Instance,
    }

    /// The pressed chip of `module`, else its first chip on `focused`, else its first anywhere — TA-2's first-instance rule, with a press naming its own instance. By module rather than by instance, because `panel toggle <module>` and a keybind name a module (F-1.2); it reads the chips [`crate::rects`] holds.
    pub fn find(
        module: &str,
        pressed: Option<ui::module::Pressed>,
        focused: Option<&str>,
    ) -> Option<Found> {
        let of_module: Vec<(crate::rects::Chip, Rect)> = crate::rects::chips()
            .into_iter()
            .filter(|(chip, _)| &*chip.instance.module == module)
            .collect();
        let on = |output: Option<&str>| -> Vec<&(crate::rects::Chip, Rect)> {
            of_module
                .iter()
                .filter(|(chip, _)| chip.site.output.as_deref() == output)
                .collect()
        };
        let chosen = match &pressed {
            Some(pressed) => {
                let there = on(pressed.output.as_deref());
                there
                    .iter()
                    .copied()
                    .find(|(_, rect)| *rect == pressed.rect)
                    .or_else(|| there.first().copied())
            }
            None => on(focused).first().copied(),
        }
        .or_else(|| of_module.first())?;
        let (chip, at) = chosen;
        let rect = pressed
            .filter(|pressed| pressed.output == chip.site.output)
            .map_or(*at, |pressed| pressed.rect);
        Some(Found {
            anchor: chip.site.anchor(rect),
            instance: chip.instance.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn chrome() -> Chrome {
        Chrome::global(Arc::new(Config::default()), None)
    }

    fn anchor(edge: Edge, rect: Rect) -> Anchor {
        Anchor {
            output: None,
            layer: LayerKind::Top,
            edge,
            rect,
            chrome: chrome(),
            gap: 8.0,
        }
    }

    fn content() -> Content {
        Rc::new(|_: &Chrome| Ok(Box::new(Container::new(LayoutStyle::new(), vec![])?) as _))
    }

    fn spec(id: &str, slot: Slot) -> Spec {
        Spec::new(id, Place::Centred, content()).slot(slot)
    }

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 34.0,
        width: 1920.0,
        height: 1046.0,
    };

    #[test]
    fn a_transient_hangs_off_its_chip_and_stays_on_screen_on_every_edge() {
        let panel = (300.0, 200.0);
        for edge in Edge::ALL {
            for along in [0.0f32, 500.0, 5000.0] {
                let chip = match edge {
                    Edge::Left => Rect::new(0.0, along.min(1040.0), 34.0, 30.0),
                    Edge::Right => Rect::new(1886.0, along.min(1040.0), 34.0, 30.0),
                    Edge::Top => Rect::new(along.min(1890.0), 0.0, 30.0, 34.0),
                    Edge::Bottom => Rect::new(along.min(1890.0), 1046.0, 30.0, 34.0),
                };
                let (x, y) = beside(&anchor(edge, chip), panel, SCREEN);
                if edge.is_vertical() {
                    assert!(
                        y >= SCREEN.y + 8.0 && y + panel.1 <= SCREEN.y + SCREEN.height - 8.0,
                        "{edge:?} at {along}: y {y} runs past the usable area"
                    );
                } else {
                    assert!(
                        x >= SCREEN.x + 8.0 && x + panel.0 <= SCREEN.x + SCREEN.width - 8.0,
                        "{edge:?} at {along}: x {x} runs past the usable area"
                    );
                }
            }
        }
    }

    #[test]
    fn a_transient_stands_off_its_chip_by_the_gap_on_the_side_its_bar_faces() {
        let chip = Rect::new(500.0, 0.0, 30.0, 34.0);
        assert_eq!(
            beside(&anchor(Edge::Top, chip), (300.0, 200.0), SCREEN).1,
            42.0
        );
        let chip = Rect::new(500.0, 1046.0, 30.0, 34.0);
        assert_eq!(
            beside(&anchor(Edge::Bottom, chip), (300.0, 200.0), SCREEN).1,
            1046.0 - 8.0 - 200.0
        );
        let chip = Rect::new(0.0, 300.0, 34.0, 30.0);
        assert_eq!(
            beside(&anchor(Edge::Left, chip), (300.0, 200.0), SCREEN),
            (42.0, 300.0)
        );
        let chip = Rect::new(1886.0, 300.0, 34.0, 30.0);
        assert_eq!(
            beside(&anchor(Edge::Right, chip), (300.0, 200.0), SCREEN).0,
            1886.0 - 8.0 - 300.0
        );
    }

    #[test]
    fn a_docked_transient_runs_the_length_of_its_edge_inside_the_usable_area() {
        let rect = docked(Edge::Right, 400.0, SCREEN, 8.0);
        assert_eq!(
            rect,
            Rect::new(1920.0 - 8.0 - 400.0, 42.0, 400.0, 1046.0 - 16.0)
        );
        let rect = docked(Edge::Bottom, 5000.0, SCREEN, 8.0);
        assert_eq!(rect.height, 1046.0 - 16.0, "never thicker than the screen");
    }

    /// DEC-9: a transient laid over its anchor's whole window is drawn in that window, as one beside it is; only an unanchored one goes to the overlay window.
    #[test]
    fn a_transient_over_its_anchor_is_drawn_in_the_anchors_window() {
        close_all();
        let over = anchor(Edge::Top, Rect::new(0.0, 0.0, 1920.0, 34.0));
        open(Spec::new("over", Place::Over(over), content()).output(Some("DP-1".into())));
        open(Spec::new("whole", Place::Whole, content()).output(Some("DP-1".into())));
        assert_eq!(
            drawn_in("over"),
            Some(WindowKey {
                output: None,
                layer: LayerKind::Top
            })
        );
        assert_eq!(
            drawn_in("whole").map(|window| window.layer),
            Some(LayerKind::Overlay)
        );
        close_all();
        assert_eq!(drawn_in("over"), None);
    }

    #[test]
    fn a_standing_window_takes_the_screen_from_the_drawer_and_from_nothing_else() {
        close_all();
        open(spec("network", Slot::Drawer));
        open(spec("mixer", Slot::Standing));
        open(spec("popout", Slot::Free));
        open(spec("launcher", Slot::Standing));
        assert_eq!(open_ids(), vec!["launcher", "mixer", "popout"]);
        close_all();
    }

    #[test]
    fn toggling_an_open_transient_closes_it_and_only_it() {
        close_all();
        open(spec("sidebar", Slot::Standing));
        open(spec("settings", Slot::Standing));
        toggle(spec("settings", Slot::Standing));
        assert_eq!(open_ids(), vec!["sidebar"]);
        close("settings");
        assert_eq!(open_ids(), vec!["sidebar"], "closing twice is a no-op");
        close_all();
        assert!(open_ids().is_empty());
    }

    #[test]
    fn a_press_on_one_of_two_identical_chips_opens_on_its_own_screen() {
        telar::reset_runtime();
        let _scope = telar::owner_scope();
        let at = Rect::new(500.0, 0.0, 30.0, 34.0);
        for output in ["DP-1", "HDMI-A-1"] {
            let site = chips::Site {
                output: Some(output.to_string()),
                layer: LayerKind::Top,
                edge: Edge::Top,
                chrome: chrome(),
                gap: 8.0,
            };
            let options = toml::Table::from_iter([(
                "drawer_width".to_string(),
                toml::Value::Integer(if output == "DP-1" { 300 } else { 500 }),
            )]);
            crate::rects::track_chip(
                crate::rects::Node::area(Some(output), LayerKind::Top, &layout::AreaId::new("bar"))
                    .instance(
                        &layout::GroupId::new("end"),
                        &layout::InstanceId::new("clock"),
                    ),
                signal(at),
                crate::rects::Chip {
                    instance: ui::host::Instance::new(
                        ui::host::InstanceId::of_module("clock"),
                        "clock",
                        options,
                    ),
                    site,
                },
            );
        }
        let pressed = ui::module::Pressed {
            rect: at,
            output: Some("HDMI-A-1".into()),
        };
        let found = chips::find("clock", Some(pressed), Some("DP-1")).expect("a clock chip");
        assert_eq!(found.anchor.output.as_deref(), Some("HDMI-A-1"));
        assert_eq!(
            found.instance.options.get("drawer_width"),
            Some(&toml::Value::Integer(500)),
            "and the options of the instance that was pressed"
        );
        let unpressed = chips::find("clock", None, Some("HDMI-A-1")).expect("a clock chip");
        assert_eq!(
            unpressed.anchor.output.as_deref(),
            Some("HDMI-A-1"),
            "with no press, the focused screen's chip"
        );
    }

    #[test]
    fn one_drawer_at_a_time() {
        close_all();
        open(spec("network", Slot::Drawer));
        open(spec("clock", Slot::Drawer));
        assert_eq!(open_ids(), vec!["clock"]);
        assert!(drawer_is_open("clock"));
        close_all();
    }

    #[test]
    fn closing_runs_the_openers_goodbye_once() {
        close_all();
        let said = Rc::new(Cell::new(0));
        let counter = Rc::clone(&said);
        open(spec("notes", Slot::Drawer).on_close(move || counter.set(counter.get() + 1)));
        close("notes");
        close("notes");
        assert_eq!(said.get(), 1);
        close_all();
    }
}
