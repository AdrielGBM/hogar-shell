//! The synthetic scene both modes draw, built from telar's primitives.
//!
//! Two bars of forty chips between them, a 1 Hz clock inside a rounded chip, a chip whose rounded clip widens and narrows with its label, a stack notification cards arrive in at the top, and a drawer that slides and fades. The shell's own modules are deliberately absent: they bring services, config and a surface environment that would each add frames and memory of their own, and the question is what the *window model* costs. The `ui` crate's chip and card helpers are skipped for the same reason — they resolve their radii and spacing through the surface environment a real bar installs — so this builds straight from the primitives those helpers lower to: `StyledContainer` boxes painted with `RectStyle::filled`, `Text`, a rounded `ClippedItem`, a keyed `ReactiveList`.
//!
//! Every builder produces the same subtree whichever mode mounts it. What differs is only what surrounds it: its own layer surface in the per-surface model (today), or an absolutely placed box inside one fullscreen Top window. That is what makes the two runs comparable.
//!
//! The builders publish the layout rects of whatever the director is going to change — the clock, the widening chip, each card — so that when it changes something it can read, from the layout itself, which rects that change should repaint.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use telar::motion::{Animated, Easing, tween};
use telar::{
    AlignItems, App, Border, BorderRadius, Clip, ClippedItem, Color, Component, Container, Event,
    EventResult, JustifyContent, LayoutError, LayoutItem, LayoutStyle, ReactiveList, Rect,
    RectStyle, RenderNode, RwSignal, SizeDimension, StyledContainer, Text, TextStyle, Transform,
    WindowConfig, WindowRoot, box_item, signal, track_layout,
};

use crate::timeline::LogicalRect;

pub const BAR_HEIGHT: f32 = 36.0;
pub const GAP: f32 = 8.0;
pub const STACK_WIDTH: f32 = 380.0;
pub const STACK_HEIGHT: f32 = 480.0;
pub const DRAWER_WIDTH: f32 = 420.0;
pub const DRAWER_HEIGHT: f32 = 600.0;
/// How far the drawer travels while it fades — telar's own surface transition distance, so the drawer moves exactly as today's drawer surfaces do.
const DRAWER_SLIDE: f32 = 24.0;
const DRAWER_MOTION: Duration = Duration::from_millis(200);
const CLOCK_WIDTH: f32 = 104.0;
const CHIP_RADIUS: f32 = 8.0;
const PILL_RADIUS: f32 = 14.0;

pub const SHORT_LABEL: &str = "wifi";
pub const LONG_LABEL: &str = "wifi · connected to a rather long network name";

const BAR: Color = Color::rgba(0.180, 0.204, 0.251, 0.94);
const CHIP: Color = Color::rgb(0.231, 0.259, 0.322);
const TEXT: Color = Color::rgb(0.925, 0.937, 0.957);
const MUTED: Color = Color::rgb(0.847, 0.871, 0.914);
const ACCENT: Color = Color::rgb(0.533, 0.753, 0.816);
const ON_ACCENT: Color = Color::rgb(0.180, 0.204, 0.251);
const WIDE: Color = Color::rgb(0.369, 0.506, 0.675);
const CARD: Color = Color::rgb(0.263, 0.298, 0.369);
const PANEL: Color = Color::rgba(0.231, 0.259, 0.322, 0.97);
const ROW: Color = Color::rgb(0.263, 0.298, 0.369);
const BAR_TARGET: Color = Color::rgb(0.922, 0.796, 0.545);
const EMPTY_TARGET: Color = Color::rgb(0.639, 0.745, 0.549);
const CATCHER: Color = Color::rgba(0.180, 0.204, 0.251, 0.35);

/// Which surface a tree is mounted on. Its namespace is how the report tells the surfaces apart in the protocol log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The one fullscreen Top window of the merged model, holding everything.
    Top,
    BarTop,
    BarBottom,
    Stack,
    Drawer,
    /// A fullscreen Bottom-layer surface that counts the clicks the Top window lets through.
    Catcher,
}

pub const NAMESPACE_PREFIX: &str = "hogar-shell-spike-";

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Top => "top",
            Role::BarTop => "bar-top",
            Role::BarBottom => "bar-bottom",
            Role::Stack => "stack",
            Role::Drawer => "drawer",
            Role::Catcher => "catcher",
        }
    }

    /// Prefixed `hogar-shell` on purpose: a compositor rule written for the shell's layers (a blur, an animation) applies to these too, so a live session treats them as it would the shell's own.
    pub fn namespace(self) -> String {
        format!("{NAMESPACE_PREFIX}{}", self.name())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetClass {
    /// On a bar's background: the Top window must take the click.
    Bar,
    /// Over empty space: the click must reach whatever is below.
    Empty,
}

impl TargetClass {
    pub fn name(self) -> &'static str {
        match self {
            TargetClass::Bar => "bar",
            TargetClass::Empty => "empty",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub class: TargetClass,
    pub id: String,
    pub rect: LogicalRect,
}

/// What the director snapshots around a change, to work out what it should have repainted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    Clock,
    Wide,
    Cards,
}

#[derive(Default)]
struct Published {
    clock: Option<RwSignal<Rect>>,
    wide: Option<RwSignal<Rect>>,
    spacers: Vec<RwSignal<Rect>>,
    window: Option<RwSignal<Rect>>,
    cards: HashMap<u64, RwSignal<Rect>>,
}

type PressHook = Box<dyn Fn(Role, f64, f64)>;

/// The scene's state, shared by every surface on the driver thread. The signals are created outside any surface, so one change reaches whichever surface draws it — a bar's own surface in one mode, the one fullscreen window in the other.
pub struct Scene {
    pub clock: RwSignal<String>,
    pub wide: RwSignal<String>,
    pub cards: RwSignal<Vec<Card>>,
    /// Holds one entry while the merged window's drawer is mounted, none while it is not — the in-tree counterpart of opening and closing a drawer surface.
    pub drawer_slot: RwSignal<Vec<u8>>,
    pub drawer: Animated<f32>,
    pub targets: RwSignal<Vec<Target>>,
    pub status: RwSignal<String>,
    published: RefCell<Published>,
    on_press: RefCell<Option<PressHook>>,
}

impl Scene {
    pub fn new(clock: String) -> Rc<Self> {
        Rc::new(telar::detached(|| Scene {
            clock: signal(clock),
            wide: signal(SHORT_LABEL.to_owned()),
            cards: signal(Vec::new()),
            drawer_slot: signal(Vec::new()),
            drawer: Animated::new(0.0, tween(DRAWER_MOTION, Easing::EaseOut)),
            targets: signal(Vec::new()),
            status: signal(String::new()),
            published: RefCell::default(),
            on_press: RefCell::default(),
        }))
    }

    pub fn on_press(&self, hook: impl Fn(Role, f64, f64) + 'static) {
        *self.on_press.borrow_mut() = Some(Box::new(hook));
    }

    fn pressed(&self, role: Role, x: f64, y: f64) {
        if let Some(hook) = self.on_press.borrow().as_ref() {
            hook(role, x, y);
        }
    }

    pub fn forget_cards(&self) {
        self.published.borrow_mut().cards.clear();
    }

    /// The laid-out rects `watch` covers, keyed so a before and an after can be matched up. Rects of nodes that no longer exist are left out.
    pub fn snapshot(&self, watch: Watch) -> Vec<(u64, LogicalRect)> {
        let published = self.published.borrow();
        let live = |signal: &RwSignal<Rect>| signal.is_alive().then(|| logical(signal.peek()));
        match watch {
            Watch::Clock => published
                .clock
                .as_ref()
                .and_then(live)
                .map(|r| (0, r))
                .into_iter()
                .collect(),
            Watch::Wide => published
                .wide
                .as_ref()
                .and_then(live)
                .map(|r| (0, r))
                .into_iter()
                .collect(),
            Watch::Cards => {
                let mut cards: Vec<(u64, LogicalRect)> = published
                    .cards
                    .iter()
                    .filter_map(|(id, signal)| live(signal).map(|r| (*id, r)))
                    .collect();
                cards.sort_by_key(|(id, _)| *id);
                cards
            }
        }
    }

    /// Where the user is asked to click: the middle of every stretch of bar with nothing on it, and four spots of open desktop clear of the bars, the stack and the drawer. Only meaningful in the merged window, whose coordinates are the output's.
    pub fn click_targets(&self) -> Vec<Target> {
        let published = self.published.borrow();
        let mut targets = Vec::new();
        for spacer in published.spacers.iter().filter(|s| s.is_alive()) {
            let r = spacer.peek();
            if r.width < 48.0 {
                continue;
            }
            let w = (r.width - 16.0).min(96.0);
            targets.push(Target {
                class: TargetClass::Bar,
                id: format!("B{}", targets.len() + 1),
                rect: LogicalRect::new(
                    f64::from(r.x + (r.width - w) / 2.0),
                    f64::from(r.y + 2.0),
                    f64::from(w),
                    f64::from(r.height - 4.0),
                ),
            });
        }
        let Some(window) = published.window.filter(|w| w.is_alive()).map(|w| w.peek()) else {
            return targets;
        };
        let (w, h) = (120.0, 72.0);
        for (i, (fx, fy)) in [(0.40, 0.45), (0.60, 0.45), (0.40, 0.70), (0.60, 0.70)]
            .into_iter()
            .enumerate()
        {
            targets.push(Target {
                class: TargetClass::Empty,
                id: format!("E{}", i + 1),
                rect: LogicalRect::new(
                    f64::from(window.width) * fx - w / 2.0,
                    f64::from(window.height) * fy - h / 2.0,
                    w,
                    h,
                ),
            });
        }
        targets
    }
}

fn logical(r: Rect) -> LogicalRect {
    LogicalRect::new(
        f64::from(r.x),
        f64::from(r.y),
        f64::from(r.width),
        f64::from(r.height),
    )
}

type Built = Result<Box<dyn LayoutItem>, LayoutError>;

fn label(text: impl Fn() -> String + 'static, size: f32, color: Color) -> Built {
    Ok(box_item(Text::new(text, LayoutStyle::new(), move || {
        TextStyle::new(size, color)
    })?))
}

fn chip(index: usize) -> Built {
    let text = format!("c{index:02}");
    Ok(box_item(StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .padding_horizontal(8.0)
            .flex_shrink(0.0),
        |_| RectStyle::filled(CHIP, CHIP_RADIUS),
        vec![label(move || text.clone(), 13.0, TEXT)?],
    )?))
}

fn zone(children: Vec<Box<dyn LayoutItem>>) -> Built {
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(4.0)
            .align_items(AlignItems::STRETCH)
            .height(SizeDimension::Percent(1.0))
            .flex_shrink(0.0),
        children,
    )?))
}

/// The empty stretch of bar between zones. It paints nothing — the bar's own background shows through — which is what makes it where a "bar background" click target goes.
fn spacer(scene: &Scene) -> Built {
    let spacer = Container::new(
        LayoutStyle::new()
            .flex_grow(1.0)
            .height(SizeDimension::Percent(1.0)),
        Vec::new(),
    )?;
    if let Some(rect) = track_layout(spacer.layout_node()) {
        scene.published.borrow_mut().spacers.push(rect);
    }
    Ok(box_item(spacer))
}

fn clock_chip(scene: &Scene) -> Built {
    let clock = scene.clock;
    // A fixed width, so a tick repaints the digits and never re-lays-out the bar around them: the clip that resizes with its text is the widening chip's job, measured on its own.
    let chip = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER)
            .width(CLOCK_WIDTH)
            .flex_shrink(0.0),
        |_| RectStyle::filled(ACCENT, PILL_RADIUS),
        vec![label(move || clock.get(), 13.0, ON_ACCENT)?],
    )?;
    scene.published.borrow_mut().clock = track_layout(chip.layout_node());
    Ok(box_item(chip))
}

fn wide_chip(scene: &Scene) -> Built {
    let wide = scene.wide;
    let body = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .padding_horizontal(12.0)
            .flex_shrink(0.0),
        |_| RectStyle::filled(WIDE, PILL_RADIUS),
        vec![label(move || wide.get(), 13.0, TEXT)?],
    )?;
    scene.published.borrow_mut().wide = track_layout(body.layout_node());
    Ok(box_item(ClippedItem::new(
        box_item(body),
        Clip::both().rounded(PILL_RADIUS),
    )))
}

/// A bar: twenty chips, and on the top bar the clock between two stretches of empty bar, on the bottom one the widening chip ending its start zone so it grows into empty bar and moves nothing.
///
/// Its background is `input_opaque`: in the merged window that is what claims the whole bar for the input region, chips and gaps alike, while the space around it stays click-through.
pub fn bar(scene: &Scene, edge: Edge, outer: LayoutStyle) -> Built {
    let chips = |from: usize, count: usize| {
        (from..from + count)
            .map(chip)
            .collect::<Result<Vec<_>, _>>()
    };
    let items = match edge {
        Edge::Top => vec![
            zone(chips(1, 10)?)?,
            spacer(scene)?,
            clock_chip(scene)?,
            spacer(scene)?,
            zone(chips(11, 10)?)?,
        ],
        Edge::Bottom => {
            let mut start = chips(21, 9)?;
            start.push(wide_chip(scene)?);
            vec![zone(start)?, spacer(scene)?, zone(chips(30, 11)?)?]
        }
    };
    let bar = StyledContainer::new(
        outer
            .flex_row()
            .align_items(AlignItems::STRETCH)
            .padding_horizontal(GAP)
            .padding_vertical(4.0)
            .gap(6.0),
        |_| RectStyle::filled(BAR, 0.0),
        items,
    )?
    .input_opaque();
    Ok(box_item(bar))
}

fn card(scene: &Scene, card: Card) -> Built {
    let id = card.id;
    let item = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .gap(4.0)
            .padding_all(12.0)
            .width(SizeDimension::Percent(1.0))
            .flex_shrink(0.0),
        |_| RectStyle::filled(CARD, 10.0),
        vec![
            label(move || format!("Notification {id}"), 14.0, TEXT)?,
            label(
                || "A synthetic body line for the stack".to_owned(),
                12.0,
                MUTED,
            )?,
        ],
    )?
    .input_opaque();
    if let Some(rect) = track_layout(item.layout_node()) {
        scene.published.borrow_mut().cards.insert(id, rect);
    }
    Ok(box_item(item))
}

/// The notification column. Keyed by card id, so an arrival at the top is one new node and every card below it keeps its own and moves — the insertion the damage check is about.
pub fn stack(scene: &Rc<Scene>, outer: LayoutStyle) -> Built {
    let cards = scene.cards;
    let builder = Rc::clone(scene);
    Ok(box_item(ReactiveList::with_style(
        outer.flex_column().gap(GAP),
        move || cards.get(),
        |c: &Card| c.id,
        move |c| card(&builder, c),
    )?))
}

fn drawer_row(index: usize) -> Built {
    Ok(box_item(StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .padding_horizontal(12.0)
            .height(40.0)
            .flex_shrink(0.0),
        |_| RectStyle::filled(ROW, CHIP_RADIUS),
        vec![
            label(move || format!("Drawer row {index}"), 13.0, TEXT)?,
            label(move || format!("{}%", index * 9), 12.0, MUTED)?,
        ],
    )?))
}

/// The drawer panel, sliding in from the left edge while it fades, driven by one shared progress value exactly as telar's `SurfaceTransition` drives a drawer surface today.
pub fn drawer(scene: &Scene, outer: LayoutStyle) -> Built {
    let rows = (1..=10).map(drawer_row).collect::<Result<Vec<_>, _>>()?;
    let progress = scene.drawer;
    let panel = StyledContainer::new(
        outer.flex_column().gap(GAP).padding_all(16.0),
        |_| RectStyle::filled(PANEL, 12.0),
        rows,
    )?
    .with_opacity(move || progress.get())
    .with_transform(move |_| {
        Some(Transform::translate(-DRAWER_SLIDE * (1.0 - progress.get()), 0.0).to_array())
    })
    .input_opaque();
    Ok(box_item(panel))
}

fn target_marker(target: Target) -> Built {
    let color = match target.class {
        TargetClass::Bar => BAR_TARGET,
        TargetClass::Empty => EMPTY_TARGET,
    };
    let r = target.rect;
    let id = target.id;
    // No handler of any kind: a marker must never claim input itself, or it would answer the very question it is there to ask.
    Ok(box_item(StyledContainer::new(
        LayoutStyle::new()
            .absolute()
            .inset_top(r.y as f32)
            .inset_start(r.x as f32)
            .width(r.w as f32)
            .height(r.h as f32)
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        move |_| RectStyle {
            border: Some(Border::uniform(color, 2.0)),
            radius: BorderRadius::all(6.0),
            ..RectStyle::default()
        },
        vec![label(move || id.clone(), 12.0, color)?],
    )?))
}

/// The merged model's single fullscreen tree: both bars, the stack and the drawer placed where their surfaces sit today, plus the click targets and the tally shown only while the user is asked to click.
fn merged(scene: &Rc<Scene>) -> Built {
    let fill = || LayoutStyle::new().absolute();
    let top = bar(
        scene,
        Edge::Top,
        fill()
            .inset_top(0.0)
            .inset_start(0.0)
            .width(SizeDimension::Percent(1.0))
            .height(BAR_HEIGHT),
    )?;
    let bottom = bar(
        scene,
        Edge::Bottom,
        fill()
            .inset_bottom(0.0)
            .inset_start(0.0)
            .width(SizeDimension::Percent(1.0))
            .height(BAR_HEIGHT),
    )?;
    let stack = stack(
        scene,
        fill()
            .inset_top(BAR_HEIGHT + GAP)
            .inset_end(GAP)
            .width(STACK_WIDTH),
    )?;
    let slot = scene.drawer_slot;
    let builder = Rc::clone(scene);
    let drawer = box_item(ReactiveList::with_style(
        fill()
            .inset_top(BAR_HEIGHT + GAP)
            .inset_start(GAP)
            .width(DRAWER_WIDTH)
            .height(DRAWER_HEIGHT),
        move || slot.get(),
        |k: &u8| *k,
        move |_| drawer(&builder, whole()),
    )?);
    let targets = scene.targets;
    let markers = box_item(ReactiveList::with_style(
        LayoutStyle::new().absolute_fill(),
        move || targets.get(),
        |t: &Target| t.id.clone(),
        target_marker,
    )?);
    let status = scene.status;
    let tally = box_item(Container::new(
        fill()
            .inset_top(BAR_HEIGHT + 3.0 * GAP)
            .inset_start(0.0)
            .width(SizeDimension::Percent(1.0))
            .flex_row()
            .justify_content(JustifyContent::CENTER),
        vec![label(move || status.get(), 16.0, TEXT)?],
    )?);
    let root = Container::new(whole(), vec![top, bottom, stack, drawer, markers, tally])?;
    scene.published.borrow_mut().window = track_layout(root.layout_node());
    Ok(box_item(root))
}

fn whole() -> LayoutStyle {
    LayoutStyle::new()
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
}

fn catcher() -> Built {
    Ok(box_item(StyledContainer::new(
        whole()
            .flex_column()
            .justify_content(JustifyContent::END)
            .padding_all(24.0),
        |_| RectStyle::filled(CATCHER, 0.0),
        vec![label(
            || {
                "hogar-shell-spike catcher (Bottom layer) — a press that lands here went through the Top window".to_owned()
            },
            14.0,
            TEXT,
        )?],
    )?))
}

/// Counts every press a surface's tree receives before handing the event on, whether or not anything in the tree answers it: the question is where the compositor sent the click, not what the scene did with it.
struct Probe {
    inner: Box<dyn Component>,
    role: Role,
    scene: Rc<Scene>,
}

impl Component for Probe {
    fn view(&self) -> RenderNode {
        self.inner.view()
    }

    fn on_event(&mut self, event: &Event) -> EventResult {
        if let Event::PointerPressed { x, y, .. } = event {
            self.scene.pressed(self.role, *x, *y);
        }
        self.inner.on_event(event)
    }

    fn debug_name(&self) -> &'static str {
        "SpikeProbe"
    }
}

/// One surface of the scene.
pub struct SurfaceApp {
    pub scene: Rc<Scene>,
    pub role: Role,
}

impl SurfaceApp {
    fn content(&self) -> Result<Box<dyn Component>, LayoutError> {
        let scene = &self.scene;
        Ok(match self.role {
            Role::Top => Box::new(WindowRoot::new(merged(scene)?)),
            Role::BarTop => Box::new(WindowRoot::new(bar(scene, Edge::Top, whole())?)),
            Role::BarBottom => Box::new(WindowRoot::new(bar(scene, Edge::Bottom, whole())?)),
            Role::Stack => Box::new(WindowRoot::wrapping(stack(
                scene,
                LayoutStyle::new().width(SizeDimension::Percent(1.0)),
            )?)?),
            Role::Drawer => Box::new(WindowRoot::new(drawer(scene, whole())?)),
            Role::Catcher => Box::new(WindowRoot::new(catcher()?)),
        })
    }
}

impl App for SurfaceApp {
    fn root(&self) -> Box<dyn Component> {
        telar::reset_layout_runtime();
        let inner = self.content().expect("the spike's scene lays out");
        Box::new(Probe {
            inner,
            role: self.role,
            scene: Rc::clone(&self.scene),
        })
    }

    fn window_config(&self) -> Option<WindowConfig> {
        Some(WindowConfig {
            is_transparent: true,
            ..WindowConfig::default()
        })
    }

    fn clear_color(&self) -> Option<Color> {
        None
    }
}
