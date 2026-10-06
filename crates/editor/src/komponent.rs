//! Komponents in the editor (TA-6): saving a group as one, detaching a use back into instances of its own, the context-menu rows that reach both, and putting a komponent from the library into a group ([`plan_use`], which the palette and `komponent use` share).
//!
//! **Using** never replaces what a group holds: a komponent goes into a new group with a readable id made from its name — the end zone of a bar, the free cells of a grid — or into a group that holds nothing, and a group that holds modules or draws a komponent already is refused. The palette lists the library under "Komponents", filtered by the same checks the layout makes ([`offered`]); a pointer drop, a pick and a press, and Enter all put it through [`crate::modes::desktop::put`] as one undo entry. Only grids have a palette; a bar's menu puts one at the end of the zone picked from it ([`bar_entry`], [`add_to_bar`]) as `komponent use --zone` does, and docks and free areas take one from `komponent use` alone.
//!
//! **Saving** opens a card beside the group: the komponent's name, and a switch for each value of the group a parameter can be made of — an option an expression can spell, or a binding ([`layout::components::candidates`]). Save writes `components/<name>.toml` through the store, which owns the file, and makes the group draw it, as one undo entry; Cancel, Esc or a press outside changes nothing. **Detaching** is one menu row and one undo entry. What a use sets its parameters to is edited in its area's popover ([`crate::popover::area::parameter_rows`]).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use telar::{
    AlignItems, Container, LayoutStyle, MenuEntry, Reactive, RectStyle, RwSignal, StyledContainer,
    Text, box_item, signal, use_theme,
};

use config::Edge;
use config::theme::{FontRole, NordTheme};
use layout::components::{self, Candidate};
use layout::{
    AreaId, Catalogue, Expr, Group, GroupId, GroupKind, KomponentId, LayerKind, Layout, LayoutOp,
    Library, Resolved, ResolvedArea, ResolvedAreaKind, ResolvedGroup, UseError, WorkspaceMatch,
    Zone,
};
use platform_wayland::KeyboardMode;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node};
use surfaces::transient::{self, Anchor, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::Built;

use crate::mode::said;
use crate::modes::desktop;
use crate::modes::grid::Cells;
use crate::popover::{self, place, rows};
use crate::session::{self, EditError};
use crate::written::{Work, Written, known};

/// The transient the save card is, one at a time.
pub const ID: &str = "editor:komponent";

const WIDTH: f32 = 340.0;

/// The save card that is open: the group it saves, as its files write it ([`crate::written::as_written`]), and what the card's controls hold.
struct Saving {
    /// The group's area on its screen.
    area: Node,
    group: GroupId,
    drawn: ResolvedGroup,
    edge: Option<Edge>,
    name: RwSignal<String>,
    offered: Vec<(Candidate, RwSignal<bool>)>,
    /// Why the last Save was refused.
    said: RwSignal<Option<String>>,
    /// What holds the signals above: let go once the card is closed and no tree of it is left to read them.
    owner: telar::OwnerId,
    trees: Cell<usize>,
    closed: Cell<bool>,
    released: Cell<bool>,
}

impl Saving {
    fn release_when_done(&self) {
        if self.closed.get() && self.trees.get() == 0 && !self.released.replace(true) {
            telar::dispose_owner(self.owner);
        }
    }
}

thread_local! {
    static SAVING: RefCell<Option<Rc<Saving>>> = const { RefCell::new(None) };
}

/// The rows the menu of an instance of `group` gets for komponents: for a use, what its parameters are set to and detaching it; for any other group, saving it as one.
pub(crate) fn rows(area: &ResolvedArea, node: &Node, group: &ResolvedGroup) -> Vec<MenuEntry> {
    let area_node = Node::area(node.output.as_deref(), node.layer, &area.id);
    let Some(used) = &group.komponent else {
        let (at, id) = (area_node, group.id.clone());
        return vec![MenuEntry::row(
            telar::t!("editor.komponent.save"),
            "",
            move || said(open_save(&at, &id)),
        )];
    };
    let mut entries = Vec::new();
    let name = used.id.to_string();
    if !used.parameters.is_empty() {
        let at = area_node.clone();
        entries.push(MenuEntry::row(
            telar::t!("editor.komponent.parameters_of", komponent = name.clone()),
            "",
            move || said(popover::open_area(at.clone())),
        ));
    }
    let (at, id) = (area_node, group.id.clone());
    entries.push(MenuEntry::row(
        telar::t!("editor.komponent.detach", komponent = name.clone()),
        "",
        move || said(detach(&at, &id, &name)),
    ));
    entries
}

/// Turns the komponent the group `group` of the area `area` names draws back into instances of its own, where the edited level makes it draw one, as one undo entry ([`layout::components::detach`]).
pub fn detach(area: &Node, group: &GroupId, name: &str) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(area.output.as_deref()).ok_or_else(EditError::no_output)?;
    let layout = session::draft().peek();
    let written = Written::area(
        &layout,
        area.output.as_deref(),
        area.layer,
        &area.area,
        crate::variant::editing().as_ref(),
    )
    .map_err(EditError::Refused)?;
    let ops = components::detach(
        &layout,
        &known(),
        (&written.site, &area.area, group),
        (
            &desktop.resolved.output,
            desktop.resolved.workspace.as_ref(),
        ),
    )
    .map_err(|why| EditError::Refused(why.message()))?;
    crate::context::commit(
        telar::t!("editor.komponent.detached", komponent = name),
        ops,
    )
}

/// Opens the card that saves the group `group` of the area `area` names as a komponent, closing any popover or other card first.
pub fn open_save(area: &Node, group: &GroupId) -> Result<(), EditError> {
    popover::close();
    close();
    if surfaces::layouts::read(layout::LayoutStore::is_safe).unwrap_or(false) {
        return Err(EditError::Safe);
    }
    let desktop =
        reconcile::desktop_now(area.output.as_deref()).ok_or_else(EditError::no_output)?;
    let resolved = desktop
        .resolving(&session::draft().peek(), &known())
        .resolved
        .area(area.layer, &area.area)
        .cloned()
        .ok_or_else(|| EditError::gone(&area.area))?;
    let drawn = resolved
        .groups
        .iter()
        .find(|held| held.id == *group)
        .cloned()
        .ok_or_else(|| EditError::gone(&area.area))?;
    if let Some(used) = &drawn.komponent {
        return Err(EditError::Refused(util::message!(
            "editor.komponent.already",
            komponent = used.id.to_string()
        )));
    }
    let catalogue = surfaces::catalogue::Descriptors::installed();
    let offered = components::candidates(&drawn, &|module, key| {
        catalogue.binding_type(module, key).ok()
    });
    let owner = telar::detached(|| telar::owner_scope().id());
    let saving = telar::with_owner(Some(owner), || Saving {
        area: area.clone(),
        group: group.clone(),
        name: signal(free_name(group)),
        offered: offered
            .into_iter()
            .map(|candidate| (candidate, signal(false)))
            .collect(),
        said: signal(None),
        edge: resolved.kind.edge(),
        drawn,
        owner,
        trees: Cell::new(0),
        closed: Cell::new(false),
        released: Cell::new(false),
    });
    SAVING.with(|held| *held.borrow_mut() = Some(Rc::new(saving)));
    let window = match crate::mode::current()
        .is_some_and(|mode| area.output.as_deref() == Some(mode.output.as_str()))
    {
        true => layout::LayerKind::Overlay,
        false => surfaces::layer_window::window_of(area.layer, &resolved),
    };
    let anchor = Anchor {
        output: area.output.clone(),
        layer: window,
        edge: resolved.kind.edge().unwrap_or(Edge::Top),
        rect: rects::rect(&area.group(group)).unwrap_or_default(),
        chrome: Chrome::global(desktop.config.clone(), area.output.clone()),
        gap: place::GAP,
    };
    transient::open(
        Spec::new(ID, Place::Over(anchor), Rc::new(|_: &Chrome| tree()))
            .output(area.output.clone())
            .keyboard(KeyboardMode::Exclusive)
            .dismiss_on_outside()
            .on_close(forget),
    );
    Ok(())
}

/// Closes the save card, saving nothing.
pub fn close() {
    if SAVING.with(|held| held.borrow().is_some()) {
        transient::close(ID);
        forget();
    }
}

fn forget() {
    if let Some(saving) = SAVING.with(|held| held.borrow_mut().take()) {
        saving.closed.set(true);
        saving.release_when_done();
    }
}

/// A name no komponent has yet, from the group's id: `end`, then `end-2`.
fn free_name(group: &GroupId) -> String {
    let taken = |name: &str| {
        surfaces::layouts::read(|store| store.komponent(&KomponentId::new(name)).is_some())
            .unwrap_or(false)
    };
    let stem = group.to_string();
    (1..)
        .map(|nth| match nth {
            1 => stem.clone(),
            nth => format!("{stem}-{nth}"),
        })
        .find(|name| !taken(name))
        .expect("the counting runs out long after the names do")
}

/// Saves the open card's group as the komponent it names, with the values switched on as parameters, and makes the group draw it: one undo entry. A refusal is said on the card, which stays open.
pub fn save() {
    let Some(saving) = SAVING.with(|held| held.borrow().clone()) else {
        return;
    };
    match saved(&saving) {
        Ok(()) => close(),
        Err(why) => saving.said.set(Some(why.to_string())),
    }
}

fn saved(saving: &Saving) -> Result<(), EditError> {
    let name = saving.name.peek().trim().to_string();
    if !layout::is_komponent_name(&name) {
        return Err(EditError::Refused(util::message!(
            "editor.komponent.bad_name",
            name = name
        )));
    }
    let id = KomponentId::new(&name);
    let chosen: Vec<Candidate> = saving
        .offered
        .iter()
        .filter(|(_, on)| on.peek())
        .map(|(candidate, _)| candidate.clone())
        .collect();
    let komponent = components::saved(&saving.drawn, &chosen);
    let layout = session::draft().peek();
    let written = Written::area(
        &layout,
        saving.area.output.as_deref(),
        saving.area.layer,
        &saving.area.area,
        crate::variant::editing().as_ref(),
    )
    .map_err(EditError::Refused)?;
    let mut changed = written.area.clone();
    let at = match changed
        .groups
        .iter()
        .position(|held| held.id == saving.group)
    {
        Some(at) => at,
        None => {
            changed.groups.push(layout::Group {
                id: saving.group.clone(),
                ..layout::Group::default()
            });
            changed.groups.len() - 1
        }
    };
    changed.groups[at] = components::used(&changed.groups[at], &id);
    let followed = komponent.children.iter().flat_map(|child| {
        let drawn = layout::InstanceId::in_komponent(&saving.area.area, &saving.group, &child.id);
        layout::ops::reowned(&layout, &child.id, &drawn)
    });
    let ops = written.ops(&changed).into_iter().chain(followed).collect();
    let label = telar::t!(
        "editor.komponent.saved",
        group = saving.group.to_string(),
        komponent = name
    );
    surfaces::layouts::with_komponent(id, komponent, EditError::Refused, || {
        crate::context::commit(label, ops)
    })
}

/// The open card's tree: the name, a switch per value that can be a parameter, why the last Save was refused, and Save and Cancel.
fn tree() -> Built {
    let Some(saving) = SAVING.with(|held| held.borrow().clone()) else {
        return Ok(Box::new(crate::host::passthrough(
            crate::host::whole(),
            Vec::new(),
        )?));
    };
    saving.trees.set(saving.trees.get() + 1);
    let leased = Rc::clone(&saving);
    telar::on_cleanup(move || {
        leased.trees.set(leased.trees.get() - 1);
        leased.release_when_done();
    });
    let theme = use_theme::<NordTheme>();
    let pad = ui::scale::space::lg();
    let inner = WIDTH - 2.0 * pad;
    let group = saving.group.to_string();
    let mut items = vec![box_item(Text::new(
        move || telar::t!("editor.komponent.save_title", group = group.clone()),
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_font_weight(700)
        },
    )?)];
    items.push(rows::text(
        Reactive::of(|| telar::t!("editor.komponent.name")),
        None,
        saving.name,
    )?);
    match saving.offered.is_empty() {
        true => items.push(rows::note(|| telar::t!("editor.komponent.no_parameters"))?),
        false => {
            items.push(rows::heading(|| {
                telar::t!("editor.komponent.make_parameters")
            })?);
            for (candidate, on) in &saving.offered {
                let shown = telar::t!(
                    "editor.komponent.candidate",
                    name = candidate.name.clone(),
                    child = candidate.child.to_string(),
                    key = candidate.key.clone()
                );
                items.push(rows::toggle(
                    Reactive::of(move || shown.clone()),
                    None,
                    *on,
                )?);
            }
        }
    }
    let refusal = saving.said;
    items.push(rows::note(move || refusal.get().unwrap_or_default())?);
    let buttons = Container::new(
        LayoutStyle::new()
            .flex_row()
            .gap(ui::scale::space::sm())
            .align_items(AlignItems::CENTER),
        vec![
            rows::action(|| telar::t!("editor.komponent.save_button"), save)?,
            rows::action(|| telar::t!("editor.komponent.cancel"), close)?,
        ],
    )?;
    items.push(box_item(buttons));
    let column = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::scale::space::sm())
            .width(inner),
        items,
    )?;
    let (at, output, edge) = (
        saving.area.group(&saving.group),
        saving.area.output.clone(),
        saving.edge,
    );
    let card = StyledContainer::new(
        LayoutStyle::new()
            .absolute()
            .inset_start(0.0)
            .inset_top(0.0)
            .width(WIDTH)
            .flex_column()
            .padding_all(pad),
        move |_| RectStyle::filled(theme.surface, ui::scale::corner::xl()),
        vec![box_item(column)],
    )?
    .input_opaque()
    .with_transform(move |laid| {
        let item = rects::rect(&at).unwrap_or_default();
        let (x, y) = place::card_at(
            item,
            edge,
            (laid.width, laid.height),
            crate::host::usable(output.as_deref()),
        );
        Some([1.0, 0.0, 0.0, 1.0, x - laid.x, y - laid.y])
    });
    Ok(Box::new(crate::host::passthrough(
        crate::host::whole(),
        vec![Box::new(card)],
    )?))
}

/// The open card's tree, built as its window would build it.
#[cfg(test)]
pub(crate) fn built() -> Option<Built> {
    SAVING.with(|held| held.borrow().is_some()).then(tree)
}

/// A komponent to draw in a group, and what its parameters are set to there; a parameter left out reads its default.
#[derive(Clone, Debug, PartialEq)]
pub struct Use {
    pub komponent: KomponentId,
    pub parameters: BTreeMap<String, Expr>,
}

impl Use {
    /// The komponent `komponent`, every parameter at its default.
    pub fn of(komponent: KomponentId) -> Self {
        Self {
            komponent,
            parameters: BTreeMap::new(),
        }
    }
}

/// Where a use goes: the area, and the group of it that draws the komponent, or a new one where `group` names none yet or is left out.
#[derive(Clone, Copy, Debug)]
pub struct Placing<'a> {
    pub layer: LayerKind,
    pub area: &'a AreaId,
    pub group: Option<&'a GroupId>,
    /// The screen the level is written for, and the workspace rule it is written in.
    pub output: Option<&'a str>,
    pub workspace: Option<&'a WorkspaceMatch>,
    /// On a grid, the first cell a new group covers where a pointer put it; else the free cells nearest `near`.
    pub cell: Option<(u32, u32)>,
    pub near: (u32, u32),
    /// On a bar, the zone a new group goes at the end of: the end zone where none is named. Naming one for an area that is not a bar, or for a group that exists, is refused.
    pub zone: Option<Zone>,
}

/// Why a use was not planned: what it is or where it goes ([`UseError`]), or how the layout refused the edit.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    Use(UseError),
    Edit(EditError),
}

impl Refusal {
    /// As the command line says it, in English where the reason is the use's.
    pub fn english(&self) -> String {
        match self {
            Refusal::Use(why) => why.english(),
            Refusal::Edit(why) => why.message().english(),
        }
    }

    /// As the editor says it, in the user's language.
    pub fn into_edit(self) -> EditError {
        match self {
            Refusal::Use(why) => EditError::Refused(why.message()),
            Refusal::Edit(why) => why,
        }
    }
}

impl From<UseError> for Refusal {
    fn from(why: UseError) -> Self {
        Refusal::Use(why)
    }
}

impl From<EditError> for Refusal {
    fn from(why: EditError) -> Self {
        Refusal::Edit(why)
    }
}

/// The cells a group drawing `komponent` covers on a grid: those of its widest and tallest child.
pub(crate) fn footprint(komponent: &layout::Komponent) -> Cells {
    komponent
        .children
        .iter()
        .map(|child| child.representation.map_or(Cells::ONE, desktop::footprint))
        .fold(Cells::ONE, |most, cells| Cells {
            cols: most.cols.max(cells.cols),
            rows: most.rows.max(cells.rows),
            ..most
        })
}

/// The operations that make a group of the area `place` names draw `request`, and the group's id. A group that does not exist yet is made — a fresh readable id from the komponent's name where none is named — at the end of the zone `place` names of a bar (the end zone where it names none), on the free cells of a grid or a panel, or unplaced in a dock or free area; one that exists is used as it is where it holds nothing, and refused where it holds modules (replacing them is destructive) or already draws a komponent. The use is checked as the layout is ([`layout::check_use`]): the komponent exists, every parameter is declared and of its type, a grid cell does not repeat, and the lock layer holds readings only.
///
/// `screen` is the screen the area is drawn on, `desktop` the one the editor or the shell is running where a grid has to find free cells.
pub fn plan_use(
    layout: &Layout,
    library: &Library,
    catalogue: &dyn Catalogue,
    screen: (&Resolved, Option<&Desktop>),
    place: &Placing,
    request: &Use,
) -> Result<(Vec<LayoutOp>, GroupId), Refusal> {
    let (resolved, desktop) = screen;
    let area = resolved
        .area(place.layer, place.area)
        .ok_or_else(|| UseError::NoArea(place.area.clone()))?;
    let placed_in_zone = matches!(area.kind, ResolvedAreaKind::Bar { .. });
    let on_grid = area.kind.places_on_cells();
    if !placed_in_zone
        && !on_grid
        && !matches!(
            area.kind,
            ResolvedAreaKind::Dock { .. } | ResolvedAreaKind::Free { .. }
        )
    {
        return Err(UseError::NoGroups(place.area.clone(), area.kind.name()).into());
    }
    let existing = place
        .group
        .and_then(|named| area.groups.iter().find(|held| held.id == *named));
    if let Some(held) = existing {
        if let Some(used) = &held.komponent {
            return Err(
                UseError::Drawing(place.area.clone(), held.id.clone(), used.id.clone()).into(),
            );
        }
        if !held.children.is_empty() {
            return Err(UseError::Occupied(place.area.clone(), held.id.clone()).into());
        }
    }
    if place.zone.is_some() {
        if !placed_in_zone {
            return Err(UseError::ZoneOffBar(place.area.clone(), area.kind.name()).into());
        }
        if let Some(held) = existing {
            return Err(UseError::ZoneOfExisting(place.area.clone(), held.id.clone()).into());
        }
    }
    let in_cell = match existing {
        Some(held) => matches!(held.kind, GroupKind::Cell { .. }),
        None => on_grid,
    };
    let komponent = layout::check_use(
        library,
        catalogue,
        (&request.komponent, &request.parameters),
        place.layer,
        in_cell,
    )?;
    let id = match place.group {
        Some(named) => named.clone(),
        None => layout::ops::free_group_id(
            layout,
            library,
            place.layer,
            place.area,
            request.komponent.as_str(),
        ),
    };
    let entry = |kind: Option<GroupKind>| Group {
        id: id.clone(),
        kind,
        komponent: Some(request.komponent.clone()),
        parameters: request.parameters.clone(),
        ..Group::default()
    };
    if on_grid && existing.is_none() {
        let desktop = desktop.ok_or_else(|| UseError::NoScreen(place.area.clone()))?;
        let mut work = Work::new(layout, desktop, place.layer);
        work.workspace = place.workspace.cloned();
        desktop::put_group(
            &mut work,
            place.area,
            (id.clone(), footprint(komponent)),
            (place.cell, place.near),
            |kind| entry(Some(kind)),
        )?;
        return Ok((work.done(), id));
    }
    let written = Written::area(
        layout,
        place.output,
        place.layer,
        place.area,
        place.workspace,
    )
    .map_err(EditError::Refused)?;
    let mut changed = written.area.clone();
    match changed.groups.iter().position(|held| held.id == id) {
        Some(at) => {
            changed.groups[at] = components::used_with(
                &changed.groups[at],
                &request.komponent,
                request.parameters.clone(),
            )
        }
        None => {
            let kind = match (existing, placed_in_zone) {
                (None, true) => Some(GroupKind::Zone {
                    zone: place.zone.unwrap_or(Zone::End),
                }),
                _ => None,
            };
            changed.groups.push(entry(kind));
        }
    }
    Ok((written.ops(&changed), id))
}

/// A komponent the palette offers on `layer`: its name, how many parameters it takes, and its use as the palette puts it — every parameter at its default.
#[derive(Clone, Debug, PartialEq)]
pub struct Offered {
    pub id: KomponentId,
    pub parameters: usize,
}

/// The komponents of `library` a new group on `layer` may draw as they are — in a grid cell where `in_cell`, else in a bar's zone, a dock or a free area: those [`layout::check_use`] accepts there with every parameter at its default. On the lock layer that leaves out any with a child that answers the pointer or has actions, and a grid cell leaves out any that repeats.
pub fn offered(
    library: &Library,
    catalogue: &dyn Catalogue,
    layer: LayerKind,
    in_cell: bool,
) -> Vec<Offered> {
    library
        .komponents
        .iter()
        .filter(|(id, _)| {
            layout::check_use(library, catalogue, (id, &BTreeMap::new()), layer, in_cell).is_ok()
        })
        .map(|(id, komponent)| Offered {
            id: id.clone(),
            parameters: komponent.parameters.len(),
        })
        .collect()
}

/// The bar menu's "Add komponent" row: a submenu of the bar's zones, each a submenu of the komponents a new group there may draw, a pick putting it at the end of that zone ([`add_to_bar`]). Nothing where the library holds no komponent the bar's layer takes. The menu is the keyboard's path too: the menu key opens it on the selected bar, and the arrows, Right and Enter pick from it.
pub(crate) fn bar_entry(node: &Node) -> Option<MenuEntry> {
    let offered = offered(
        &known(),
        &surfaces::catalogue::Descriptors::installed(),
        node.layer,
        false,
    );
    if offered.is_empty() {
        return None;
    }
    let zones = [Zone::Start, Zone::Center, Zone::End]
        .into_iter()
        .map(|zone| MenuEntry::Sub {
            label: zone_name(zone),
            entries: offered
                .iter()
                .map(|found| {
                    let (bar, id) = (node.clone(), found.id.clone());
                    MenuEntry::row(found.id.to_string(), "", move || {
                        said(add_to_bar(&bar, zone, &id))
                    })
                })
                .collect(),
        })
        .collect();
    Some(MenuEntry::Sub {
        label: telar::t!("editor.komponent.add"),
        entries: zones,
    })
}

/// What a bar's zone is called where one is picked: the menus' and the palette's tag.
pub(crate) fn zone_name(zone: Zone) -> String {
    match zone {
        Zone::Start => telar::t!("editor.komponent.zone.start"),
        Zone::Center => telar::t!("editor.komponent.zone.center"),
        Zone::End => telar::t!("editor.komponent.zone.end"),
    }
}

/// Where the editor puts a new group drawing a komponent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Where {
    /// On the cells of a grid or a panel: its first cell `at` where a pointer put it, else the free cells nearest `near`.
    Cell {
        at: Option<(u32, u32)>,
        near: (u32, u32),
    },
    /// At the end of this zone of a bar.
    Zone(Zone),
}

/// The operations that make a new group of the area `area` of `layer` on `desktop`'s screen draw the komponent `id`, every parameter at its default, `place` saying where, and the group's id: the one planner the palette, the bar's menu and the desktop share ([`plan_use`]).
pub(crate) fn planned(
    layout: &Layout,
    desktop: &Desktop,
    layer: LayerKind,
    area: &AreaId,
    id: &KomponentId,
    place: Where,
) -> Result<(Vec<LayoutOp>, GroupId), EditError> {
    let library = known();
    let workspace = crate::variant::editing();
    let screen = desktop.resolving(layout, &library);
    let (cell, near, zone) = match place {
        Where::Cell { at, near } => (at, near, None),
        Where::Zone(zone) => (None, (0, 0), Some(zone)),
    };
    plan_use(
        layout,
        &library,
        &surfaces::catalogue::Descriptors::installed(),
        (&screen.resolved, Some(desktop)),
        &Placing {
            layer,
            area,
            group: None,
            output: desktop.output.as_deref(),
            workspace: workspace.as_ref(),
            cell,
            near,
            zone,
        },
        &Use::of(id.clone()),
    )
    .map_err(Refusal::into_edit)
}

/// Puts the komponent `id` in a new group at the end of the zone `zone` of the bar `bar` names, every parameter at its default ([`planned`]): one undo entry, and the new group selected where an edit mode is on that bar's screen.
pub fn add_to_bar(bar: &Node, zone: Zone, id: &KomponentId) -> Result<(), EditError> {
    let desktop = reconcile::desktop_now(bar.output.as_deref()).ok_or_else(EditError::no_output)?;
    let layout = session::draft().peek();
    let (ops, group) = planned(
        &layout,
        &desktop,
        bar.layer,
        &bar.area,
        id,
        Where::Zone(zone),
    )?;
    crate::context::commit(
        telar::t!("editor.komponent.used", komponent = id.to_string()),
        ops,
    )?;
    session::select(session::Selection::Group(bar.group(&group)));
    Ok(())
}
