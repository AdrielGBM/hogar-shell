//! Customization popovers (DEC-3, TA-4): a card beside the real item that edits it, and handles on the item itself.
//!
//! **One transaction.** A popover is one [`Edit`] handed to telar's [`register_transaction`]: every row and handle previews into it live, Esc puts the layout back exactly as it was when the popover opened, and Enter, a click outside, Done or any other way of closing it records what it previewed as one entry in the history (F-7).
//!
//! **Where it is drawn.** A popover is a transient laid over the whole window its item is drawn in (F-2.3, DEC-9), so its card and its handles are nodes of the same window as the item and appear on the next frame, with nothing mapped. In an edit mode that window is under the mode's host, so there the popover is drawn in the host's own window, above it.
//!
//! **What it shows.** An area's popover is its kind's tools and then what every area has; an instance's is generated from its module's options (F-3.2). Either is extended by adding tools ([`add_area_tool`], [`add_instance_tool`]), each giving rows for the card and handles for the item, which share values by name through the draft.

pub(crate) mod area;
pub(crate) mod bindings;
mod draft;
pub mod handles;
mod instance;
pub(crate) mod origin;
pub(crate) mod place;
pub mod rows;
pub(crate) mod value;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use telar::{
    AlignItems, Children, Container, JustifyContent, LayoutError, LayoutItem, LayoutScrollArea,
    LayoutStyle, OwnerId, RectStyle, RwSignal, SizeDimension, StyledContainer, Text, box_item,
    effect, on_cleanup, register_transaction, signal, use_theme,
};

use config::Edge;
use config::theme::{FontRole, NordTheme};
use layout::{LayerKind, LayoutStore, ResolvedArea};
use platform_wayland::KeyboardMode;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node, Part};
use surfaces::transient::{self, Anchor, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;

use crate::mode;
use crate::session::{self, Edit, EditError, Selection};
use crate::written::Written;

use rows::label;

pub use area::{help, parsed, spelled};
pub use draft::{AreaDraft, InstanceDraft, Settle};
pub(crate) use draft::{kind_field, kind_read};
pub use instance::{option, shown};
pub use origin::Provenance;
pub use value::{Path, Step, path_of};

/// The transient every popover is, one at a time.
pub const ID: &str = "editor:popover";

/// How wide a popover's card is.
const WIDTH: f32 = 360.0;
/// The most of the usable height a card takes before its rows scroll.
const TALLEST: f32 = 0.7;

/// What one tool adds to a popover: rows for its card, and handles laid over the item it customizes.
#[derive(Default)]
pub struct Inspector {
    pub rows: Vec<Box<dyn LayoutItem>>,
    pub handles: Vec<Box<dyn LayoutItem>>,
}

/// What a tool for areas of one kind adds to their popovers.
pub type AreaTool = fn(&AreaDraft) -> Result<Inspector, LayoutError>;
/// What a tool for instances adds to their popovers.
pub type InstanceTool = fn(&InstanceDraft) -> Result<Inspector, LayoutError>;

thread_local! {
    static AREA_TOOLS: RefCell<Vec<(&'static str, AreaTool)>> = const { RefCell::new(Vec::new()) };
    static INSTANCE_TOOLS: RefCell<Vec<InstanceTool>> = const { RefCell::new(Vec::new()) };
    static SETTLES: RefCell<Vec<(&'static str, Settle)>> = const { RefCell::new(Vec::new()) };
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static SERIAL: Cell<u64> = const { Cell::new(0) };
}

/// Adds `tool` to the popover of every area of the kind `kind` (`bar`, `grid`, … as the layout file spells it), after the tools added before it and before the rows every area has.
pub fn add_area_tool(kind: &'static str, tool: AreaTool) {
    AREA_TOOLS.with(|tools| tools.borrow_mut().push((kind, tool)));
}

/// Adds `tool` to every instance's popover, after its generated rows and the tools added before it.
pub fn add_instance_tool(tool: InstanceTool) {
    INSTANCE_TOOLS.with(|tools| tools.borrow_mut().push(tool));
}

/// Makes every change an instance's popover previews for an instance in an area of the kind `kind` bring `settle` with it: the operations it adds to the layout the change leaves, such as the widgets a grown one now covers moving out of its way.
pub fn settle_instances_in(kind: &'static str, settle: Settle) {
    SETTLES.with(|settles| settles.borrow_mut().push((kind, settle)));
}

pub(crate) fn install() {
    area::install();
    add_instance_tool(bindings::tool);
}

/// The popover that is open.
struct Open {
    serial: u64,
    node: Node,
    edit: Edit,
    open: RwSignal<bool>,
    lease: Rc<Lease>,
    subject: Subject,
    /// Builds the popover's tree, which the transient does in the window it is drawn in.
    tree: Rc<dyn Fn() -> Built>,
}

/// What a popover holds — its edit, its registration on the dismiss stack, the values its controls share — owned apart from its tree, so a rebuilt tree keeps them; let go once the popover is closed and no tree of it is left to read them.
struct Lease {
    owner: OwnerId,
    trees: Cell<usize>,
    closed: Cell<bool>,
    released: Cell<bool>,
}

impl Lease {
    fn release_when_done(&self) {
        if self.closed.get() && self.trees.get() == 0 && !self.released.replace(true) {
            telar::dispose_owner(self.owner);
        }
    }
}

#[derive(Clone)]
enum Subject {
    Area(AreaDraft),
    Instance(InstanceDraft),
}

/// Opens the popover for what `selection` names: an instance's for an instance, its area's for an area or a group.
pub fn open_for(selection: &Selection) -> Result<(), EditError> {
    match selection {
        Selection::None => Err(EditError::nothing()),
        Selection::Area(node) | Selection::Group(node) => {
            open_area(Node::area(node.output.as_deref(), node.layer, &node.area))
        }
        Selection::Instance(node) => open_instance(node.clone()),
    }
}

/// Opens the popover of the area `node` names, inside an edit mode or outside one, closing (and so committing) whichever popover was open.
pub fn open_area(node: Node) -> Result<(), EditError> {
    open(
        Node::area(node.output.as_deref(), node.layer, &node.area),
        false,
    )
}

/// Opens the popover of the instance `node` names — or, for a child of a komponent a group draws, which only the komponent's file writes, its area's, where the use's parameters are set.
pub fn open_instance(node: Node) -> Result<(), EditError> {
    let Part::Instance(_, id) = &node.part else {
        return Err(EditError::nothing());
    };
    if id.komponent_child().is_some() {
        return open_area(node);
    }
    open(node, true)
}

/// Closes the popover that is open, keeping what it changed.
pub fn close() {
    if let Some(serial) = OPEN.with(|open| open.borrow().as_ref().map(|open| open.serial)) {
        finish(serial);
    }
}

/// What the open popover customizes.
pub fn current() -> Option<Node> {
    OPEN.with(|open| open.borrow().as_ref().map(|open| open.node.clone()))
}

/// The open popover's own open state, read reactively: what its expression fields gate their readings on. `None` while no popover is open.
pub(crate) fn showing() -> Option<RwSignal<bool>> {
    OPEN.with(|open| open.borrow().as_ref().map(|open| open.open))
}

/// The value called `name` that the open area popover's rows and handles share ([`AreaDraft::value`]): a corner radius, a handle's clamped state. `None` while no area popover is open, or none of its tools made one by that name and type.
pub fn shared<T: 'static>(name: &str) -> Option<RwSignal<T>> {
    OPEN.with(|open| match &open.borrow().as_ref()?.subject {
        Subject::Area(draft) => draft.shared(name),
        Subject::Instance(_) => None,
    })
}

/// Whether the open area popover has a control for the value called `name` ([`AreaDraft::edits`]): what an operation the key table counts as reached through it is checked against ([`crate::keys::CUSTOMIZED`]).
pub fn edits(name: &str) -> bool {
    OPEN.with(
        |open| match open.borrow().as_ref().map(|open| &open.subject) {
            Some(Subject::Area(draft)) => draft.edits(name),
            _ => false,
        },
    )
}

fn open(node: Node, of_instance: bool) -> Result<(), EditError> {
    close();
    if session::open().is_some() {
        return Err(EditError::Nested);
    }
    if surfaces::layouts::read(LayoutStore::is_safe).unwrap_or(false) {
        return Err(EditError::Safe);
    }
    let editing_here =
        mode::current().filter(|mode| node.output.as_deref() == Some(mode.output.as_str()));
    if node.layer == LayerKind::Lock
        && editing_here.as_ref().map(|mode| mode.layer) != Some(LayerKind::Lock)
    {
        return Err(EditError::Refused(telar::t!("editor.popover.lock")));
    }
    let desktop =
        reconcile::desktop(node.output.as_deref()).ok_or_else(|| EditError::gone(&node.area))?;
    let area = desktop
        .resolved
        .area(node.layer, &node.area)
        .cloned()
        .ok_or_else(|| EditError::gone(&node.area))?;
    let layout = session::draft().peek();
    let workspace = crate::variant::applies_to(&node)
        .then(crate::variant::editing)
        .flatten();
    let written = Written::area(
        &layout,
        node.output.as_deref(),
        node.layer,
        &node.area,
        workspace.as_ref(),
    )
    .map_err(EditError::Refused)?;

    let serial = SERIAL.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    let owner = telar::detached(|| telar::owner_scope().id());
    let built = telar::with_owner(Some(owner), || {
        subject(&node, of_instance, &area, &desktop, written).map(|(make, name)| {
            let edit = Edit::new(telar::t!("editor.popover.customize", name = name.clone()));
            let open = signal(false);
            register_transaction(open, edit.transaction());
            (make(&edit), name, edit, open)
        })
    });
    let (subject, name, edit, open) = match built {
        Ok(built) => built,
        Err(why) => {
            telar::dispose_owner(owner);
            return Err(why);
        }
    };
    let edge = area.kind.edge();
    let lease = Rc::new(Lease {
        owner,
        trees: Cell::new(0),
        closed: Cell::new(false),
        released: Cell::new(false),
    });
    let (shown, leased) = (subject.clone(), Rc::clone(&lease));
    let tree: Rc<dyn Fn() -> Built> =
        Rc::new(move || content(serial, &leased, &shown, &name, edge, open));
    OPEN.with(|held| {
        *held.borrow_mut() = Some(Open {
            serial,
            node: node.clone(),
            edit,
            open,
            lease,
            subject,
            tree: Rc::clone(&tree),
        })
    });

    let window = match editing_here {
        Some(_) => LayerKind::Overlay,
        None => surfaces::layer_window::window_of(node.layer, &area),
    };
    let anchor = Anchor {
        output: node.output.clone(),
        layer: window,
        edge: area.kind.edge().unwrap_or(Edge::Top),
        rect: rects::rect(&node).unwrap_or_default(),
        chrome: Chrome::global(desktop.config.clone(), node.output.clone()),
        gap: place::GAP,
    };
    transient::open(
        Spec::new(
            ID,
            Place::Over(anchor),
            Rc::new(move |_: &Chrome| tree_of(serial)),
        )
        .output(node.output.clone())
        .keyboard(KeyboardMode::Exclusive)
        .dismiss_on_outside()
        .on_close(move || finish(serial)),
    );
    Ok(())
}

type Made = Box<dyn FnOnce(&Edit) -> Subject>;

/// What the popover of `node` edits, made once its edit exists, and what its title calls it.
fn subject(
    node: &Node,
    of_instance: bool,
    area: &ResolvedArea,
    desktop: &Desktop,
    written: Written,
) -> Result<(Made, String), EditError> {
    let (node, area, config, screen) = (
        node.clone(),
        area.clone(),
        desktop.config.clone(),
        desktop.size,
    );
    if !of_instance {
        let name = area.id.to_string();
        let made: Made = Box::new(move |edit| {
            Subject::Area(AreaDraft::new(edit, node, area, config, screen, written))
        });
        return Ok((made, name));
    }
    let Part::Instance(group, id) = node.part.clone() else {
        return Err(EditError::nothing());
    };
    let resolved = area
        .groups
        .iter()
        .find(|held| held.id == group)
        .and_then(|held| held.children.iter().find(|child| child.id == id.template()))
        .cloned()
        .ok_or_else(|| EditError::gone(&node.area))?;
    let name = ui::descriptor::find(&resolved.module)
        .map_or_else(|| resolved.module.clone(), |module| module.name.to_string());
    let shown = instance::shown(&config, &resolved.module, &resolved.options);
    let kind = area.kind.name();
    let settle = SETTLES.with(|settles| {
        settles
            .borrow()
            .iter()
            .find(|(of, _)| *of == kind)
            .map(|(_, settle)| (*settle, desktop.clone()))
    });
    let made: Made = Box::new(move |edit| {
        Subject::Instance(InstanceDraft::new(
            edit,
            node,
            resolved,
            kind,
            shown,
            written.instance(&group, &id.template()),
            settle,
        ))
    });
    Ok((made, name))
}

/// Closes the popover `serial` opened, if it is still the one open: keeps what it previewed unless it was reverted already, and lets go of everything it held.
fn finish(serial: u64) {
    let Some(open) = OPEN.with(|held| {
        let mut held = held.borrow_mut();
        match held.as_ref().is_some_and(|open| open.serial == serial) {
            true => held.take(),
            false => None,
        }
    }) else {
        return;
    };
    open.open.set(false);
    if open.edit.is_open()
        && let Err(why) = open.edit.commit()
    {
        tracing::warn!("the popover's change was not kept: {why}");
    }
    transient::close(ID);
    open.lease.closed.set(true);
    open.lease.release_when_done();
}

fn still_open(serial: u64) -> bool {
    OPEN.with(|held| {
        held.borrow()
            .as_ref()
            .is_some_and(|open| open.serial == serial)
    })
}

/// The popover's whole tree: the handles over the item, and the card beside it.
fn content(
    serial: u64,
    lease: &Rc<Lease>,
    subject: &Subject,
    name: &str,
    edge: Option<Edge>,
    open: RwSignal<bool>,
) -> Built {
    // Opened here rather than when the popover is asked for: the transient puts its own dismiss entry on the stack as it builds, and the transaction's must be above it for Esc to reach the transaction first.
    if !open.peek() {
        open.set(true);
    }
    let was_open = Cell::new(false);
    effect(move || {
        let now = open.get();
        if was_open.replace(now) && !now && still_open(serial) {
            transient::close(ID);
        }
    });
    lease.trees.set(lease.trees.get() + 1);
    let leased = Rc::clone(lease);
    on_cleanup(move || {
        if !transient::is_open(ID) {
            finish(serial);
        }
        leased.trees.set(leased.trees.get() - 1);
        leased.release_when_done();
    });

    let (inspector, node) = match subject {
        Subject::Area(draft) => (area_inspector(draft)?, draft.node.clone()),
        Subject::Instance(draft) => (instance_inspector(draft)?, draft.node.clone()),
    };
    let handles = crate::host::passthrough(crate::host::whole(), inspector.handles)?;
    let card = card(name, inspector.rows, node, edge, open)?;
    Ok(Box::new(crate::host::passthrough(
        crate::host::whole(),
        vec![Box::new(handles), card],
    )?))
}

fn area_inspector(draft: &AreaDraft) -> Result<Inspector, LayoutError> {
    let tools: Vec<AreaTool> = AREA_TOOLS.with(|tools| {
        tools
            .borrow()
            .iter()
            .filter(|(kind, _)| *kind == draft.kind())
            .map(|(_, tool)| *tool)
            .collect()
    });
    let mut whole = Inspector::default();
    for tool in tools {
        let Inspector { rows, handles } = tool(draft)?;
        whole.rows.extend(rows);
        whole.handles.extend(handles);
    }
    whole.rows.extend(area::common(draft)?);
    Ok(whole)
}

fn instance_inspector(draft: &InstanceDraft) -> Result<Inspector, LayoutError> {
    let mut whole = Inspector {
        rows: instance::rows(draft)?,
        handles: Vec::new(),
    };
    let tools: Vec<InstanceTool> = INSTANCE_TOOLS.with(|tools| tools.borrow().clone());
    for tool in tools {
        let Inspector { rows, handles } = tool(draft)?;
        whole.rows.extend(rows);
        whole.handles.extend(handles);
    }
    whole.rows.extend(area::variant_rows(&draft.node)?);
    Ok(whole)
}

/// The card: what it customizes and a Done button, over a column of rows that scrolls when there are more than fit.
fn card(
    name: &str,
    rows: Vec<Box<dyn LayoutItem>>,
    node: Node,
    edge: Option<Edge>,
    open: RwSignal<bool>,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let pad = ui::scale::space::lg();
    let radius = ui::scale::corner::xl();
    let name = name.to_string();
    let title = Text::new(
        move || telar::t!("editor.popover.customize", name = name.clone()),
        LayoutStyle::new().flex_grow(1.0),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?;
    let done = telar::button(
        telar::ButtonProps::props()
            .label(label!("editor.done"))
            .on_press(Rc::new(move || open.set(false)))
            .build(),
        Children::default(),
    )?;
    let header = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .width(SizeDimension::Percent(1.0)),
        vec![box_item(title), done],
    )?;
    let output = node.output.clone();
    let inner = WIDTH - 2.0 * pad;
    let gap = ui::scale::space::md();
    let header_height = telar::track_layout(header.layout_node())
        .ok_or_else(|| LayoutError::Engine("a popover's header has no layout node".to_string()))?;
    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::sm())
            .width(inner),
        rows,
    )?;
    let rows_room = {
        let output = output.clone();
        move || {
            let usable = crate::host::usable(output.as_deref());
            let card = (usable.height * TALLEST).min(usable.height - 2.0 * place::GAP);
            (card - header_height.get().height - gap - 2.0 * pad).max(0.0)
        }
    };
    let (rows_box, viewport) = capped_rows(box_item(column), inner, rows_room)?;
    keep_focus_in_view(viewport);
    let body = StyledContainer::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_top(0.0)
            .width(WIDTH)
            .flex_column()
            .gap(gap)
            .padding_all(pad),
        move |_| RectStyle::filled(theme.surface, radius),
        vec![box_item(header), rows_box],
    )?
    .input_opaque()
    .with_transform(move |laid| {
        let item = rects::rect(&node).unwrap_or_default();
        let (x, y) = place::card_at(
            item,
            edge,
            (laid.width, laid.height),
            crate::host::usable(output.as_deref()),
        );
        Some([1.0, 0.0, 0.0, 1.0, x - laid.x, y - laid.y])
    });
    Ok(Box::new(body))
}

/// `content` in a scroll area `width` wide, in a box as tall as `content` up to what `room` answers: the rows of a card whose height is capped, filling what the cap leaves them and scrolling past it. A scroll area has no height of its own to give a column, so a card that left its rows to flex inside an auto-height column would lay them out below itself, out of reach of the pointer.
pub(crate) fn capped_rows(
    content: Box<dyn LayoutItem>,
    width: f32,
    room: impl Fn() -> f32 + 'static,
) -> Result<(Box<dyn LayoutItem>, telar::ScrollViewport), LayoutError> {
    let tall = telar::track_layout(content.layout_node())
        .ok_or_else(|| LayoutError::Engine("a card's rows have no layout node".to_string()))?;
    let viewport = Rc::new(RefCell::new(None));
    let scroll = {
        let captured = Rc::clone(&viewport);
        LayoutScrollArea::new_with(
            LayoutStyle::new()
                .width(width)
                .height(SizeDimension::Percent(1.0)),
            move |scrolling| {
                *captured.borrow_mut() = Some(scrolling);
                Ok(content)
            },
        )?
    };
    let viewport = viewport
        .take()
        .ok_or_else(|| LayoutError::Engine("a scroll area handed out no viewport".to_string()))?;
    let fitted = move || {
        LayoutStyle::new()
            .width(width)
            .height(tall.get().height.min(room()))
    };
    let rows = StyledContainer::new(fitted(), |_| RectStyle::default(), vec![Box::new(scroll)])?
        .styled_by(fitted);
    Ok((Box::new(rows), viewport))
}

/// Scrolls the card's rows so the one holding keyboard focus is on screen, which moving focus by Tab or arrows would otherwise leave behind the clip.
fn keep_focus_in_view(viewport: telar::ScrollViewport) {
    effect(move || {
        let Some(focused) = telar::focus::current() else {
            return;
        };
        let Some(row) = telar::focus::exposed()
            .into_iter()
            .find(|at| at.id == focused)
        else {
            return;
        };
        let inside = telar::enclosing_scroll_viewport(row.node)
            .is_some_and(|found| found.area() == viewport.area());
        if inside {
            viewport.reveal(row.node, place::GAP);
        }
    });
}

/// The tree of the popover `serial` opened, or nothing once it is closed.
fn tree_of(serial: u64) -> Built {
    let tree = OPEN.with(|held| {
        held.borrow()
            .as_ref()
            .filter(|open| open.serial == serial)
            .map(|open| Rc::clone(&open.tree))
    });
    match tree {
        Some(tree) => tree(),
        None => Ok(Box::new(crate::host::passthrough(
            crate::host::whole(),
            Vec::new(),
        )?)),
    }
}

#[cfg(test)]
pub(crate) fn tree() -> Option<Built> {
    let serial = OPEN.with(|held| held.borrow().as_ref().map(|open| open.serial))?;
    Some(tree_of(serial))
}

#[cfg(test)]
pub(crate) fn area_draft() -> Option<AreaDraft> {
    OPEN.with(|held| match &held.borrow().as_ref()?.subject {
        Subject::Area(draft) => Some(draft.clone()),
        Subject::Instance(_) => None,
    })
}

/// The open instance popover's draft.
#[cfg(test)]
pub(crate) fn instance_draft() -> Option<InstanceDraft> {
    OPEN.with(|held| match &held.borrow().as_ref()?.subject {
        Subject::Instance(draft) => Some(draft.clone()),
        Subject::Area(_) => None,
    })
}

/// Why Remove does not take back an expression `writer` wrote: the level the popover writes comes before it, or it is a komponent's own, so taking it back there would change nothing on screen.
pub(crate) fn beyond(writer: &layout::Origin) -> String {
    match writer {
        layout::Origin::Level(level) => telar::t!(
            "editor.expr.beyond",
            rule = level.rule(),
            file = level.file()
        ),
        layout::Origin::Komponent(_) => {
            telar::t!("editor.expr.beyond_komponent", file = writer.file())
        }
    }
}
