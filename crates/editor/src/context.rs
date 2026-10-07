//! Context menus on every item and area (TA-4): a secondary press on a chip, a widget, a card or an area's empty space opens one, and so does the menu key (or Shift+F10) on what has the focus, in an edit mode and outside one.
//!
//! **What is in it.** An instance's module's own actions, "Customize…", "Duplicate", "Order ▸" in a `free` container ([`crate::stacking`]), for a child of a container "Customize the container…" and "Take out of the container" ([`crate::modes::container`]), a move to an area that draws it the other way (chip ↔ widget, keeping its id, options and state), saving its group as a komponent, "Remove" and "Edit <layer>…" — or, for a child of a komponent a group draws, its module's actions, the use's parameters, "Detach" and "Edit <layer>…" ([`crate::komponent`]); an area's own bound actions, "Customize…", "Duplicate" where its kind can be copied ([`crate::duplicate`]), "Order ▸", what the tools for its kind add ([`add_area_rows`]) and "Edit <layer>…". A placeholder — a module this build does not have, or cannot draw the way the layout asks — gets the "Fix…" rows instead, remove and reset, which act on the layout rather than on config. On the layer an edit mode is editing, every menu ends with undo, redo, the history to jump through ([`crate::history`]) and the strip's actions. Nothing is offered on the lock layer.
//!
//! **Where.** A menu is a transient laid over the whole window it was asked in (F-2.3, DEC-9): the item's own, or the overlay window where that layer is hidden or an edit mode's host is over it. It opens at the pointer, or on the item when the keyboard asked, and never past an edge of the screen.
//!
//! **One undo entry a row.** A row that changes the layout is one edit through [`session`], so a remove, a reset and a move to another area are each taken back by one undo.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{
    AlignItems, Children, Container, ContextMenuProps, JustifyContent, LayoutStyle, MenuEntry,
    MenuStyle, Rect, SizeDimension, Text, context_menu, use_theme,
};

use config::Edge;
use config::theme::{FontRole, NordTheme};
use layout::{
    Action, AreaId, GroupId, Instance, InstanceId, LayerKind, Layout, LayoutOp, Library,
    Representation, ResolvedArea, ResolvedAreaKind, ResolvedInstance, Trigger,
};
use platform_wayland::KeyboardMode;
use surfaces::menu::Asked;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node, Part};
use surfaces::transient::{self, Anchor, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::{Built, ModuleDescriptor};

use crate::keys::Chord;
use crate::mode::{self, said};
use crate::popover;
use crate::session::{self, EditError, Way};
use crate::written::{Written, known};

/// The transient every context menu is, one at a time.
pub const ID: &str = "editor:menu";

const WIDTH: f32 = 260.0;
const ROW: f32 = 28.0;
/// telar's panel pads itself by 4 px whatever the style says, and keeps itself on screen by a height reckoned from the style's padding: the two agree only while the style says 4 too.
const PADDING: f32 = 4.0;

/// The menu that is open: where it was asked for, the screen it is kept inside, and its rows.
#[derive(Clone)]
struct Shown {
    at: (f32, f32),
    within: Rect,
    entries: Vec<MenuEntry>,
}

/// The rows a tool adds to the menu of every area of one kind, given the area as the screen shows it and the node it is.
pub type AreaRows = fn(&ResolvedArea, &Node) -> Vec<MenuEntry>;

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    static AREA_ROWS: RefCell<Vec<(&'static str, AreaRows)>> = const { RefCell::new(Vec::new()) };
}

/// Adds `rows` to the menu of every area of the kind `kind` (`wallpaper_region`, … as the layout file spells it), after "Customize…" and before "Edit <layer>…". Registered inside [`crate::install`].
pub fn add_area_rows(kind: &'static str, rows: AreaRows) {
    AREA_ROWS.with(|tools| tools.borrow_mut().push((kind, rows)));
}

/// Makes every secondary press and menu key the surfaces offer a menu for open one here. Installed once, before any window builds.
pub(crate) fn install() {
    surfaces::menu::install(|asked| {
        if let Err(why) = open(asked) {
            tracing::info!("no context menu: {why}");
        }
    });
}

/// Opens the menu `asked` is for, closing (and so keeping what it changed) whichever popover was open.
pub fn open(asked: Asked) -> Result<(), EditError> {
    let Asked { node, window, at } = asked;
    if node.layer == LayerKind::Lock {
        return Err(EditError::Refused(util::message!("editor.menu.lock")));
    }
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let area = desktop
        .resolved
        .area(node.layer, &node.area)
        .cloned()
        .ok_or_else(|| EditError::gone(&node.area))?;
    let mut entries = match &node.part {
        Part::Instance(group, id) => {
            instance_entries(&desktop, &area, &node, group, &id.template())?
        }
        Part::Area | Part::Group(_) => area_entries(
            &area,
            &Node::area(node.output.as_deref(), node.layer, &node.area),
        ),
    };
    entries.extend(mode_rows(&node));
    let window = match mode::current()
        .is_some_and(|mode| node.output.as_deref() == Some(mode.output.as_str()))
    {
        true => LayerKind::Overlay,
        false => window,
    };
    let rect = rects::rect(&node).unwrap_or_default();
    show(
        &desktop,
        (window, area.kind.edge().unwrap_or(Edge::Top)),
        rect,
        at.unwrap_or((rect.x, rect.y + rect.height)),
        entries,
    );
    Ok(())
}

/// Shows `entries` as the one open menu at `at`, kept inside `desktop`'s screen, in the window of `layer` hanging off `rect` by `edge`, closing (and so keeping what it changed) whichever popover or menu was open.
pub(crate) fn show(
    desktop: &Desktop,
    (layer, edge): (LayerKind, Edge),
    rect: Rect,
    at: (f32, f32),
    entries: Vec<MenuEntry>,
) {
    popover::close();
    transient::close(ID);
    SHOWN.with(|shown| {
        *shown.borrow_mut() = Some(Shown {
            at,
            within: Rect::new(0.0, 0.0, desktop.size.0, desktop.size.1),
            entries,
        })
    });
    let output = desktop.output.clone();
    let anchor = Anchor {
        output: output.clone(),
        layer,
        edge,
        rect,
        chrome: Chrome::global(desktop.config.clone(), output.clone()),
        gap: 0.0,
    };
    transient::open(
        Spec::new(ID, Place::Over(anchor), Rc::new(|_: &Chrome| tree()))
            .output(output)
            .keyboard(KeyboardMode::Exclusive)
            .on_close(|| SHOWN.with(|shown| *shown.borrow_mut() = None)),
    );
}

/// Opens the menu of what an edit mode has selected, answering whether anything was: in an edit mode the selection is what has the focus, so this is what the menu key opens there.
pub(crate) fn open_selected() -> bool {
    let Some(node) = session::selected().node().cloned() else {
        return false;
    };
    let about = match node.part {
        Part::Group(_) => Node::area(node.output.as_deref(), node.layer, &node.area),
        _ => node,
    };
    if let Err(why) = open(Asked {
        node: about,
        window: LayerKind::Overlay,
        at: None,
    }) {
        mode::refuse(why);
    }
    true
}

/// The open menu, drawn from the rows it was opened with.
fn tree() -> Built {
    let Some(shown) = SHOWN.with(|shown| shown.borrow().clone()) else {
        return Ok(Box::new(crate::host::passthrough(
            crate::host::whole(),
            Vec::new(),
        )?));
    };
    let theme = use_theme::<NordTheme>();
    context_menu(
        ContextMenuProps::props()
            .at(shown.at)
            .entries(shown.entries)
            .on_close(Rc::new(|| transient::close(ID)))
            .width(WIDTH)
            .within(shown.within)
            .style(style(theme))
            .build(),
        Children::default(),
    )
}

fn style(theme: NordTheme) -> MenuStyle {
    MenuStyle {
        background: theme.surface,
        border: theme.overlay,
        label: theme.text,
        faint: theme.muted,
        highlight: theme.overlay,
        radius: ui::scale::corner::md(),
        font_size: theme.font(FontRole::Body),
        row_height: ROW,
        padding: PADDING,
        ..MenuStyle::default()
    }
}

/// An instance's rows: its module's actions, customizing it, moving it where it is drawn the other way, removing it and editing its layer — or, for a placeholder, the rows that fix it.
fn instance_entries(
    desktop: &Desktop,
    area: &ResolvedArea,
    node: &Node,
    group: &GroupId,
    id: &InstanceId,
) -> Result<Vec<MenuEntry>, EditError> {
    let holder = area
        .groups
        .iter()
        .find(|held| held.id == *group)
        .ok_or_else(|| EditError::gone(&node.area))?;
    let resolved = holder
        .children
        .iter()
        .find(|child| child.id == *id)
        .cloned()
        .ok_or_else(|| EditError::gone(&node.area))?;
    if holder.komponent.is_some() {
        let mut rows = module_actions(&resolved);
        rows.extend(crate::komponent::rows(area, node, holder));
        rows.extend(edit_row(node));
        return Ok(rows);
    }
    let drawn = ui::descriptor::find(&resolved.module).filter(|module| {
        module
            .input(surfaces::area::representation(resolved.representation))
            .is_some()
    });
    let Some(module) = drawn else {
        let mut rows = vec![
            remove_row(node, &resolved.module),
            reset_row(node, &resolved),
        ];
        rows.extend(edit_row(node));
        return Ok(rows);
    };
    let mut rows = module_actions(&resolved);
    let customized = node.clone();
    rows.push(MenuEntry::row(
        telar::t!("editor.menu.customize", name = module.name),
        "",
        move || said(popover::open_instance(customized.clone())),
    ));
    rows.push(duplicate_row(node));
    rows.extend(crate::stacking::menu(node));
    rows.extend(crate::modes::container::menu_rows(area, node, holder));
    rows.extend(
        destinations(desktop, node, &resolved, module)
            .into_iter()
            .map(|to| move_row(node, module.name, to)),
    );
    rows.extend(crate::komponent::rows(area, node, holder));
    rows.extend(crate::panel::rows(node));
    rows.push(remove_row(node, module.name));
    rows.extend(edit_row(node));
    Ok(rows)
}

/// A row for each action the module `resolved` shows declares, which runs it.
fn module_actions(resolved: &ResolvedInstance) -> Vec<MenuEntry> {
    let Some(module) = ui::descriptor::find(&resolved.module) else {
        return Vec::new();
    };
    module
        .actions
        .iter()
        .map(|action| {
            let line = Action(vec![action.command.to_string()]);
            MenuEntry::row(action_label(action.id), "", move || {
                surfaces::actions::run(&line)
            })
        })
        .collect()
}

/// An area's rows: what its own empty space binds, customizing it and editing its layer.
fn area_entries(area: &ResolvedArea, node: &Node) -> Vec<MenuEntry> {
    let mut rows: Vec<MenuEntry> = area
        .actions
        .iter()
        .map(|(trigger, action)| {
            let action = action.clone();
            MenuEntry::row(action.0.join(" ; "), trigger_name(*trigger), move || {
                surfaces::actions::run(&action)
            })
        })
        .collect();
    let customized = node.clone();
    rows.push(MenuEntry::row(
        telar::t!("editor.menu.customize", name = kind_name(&area.kind)),
        "",
        move || said(popover::open_area(customized.clone())),
    ));
    if crate::duplicate::refusal(&area.kind).is_none() {
        rows.push(duplicate_row(node));
    }
    rows.extend(crate::stacking::menu(node));
    let tools: Vec<AreaRows> = AREA_ROWS.with(|tools| {
        tools
            .borrow()
            .iter()
            .filter(|(kind, _)| *kind == area.kind.name())
            .map(|(_, rows)| *rows)
            .collect()
    });
    for tool in tools {
        rows.extend(tool(area, node));
    }
    if !matches!(area.kind, ResolvedAreaKind::Prompt { .. }) {
        let removed = node.clone();
        let name = kind_name(&area.kind);
        rows.push(MenuEntry::row(
            telar::t!("editor.menu.remove"),
            "",
            move || said(remove_area(&removed, &name)),
        ));
    }
    rows.extend(edit_row(node));
    rows
}

fn mode_rows(node: &Node) -> Vec<MenuEntry> {
    match mode::editing(node) {
        true => edit_mode_rows(),
        false => Vec::new(),
    }
}

/// What the mode adds to the menu of anything on the layer it edits: undo and redo, the history to jump through, and every action of the strip ([`crate::host::add_strip_action`]).
pub(crate) fn edit_mode_rows() -> Vec<MenuEntry> {
    let history = crate::history::current();
    let walk = |label: String, chords: Vec<Chord>, way: Way, possible: bool| {
        let hint = chords.first().map(Chord::spelled).unwrap_or_default();
        let row = MenuEntry::row(label, hint, move || session::travel_saying(way.steps()));
        match possible {
            true => row,
            false => row.disabled(),
        }
    };
    let mut rows = vec![
        MenuEntry::Separator,
        walk(
            telar::t!("editor.history.undo"),
            crate::keys::undo_chords(),
            Way::Undo,
            !history.undo.is_empty(),
        ),
        walk(
            telar::t!("editor.history.redo"),
            crate::keys::redo_chords(),
            Way::Redo,
            !history.redo.is_empty(),
        ),
    ];
    rows.extend(crate::history::menu(&history));
    rows.extend(
        crate::host::strip_actions()
            .into_iter()
            .map(|(label, act)| MenuEntry::row(label(), "", act)),
    );
    rows
}

/// A row drawn in the faint ink of a hint that still picks like any other: an entry there to reach that is not where things stand, as a redo in the history is.
pub(crate) fn dimmed_row(label: String, hint: String, act: impl Fn() + 'static) -> MenuEntry {
    MenuEntry::Custom {
        widget: Rc::new(move || dimmed_face(label.clone(), hint.clone())),
        act: Some(Rc::new(act)),
    }
}

fn dimmed_face(label: String, hint: String) -> Built {
    let theme = use_theme::<NordTheme>();
    let text = move |said: String| {
        Text::declaring(
            move || said.clone(),
            LayoutStyle::new(),
            move |inherited| theme.text_over(inherited, FontRole::Body, theme.muted),
        )
    };
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_row()
            .width(SizeDimension::Percent(1.0))
            .height(ROW)
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::SPACE_BETWEEN)
            .padding_horizontal(PADDING * 1.5),
        vec![Box::new(text(label)?), Box::new(text(hint)?)],
    )?))
}

/// "Edit <layer>…", unless that layer is the one being edited on this screen already.
fn edit_row(node: &Node) -> Option<MenuEntry> {
    if mode::editing(node) {
        return None;
    }
    let (layer, output) = (node.layer, node.output.clone());
    Some(MenuEntry::row(
        telar::t!("editor.menu.edit", layer = mode::name_of(layer)),
        "",
        move || {
            if let Err(why) = mode::enter(layer, output.as_deref()) {
                tracing::info!("{}", why.english());
            }
        },
    ))
}

fn duplicate_row(node: &Node) -> MenuEntry {
    let selection = session::Selection::of(node.clone());
    MenuEntry::row(
        telar::t!("editor.menu.duplicate"),
        crate::duplicate::chord().spelled(),
        move || said(crate::duplicate::duplicate(&selection)),
    )
}

fn remove_row(node: &Node, name: &str) -> MenuEntry {
    let (node, name) = (node.clone(), name.to_string());
    MenuEntry::row(telar::t!("editor.menu.remove"), "", move || {
        said(remove(&node, &name))
    })
}

fn reset_row(node: &Node, resolved: &ResolvedInstance) -> MenuEntry {
    let (node, id) = (node.clone(), resolved.id.clone());
    let name = resolved.module.clone();
    MenuEntry::row(telar::t!("editor.menu.reset"), "", move || {
        said(reset(&node, &id, &name))
    })
}

fn move_row(node: &Node, name: &str, to: Destination) -> MenuEntry {
    let label = match to.representation {
        Representation::Chip => telar::t!("editor.menu.to_chip", place = to.place.clone()),
        _ => telar::t!("editor.menu.to_widget", place = to.place.clone()),
    };
    let (node, name) = (node.clone(), name.to_string());
    MenuEntry::row(label, "", move || said(convert(&node, &name, &to)))
}

/// Takes the instance `node` names out of the layout being edited, as one undo entry.
pub fn remove(node: &Node, name: &str) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let ops = removal(&session::draft().peek(), &desktop, node)?;
    commit(telar::t!("editor.menu.removed", name = name), ops)
}

/// Takes the area `node` names off its screen, as one undo entry: out of the rule that writes it, and over whatever broader level still places it there ([`crate::written::area_removal`]).
pub fn remove_area(node: &Node, name: &str) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let layout = session::draft().peek();
    let known = known();
    let ops = crate::written::area_removal(
        &layout,
        &known,
        &desktop.resolving(&layout, &known).resolved,
        node.layer,
        &node.area,
        crate::variant::editing().as_ref(),
    )
    .map_err(EditError::Refused)?;
    commit(telar::t!("editor.menu.removed", name = name), ops)
}

/// Puts the instance `node` names back as the layout it extends — or the built-in one — has it, as one undo entry; one that layout does not have is taken away.
pub fn reset(node: &Node, id: &InstanceId, name: &str) -> Result<(), EditError> {
    let layout = session::draft().peek();
    let base = layout::reset::base_of(&layout, &known());
    let ops = layout::reset::ops(&layout, &base, layout::reset::Target::Instance(id))
        .ok_or_else(|| EditError::gone(&node.area))?;
    commit(telar::t!("editor.menu.instance_reset", name = name), ops)
}

/// Moves the instance `node` names into `to`, drawn the way `to` draws it, keeping its id, options and bindings: a chip becomes a widget and back without becoming another instance (TA-3). One undo entry.
fn convert(node: &Node, name: &str, to: &Destination) -> Result<(), EditError> {
    let desktop =
        reconcile::desktop_now(node.output.as_deref()).ok_or_else(EditError::no_output)?;
    let layout = session::draft().peek();
    let label = telar::t!("editor.menu.moved", name = name, place = to.place.clone());
    if to.onto_grid {
        let ops = crate::modes::desktop::moved_onto(
            &layout,
            &desktop,
            node,
            (to.layer, &to.area),
            to.representation,
            None,
        )?;
        return commit(label, ops);
    }
    let shown = crate::modes::desktop::shown_instance(&desktop.resolving(&layout, &known()), node)
        .ok_or_else(|| EditError::gone(&node.area))?;
    let mut ops = removal(&layout, &desktop, node)?;
    let mut after = layout.clone();
    layout::ops::apply_all(&mut after, &ops)?;
    let moved = Instance {
        representation: Some(to.representation),
        ..crate::modes::desktop::placed_as(&shown)
    };
    let written = Written::area(
        &after,
        node.output.as_deref(),
        to.layer,
        &to.area,
        crate::variant::editing().as_ref(),
    )
    .map_err(EditError::Refused)?;
    ops.extend(written.instance(&to.group, &moved.id).ops(&moved));
    commit(label, ops)
}

/// What takes the instance `node` names off its screen: out of the rule that writes it, and, where a broader rule or a layout this one extends still places it there, named in the `remove` of its group as well. A grid's group left with nothing in it goes too, and a stack left with one widget is a widget again ([`crate::modes::desktop::tidied`]).
pub(crate) fn removal(
    layout: &Layout,
    desktop: &Desktop,
    node: &Node,
) -> Result<Vec<LayoutOp>, EditError> {
    let Part::Instance(group, id) = &node.part else {
        return Err(EditError::nothing());
    };
    let known = known();
    let id = &id.template();
    let mut after = layout.clone();
    let mut ops = Vec::new();
    for round in 0.. {
        if !placed(&after, &known, desktop, node, id) {
            break;
        }
        if round == 2 {
            return Err(EditError::Refused(util::message!(
                "editor.menu.still_placed",
                id = id.to_string()
            )));
        }
        let step = Written::area(
            &after,
            node.output.as_deref(),
            node.layer,
            &node.area,
            crate::variant::editing().as_ref(),
        )
        .map_err(EditError::Refused)?
        .instance(group, id)
        .removal();
        layout::ops::apply_all(&mut after, &step)?;
        ops.extend(step);
    }
    ops.extend(crate::modes::desktop::tidied(
        &after, desktop, node.layer, &node.area, group,
    )?);
    Ok(ops)
}

/// Whether `layout` still places `id` in the area `node` names on its screen.
fn placed(
    layout: &Layout,
    known: &Library,
    desktop: &Desktop,
    node: &Node,
    id: &InstanceId,
) -> bool {
    let (resolved, _) = layout::resolve(
        layout,
        known,
        &desktop.resolved.output,
        desktop.resolved.workspace.as_ref(),
    );
    resolved.area(node.layer, &node.area).is_some_and(|area| {
        area.groups
            .iter()
            .any(|group| group.children.iter().any(|child| child.id == *id))
    })
}

/// `ops` as one edit, previewed and committed at once: one entry in the history.
pub(crate) fn commit(label: String, ops: Vec<LayoutOp>) -> Result<(), EditError> {
    let edit = session::begin(label)?;
    if let Err(why) = edit.preview(ops) {
        let _ = edit.revert();
        return Err(why);
    }
    edit.commit()
}

/// Makes a new area on the edited layer of the screen being edited, as one undo entry called what `label` says of it, and selects it: `plan` gives the operations that make it, on the layout and screen as they are now, and its id.
pub(crate) fn make(
    plan: impl FnOnce(&Layout, &Desktop, LayerKind) -> Result<(Vec<LayoutOp>, AreaId), EditError>,
    label: fn(&AreaId) -> String,
) -> Result<(), EditError> {
    let mode = mode::required()?;
    let desktop = reconcile::desktop_now(Some(&mode.output)).ok_or_else(EditError::no_output)?;
    let (ops, id) = plan(&session::draft().peek(), &desktop, mode.layer)?;
    commit(label(&id), ops)?;
    session::select(session::Selection::Area(Node::area(
        Some(&mode.output),
        mode.layer,
        &id,
    )));
    Ok(())
}

/// An area an instance can be moved to, drawn the other way there.
#[derive(Clone, Debug, PartialEq)]
struct Destination {
    layer: LayerKind,
    area: AreaId,
    group: GroupId,
    representation: Representation,
    /// A grid, where it goes on cells of its own rather than into a group.
    onto_grid: bool,
    /// What the row calls it.
    place: String,
}

/// Where the instance `node` names can go to be drawn the other way: a chip to every grid and free area of its own layer as a widget, a widget or a card to every bar and dock of its own layer as a chip — as far as its module can be drawn that way. Never onto another layer: what is on one layer has nothing to do with what is on another.
fn destinations(
    desktop: &Desktop,
    node: &Node,
    resolved: &ResolvedInstance,
    module: &ModuleDescriptor,
) -> Vec<Destination> {
    let is_chip = resolved.representation == Representation::Chip;
    let chip = module
        .input(surfaces::area::representation(Representation::Chip))
        .is_some();
    let (layout, known) = (session::draft().peek(), known());
    let mut found: Vec<Destination> = Vec::new();
    let layer = node.layer;
    if let Some(areas) = desktop.resolved.layer(layer) {
        for area in &areas.areas {
            if area.id == node.area {
                continue;
            }
            let (representation, place) = match &area.kind {
                ResolvedAreaKind::Bar { .. } if !is_chip && chip => {
                    (Representation::Chip, telar::t!("editor.menu.place.bar"))
                }
                ResolvedAreaKind::Dock { .. } if !is_chip && chip => {
                    (Representation::Chip, telar::t!("editor.menu.place.dock"))
                }
                ResolvedAreaKind::Grid { .. } | ResolvedAreaKind::Free { .. } if is_chip => {
                    match crate::modes::palette::offered(module, layer) {
                        Some(widget) => (widget, place_of(layer)),
                        None => continue,
                    }
                }
                _ => continue,
            };
            let group = area
                .groups
                .iter()
                .find(|group| group.arrange.is_none())
                .map_or_else(
                    || {
                        layout::ops::free_group_id(
                            &layout,
                            &known,
                            layer,
                            &area.id,
                            resolved.id.as_str(),
                        )
                    },
                    |group| group.id.clone(),
                );
            found.push(Destination {
                layer,
                area: area.id.clone(),
                group,
                representation,
                onto_grid: matches!(area.kind, ResolvedAreaKind::Grid { .. }),
                place,
            });
        }
    }
    let named: Vec<String> = found.iter().map(|to| to.place.clone()).collect();
    for to in &mut found {
        if named.iter().filter(|place| **place == to.place).count() > 1 {
            to.place = telar::t!(
                "editor.menu.place.named",
                place = to.place.clone(),
                id = to.area.to_string()
            );
        }
    }
    found
}

/// What a move to a grid or free area on `layer` calls where it goes.
fn place_of(layer: LayerKind) -> String {
    match layer {
        LayerKind::Background => telar::t!("editor.menu.place.background"),
        LayerKind::Desktop => telar::t!("editor.menu.place.desktop"),
        LayerKind::Top => telar::t!("editor.menu.place.top"),
        LayerKind::Overlay | LayerKind::Lock => telar::t!("editor.menu.place.overlay"),
    }
}

/// What "Customize…" calls an area of this kind.
pub(crate) fn kind_name(kind: &ResolvedAreaKind) -> String {
    match kind {
        ResolvedAreaKind::Bar { .. } => telar::t!("editor.menu.kind.bar"),
        ResolvedAreaKind::Grid { .. } => telar::t!("editor.menu.kind.grid"),
        ResolvedAreaKind::Stack { .. } => telar::t!("editor.menu.kind.stack"),
        ResolvedAreaKind::WallpaperRegion { .. } => telar::t!("editor.menu.kind.wallpaper_region"),
        ResolvedAreaKind::Texture { .. } => telar::t!("editor.menu.kind.texture"),
        ResolvedAreaKind::Dock { .. } => telar::t!("editor.menu.kind.dock"),
        ResolvedAreaKind::Free { .. } => telar::t!("editor.menu.kind.free"),
        ResolvedAreaKind::Panel { .. } => telar::t!("editor.menu.kind.panel"),
        ResolvedAreaKind::Prompt { .. } => telar::t!("editor.menu.kind.prompt"),
    }
}

/// The gesture an area's bound action is on, drawn where a row's shortcut would be.
pub(crate) fn trigger_name(trigger: Trigger) -> String {
    match trigger {
        Trigger::Press => telar::t!("editor.menu.trigger.press"),
        Trigger::LongPress => telar::t!("editor.menu.trigger.long_press"),
        Trigger::ScrollUp => telar::t!("editor.menu.trigger.scroll_up"),
        Trigger::ScrollDown => telar::t!("editor.menu.trigger.scroll_down"),
        Trigger::Middle => telar::t!("editor.menu.trigger.middle"),
        Trigger::Secondary => telar::t!("editor.menu.trigger.secondary"),
    }
}

/// What a module's action is called in a menu: the catalogue's `editor.action.<id>` in the active locale, else the id as words — an action a module adds later still reads as something.
pub fn action_label(id: &str) -> String {
    let catalog = &crate::__rsx_i18n::CATALOG;
    let active = telar::use_locale();
    let locale = active.as_deref().unwrap_or(catalog.default_locale);
    match catalog.message(&format!("editor.action.{id}"), locale) {
        Some(message) => message.select(locale, &[]).render(&[]),
        None => {
            let words = id.replace(['-', '_'], " ");
            let mut chars = words.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

/// Whether the catalogue names the action `id`, which is what a module's new action needs before its menu row reads as more than its id.
pub fn names_action(id: &str) -> bool {
    let catalog = &crate::__rsx_i18n::CATALOG;
    catalog.locales.iter().all(|locale| {
        catalog
            .message(&format!("editor.action.{id}"), locale)
            .is_some()
    })
}

/// The open menu's tree, built as its window would build it.
#[cfg(test)]
pub(crate) fn built() -> Option<Built> {
    SHOWN.with(|shown| shown.borrow().is_some()).then(tree)
}

/// What the open menu's rows say, top to bottom.
#[cfg(test)]
pub(crate) fn rows() -> Vec<String> {
    SHOWN.with(|shown| {
        shown
            .borrow()
            .iter()
            .flat_map(|shown| shown.entries.iter())
            .filter_map(|entry| match entry {
                MenuEntry::Row { label, .. } | MenuEntry::Sub { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect()
    })
}

/// What the rows of the open menu's submenu `label` say, top to bottom; empty where it has no such submenu.
#[cfg(test)]
pub(crate) fn sub_rows(label: &str) -> Vec<String> {
    SHOWN.with(|shown| {
        shown
            .borrow()
            .iter()
            .flat_map(|shown| shown.entries.iter())
            .find_map(|entry| match entry {
                MenuEntry::Sub {
                    label: said,
                    entries,
                } if said == label => Some(
                    entries
                        .iter()
                        .filter_map(|entry| match entry {
                            MenuEntry::Row { label, .. } | MenuEntry::Sub { label, .. } => {
                                Some(label.clone())
                            }
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
            .unwrap_or_default()
    })
}

/// Picks the row that says `label`, in the menu or in any of its submenus, as the panel does: the menu closes, then the row acts.
#[cfg(test)]
pub(crate) fn pick(label: &str) {
    fn found(entries: &[MenuEntry], label: &str) -> Option<Rc<dyn Fn()>> {
        entries.iter().find_map(|entry| match entry {
            MenuEntry::Row {
                label: said, act, ..
            } if said == label => Some(Rc::clone(act)),
            MenuEntry::Sub { entries, .. } => found(entries, label),
            _ => None,
        })
    }
    let act = SHOWN.with(|shown| {
        shown
            .borrow()
            .as_ref()
            .and_then(|shown| found(&shown.entries, label))
    });
    let act = act.unwrap_or_else(|| panic!("no row says {label:?}: {:?}", rows()));
    transient::close(ID);
    act();
}
