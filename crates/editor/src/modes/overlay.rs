//! The overlay mode's tools (TA-5): stacks made, pinned to one of nine anchors and moved off it, made wider or narrower, told which cards they take, and chosen as where volume and brightness and the launcher appear.
//!
//! **Where a card goes (F-2.8).** A card is offered the stacks of its screen in order — layer by layer from the bottom, then in each layer's order — and goes to the first with a route that takes it; a stack with no routes takes whatever no route on that screen takes, the first such stack taking all of it ([`layout::route_card`]). So several stacks share a screen without their order mattering for the catch-all: critical notifications to one in the middle, everything else to the corner. The popover says so, and says what each stack takes now ([`takes`]).
//!
//! **A stack is moved by its first card.** Each stack shows a box where its first card would be; dragged, the stack is pinned to the ninth of its screen that box is let go over, and moved off that anchor by as much as it was dropped away from where the anchor alone puts it — not at all within a short distance, so a drop near an anchor lands exactly on it ([`landing`]). The selected stack shows the nine anchors as points to press, which pins it there exactly.
//!
//! **Volume and brightness, and the launcher.** An OSD is a card like the others, so where it appears is a route of kind `osd`, which "Show volume and brightness here" adds and takes away — on any stack but the one taking what no route takes, which has them already and would take nothing else with a route of its own. The launcher is not a card: it opens where the first stack on its screen with `launcher = true` is pinned, and in the middle of the screen where none says so (`modules::launcher::placement`).
//!
//! **Every drag has a key (WCAG 2.5.7).** Shift+N makes a stack, Shift+arrows step its anchor, Alt+Shift+arrows move it a few pixels off it, Ctrl+arrows make it wider or narrower, `v` shows volume and brightness in it or stops, Shift+O opens the launcher at it or stops, and Enter opens the popover with its routes. Each is one undo entry.
//!
//! **Trying the routes.** `t` sends the next sample — a notification, a critical one, a toast, a volume OSD, the launcher — each staying as long as the real one would ([`card_samples::lifetime`]), and a stack's "Try cards" menu sends any of them. A sample is routed by [`layout::route_card`] among the stacks of the screen and drawn in the one it lands in ([`surfaces::card_samples`]), so a critical sample shows which stack takes critical notifications. Samples are previews: they never reach the notification daemon, its history or the toaster, and leaving the mode takes them all away.

use std::cell::Cell;
use std::rc::Rc;

use telar::{
    Border, Children, Color, Cursor, JustifyContent, LayoutError, LayoutStyle, ReactiveList, Rect,
    RectStyle, RwSignal, StyledContainer, detached, effect, signal, use_theme,
};

use config::theme::NordTheme;
use layout::{
    Anchor, Area, AreaId, AreaKind, CardKind, LayerKind, Layout, LayoutOp, Offset, ResolvedArea,
    ResolvedAreaKind, Route, StackOutputPolicy, Urgency, Within,
};
use surfaces::card_samples::{self, Launcher, Sample, Shown};
use surfaces::pinned::{self, Side};
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::Node;
use ui::descriptor::Built;

use crate::context;
use crate::host::{passthrough, see_through, usable, whole};
use crate::keys::{self, Chord, Direction, KeyOp, Run};
use crate::mode::{self, Mode, said};
use crate::popover::area::{chosen, variants};
use crate::popover::rows::{self, Range, label};
use crate::popover::{AreaDraft, Inspector, help, kind_field, kind_read, parsed, spelled};
use crate::session::{self, Edit, EditError, Selection};
use crate::written::Work;

use super::gesture::{self, held, pressable};

/// How tall the box standing for a stack's first card is.
const GHOST: f32 = 96.0;
/// How near to where an anchor alone puts a stack a drag can let it go and still land exactly there.
const SNAP: f32 = 24.0;
/// How big a point of the anchor picker is across.
const DOT: f32 = 14.0;
/// How far one Alt+Shift+arrow moves a stack off its anchor.
const NUDGE: f32 = 8.0;
/// How wide a new stack is, as wide as the shipped one.
const NEW_WIDTH: f32 = 380.0;
/// The anchors a new stack takes, the first no stack of its screen is pinned to: the middle first, where a stack the corner one leaves alone is most often wanted.
const NEW_ANCHORS: [Anchor; 9] = [
    Anchor::Center,
    Anchor::Top,
    Anchor::Bottom,
    Anchor::TopLeft,
    Anchor::BottomLeft,
    Anchor::BottomRight,
    Anchor::Left,
    Anchor::Right,
    Anchor::TopRight,
];

pub(crate) fn install() {
    crate::host::add_tool(LayerKind::Overlay, tool);
    crate::host::set_add(LayerKind::Overlay, add_stack);
    crate::host::add_adding_button(
        LayerKind::Overlay,
        (
            || telar::t!("editor.overlay.new_stack"),
            || said(add_stack()),
        ),
    );
    crate::host::add_toolbar_button(
        LayerKind::Overlay,
        (
            || telar::t!("editor.overlay.try.button"),
            || said(try_next()),
        ),
    );
    keys::add_mode_key_op(
        LayerKind::Overlay,
        KeyOp {
            name: "cards-try",
            keys: vec![Chord::char('t')],
            label: || telar::t!("editor.keys.op.cards-try"),
            run: Run::Act(|_| try_next()),
        },
    );
    detached(|| effect(clear_samples_outside_the_mode));
    crate::popover::add_area_tool("stack", stack_tool);
    context::add_area_rows("stack", stack_rows);
    keys::add_mode_key_op(
        LayerKind::Overlay,
        KeyOp {
            name: "stack-add",
            keys: vec![stack_key()],
            label: || telar::t!("editor.keys.op.stack-add"),
            run: Run::Act(|_| add_stack()),
        },
    );
    keys::add_key_op(
        "stack",
        KeyOp {
            name: "stack-offset",
            keys: keys::arrows_with(|chord| chord.alt().shift()),
            label: || telar::t!("editor.keys.op.stack-offset"),
            run: Run::Toward(nudge_selected),
        },
    );
    keys::add_key_op(
        "stack",
        KeyOp {
            name: "osd-placement",
            keys: vec![osd_key()],
            label: || telar::t!("editor.keys.op.osd-placement"),
            run: Run::Act(|selection| toggle_osd(&selected_stack(selection)?)),
        },
    );
    keys::add_key_op(
        "stack",
        KeyOp {
            name: "launcher-placement",
            keys: vec![launcher_key()],
            label: || telar::t!("editor.keys.op.launcher-placement"),
            run: Run::Act(|selection| toggle_launcher(&selected_stack(selection)?)),
        },
    );
}

fn stack_key() -> Chord {
    Chord::char('n').shift()
}

fn osd_key() -> Chord {
    Chord::char('v')
}

fn launcher_key() -> Chord {
    Chord::char('o').shift()
}

/// A stack of a screen, as the overlay tools read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub layer: LayerKind,
    pub id: AreaId,
    pub anchor: Anchor,
    pub offset: Offset,
    pub width: f32,
    pub routes: Vec<Route>,
    pub launcher: bool,
    /// The box its geometry is measured in.
    pub bounds: Rect,
}

impl Placed {
    fn of(desktop: &Desktop, layer: LayerKind, area: &ResolvedArea) -> Option<Self> {
        let ResolvedAreaKind::Stack {
            anchor,
            offset,
            width,
            routes,
            launcher,
            ..
        } = &area.kind
        else {
            return None;
        };
        Some(Self {
            layer,
            id: area.id.clone(),
            anchor: *anchor,
            offset: *offset,
            width: *width,
            routes: routes.clone(),
            launcher: *launcher,
            bounds: desktop.reserved.box_of(area.within, desktop.size),
        })
    }

    /// Where its column is drawn.
    pub fn column(&self) -> Rect {
        pinned::column(self.bounds, self.anchor, self.width, self.offset)
    }

    /// Where its first card is drawn.
    pub fn ghost(&self) -> Rect {
        ghost(self.column(), self.anchor)
    }
}

/// Every stack of a screen, in the order a card is offered to them.
pub fn stacks_of(desktop: &Desktop) -> Vec<Placed> {
    desktop
        .resolved
        .areas()
        .filter_map(|(layer, area)| Placed::of(desktop, layer, area))
        .collect()
}

fn stack_on(desktop: &Desktop, layer: LayerKind, id: &AreaId) -> Result<Placed, EditError> {
    stacks_of(desktop)
        .into_iter()
        .find(|placed| placed.layer == layer && placed.id == *id)
        .ok_or_else(|| {
            EditError::refused(telar::t!("editor.overlay.no_stack", id = id.to_string()))
        })
}

/// The box of a column's first card: as wide as the column, at the end its cards grow from.
pub fn ghost(column: Rect, anchor: Anchor) -> Rect {
    let height = GHOST.min(column.height);
    let y = match pinned::sides(anchor).1 {
        Side::Start => column.y,
        Side::Middle => column.y + (column.height - height) / 2.0,
        Side::End => column.y + column.height - height,
    };
    Rect::new(column.x, y, column.width, height)
}

/// Where a stack `width` wide lands when the box of its first card is let go with its top left corner `at`, inside `bounds`: pinned to the ninth of `bounds` the box's middle is over, and moved off that anchor by as far as the box is from where the anchor alone puts it — each way, not at all within [`SNAP`] of it.
pub fn landing(bounds: Rect, width: f32, at: (f32, f32)) -> (Anchor, Offset) {
    let height = GHOST.min(bounds.height);
    let x =
        at.0.clamp(bounds.x, (bounds.x + bounds.width - width).max(bounds.x));
    let y =
        at.1.clamp(bounds.y, (bounds.y + bounds.height - height).max(bounds.y));
    let anchor = pinned::anchor_at(bounds, (x + width / 2.0, y + height / 2.0));
    let natural = ghost(pinned::column(bounds, anchor, width, Offset::ZERO), anchor);
    let snapped = |by: f32| match by.abs() < SNAP {
        true => 0.0,
        false => by.round(),
    };
    (
        anchor,
        Offset {
            x: snapped(x - natural.x),
            y: snapped(y - natural.y),
        },
    )
}

/// The stack's own fields of `area`, as it writes them, made a partial stack first where it names no kind.
fn stack_mut(area: &mut Area) -> Option<StackFields<'_>> {
    match AreaDraft::kind_mut(area, "stack")? {
        AreaKind::Stack {
            anchor,
            offset,
            routes,
            launcher,
            ..
        } => Some(StackFields {
            anchor,
            offset,
            routes,
            launcher,
        }),
        _ => None,
    }
}

struct StackFields<'a> {
    anchor: &'a mut Option<Anchor>,
    offset: &'a mut Option<Offset>,
    routes: &'a mut Vec<Route>,
    launcher: &'a mut Option<bool>,
}

/// What an edit of a stack planned in `work` did, refused where it changes nothing.
fn changed(work: Work) -> Result<Vec<LayoutOp>, EditError> {
    let ops = work.done();
    match ops.is_empty() {
        true => Err(EditError::refused(telar::t!("editor.overlay.unchanged"))),
        false => Ok(ops),
    }
}

/// A new stack on `layer` of the screen `desktop` shows, pinned to the first of [`NEW_ANCHORS`] no stack of that screen is pinned to. It routes nothing, so it takes nothing while another stack with no routes comes before it — its routes are what it is made for.
pub(crate) fn added(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
) -> Result<(Vec<LayoutOp>, AreaId), EditError> {
    let mut work = Work::new(layout, desktop, layer);
    let id = layout::ops::free_area_id(layout, &work.known, layer, "stack");
    let taken: Vec<Anchor> = stacks_of(desktop)
        .iter()
        .map(|placed| placed.anchor)
        .collect();
    let anchor = NEW_ANCHORS
        .into_iter()
        .find(|anchor| !taken.contains(anchor))
        .unwrap_or(Anchor::Center);
    let area = Area {
        id: id.clone(),
        kind: Some(AreaKind::Stack {
            anchor: Some(anchor),
            offset: None,
            width: Some(NEW_WIDTH),
            output_policy: Some(StackOutputPolicy::Focused),
            routes: Vec::new(),
            launcher: None,
        }),
        within: Some(Within::Usable),
        ..Area::default()
    };
    work.add(layer, area)?;
    Ok((work.done(), id))
}

/// The stack `id` of `layer` pinned to `anchor` and moved `offset` off it.
pub(crate) fn moved(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
    anchor: Anchor,
    offset: Offset,
) -> Result<Vec<LayoutOp>, EditError> {
    let mut work = Work::new(layout, desktop, layer);
    work.rewrite(layer, id, |area| {
        if let Some(stack) = stack_mut(area) {
            *stack.anchor = Some(anchor);
            *stack.offset = Some(offset);
        }
    })?;
    changed(work)
}

/// The stack `id` of `layer` moved [`NUDGE`] further off its anchor the way `direction` points, as long as that moves its column at all.
pub(crate) fn nudged(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let placed = stack_on(desktop, layer, id)?;
    let (dx, dy) = match direction {
        Direction::Left => (-NUDGE, 0.0),
        Direction::Right => (NUDGE, 0.0),
        Direction::Up => (0.0, -NUDGE),
        Direction::Down => (0.0, NUDGE),
    };
    let further = Placed {
        offset: Offset {
            x: placed.offset.x + dx,
            y: placed.offset.y + dy,
        },
        ..placed.clone()
    };
    if further.column() == placed.column() {
        return Err(EditError::no_way(id));
    }
    moved(layout, desktop, layer, id, placed.anchor, further.offset)
}

/// A route that takes volume and brightness, and nothing more particular.
pub(crate) fn osd_route() -> Route {
    Route {
        kind: Some(CardKind::Osd),
        app: None,
        urgency: None,
    }
}

/// Whether `routes` say volume and brightness come here.
pub fn shows_osd(routes: &[Route]) -> bool {
    routes.contains(&osd_route())
}

/// `routes` with volume and brightness added, or taken away where they are there already.
pub(crate) fn osd_toggled(mut routes: Vec<Route>) -> Vec<Route> {
    match shows_osd(&routes) {
        true => routes.retain(|route| *route != osd_route()),
        false => routes.push(osd_route()),
    }
    routes
}

/// The stack `id` of `layer` showing volume and brightness, or no longer: a route of kind `osd` added to its routes or taken out of them. Added to a stack with no routes, it makes that stack take volume and brightness alone.
pub(crate) fn osd_here(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
    routes: &[Route],
) -> Result<Vec<LayoutOp>, EditError> {
    let toggled = osd_toggled(routes.to_vec());
    let mut work = Work::new(layout, desktop, layer);
    work.rewrite(layer, id, |area| {
        if let Some(stack) = stack_mut(area) {
            *stack.routes = toggled;
        }
    })?;
    changed(work)
}

/// The launcher opening at the stack `id` of `layer`, and at no other stack of its screen — or, where it opens there already, in the middle of the screen again.
pub(crate) fn launcher_here(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    id: &AreaId,
) -> Result<Vec<LayoutOp>, EditError> {
    let stacks = stacks_of(desktop);
    let this = stack_on(desktop, layer, id)?;
    let mut work = Work::new(layout, desktop, layer);
    work.rewrite(layer, id, |area| {
        if let Some(stack) = stack_mut(area) {
            *stack.launcher = Some(!this.launcher);
        }
    })?;
    if !this.launcher {
        for other in stacks
            .iter()
            .filter(|other| other.launcher && *other != &this)
        {
            work.rewrite(other.layer, &other.id, |area| {
                if let Some(stack) = stack_mut(area) {
                    *stack.launcher = Some(false);
                }
            })?;
        }
    }
    changed(work)
}

/// The stack the selection is, or is in.
fn selected_stack(selection: &Selection) -> Result<Node, EditError> {
    let node = selection.node().ok_or_else(EditError::nothing)?;
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    stack_on(&desktop, node.layer, &node.area)?;
    Ok(Node::area(node.output.as_deref(), node.layer, &node.area))
}

fn nudge_selected(
    selection: &Selection,
    layout: &Layout,
    direction: Direction,
) -> Result<Vec<LayoutOp>, EditError> {
    let node = selected_stack(selection)?;
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    nudged(layout, &desktop, node.layer, &node.area, direction)
}

/// Makes a stack on the edited screen and selects it, as one undo entry.
pub(crate) fn add_stack() -> Result<(), EditError> {
    crate::popover::close();
    context::make(added, |id| {
        telar::t!("editor.overlay.made", name = id.to_string())
    })
}

/// Pins the stack `node` names to `anchor`, exactly, as one undo entry.
pub(crate) fn pin(node: &Node, anchor: Anchor) -> Result<(), EditError> {
    crate::popover::close();
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let ops = moved(
        &session::draft().peek(),
        &desktop,
        node.layer,
        &node.area,
        anchor,
        Offset::ZERO,
    )?;
    context::commit(
        telar::t!(
            "editor.overlay.pinned",
            name = node.area.to_string(),
            anchor = anchor_name(anchor)
        ),
        ops,
    )
}

/// Shows volume and brightness in the stack `node` names, or stops, as one undo entry.
pub(crate) fn toggle_osd(node: &Node) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let placed = stack_on(&desktop, node.layer, &node.area)?;
    if takes_the_rest(&desktop, &placed) {
        return Err(EditError::refused(catch_all_said(&desktop, &placed)));
    }
    let ops = osd_here(
        &session::draft().peek(),
        &desktop,
        node.layer,
        &node.area,
        &placed.routes,
    )?;
    let label = match shows_osd(&placed.routes) {
        true => telar::t!("editor.overlay.osd_off"),
        false => telar::t!("editor.overlay.osd_here"),
    };
    context::commit(label, ops)
}

/// Whether `placed` is the stack of its screen that takes every card no route takes: the first with no routes.
fn takes_the_rest(desktop: &Desktop, placed: &Placed) -> bool {
    stacks_of(desktop)
        .iter()
        .find(|stack| stack.routes.is_empty())
        .is_some_and(|first| first == placed)
}

/// Why volume and brightness are not routed to the stack that takes what no route takes: it takes them already, unless another stack's routes name them, and a route of its own would make it take them alone.
fn catch_all_said(desktop: &Desktop, placed: &Placed) -> String {
    let stacks = stacks_of(desktop);
    let osd = layout::RoutedCard {
        kind: CardKind::Osd,
        app: None,
        urgency: None,
    };
    match layout::route_card(&stacks, |stack| &stack.routes, &osd) {
        Some(other) if other != placed => telar::t!(
            "editor.overlay.osd_elsewhere",
            name = placed.id.to_string(),
            other = other.id.to_string()
        ),
        _ => telar::t!("editor.overlay.osd_already", name = placed.id.to_string()),
    }
}

/// Opens the launcher at the stack `node` names, or stops, as one undo entry.
pub(crate) fn toggle_launcher(node: &Node) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let placed = stack_on(&desktop, node.layer, &node.area)?;
    let ops = launcher_here(&session::draft().peek(), &desktop, node.layer, &node.area)?;
    let label = match placed.launcher {
        true => telar::t!("editor.overlay.launcher_off"),
        false => telar::t!("editor.overlay.launcher_here"),
    };
    context::commit(label, ops)
}
/// What an anchor is called in a sentence.
pub(crate) fn anchor_name(anchor: Anchor) -> String {
    match anchor {
        Anchor::TopLeft => telar::t!("editor.overlay.anchor.top_left"),
        Anchor::Top => telar::t!("editor.overlay.anchor.top"),
        Anchor::TopRight => telar::t!("editor.overlay.anchor.top_right"),
        Anchor::Left => telar::t!("editor.overlay.anchor.left"),
        Anchor::Center => telar::t!("editor.overlay.anchor.center"),
        Anchor::Right => telar::t!("editor.overlay.anchor.right"),
        Anchor::BottomLeft => telar::t!("editor.overlay.anchor.bottom_left"),
        Anchor::Bottom => telar::t!("editor.overlay.anchor.bottom"),
        Anchor::BottomRight => telar::t!("editor.overlay.anchor.bottom_right"),
    }
}

fn card_name(kind: Option<CardKind>) -> String {
    match kind {
        None => telar::t!("editor.overlay.card.any"),
        Some(CardKind::Notification) => telar::t!("editor.overlay.card.notification"),
        Some(CardKind::Toast) => telar::t!("editor.overlay.card.toast"),
        Some(CardKind::Osd) => telar::t!("editor.overlay.card.osd"),
    }
}

fn urgency_name(urgency: Option<Urgency>) -> String {
    match urgency {
        None => String::new(),
        Some(Urgency::Low) => telar::t!("editor.overlay.urgent.low"),
        Some(Urgency::Normal) => telar::t!("editor.overlay.urgent.normal"),
        Some(Urgency::Critical) => telar::t!("editor.overlay.urgent.critical"),
    }
}

/// One route, as a sentence says what it takes: "critical notifications from Mail".
pub fn route_said(route: &Route) -> String {
    let app = route
        .app
        .as_deref()
        .map(|app| telar::t!("editor.overlay.from", app = app.to_string()))
        .unwrap_or_default();
    let line = telar::t!(
        "editor.overlay.route_line",
        urgency = urgency_name(route.urgency),
        kind = card_name(route.kind),
        app = app
    );
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What the stack `id` takes, among `stacks` in the order a card is offered to them, as the popover says it — the same rule [`layout::route_card`] follows.
pub fn takes(stacks: &[(AreaId, Vec<Route>)], id: &AreaId) -> String {
    let Some((_, routes)) = stacks.iter().find(|(held, _)| held == id) else {
        return String::new();
    };
    if !routes.is_empty() {
        let said: Vec<String> = routes.iter().map(route_said).collect();
        return telar::t!("editor.overlay.takes", cards = said.join(", "));
    }
    match stacks.iter().find(|(_, routes)| routes.is_empty()) {
        Some((first, _)) if first != id => {
            telar::t!("editor.overlay.takes_nothing", first = first.to_string())
        }
        _ => telar::t!("editor.overlay.takes_rest"),
    }
}

/// Whether every stack of a screen routes something, so a card no route takes is shown nowhere on it.
pub fn rest_shown_nowhere(stacks: &[(AreaId, Vec<Route>)]) -> bool {
    !stacks.is_empty() && stacks.iter().all(|(_, routes)| !routes.is_empty())
}

/// The application names the notifications still held came from, for a route to be pointed at.
fn recent_apps() -> Vec<String> {
    let Some(snapshot) = services::notifications::snapshot_now() else {
        return Vec::new();
    };
    let mut apps: Vec<String> = snapshot
        .active
        .iter()
        .map(|notification| notification.app_name.clone())
        .chain(snapshot.muted_apps.iter().cloned())
        .filter(|app| !app.trim().is_empty())
        .collect();
    apps.sort_by_key(|app| app.to_lowercase());
    apps.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    apps
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Try {
    Notification,
    Critical,
    Toast,
    Osd,
    Launcher,
}

impl Try {
    const ALL: [Try; 5] = [
        Try::Notification,
        Try::Critical,
        Try::Toast,
        Try::Osd,
        Try::Launcher,
    ];

    fn label(self) -> String {
        match self {
            Try::Notification => telar::t!("editor.overlay.try.notification"),
            Try::Critical => telar::t!("editor.overlay.try.critical"),
            Try::Toast => telar::t!("editor.overlay.try.toast"),
            Try::Osd => telar::t!("editor.overlay.try.osd"),
            Try::Launcher => telar::t!("editor.overlay.try.launcher"),
        }
    }

    fn sample(self) -> Option<Sample> {
        match self {
            Try::Notification => Some(Sample::notification(
                "Signal",
                Urgency::Normal,
                telar::t!("editor.overlay.try.notification_title"),
                telar::t!("editor.overlay.try.notification_body"),
            )),
            Try::Critical => Some(Sample::notification(
                "UPower",
                Urgency::Critical,
                telar::t!("editor.overlay.try.critical_title"),
                telar::t!("editor.overlay.try.critical_body"),
            )),
            Try::Toast => Some(Sample::toast(
                "camera",
                telar::t!("editor.overlay.try.toast_title"),
                String::new(),
            )),
            Try::Osd => Some(Sample::osd(64)),
            Try::Launcher => None,
        }
    }
}

thread_local! {
    static NEXT_TRY: Cell<usize> = const { Cell::new(0) };
}

/// Takes the samples away whenever the mode being edited changes, which leaving the overlay mode does.
fn clear_samples_outside_the_mode() {
    mode::active().get();
    card_samples::clear();
    NEXT_TRY.with(|next| next.set(0));
}

/// Sends the sample after the last one `t` sent, round the five. A sample no stack takes is refused and the next press goes on to the one after it.
pub(crate) fn try_next() -> Result<(), EditError> {
    let at = NEXT_TRY.with(|next| next.replace(next.get() + 1));
    try_card(Try::ALL[at % Try::ALL.len()])
}

/// Puts a sample of `kind` up in the stack of the edited screen that [`layout::route_card`] sends it to, for as long as the edited screen's config keeps the real card up, or opens or closes the sample launcher. Nothing is sent to the notification daemon, its history or the toaster.
pub(crate) fn try_card(kind: Try) -> Result<(), EditError> {
    let mode = mode::current()
        .filter(|mode| mode.layer == LayerKind::Overlay)
        .ok_or_else(|| EditError::refused(telar::t!("editor.overlay.try.only_here")))?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let Some(sample) = kind.sample() else {
        card_samples::toggle_launcher(Launcher {
            search: telar::t!("editor.overlay.try.search"),
            apps: vec![
                "Firefox".to_string(),
                "Terminal".to_string(),
                telar::t!("editor.overlay.try.files"),
                telar::t!("editor.overlay.try.settings"),
            ],
        });
        return Ok(());
    };
    let stacks = stacks_of(&desktop);
    let Some(stack) = layout::route_card(&stacks, |placed| &placed.routes, &sample.routed()) else {
        return Err(EditError::refused(telar::t!(
            "editor.overlay.try.nowhere",
            card = kind.label()
        )));
    };
    mode::confirm(telar::t!(
        "editor.overlay.try.landed",
        card = kind.label(),
        name = stack.id.to_string()
    ));
    let lifetime = card_samples::lifetime(&sample, &desktop.config);
    card_samples::push(sample, lifetime);
    Ok(())
}

/// Whether the overlay mode is being edited on the screen `node` is on.
fn trying_on(node: &Node) -> bool {
    mode::current().is_some_and(|mode| {
        mode.layer == LayerKind::Overlay && node.output.as_deref() == Some(mode.output.as_str())
    })
}

fn try_rows() -> telar::MenuEntry {
    telar::MenuEntry::Sub {
        label: telar::t!("editor.overlay.try.menu"),
        entries: Try::ALL
            .into_iter()
            .map(|kind| {
                telar::MenuEntry::row(kind.label(), String::new(), move || said(try_card(kind)))
            })
            .collect(),
    }
}

/// A stack's menu rows: volume and brightness here or not — except on the stack that takes what no route takes, which takes them already — the launcher here or not, and in its mode a new stack.
fn stack_rows(area: &ResolvedArea, node: &Node) -> Vec<telar::MenuEntry> {
    let ResolvedAreaKind::Stack {
        routes, launcher, ..
    } = &area.kind
    else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let rest = reconcile::desktop(node.output.as_deref()).is_some_and(|desktop| {
        stack_on(&desktop, node.layer, &node.area)
            .is_ok_and(|placed| takes_the_rest(&desktop, &placed))
    });
    if !rest {
        let at_osd = node.clone();
        rows.push(telar::MenuEntry::row(
            match shows_osd(routes) {
                true => telar::t!("editor.overlay.osd_off"),
                false => telar::t!("editor.overlay.osd_here"),
            },
            osd_key().spelled(),
            move || said(toggle_osd(&at_osd)),
        ));
    }
    let at_launcher = node.clone();
    rows.push(telar::MenuEntry::row(
        match launcher {
            true => telar::t!("editor.overlay.launcher_off"),
            false => telar::t!("editor.overlay.launcher_here"),
        },
        launcher_key().spelled(),
        move || said(toggle_launcher(&at_launcher)),
    ));
    if trying_on(node) {
        rows.push(try_rows());
    }
    if crate::mode::editing(node) {
        rows.push(telar::MenuEntry::row(
            telar::t!("editor.overlay.new_stack"),
            stack_key().spelled(),
            || said(add_stack()),
        ));
    }
    rows
}

/// The overlay mode's layer over the edited screen: the box of every stack's first card, dragged to move the stack, and the nine anchors of the selected stack.
pub(crate) fn tool(mode: &Mode) -> Built {
    let (output, layer) = (mode.output.clone(), mode.layer);
    let frozen = signal(false);
    let samples = sample_cards(output.clone())?;
    let ghosts = {
        let (listing, building) = (output.clone(), output.clone());
        ReactiveList::with_style(
            whole(),
            held(frozen, move || {
                reconcile::desktop(Some(&listing))
                    .map(|desktop| {
                        stacks_of(&desktop)
                            .into_iter()
                            .filter(|placed| placed.layer == layer)
                            .map(|placed| placed.id)
                            .collect()
                    })
                    .unwrap_or_default()
            }),
            |id: &AreaId| id.clone(),
            move |id: AreaId| ghost_target(Node::area(Some(&building), layer, &id), frozen),
        )?
    };
    let picker = {
        let (listing, building) = (output.clone(), output);
        ReactiveList::with_style(
            whole(),
            move || {
                let picking = !frozen.get();
                let selected = session::selection().get();
                match (picking, selected) {
                    (true, Selection::Area(node))
                        if node.layer == layer
                            && node.output.as_deref() == Some(listing.as_str())
                            && placed_now(&node).is_some() =>
                    {
                        vec![node.area]
                    }
                    _ => Vec::new(),
                }
            },
            |id: &AreaId| id.clone(),
            move |id: AreaId| anchor_picker(Node::area(Some(&building), layer, &id)),
        )?
    };
    Ok(Box::new(passthrough(
        whole(),
        vec![samples, see_through(ghosts)?, see_through(picker)?],
    )?))
}

/// The sample cards and launcher of the screen `output`, under the stacks' boxes: each stack's column holds the samples [`layout::route_card`] sends it, and the launcher is in the middle of the screen where no stack opens it.
fn sample_cards(output: String) -> Built {
    let (listing, building, centring) = (output.clone(), output.clone(), output);
    let columns = ReactiveList::with_style(
        whole(),
        move || {
            reconcile::desktop(Some(&listing))
                .map(|desktop| {
                    stacks_of(&desktop)
                        .into_iter()
                        .map(|placed| (placed.layer, placed.id))
                        .collect()
                })
                .unwrap_or_default()
        },
        |held: &(LayerKind, AreaId)| held.clone(),
        move |(layer, id): (LayerKind, AreaId)| sample_column(&building, layer, id),
    )?;
    let launcher = ReactiveList::with_style(
        whole(),
        {
            let output = centring.clone();
            move || {
                let opened_by_a_stack = reconcile::desktop(Some(&output)).is_some_and(|desktop| {
                    stacks_of(&desktop).iter().any(|placed| placed.launcher)
                });
                match opened_by_a_stack {
                    true => Vec::new(),
                    false => card_samples::launcher().get().into_iter().collect(),
                }
            }
        },
        |_: &Launcher| (),
        move |launcher: Launcher| {
            let output = centring.clone();
            card_samples::centred_launcher(launcher, move || usable(Some(&output)))
        },
    )?;
    Ok(Box::new(passthrough(
        whole(),
        vec![Box::new(columns), Box::new(launcher)],
    )?))
}

fn sample_column(output: &str, layer: LayerKind, id: AreaId) -> Built {
    let node = Node::area(Some(output), layer, &id);
    let output = output.to_string();
    card_samples::column(
        move || shown_in(&output, layer, &id),
        move || placed_now(&node).map(|placed| (placed.column(), placed.anchor)),
        modules::stack::sample::preview_card,
    )
}

/// What the stack `id` of `layer` shows of the samples: the launcher where it is the one that opens it, and every sample card [`layout::route_card`] sends it, a critical one above the rest.
pub(crate) fn shown_in(output: &str, layer: LayerKind, id: &AreaId) -> Vec<Shown> {
    let Some(desktop) = reconcile::desktop(Some(output)) else {
        return Vec::new();
    };
    let stacks = stacks_of(&desktop);
    let here = |placed: &Placed| placed.layer == layer && placed.id == *id;
    let mut shown = Vec::new();
    if stacks
        .iter()
        .find(|placed| placed.launcher)
        .is_some_and(here)
        && let Some(launcher) = card_samples::launcher().get()
    {
        shown.push(Shown::Launcher(launcher));
    }
    let mut cards: Vec<Sample> = card_samples::samples()
        .get()
        .into_iter()
        .filter(|sample| {
            layout::route_card(&stacks, |placed| &placed.routes, &sample.routed()).is_some_and(here)
        })
        .collect();
    cards.sort_by_key(|sample| sample.urgency != Some(Urgency::Critical));
    shown.extend(cards.into_iter().map(Shown::Card));
    shown
}

/// The stack `node` names, as its screen shows it now.
fn placed_now(node: &Node) -> Option<Placed> {
    let desktop = reconcile::desktop(node.output.as_deref())?;
    stack_on(&desktop, node.layer, &node.area).ok()
}

/// Whether samples are up in the stack `node` names, which the box of its first card then leaves visible instead of washing over.
fn sampled(node: &Node) -> bool {
    node.output
        .as_deref()
        .is_some_and(|output| !shown_in(output, node.layer, &node.area).is_empty())
}

/// The box of one stack's first card: a press selects the stack, a secondary press opens its menu, and a drag carries it — pinned, as it goes, where [`landing`] says.
fn ghost_target(node: Node, frozen: RwSignal<bool>) -> Built {
    let theme = use_theme::<NordTheme>();
    let edit = Edit::new(telar::t!(
        "editor.overlay.moved",
        name = node.area.to_string()
    ));
    let box_of = {
        let node = node.clone();
        move || {
            placed_now(&node)
                .map(|placed| placed.ghost())
                .unwrap_or_default()
        }
    };
    let label = {
        let node = node.clone();
        move || {
            if sampled(&node) {
                return String::new();
            }
            let stacks: Vec<(AreaId, Vec<Route>)> = reconcile::desktop(node.output.as_deref())
                .map(|desktop| {
                    stacks_of(&desktop)
                        .into_iter()
                        .map(|placed| (placed.id, placed.routes))
                        .collect()
                })
                .unwrap_or_default();
            format!("{} · {}", node.area, takes(&stacks, &node.area))
        }
    };
    let area = {
        let node = node.clone();
        move || Some(node.clone())
    };
    let painting = node.clone();
    let previewing = edit.clone();
    let taking = node.clone();
    Ok(Box::new(gesture::drag(
        pressable(
            StyledContainer::new(
                LayoutStyle::new(),
                move |_| {
                    let wash = if sampled(&painting) { 0.0 } else { 0.12 };
                    RectStyle::filled(theme.accent.with_alpha(wash), ui::scale::corner::md())
                        .with_border(Border::uniform(theme.accent, 1.0))
                },
                vec![rows::note(label)?],
            )?
            .input_opaque(),
            area,
        )
        .styled_by(move || {
            surfaces::area::at(box_of())
                .padding_all(ui::scale::space::sm())
                .flex_column()
                .justify_content(JustifyContent::CENTER)
        })
        .cursor(Cursor::Grab),
        edit.transaction(),
        move |pressed| {
            let placed = placed_now(&taking)?;
            frozen.set(true);
            Some((pressed, placed))
        },
        move |(from, placed): &((f32, f32), Placed), _| {
            let (Some(point), Some(before), Some(desktop)) = (
                surfaces::menu::pointer(),
                previewing.transaction().before(),
                reconcile::desktop_now(node.output.as_deref()),
            ) else {
                return;
            };
            let at = placed.ghost();
            let (anchor, offset) = landing(
                placed.bounds,
                placed.width,
                (at.x + point.0 - from.0, at.y + point.1 - from.1),
            );
            let planned = moved(&before, &desktop, placed.layer, &placed.id, anchor, offset);
            let _ = previewing.preview(planned.unwrap_or_default());
        },
        move |_, _| frozen.set(false),
    )))
}

/// The nine anchors of the selected stack, the one it is pinned to filled: pressing one pins it there exactly.
fn anchor_picker(node: Node) -> Built {
    let (reading, placing) = (node.clone(), node.clone());
    anchor_dots(
        move || placed_now(&reading).map(|placed| placed.bounds),
        move || {
            placed_now(&placing)
                .filter(|placed| placed.offset == Offset::ZERO)
                .map(|placed| placed.anchor)
        },
        move |anchor| said(pin(&node, anchor)),
    )
}

/// The nine anchors of whatever `bounds` answers, each where [`pinned::point`] puts it and the one `pinned_to` answers filled; pressing one hands it to `pick`.
pub(crate) fn anchor_dots(
    bounds: impl Fn() -> Option<Rect> + Clone + 'static,
    pinned_to: impl Fn() -> Option<Anchor> + Clone + 'static,
    pick: impl Fn(Anchor) + Clone + 'static,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let dots = Anchor::ALL
        .into_iter()
        .map(|anchor| -> Built {
            let (bounds, pinned_to, pick) = (bounds.clone(), pinned_to.clone(), pick.clone());
            let dot = StyledContainer::new(
                LayoutStyle::new(),
                move |_| {
                    let fill = match pinned_to() == Some(anchor) {
                        true => theme.accent,
                        false => theme.surface,
                    };
                    RectStyle::filled(fill, DOT / 2.0)
                        .with_border(Border::uniform(theme.accent, 2.0))
                },
                Vec::new(),
            )?;
            Ok(Box::new(
                dot.styled_by(move || {
                    let (x, y) = pinned::point(bounds().unwrap_or_default(), anchor);
                    surfaces::area::at(Rect::new(x - DOT / 2.0, y - DOT / 2.0, DOT, DOT))
                })
                .cursor(Cursor::Pointer)
                .input_opaque()
                .on_press(move || pick(anchor)),
            ))
        })
        .collect::<Result<Vec<_>, LayoutError>>()?;
    Ok(Box::new(passthrough(whole(), dots)?))
}

/// A stack's popover rows: its anchor, width and screens, how far it is moved off its anchor, which cards it takes, whether volume and brightness and the launcher appear in it, and its width as a handle on the column.
fn stack_tool(draft: &AreaDraft) -> Result<Inspector, LayoutError> {
    let ResolvedAreaKind::Stack {
        anchor,
        offset,
        width,
        output_policy,
        routes,
        launcher,
    } = draft.resolved.kind.clone()
    else {
        return Ok(Inspector::default());
    };
    let mut list = chosen(
        draft,
        "anchor",
        label!("editor.area.anchor"),
        help("AreaKind::Stack", "anchor"),
        variants("Anchor"),
        kind_read!(Stack { anchor }, anchor),
        |area, anchor: Anchor| kind_field!(area, "stack", Stack { anchor }, anchor),
    )?;
    let wide = draft.setting(
        "width",
        "width",
        kind_read!(Stack { width }, width),
        |area, value: &f32| kind_field!(area, "stack", Stack { width }, *value),
    );
    list.push(draft.marked(
        &["width"],
        rows::number(
            label!("editor.area.width"),
            help("AreaKind::Stack", "width"),
            wide,
            Range::whole(crate::steps::WIDTHS.0, crate::steps::WIDTHS.1),
        )?,
    )?);
    list.extend(chosen(
        draft,
        "output_policy",
        label!("editor.area.output_policy"),
        help("AreaKind::Stack", "output_policy"),
        variants("StackOutputPolicy"),
        move |area| match &area.kind {
            ResolvedAreaKind::Stack {
                output_policy: now, ..
            } => now.clone(),
            _ => output_policy.clone(),
        },
        |area, policy: StackOutputPolicy| {
            kind_field!(area, "stack", Stack { output_policy }, policy)
        },
    )?);
    let reach = Range::whole(-3840.0, 3840.0);
    let offset_of = kind_read!(Stack { offset }, offset);
    let across = draft.setting(
        "offset.x",
        "offset",
        move |area| offset_of(area).x,
        move |area, x: &f32| {
            if let Some(stack) = stack_mut(area) {
                stack.offset.get_or_insert(offset).x = *x;
            }
        },
    );
    let down = draft.setting(
        "offset.y",
        "offset",
        move |area| offset_of(area).y,
        move |area, y: &f32| {
            if let Some(stack) = stack_mut(area) {
                stack.offset.get_or_insert(offset).y = *y;
            }
        },
    );
    list.push(draft.marked(
        &["offset"],
        rows::together(vec![
            rows::number(
                label!("editor.overlay.offset_x"),
                help("AreaKind::Stack", "offset"),
                across,
                reach,
            )?,
            rows::number(
                label!("editor.overlay.offset_y"),
                help("AreaKind::Stack", "offset"),
                down,
                reach,
            )?,
        ])?,
    )?);
    list.extend(route_rows(draft, routes)?);
    let opens = draft.setting(
        "launcher",
        "launcher",
        kind_read!(Stack { launcher }, launcher),
        |area, on: &bool| {
            if let Some(stack) = stack_mut(area) {
                *stack.launcher = Some(*on);
            }
        },
    );
    list.push(draft.marked(
        &["launcher"],
        rows::toggle(
            label!("editor.overlay.launcher_here"),
            help("AreaKind::Stack", "launcher"),
            opens,
        )?,
    )?);
    let before: Option<AreaId> =
        reconcile::desktop(draft.node.output.as_deref()).and_then(|desktop| {
            stacks_of(&desktop)
                .into_iter()
                .take_while(|placed| {
                    !(placed.layer == draft.node.layer && placed.id == draft.node.area)
                })
                .find(|placed| placed.launcher)
                .map(|placed| placed.id)
        });
    list.push(rows::note(move || match (opens.get(), &before) {
        (true, Some(first)) => {
            telar::t!("editor.overlay.launcher_first", first = first.to_string())
        }
        _ => String::new(),
    })?);
    Ok(Inspector {
        rows: list,
        handles: vec![width_handle(draft, wide)?],
    })
}

/// The routes rows: what the routing rule is, what this stack takes now, one block per route — its kind, app and urgency, moved earlier or later or taken away — a row to add one, and volume and brightness here or not.
fn route_rows(
    draft: &AreaDraft,
    seed: Vec<Route>,
) -> Result<Vec<Box<dyn telar::LayoutItem>>, LayoutError> {
    let routes: RwSignal<Vec<Route>> = draft.setting(
        "routes",
        "routes",
        kind_read!(Stack { routes }, seed),
        |area, routes: &Vec<Route>| {
            if let Some(stack) = stack_mut(area) {
                *stack.routes = routes.clone();
            }
        },
    );
    let screen: Vec<(LayerKind, AreaId, Vec<Route>)> =
        reconcile::desktop(draft.node.output.as_deref())
            .map(|desktop| {
                stacks_of(&desktop)
                    .into_iter()
                    .map(|placed| (placed.layer, placed.id, placed.routes))
                    .collect()
            })
            .unwrap_or_default();
    let (layer, id) = (draft.node.layer, draft.node.area.clone());
    let on_screen = move || -> Vec<(AreaId, Vec<Route>)> {
        screen
            .iter()
            .map(|(held_layer, held, held_routes)| {
                let routes = match *held_layer == layer && *held == id {
                    true => routes.get(),
                    false => held_routes.clone(),
                };
                (held.clone(), routes)
            })
            .collect()
    };
    let this = draft.node.area.clone();
    let reading = on_screen.clone();
    let mut list = vec![
        rows::heading(|| telar::t!("editor.overlay.cards"))?,
        rows::note(|| telar::t!("editor.overlay.routing"))?,
        rows::note(move || takes(&reading(), &this))?,
        rows::note(move || match rest_shown_nowhere(&on_screen()) {
            true => telar::t!("editor.overlay.rest_nowhere"),
            false => String::new(),
        })?,
    ];
    let shape = signal(0u64);
    let resetting = draft.clone();
    effect(move || {
        routes.with(|_| ());
        if resetting.is_resetting() {
            shape.update(|generation| *generation += 1);
        }
    });
    let mut block: Vec<Box<dyn telar::LayoutItem>> = Vec::new();
    let apps: Rc<[(String, String)]> =
        std::iter::once((String::new(), telar::t!("editor.overlay.any_app")))
            .chain(recent_apps().into_iter().map(|app| (app.clone(), app)))
            .collect();
    let blocks = ReactiveList::with_style(
        LayoutStyle::new().flex_column().gap(ui::scale::space::sm()),
        move || {
            let generation = shape.get();
            let count = routes.with(Vec::len);
            (0..count).map(|at| (generation, count, at)).collect()
        },
        |key: &(u64, usize, usize)| *key,
        move |(_, _, at): (u64, usize, usize)| route_block(routes, shape, at, Rc::clone(&apps)),
    )?;
    block.push(Box::new(blocks));
    block.push(rows::action(
        || telar::t!("editor.overlay.add_route"),
        move || {
            routes.update(|held| {
                held.push(Route {
                    kind: Some(CardKind::Notification),
                    ..Route::default()
                })
            });
            shape.update(|generation| *generation += 1);
        },
    )?);
    let osd = signal(routes.with(|held| shows_osd(held)));
    effect(move || {
        let wanted = osd.get();
        if routes.with(|held| shows_osd(held)) != wanted {
            routes.update(|held| *held = osd_toggled(std::mem::take(held)));
            shape.update(|generation| *generation += 1);
        }
    });
    effect(move || {
        let now = routes.with(|held| shows_osd(held));
        if osd.peek() != now {
            osd.set(now);
        }
    });
    block.push(rows::toggle(
        label!("editor.overlay.osd_here"),
        help("Route", "kind"),
        osd,
    )?);
    list.push(draft.marked(&["routes"], rows::together(block)?)?);
    Ok(list)
}

/// A value of route `at` that its control edits: its spelling in the file, empty for "any".
fn route_field<T: serde::Serialize + serde::de::DeserializeOwned + Clone + 'static>(
    routes: RwSignal<Vec<Route>>,
    at: usize,
    read: fn(&Route) -> Option<T>,
    write: fn(&mut Route, Option<T>),
) -> RwSignal<String> {
    let seed = routes.with(|held| held.get(at).and_then(read).map(|value| spelled(&value)));
    let value = signal(seed.unwrap_or_default());
    effect(move || {
        let text = value.get();
        let wanted = (!text.is_empty()).then(|| parsed::<T>(&text)).flatten();
        let differs = routes.with(|held| {
            held.get(at).is_some_and(|route| {
                read(route).map(|now| spelled(&now)) != wanted.as_ref().map(spelled)
            })
        });
        if differs {
            routes.update(|held| {
                if let Some(route) = held.get_mut(at) {
                    write(route, wanted);
                }
            });
        }
    });
    value
}

/// One route's block: its kind, the app it comes from (typed, or picked from the apps that sent the notifications still held), its urgency, and buttons that move it earlier or later or take it away.
fn route_block(
    routes: RwSignal<Vec<Route>>,
    shape: RwSignal<u64>,
    at: usize,
    apps: Rc<[(String, String)]>,
) -> Built {
    let kinds: Rc<[(String, String)]> = Rc::from(vec![
        (String::new(), telar::t!("editor.overlay.any_card")),
        (
            spelled(&CardKind::Notification),
            telar::t!("editor.overlay.kinds.notification"),
        ),
        (
            spelled(&CardKind::Toast),
            telar::t!("editor.overlay.kinds.toast"),
        ),
        (
            spelled(&CardKind::Osd),
            telar::t!("editor.overlay.kinds.osd"),
        ),
    ]);
    let urgencies: Rc<[(String, String)]> = Rc::from(vec![
        (String::new(), telar::t!("editor.overlay.any_urgency")),
        (
            spelled(&Urgency::Low),
            telar::t!("editor.overlay.urgencies.low"),
        ),
        (
            spelled(&Urgency::Normal),
            telar::t!("editor.overlay.urgencies.normal"),
        ),
        (
            spelled(&Urgency::Critical),
            telar::t!("editor.overlay.urgencies.critical"),
        ),
    ]);
    let kind = route_field(
        routes,
        at,
        |route| route.kind,
        |route, kind| route.kind = kind,
    );
    let urgency = route_field(
        routes,
        at,
        |route| route.urgency,
        |route, urgency| route.urgency = urgency,
    );
    let app = signal(routes.with(|held| {
        held.get(at)
            .and_then(|route| route.app.clone())
            .unwrap_or_default()
    }));
    effect(move || {
        let typed = app.get();
        let wanted = (!typed.trim().is_empty()).then(|| typed.trim().to_string());
        if routes.with(|held| held.get(at).is_some_and(|route| route.app != wanted)) {
            routes.update(|held| {
                if let Some(route) = held.get_mut(at) {
                    route.app = wanted;
                }
            });
        }
    });
    let reorder = move |to: usize| {
        routes.update(|held| {
            if to < held.len() && at < held.len() {
                held.swap(at, to);
            }
        });
        shape.update(|generation| *generation += 1);
    };
    let count = routes.with(Vec::len);
    let mut buttons = Vec::new();
    if at > 0 {
        buttons.push(rows::action(
            || telar::t!("editor.overlay.earlier"),
            move || reorder(at - 1),
        )?);
    }
    if at + 1 < count {
        buttons.push(rows::action(
            || telar::t!("editor.overlay.later"),
            move || reorder(at + 1),
        )?);
    }
    buttons.push(rows::action(
        || telar::t!("editor.overlay.remove_route"),
        move || {
            routes.update(|held| {
                if at < held.len() {
                    held.remove(at);
                }
            });
            shape.update(|generation| *generation += 1);
        },
    )?);
    let number = at + 1;
    let mut block = vec![
        rows::heading(move || telar::t!("editor.overlay.route", number = number.to_string()))?,
        rows::listed(
            label!("editor.overlay.kind"),
            help("Route", "kind"),
            kind,
            kinds,
        )?,
        rows::text(label!("editor.overlay.app"), help("Route", "app"), app)?,
    ];
    if apps.len() > 1 {
        block.push(rows::listed(
            label!("editor.overlay.app_recent"),
            None,
            app,
            apps,
        )?);
    }
    block.push(rows::listed(
        label!("editor.overlay.urgency"),
        help("Route", "urgency"),
        urgency,
        urgencies,
    )?);
    block.push(Box::new(StyledContainer::new(
        LayoutStyle::new().flex_row().gap(ui::scale::space::xs()),
        |_| RectStyle::default(),
        buttons,
    )?));
    Ok(Box::new(StyledContainer::new(
        LayoutStyle::new().flex_column().gap(ui::scale::space::xs()),
        |_| RectStyle::filled(Color::TRANSPARENT, 0.0),
        block,
    )?))
}

/// Where the width handle of a column at `column`, pinned at `anchor`, sits when the column is `width` wide.
pub(crate) fn width_point(column: Rect, anchor: Anchor, width: f32) -> (f32, f32) {
    let (side, down) = pinned::sides(anchor);
    let card = ghost(column, pinned::anchor_of(side, down));
    let y = card.y + card.height / 2.0;
    match side {
        Side::Start => (column.x + width, y),
        Side::Middle => (column.x + column.width / 2.0 + width / 2.0, y),
        Side::End => (column.x + column.width - width, y),
    }
}

/// On the column's side away from where it is pinned across, dragged across: the column is as wide as it is dragged, and one pinned to the middle grows both ways.
fn width_handle(draft: &AreaDraft, value: RwSignal<f32>) -> Built {
    let anchor: RwSignal<String> = draft
        .shared("anchor")
        .unwrap_or_else(|| signal(String::new()));
    let seed = match draft.resolved.kind {
        ResolvedAreaKind::Stack { anchor, .. } => anchor,
        _ => Anchor::default(),
    };
    let across = move || {
        let anchor = anchor
            .with(|spelled| parsed::<Anchor>(spelled))
            .unwrap_or(seed);
        pinned::sides(anchor)
    };
    let (reading, placing) = (draft.node.clone(), draft.node.clone());
    let hd = crate::modes::gesture::HandleDragging::new();
    let to_value = Rc::new(hd.wrap_to_value(move |x: f32, _y: f32| {
        let column = surfaces::rects::rect(&reading).unwrap_or_default();
        match across().0 {
            Side::Start => x - column.x,
            Side::Middle => 2.0 * (x - (column.x + column.width / 2.0)).abs(),
            Side::End => column.x + column.width - x,
        }
    }));
    let to_point = Rc::new(move |width: f32| {
        let column = surfaces::rects::rect(&placing).unwrap_or_default();
        let (side, down) = across();
        width_point(column, pinned::anchor_of(side, down), width)
    });
    telar::handle(
        telar::HandleProps::props()
            .value(value)
            .to_value(to_value)
            .to_point(to_point)
            .min(crate::steps::WIDTHS.0)
            .max(crate::steps::WIDTHS.1)
            .step(1.0)
            .cursor(Cursor::EwResize)
            .transaction(hd.transaction(value))
            .build(),
        Children::default(),
    )
}
