use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use telar::motion::{Animated, Spring};
use telar::{
    AlignItems, Canvas, Color, DragAxis, JustifyContent, LayoutError, LayoutItem, LayoutStyle,
    Memo, ReactiveList, Rect, RectStyle, RenderNode, RwSignal, StyledContainer, Transform, effect,
    memo, on_cleanup, signal, track_layout, use_theme,
};

use config::theme::NordTheme;
use config::{Config, Edge, LauncherConfig};
use platform_wayland::{ManagedToplevel, ManagedToplevelId, SurfaceRef, ToplevelArea};
use services::apps::App;
use ui::host::{Host, OwnSecondary};
use ui::icon::{app_icon_view, icon_view};
use ui::scale::space;

use super::lens::{self, Lens};
use super::pins::{self, PinFile, Press, Slot, SlotKey};
use super::{
    DRAG_SLOP, DRAGGED_OPACITY, GLYPH, TARGETS, fills, hover_fill, on_output, order, remembered,
};

const REACH: f32 = 3.0;
const DOT_ALPHA: f32 = 0.7;

pub struct Seams {
    pub launch: Rc<dyn Fn(&App)>,
    pub activate: Rc<dyn Fn(&ManagedToplevel)>,
    pub pins: PinFile,
}

impl Seams {
    pub fn live() -> Self {
        Self {
            launch: Rc::new(services::apps::launch),
            activate: Rc::new(|window| {
                services::windows::activate(window);
            }),
            pins: PinFile::user(),
        }
    }
}

pub fn dot_fills(theme: NordTheme) -> (Color, Color) {
    (theme.text.with_alpha(DOT_ALPHA), theme.accent)
}

fn easing(config: &Config) -> Option<Spring> {
    let animation = &config.animation;
    (animation.enabled && !animation.is_reduced()).then(|| animation.spring())
}

#[derive(Clone, Copy)]
enum Level {
    Eased(Animated<f32>),
    Instant(RwSignal<f32>),
}

impl Level {
    fn new(spring: Option<Spring>) -> Self {
        match spring {
            Some(spring) => Self::Eased(Animated::new(0.0, spring)),
            None => Self::Instant(signal(0.0)),
        }
    }

    fn get(self) -> f32 {
        match self {
            Self::Eased(level) => level.get(),
            Self::Instant(level) => level.get(),
        }
    }

    fn aim(self, to: f32) {
        match self {
            Self::Eased(level) => level.retarget(to),
            Self::Instant(level) => level.set(to),
        }
    }
}

struct Dock {
    theme: NordTheme,
    radius: f32,
    edge: Edge,
    gap: f32,
    inset: f32,
    across: f32,
    icon: f32,
    key: String,
    output: Option<String>,
    launcher: LauncherConfig,
    windows: RwSignal<Vec<ManagedToplevel>>,
    pinned: RwSignal<Vec<String>>,
    shown: Memo<Vec<Slot>>,
    reordered: RwSignal<u64>,
    spans: Rc<RefCell<HashMap<SlotKey, Rect>>>,
    laid: RwSignal<u64>,
    lenses: Memo<HashMap<SlotKey, Lens>>,
    hovered: RwSignal<Option<SlotKey>>,
    dragging: RwSignal<Option<SlotKey>>,
    landed: RefCell<Option<SlotKey>>,
    moved: Cell<bool>,
    surface: Option<SurfaceRef>,
    seams: Seams,
}

pub fn dock(
    host: &Host,
    windows: RwSignal<Vec<ManagedToplevel>>,
    apps: RwSignal<Vec<App>>,
    seams: Seams,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let edge = host.axis.unwrap_or(Edge::Bottom);
    let config = host.config();
    let key = host.output.clone().unwrap_or_default();
    let remembered = remembered(&key);
    let pinned = signal(config.dock.pinned.clone());
    let reordered = signal(0u64);
    let shown = {
        let key = key.clone();
        memo(move || {
            reordered.get();
            let arranged = order::arrange(&key, &windows.get(), &remembered.get());
            pins::slots(&pinned.get(), &apps.get(), &arranged)
        })
    };

    let inset = host.inset();
    let across = (host.thickness() - 2.0 * inset).max(1.0);
    let gap = space::xs();
    let peak = config.dock.magnification();
    let level = Level::new(easing(config));
    let pointer = signal(None::<f32>);
    let dragging = signal(None::<SlotKey>);
    let spans: Rc<RefCell<HashMap<SlotKey, Rect>>> = Rc::default();
    let laid = signal(0u64);
    let lenses = {
        let spans = Rc::clone(&spans);
        memo(move || {
            laid.get();
            let level = level.get();
            let at = pointer
                .get()
                .filter(|_| level > 0.0 && dragging.with(Option::is_none));
            let mut placed: Vec<(SlotKey, (f32, f32))> = spans
                .borrow()
                .iter()
                .map(|(slot, rect)| (slot.clone(), along(edge, *rect)))
                .collect();
            placed.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
            let bounds: Vec<(f32, f32)> = placed.iter().map(|(_, span)| *span).collect();
            let grown = lens::lenses(
                &bounds,
                at,
                1.0 + (peak - 1.0) * level,
                REACH * (across + gap),
            );
            placed
                .into_iter()
                .map(|(slot, _)| slot)
                .zip(grown)
                .collect::<HashMap<_, _>>()
        })
    };

    let dock = Rc::new(Dock {
        theme: use_theme::<NordTheme>(),
        radius: host.corner_radius(),
        edge,
        gap,
        inset,
        across,
        icon: (host.icon_size() * 0.8).round(),
        key,
        output: host.output.clone(),
        launcher: config.launcher.clone(),
        windows,
        pinned,
        shown,
        reordered,
        spans,
        laid,
        lenses,
        hovered: signal(None),
        dragging,
        landed: RefCell::new(None),
        moved: Cell::new(false),
        surface: platform_wayland::current_surface(),
        seams,
    });
    if let Some(own) = OwnSecondary::current() {
        let answering = Rc::clone(&dock);
        own.offer(move || answering.toggle_hovered());
    }

    let built = Rc::clone(&dock);
    let list = ReactiveList::with_style(
        LayoutStyle::new()
            .flex_column()
            .min_width(0.0)
            .min_height(0.0)
            .flex_shrink(1.0),
        move || vec![shown.with(Vec::is_empty)],
        |empty: &bool| *empty,
        move |empty| match empty {
            true => built.empty(),
            false => built.strip(peak > 1.0),
        },
    )?;
    let origin = signal(Rect::new(0.0, 0.0, 0.0, 0.0));
    let wrapper = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .min_width(0.0)
            .min_height(0.0),
        |_| RectStyle::default(),
        vec![Box::new(list) as Box<dyn LayoutItem>],
    )?;
    let wrapper = match peak > 1.0 {
        true => wrapper
            .on_pointer_move(move |x, y| {
                let at = origin.peek();
                pointer.set(Some(match edge.is_horizontal() {
                    true => at.x + x,
                    false => at.y + y,
                }));
            })
            .on_hover(move |inside| level.aim(if inside { 1.0 } else { 0.0 })),
        false => wrapper,
    };
    if let Some(rect) = track_layout(wrapper.layout_node()) {
        effect(move || origin.set(rect.get()));
    }
    Ok(Box::new(wrapper))
}

fn along(edge: Edge, rect: Rect) -> (f32, f32) {
    match edge.is_horizontal() {
        true => (rect.x, rect.x + rect.width),
        false => (rect.y, rect.y + rect.height),
    }
}

/// Pivoted on the side facing the dock's edge, so an entry grows into the screen and never past the edge.
fn magnified(edge: Edge, rect: Rect, lens: Lens) -> Option<[f32; 6]> {
    if lens == Lens::REST {
        return None;
    }
    let (middle_x, middle_y) = (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
    let (pivot, shift) = match edge {
        Edge::Top => ((middle_x, rect.y), (lens.shift, 0.0)),
        Edge::Bottom => ((middle_x, rect.y + rect.height), (lens.shift, 0.0)),
        Edge::Left => ((rect.x, middle_y), (0.0, lens.shift)),
        Edge::Right => ((rect.x + rect.width, middle_y), (0.0, lens.shift)),
    };
    Some(
        Transform::scale_around(lens.scale, lens.scale, pivot.0, pivot.1)
            .then(Transform::translate(shift.0, shift.1))
            .to_array(),
    )
}

fn dots(edge: Edge, local: Rect, count: usize, fill: Color) -> RenderNode {
    if count == 0 {
        return RenderNode::Empty;
    }
    let size = (local.width.min(local.height) * 0.08)
        .round()
        .clamp(3.0, 6.0);
    let run = (2 * count - 1) as f32 * size;
    let margin = size / 2.0;
    RenderNode::group((0..count).map(|index| {
        let step = index as f32 * 2.0 * size;
        let (x, y) = match edge {
            Edge::Bottom => (
                (local.width - run) / 2.0 + step,
                local.height - margin - size,
            ),
            Edge::Top => ((local.width - run) / 2.0 + step, margin),
            Edge::Left => (margin, (local.height - run) / 2.0 + step),
            Edge::Right => (
                local.width - margin - size,
                (local.height - run) / 2.0 + step,
            ),
        };
        RenderNode::rect(
            Rect::new(x, y, size, size),
            RectStyle::filled(fill, size / 2.0),
        )
    }))
}

impl Dock {
    fn slot(&self, key: &SlotKey) -> Option<Slot> {
        self.shown
            .with(|slots| slots.iter().find(|slot| slot.key == *key).cloned())
    }

    fn icon_of(&self, slot: &Slot) -> String {
        let icon = slot.icon();
        match &slot.entry {
            Some(entry) => self.launcher.icon_for(&entry.id, &icon).to_string(),
            None => icon,
        }
    }

    fn press(&self, key: &SlotKey) {
        match self.slot(key).map(|slot| slot.press()) {
            Some(Press::Launch(app)) => (self.seams.launch)(&app),
            Some(Press::Activate(window)) => (self.seams.activate)(&window),
            Some(Press::Nothing) | None => {}
        }
    }

    fn toggle_hovered(&self) -> bool {
        let Some(slot) = self.hovered.peek().and_then(|key| self.slot(&key)) else {
            return false;
        };
        let pinned = pins::toggled(&self.pinned.peek(), &slot.pin_id());
        self.seams.pins.save(&pinned);
        self.pinned.set(pinned);
        true
    }

    fn drag(&self, key: &SlotKey, x: f32, y: f32) {
        let spans = self.spans.borrow();
        let Some(own) = spans.get(key) else {
            return;
        };
        let point = match self.edge.is_horizontal() {
            true => own.x + x,
            false => own.y + y,
        };
        let pinned = key.is_pinned();
        let mut ordered: Vec<(SlotKey, f32, f32)> = spans
            .iter()
            .filter(|(slot, _)| slot.is_pinned() == pinned)
            .map(|(slot, rect)| {
                let (start, end) = along(self.edge, *rect);
                (slot.clone(), start, end)
            })
            .collect();
        drop(spans);
        ordered.sort_by(|a, b| a.1.total_cmp(&b.1));
        self.dragging.set(Some(key.clone()));
        let landing = order::landing(&ordered, point);
        if landing.as_ref() == Some(key) {
            *self.landed.borrow_mut() = None;
            return;
        }
        let Some(onto) = landing.filter(|onto| self.landed.borrow().as_ref() != Some(onto)) else {
            return;
        };
        self.move_onto(key, &onto);
        *self.landed.borrow_mut() = Some(onto);
        self.moved.set(true);
    }

    fn move_onto(&self, dragged: &SlotKey, onto: &SlotKey) {
        match (dragged, onto) {
            (SlotKey::Pinned(dragged), SlotKey::Pinned(onto)) => {
                let moved = pins::moved(&self.pinned.peek(), dragged, onto);
                self.pinned.set(moved);
            }
            (SlotKey::Running(dragged), SlotKey::Running(onto)) => {
                let running: Vec<String> = self.shown.with(|slots| {
                    slots
                        .iter()
                        .filter_map(|slot| match &slot.key {
                            SlotKey::Running(app) => Some(app.clone()),
                            SlotKey::Pinned(_) => None,
                        })
                        .collect()
                });
                let apps = pins::moved(&running, dragged, onto);
                order::hold_grouped(&self.key, &self.windows.peek(), &apps);
                self.reordered
                    .update(|count| *count = count.wrapping_add(1));
            }
            _ => {}
        }
    }

    fn drop_dragged(&self, key: &SlotKey) {
        self.dragging.set(None);
        *self.landed.borrow_mut() = None;
        if !self.moved.replace(false) {
            return;
        }
        match key {
            SlotKey::Pinned(_) => self.seams.pins.save(&self.pinned.peek()),
            SlotKey::Running(_) => {
                let shown = order::held(&self.key, &self.windows.peek());
                let output = self.key.clone();
                services::state::update(move |state| {
                    let before = state.window_order.get(&output).cloned().unwrap_or_default();
                    state
                        .window_order
                        .insert(output, order::remembered_after(&shown, &before));
                });
            }
        }
    }

    fn strip(self: &Rc<Self>, magnifies: bool) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let style = match self.edge.is_horizontal() {
            true => LayoutStyle::new().flex_row(),
            false => LayoutStyle::new().flex_column(),
        };
        let shown = self.shown;
        let built = Rc::clone(self);
        let list = ReactiveList::with_style(
            style
                .gap(self.gap)
                .padding_all(self.inset)
                .align_items(AlignItems::CENTER)
                .min_width(0.0),
            move || shown.get(),
            |slot: &Slot| slot.key.clone(),
            move |slot: Slot| built.entry(slot.key, magnifies),
        )?;
        Ok(Box::new(list))
    }

    fn entry(
        self: &Rc<Self>,
        key: SlotKey,
        magnifies: bool,
    ) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let theme = self.theme;
        let edge = self.edge;
        let shown = self.shown;
        let live = {
            let key = key.clone();
            memo(move || shown.with(|slots| slots.iter().find(|slot| slot.key == key).cloned()))
        };
        let icon = self
            .slot(&key)
            .map(|slot| self.icon_of(&slot))
            .unwrap_or_default();
        let glyph = match app_icon_view(&icon, self.icon)? {
            Some(icon) => icon,
            None => icon_view(|| GLYPH.to_string(), move || theme.text, self.icon)?,
        };
        let (running, focused_dot) = dot_fills(theme);
        let marks = Canvas::new(LayoutStyle::new().absolute_fill(), move |local| {
            live.with(|slot| match slot {
                Some(slot) => dots(
                    edge,
                    local,
                    slot.dots(),
                    if slot.is_focused() {
                        focused_dot
                    } else {
                        running
                    },
                ),
                None => RenderNode::Empty,
            })
        })?;

        let (rest, active) = fills(theme);
        let hover = hover_fill(theme);
        let radius = self.radius;
        let focused = move || live.with(|slot| slot.as_ref().is_some_and(Slot::is_focused));
        let dragging = self.dragging;
        let hovered = self.hovered;
        let lenses = self.lenses;
        let (pressing, hovering, dragged, dropped, lensed) = (
            Rc::clone(self),
            key.clone(),
            (Rc::clone(self), key.clone()),
            (Rc::clone(self), key.clone()),
            key.clone(),
        );
        let opaque_key = key.clone();
        let item = StyledContainer::new(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER)
                .width(self.across)
                .height(self.across)
                .flex_shrink(0.0),
            move |_| RectStyle::filled(if focused() { active } else { rest }, radius),
            vec![glyph, Box::new(marks)],
        )?
        .hover_style(move |_| RectStyle::filled(if focused() { active } else { hover }, radius))
        .with_opacity(
            move || match dragging.with(|held| held.as_ref() == Some(&opaque_key)) {
                true => DRAGGED_OPACITY,
                false => 1.0,
            },
        )
        .on_press({
            let key = key.clone();
            move || pressing.press(&key)
        })
        .on_hover(move |inside| match inside {
            true => hovered.set(Some(hovering.clone())),
            false if hovered.peek().as_ref() == Some(&hovering) => hovered.set(None),
            false => {}
        })
        .drag_threshold(DRAG_SLOP)
        .drag_axis(match edge.is_horizontal() {
            true => DragAxis::Horizontal,
            false => DragAxis::Vertical,
        })
        .on_drag(move |x, y| dragged.0.drag(&dragged.1, x, y))
        .on_drag_end(move |_, _| dropped.0.drop_dragged(&dropped.1));
        let item = match magnifies {
            true => item.with_transform(move |rect| {
                lenses
                    .with(|lenses| lenses.get(&lensed).copied())
                    .and_then(|lens| magnified(edge, rect, lens))
            }),
            false => item,
        };
        self.track(key, item.layout_node(), live);
        Ok(Box::new(item))
    }

    fn track(self: &Rc<Self>, key: SlotKey, node: telar::NodeId, live: Memo<Option<Slot>>) {
        let Some(rect) = track_layout(node) else {
            return;
        };
        let (spans, laid) = (Rc::clone(&self.spans), self.laid);
        {
            let key = key.clone();
            effect(move || {
                let at = rect.get();
                let changed = spans.borrow_mut().insert(key.clone(), at) != Some(at);
                if changed {
                    laid.update(|count| *count = count.wrapping_add(1));
                }
            });
        }
        let spans = Rc::clone(&self.spans);
        on_cleanup(move || {
            spans.borrow_mut().remove(&key);
        });

        let Some(surface) = self.surface.clone() else {
            return;
        };
        let held: Rc<RefCell<HashMap<ManagedToplevelId, u64>>> = Rc::default();
        let here = Rc::clone(self);
        let (sending, from) = (Rc::clone(&held), surface.clone());
        effect(move || {
            let laid = rect.get();
            let windows = live.with(|slot| {
                slot.as_ref()
                    .map(|slot| slot.windows.clone())
                    .unwrap_or_default()
            });
            let mut held = sending.borrow_mut();
            let gone: Vec<ManagedToplevelId> = held
                .keys()
                .filter(|id| !windows.iter().any(|window| window.id == **id))
                .copied()
                .collect();
            for id in gone {
                if let Some(holder) = held.remove(&id) {
                    withdraw(&from, id, holder);
                }
            }
            for window in &windows {
                let target = (from.clone(), window.id);
                let holder = *held.entry(window.id).or_insert_with(|| {
                    TARGETS.with(|targets| targets.borrow_mut().claim(target.clone()))
                });
                let area = Some(window)
                    .filter(|window| on_output(here.output.as_deref(), window))
                    .and_then(|_| ToplevelArea::covering(laid));
                if TARGETS.with(|targets| targets.borrow_mut().send(&target, holder, area)) {
                    services::windows::set_rectangle(window.id, &from, area);
                }
            }
        });
        on_cleanup(move || {
            for (id, holder) in held.borrow_mut().drain() {
                withdraw(&surface, id, holder);
            }
        });
    }

    fn empty(&self) -> Result<Box<dyn LayoutItem>, LayoutError> {
        let muted = self.theme.muted;
        let glyph = icon_view(|| GLYPH.to_string(), move || muted, self.icon)?;
        let side = self.across + 2.0 * self.inset;
        Ok(Box::new(StyledContainer::new(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .justify_content(JustifyContent::CENTER)
                .width(side)
                .height(side)
                .flex_shrink(0.0),
            |_| RectStyle::default(),
            vec![glyph],
        )?))
    }
}

fn withdraw(surface: &SurfaceRef, id: ManagedToplevelId, holder: u64) {
    let target = (surface.clone(), id);
    if TARGETS.with(|targets| targets.borrow_mut().release(&target, holder)) {
        services::windows::set_rectangle(id, surface, None);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use telar::{
        AvailableSpace, ComponentList, Container, DrawCommand, Event, Paint, PointerButton,
        PointerSource, compute_layout, reset_layout_runtime, set_theme,
    };

    use config::Config;
    use ui::host::{Instance, Representation, Size};

    use super::*;

    const THICKNESS: f32 = 60.0;
    const LENGTH: f32 = 600.0;

    #[derive(Default)]
    struct Record {
        launched: RefCell<Vec<String>>,
        activated: RefCell<Vec<u32>>,
    }

    struct Rig {
        extent: RwSignal<Rect>,
        tree: ComponentList,
        root: telar::NodeId,
        size: (f32, f32),
        record: Rc<Record>,
        own: OwnSecondary,
        pins: std::path::PathBuf,
        owner: telar::OwnerId,
        _scope: telar::OwnerGuard,
    }

    fn app(id: &str) -> App {
        App {
            id: id.to_string(),
            name: id.to_string(),
            exec: id.to_string(),
            ..App::default()
        }
    }

    fn window(id: u32, app: &str, activated: bool) -> ManagedToplevel {
        ManagedToplevel {
            id: ManagedToplevelId::from_raw(id),
            app_id: app.to_string(),
            title: app.to_string(),
            activated,
            ..ManagedToplevel::default()
        }
    }

    fn pin_path(name: &str) -> std::path::PathBuf {
        let root = util::paths::isolated_root().expect("a test writes only its own scratch tree");
        let path = root.join("dock").join(name).join("config.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        path
    }

    fn host(edge: Edge, output: &str, pinned: &[&str], magnification: f32) -> Host {
        moving(edge, output, pinned, magnification, |animation| {
            animation.enabled = false
        })
    }

    fn moving(
        edge: Edge,
        output: &str,
        pinned: &[&str],
        magnification: f32,
        motion: impl FnOnce(&mut config::AnimationConfig),
    ) -> Host {
        let mut config = Config::starter();
        config.dock.pinned = pinned.iter().map(|pin| pin.to_string()).collect();
        config.dock.magnification = magnification;
        motion(&mut config.animation);
        let config = Arc::new(config);
        let theme = config.resolve_theme();
        let extent = match edge.is_horizontal() {
            true => Size {
                width: f32::INFINITY,
                height: THICKNESS,
            },
            false => Size {
                width: THICKNESS,
                height: f32::INFINITY,
            },
        };
        Host::placed(
            Instance::of_module("windows"),
            Arc::clone(&config),
            Representation::Chip,
            extent,
            Some(edge),
            config.shape_from(None, None, None, None),
            theme.accent,
            theme.text,
            Some(output.to_string()),
        )
        .in_dock(true)
    }

    fn rig(
        host: &Host,
        open: Vec<ManagedToplevel>,
        apps: Vec<App>,
        pins: std::path::PathBuf,
    ) -> Rig {
        reset_layout_runtime();
        set_theme(NordTheme::new());
        let scope = telar::owner_scope();
        let owner = scope.id();
        let record = Rc::new(Record::default());
        let seams = Seams {
            launch: {
                let record = Rc::clone(&record);
                Rc::new(move |app: &App| record.launched.borrow_mut().push(app.id.clone()))
            },
            activate: {
                let record = Rc::clone(&record);
                Rc::new(move |window: &ManagedToplevel| {
                    record.activated.borrow_mut().push(window.id.raw())
                })
            },
            pins: PinFile::at(pins.clone()),
        };
        let own = OwnSecondary::default();
        let item = telar::batch(|| {
            telar::Scope::with(|| {
                own.provide();
                dock(host, signal(open), signal(apps), seams)
            })
        })
        .expect("the dock builds");
        let size = match host.axis.is_some_and(Edge::is_horizontal) {
            true => (LENGTH, THICKNESS),
            false => (THICKNESS, LENGTH),
        };
        let extent = track_layout(item.layout_node()).expect("the dock is laid out");
        let page = Container::new(
            LayoutStyle::new()
                .flex_column()
                .width(size.0)
                .height(size.1),
            vec![item],
        )
        .expect("a page");
        let root = page.layout_node();
        let tree = ComponentList::new(page);
        let mut rig = Rig {
            extent,
            tree,
            root,
            size,
            record,
            own,
            pins,
            owner,
            _scope: scope,
        };
        rig.lay_out();
        rig
    }

    impl Rig {
        fn lay_out(&mut self) {
            compute_layout(
                self.root,
                AvailableSpace::Definite(self.size.0),
                AvailableSpace::Definite(self.size.1),
            )
            .expect("the dock lays out");
        }

        fn commands(&self) -> Vec<DrawCommand> {
            self.tree.commands().to_vec()
        }

        fn send(&mut self, event: Event) {
            telar::batch(|| self.tree.on_event(&event));
            self.lay_out();
        }

        fn hover(&mut self, at: (f32, f32)) {
            self.send(Event::PointerMoved {
                x: f64::from(at.0),
                y: f64::from(at.1),
                source: PointerSource::Mouse,
            });
        }

        fn click(&mut self, at: (f32, f32), button: PointerButton) {
            self.hover(at);
            self.send(Event::PointerPressed {
                x: f64::from(at.0),
                y: f64::from(at.1),
                button,
                source: PointerSource::Mouse,
            });
            self.send(Event::PointerReleased {
                x: f64::from(at.0),
                y: f64::from(at.1),
                button,
                source: PointerSource::Mouse,
            });
        }

        fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
            self.hover(from);
            self.send(Event::PointerPressed {
                x: f64::from(from.0),
                y: f64::from(from.1),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
            self.send(Event::PointerMoved {
                x: f64::from(to.0),
                y: f64::from(to.1),
                source: PointerSource::Mouse,
            });
            self.send(Event::PointerReleased {
                x: f64::from(to.0),
                y: f64::from(to.1),
                button: PointerButton::Primary,
                source: PointerSource::Mouse,
            });
        }

        fn entries(&self) -> Vec<Rect> {
            let theme = NordTheme::new();
            let (rest, active) = fills(theme);
            let painted = [rest, active, hover_fill(theme)].map(Paint::Solid);
            self.commands()
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::Rect { rect, style }
                        if style
                            .fill
                            .as_ref()
                            .is_some_and(|fill| painted.contains(fill)) =>
                    {
                        Some(*rect)
                    }
                    _ => None,
                })
                .collect()
        }

        fn centre(&self, index: usize) -> (f32, f32) {
            let rect = self.entries()[index];
            (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
        }

        fn dots(&self) -> (usize, usize) {
            let (running, focused) = dot_fills(NordTheme::new());
            self.commands()
                .iter()
                .fold((0, 0), |(plain, lit), command| match command {
                    DrawCommand::Rect { style, .. }
                        if style.fill == Some(Paint::Solid(running)) =>
                    {
                        (plain + 1, lit)
                    }
                    DrawCommand::Rect { style, .. }
                        if style.fill == Some(Paint::Solid(focused)) =>
                    {
                        (plain, lit + 1)
                    }
                    _ => (plain, lit),
                })
        }

        fn pins_written(&self) -> bool {
            self.pins.exists()
        }

        fn saved(&self) -> Vec<String> {
            Config::load(&self.pins)
                .expect("the pins were written")
                .dock
                .pinned
        }

        fn matrices(&self) -> Vec<[f32; 6]> {
            self.commands()
                .iter()
                .filter_map(|command| match command {
                    DrawCommand::PushMatrix { matrix } => Some(*matrix),
                    _ => None,
                })
                .filter(|matrix| matrix[0] > 1.0 + 1e-3)
                .collect()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            telar::dispose_owner(self.owner);
        }
    }

    #[test]
    fn a_pinned_app_that_is_not_running_launches_and_a_running_one_is_brought_up() {
        let mut rig = rig(
            &host(Edge::Bottom, "launch", &["firefox", "kitty"], 1.0),
            vec![window(7, "kitty", false)],
            vec![app("firefox"), app("kitty")],
            pin_path("launch"),
        );
        assert_eq!(rig.entries().len(), 2);
        rig.click(rig.centre(0), PointerButton::Primary);
        assert_eq!(*rig.record.launched.borrow(), ["firefox"]);
        rig.click(rig.centre(1), PointerButton::Primary);
        assert_eq!(*rig.record.activated.borrow(), [7]);
        assert_eq!(
            *rig.record.launched.borrow(),
            ["firefox"],
            "a running pin is not launched again"
        );
    }

    #[test]
    fn without_any_window_listed_the_pins_still_show_and_launch() {
        let mut rig = rig(
            &host(Edge::Left, "unlisted", &["code"], 1.0),
            Vec::new(),
            vec![app("code")],
            pin_path("unlisted"),
        );
        assert_eq!(rig.entries().len(), 1);
        rig.click(rig.centre(0), PointerButton::Primary);
        assert_eq!(*rig.record.launched.borrow(), ["code"]);
    }

    #[test]
    fn each_entry_carries_a_dot_per_window_up_to_three_and_the_focused_ones_are_lit() {
        let open = vec![
            window(1, "kitty", false),
            window(2, "kitty", false),
            window(3, "firefox", true),
            window(4, "code", false),
            window(5, "code", false),
            window(6, "code", false),
            window(7, "code", false),
        ];
        let rig = rig(
            &host(Edge::Bottom, "dots", &["kitty", "gimp"], 1.0),
            open,
            vec![app("kitty"), app("gimp")],
            pin_path("dots"),
        );
        assert_eq!(rig.entries().len(), 4, "two pins, then firefox and code");
        assert_eq!(
            rig.dots(),
            (2 + 3, 1),
            "kitty 2, gimp none, code 3 of 4; firefox lit"
        );
    }

    #[test]
    fn a_secondary_press_pins_an_entry_and_unpins_it_and_the_dock_keeps_the_pins() {
        let mut rig = rig(
            &host(Edge::Top, "secondary", &["firefox"], 1.0),
            vec![window(1, "kitty", false)],
            vec![app("firefox"), app("kitty")],
            pin_path("secondary"),
        );
        rig.hover(rig.centre(1));
        assert!(rig.own.answer(), "the entry under the pointer answers");
        assert_eq!(rig.saved(), ["firefox", "kitty"]);
        rig.lay_out();
        rig.hover(rig.centre(0));
        assert!(rig.own.answer());
        assert_eq!(rig.saved(), ["kitty"]);
        rig.lay_out();
        assert_eq!(
            rig.entries().len(),
            1,
            "firefox was neither pinned nor running"
        );

        rig.hover((-50.0, -50.0));
        assert!(
            !rig.own.answer(),
            "over no entry the press goes on to the menu"
        );
    }

    #[test]
    fn dragging_an_application_that_is_not_pinned_moves_it_among_the_others_and_never_among_the_pins()
     {
        let mut rig = rig(
            &host(Edge::Bottom, "running-order", &["pinned"], 1.0),
            vec![
                window(1, "kitty", false),
                window(2, "firefox", false),
                window(3, "code", false),
            ],
            vec![app("pinned")],
            pin_path("running-order"),
        );
        let (from, to) = (rig.centre(1), rig.centre(3));
        rig.drag(from, to);
        assert_eq!(
            services::state::get()
                .window_order
                .get("running-order")
                .cloned(),
            Some(vec![
                "firefox".to_string(),
                "code".to_string(),
                "kitty".to_string()
            ])
        );
        let (from, to) = (rig.centre(3), rig.centre(0));
        rig.drag(from, to);
        assert!(
            !rig.pins_written(),
            "a running application dragged onto a pin is not pinned"
        );
    }

    #[test]
    fn dragging_a_pin_onto_another_moves_it_there_and_saves_the_order_on_every_edge() {
        for edge in Edge::ALL {
            let name = format!("drag-{edge:?}");
            let mut rig = rig(
                &host(edge, &name, &["a", "b", "c"], 1.0),
                Vec::new(),
                vec![app("a"), app("b"), app("c")],
                pin_path(&name),
            );
            let (from, to) = (rig.centre(0), rig.centre(2));
            rig.drag(from, to);
            assert_eq!(rig.saved(), ["b", "c", "a"], "{edge:?}");
            rig.click(rig.centre(0), PointerButton::Primary);
            assert_eq!(*rig.record.launched.borrow(), ["b"], "{edge:?}");
        }
    }

    #[test]
    fn hovering_magnifies_the_entries_near_the_pointer_without_moving_or_growing_the_dock() {
        for edge in Edge::ALL {
            let name = format!("lens-{edge:?}");
            let mut rig = rig(
                &host(edge, &name, &["a", "b", "c", "d", "e"], 1.8),
                Vec::new(),
                ["a", "b", "c", "d", "e"].into_iter().map(app).collect(),
                pin_path(&name),
            );
            let laid = rig.entries();
            let extent = rig.extent.get();
            assert!(rig.matrices().is_empty(), "{edge:?}: nothing grows at rest");

            rig.hover(rig.centre(2));
            let grown = rig.matrices();
            assert!(
                !grown.is_empty(),
                "{edge:?}: the entries near the pointer grow"
            );
            let largest = grown.iter().map(|matrix| matrix[0]).fold(0.0, f32::max);
            assert!((largest - 1.8).abs() < 1e-3, "{edge:?}: {largest}");
            assert_eq!(rig.entries(), laid, "{edge:?}: the layout is untouched");
            assert_eq!(
                rig.extent.get(),
                extent,
                "{edge:?}: the dock keeps its thickness"
            );

            rig.hover((-50.0, -50.0));
            assert!(
                rig.matrices().is_empty(),
                "{edge:?}: leaving puts them back"
            );
        }
    }

    fn largest(rig: &Rig) -> f32 {
        rig.matrices()
            .iter()
            .map(|matrix| matrix[0])
            .fold(1.0, f32::max)
    }

    #[test]
    fn the_entries_grow_into_place_and_snap_there_where_motion_is_reduced() {
        let pins = ["a", "b", "c"];
        let apps = || pins.into_iter().map(app).collect::<Vec<_>>();
        let eased = moving(Edge::Bottom, "eased", &pins, 2.0, |animation| {
            animation.enabled = true;
            animation.reduced = config::ReducedMotion::Off;
        });
        let mut slow = rig(&eased, Vec::new(), apps(), pin_path("eased"));
        slow.hover(slow.centre(1));
        assert!(
            largest(&slow) < 2.0 - 1e-3,
            "not at once: {}",
            largest(&slow)
        );
        let start = std::time::Instant::now();
        for frame in 1..=120 {
            telar::motion::tick(start + std::time::Duration::from_millis(16 * frame));
        }
        slow.lay_out();
        assert!(
            (largest(&slow) - 2.0).abs() < 1e-2,
            "settled: {}",
            largest(&slow)
        );
        drop(slow);

        let reduced = moving(Edge::Bottom, "reduced", &pins, 2.0, |animation| {
            animation.enabled = true;
            animation.reduced = config::ReducedMotion::On;
        });
        let mut snapped = rig(&reduced, Vec::new(), apps(), pin_path("reduced"));
        snapped.hover(snapped.centre(1));
        assert!(
            (largest(&snapped) - 2.0).abs() < 1e-3,
            "at once: {}",
            largest(&snapped)
        );
    }
}
