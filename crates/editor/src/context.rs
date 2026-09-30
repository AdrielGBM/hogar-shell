//! Context menus on every item and area (TA-4): a secondary press on a chip, a widget, a card or an area's empty space opens one, and so does the menu key (or Shift+F10) on what has the focus, in an edit mode and outside one.
//!
//! **What is in it.** An instance's module's own actions, "Customize…", a move to an area that draws it the other way (chip ↔ widget, keeping its id, options and state), "Remove" and "Edit <layer>…"; an area's own bound actions, "Customize…" and "Edit <layer>…". A placeholder — a module this build does not have, or cannot draw the way the layout asks — gets the "Fix…" rows instead, remove and reset, which act on the layout rather than on config (TA-7). Nothing is offered on the lock layer (TA-8).
//!
//! **Where.** A menu is a transient laid over the whole window it was asked in (F-2.3, DEC-9): the item's own, or the overlay window where that layer is hidden or an edit mode's host is over it. It opens at the pointer, or on the item when the keyboard asked, and never past an edge of the screen.
//!
//! **One undo entry a row.** A row that changes the layout is one edit through [`session`], so a remove, a reset and a move to another area are each taken back by one undo.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use telar::{Children, ContextMenuProps, MenuEntry, MenuStyle, Rect, context_menu, use_theme};

use config::Edge;
use config::theme::{FontRole, NordTheme};
use layout::{
    Action, AreaId, GroupId, Instance, InstanceId, LayerKind, Layout, LayoutId, LayoutOp,
    Representation, ResolvedArea, ResolvedAreaKind, ResolvedInstance, Trigger,
};
use platform_wayland::KeyboardMode;
use surfaces::menu::Asked;
use surfaces::reconcile::{self, Desktop};
use surfaces::rects::{self, Node, Part};
use surfaces::transient::{self, Anchor, Place, Spec};
use ui::chrome::Chrome;
use ui::descriptor::{Built, ModuleDescriptor};

use crate::session::{self, EditError};
use crate::written::Written;
use crate::{mode, popover};

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

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
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
        return Err(EditError::Refused(telar::t!("editor.menu.lock")));
    }
    let desktop = desktop_of(&node)?;
    let area = area_of(&desktop, &node)?;
    let entries = match &node.part {
        Part::Instance(group, id) => instance_entries(&desktop, &area, &node, group, id)?,
        Part::Area | Part::Group(_) => area_entries(
            &area,
            &Node::area(node.output.as_deref(), node.layer, &node.area),
        ),
    };
    popover::close();
    transient::close(ID);
    let window = match mode::current()
        .is_some_and(|mode| node.output.as_deref() == Some(mode.output.as_str()))
    {
        true => LayerKind::Overlay,
        false => window,
    };
    let rect = rects::rect(&node).unwrap_or_default();
    SHOWN.with(|shown| {
        *shown.borrow_mut() = Some(Shown {
            at: at.unwrap_or((rect.x, rect.y + rect.height)),
            within: Rect::new(0.0, 0.0, desktop.size.0, desktop.size.1),
            entries,
        })
    });
    let anchor = Anchor {
        output: node.output.clone(),
        layer: window,
        edge: area.kind.edge().unwrap_or(Edge::Top),
        rect,
        chrome: Chrome::global(desktop.config.clone(), node.output.clone()),
        gap: 0.0,
    };
    transient::open(
        Spec::new(ID, Place::Over(anchor), Rc::new(|_: &Chrome| tree()))
            .output(node.output.clone())
            .keyboard(KeyboardMode::Exclusive)
            .on_close(|| SHOWN.with(|shown| *shown.borrow_mut() = None)),
    );
    Ok(())
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
        tracing::info!("no context menu: {why}");
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

fn desktop_of(node: &Node) -> Result<Desktop, EditError> {
    reconcile::desktops()
        .iter()
        .find(|desktop| desktop.output == node.output)
        .cloned()
        .ok_or_else(|| gone(&node.area))
}

fn area_of(desktop: &Desktop, node: &Node) -> Result<ResolvedArea, EditError> {
    desktop
        .resolved
        .layer(node.layer)
        .and_then(|layer| layer.areas.iter().find(|area| area.id == node.area))
        .cloned()
        .ok_or_else(|| gone(&node.area))
}

fn gone(area: &AreaId) -> EditError {
    EditError::Refused(telar::t!("editor.popover.gone", id = area.to_string()))
}

/// An instance's rows: its module's actions, customizing it, moving it where it is drawn the other way, removing it and editing its layer — or, for a placeholder, the rows that fix it.
fn instance_entries(
    desktop: &Desktop,
    area: &ResolvedArea,
    node: &Node,
    group: &GroupId,
    id: &InstanceId,
) -> Result<Vec<MenuEntry>, EditError> {
    let resolved = area
        .groups
        .iter()
        .find(|held| held.id == *group)
        .and_then(|held| held.children.iter().find(|child| child.id == *id))
        .cloned()
        .ok_or_else(|| gone(&node.area))?;
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
    let mut rows: Vec<MenuEntry> = module
        .actions
        .iter()
        .map(|action| {
            let line = Action(vec![action.command.to_string()]);
            MenuEntry::row(action_label(action.id), "", move || {
                surfaces::actions::run(&line)
            })
        })
        .collect();
    let customized = node.clone();
    rows.push(MenuEntry::row(
        telar::t!("editor.menu.customize", name = module.name),
        "",
        move || said(popover::open_instance(customized.clone())),
    ));
    rows.extend(
        destinations(desktop, node, &resolved, module)
            .into_iter()
            .map(|to| move_row(node, &resolved, module.name, to)),
    );
    rows.push(remove_row(node, module.name));
    rows.extend(edit_row(node));
    Ok(rows)
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
    rows.extend(edit_row(node));
    rows
}

/// "Edit <layer>…", unless that layer is the one being edited on this screen already.
fn edit_row(node: &Node) -> Option<MenuEntry> {
    let editing = mode::current().is_some_and(|mode| {
        mode.layer == node.layer && node.output.as_deref() == Some(mode.output.as_str())
    });
    if editing {
        return None;
    }
    let (layer, output) = (node.layer, node.output.clone());
    Some(MenuEntry::row(
        telar::t!("editor.menu.edit", layer = mode::name_of(layer)),
        "",
        move || {
            if let Err(why) = mode::enter(layer, output.as_deref()) {
                tracing::info!("{why}");
            }
        },
    ))
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

fn move_row(node: &Node, resolved: &ResolvedInstance, name: &str, to: Destination) -> MenuEntry {
    let label = match to.representation {
        Representation::Chip => telar::t!("editor.menu.to_chip", place = to.place.clone()),
        _ => telar::t!("editor.menu.to_widget", place = to.place.clone()),
    };
    let (node, resolved, name) = (node.clone(), resolved.clone(), name.to_string());
    MenuEntry::row(label, "", move || {
        said(convert(&node, &resolved, &name, &to))
    })
}

/// A row's outcome, which is nowhere to show once the menu that offered it has closed: an edit refused says why in the log.
fn said(done: Result<(), EditError>) {
    if let Err(why) = done {
        tracing::info!("{why}");
    }
}

/// Takes the instance `node` names out of the layout being edited, as one undo entry.
pub fn remove(node: &Node, name: &str) -> Result<(), EditError> {
    let desktop = desktop_of(node)?;
    let ops = removal(&session::draft().peek(), &desktop, node)?;
    commit(telar::t!("editor.menu.removed", name = name), ops)
}

/// Puts the instance `node` names back as the layout it extends — or the built-in one — has it, as one undo entry; one that layout does not have is taken away.
pub fn reset(node: &Node, id: &InstanceId, name: &str) -> Result<(), EditError> {
    let layout = session::draft().peek();
    let base = layout::reset::base_of(&layout, &known());
    let ops = layout::reset::ops(&layout, &base, layout::reset::Target::Instance(id))
        .ok_or_else(|| gone(&node.area))?;
    commit(telar::t!("editor.menu.reset_done", name = name), ops)
}

/// Moves the instance `node` names into `to`, drawn the way `to` draws it, keeping its id, options and bindings: a chip becomes a widget and back without becoming another instance (TA-3). One undo entry.
fn convert(
    node: &Node,
    resolved: &ResolvedInstance,
    name: &str,
    to: &Destination,
) -> Result<(), EditError> {
    let desktop = desktop_of(node)?;
    let layout = session::draft().peek();
    let mut ops = removal(&layout, &desktop, node)?;
    let mut after = layout.clone();
    layout::ops::apply_all(&mut after, &ops).map_err(refused)?;
    let moved = Instance {
        id: resolved.id.clone(),
        module: Some(resolved.module.clone()),
        representation: Some(to.representation),
        options: resolved.options.clone(),
        bindings: resolved.bindings.clone(),
        actions: resolved.actions.clone(),
    };
    let written = Written::area(&after, node.output.as_deref(), to.layer, &to.area)
        .map_err(EditError::Refused)?;
    ops.push(written.instance(&to.group, &resolved.id).op(&moved));
    commit(
        telar::t!("editor.menu.moved", name = name, place = to.place.clone()),
        ops,
    )
}

/// What takes the instance `node` names off its screen: out of the rule that writes it, and, where a broader rule or a layout this one extends still places it there, named in the `remove` of its group as well.
fn removal(layout: &Layout, desktop: &Desktop, node: &Node) -> Result<Vec<LayoutOp>, EditError> {
    let Part::Instance(group, id) = &node.part else {
        return Err(EditError::Refused(telar::t!("editor.popover.nothing")));
    };
    let known = known();
    let mut after = layout.clone();
    let mut ops = Vec::new();
    while placed(&after, &known, desktop, node, id) {
        if ops.len() == 2 {
            return Err(EditError::Refused(telar::t!(
                "editor.menu.still_placed",
                id = id.to_string()
            )));
        }
        let op = Written::area(&after, node.output.as_deref(), node.layer, &node.area)
            .map_err(EditError::Refused)?
            .instance(group, id)
            .removal();
        layout::ops::apply(&mut after, &op).map_err(refused)?;
        ops.push(op);
    }
    Ok(ops)
}

/// Whether `layout` still places `id` in the area `node` names on its screen.
fn placed(
    layout: &Layout,
    known: &BTreeMap<LayoutId, Layout>,
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
    resolved
        .layer(node.layer)
        .and_then(|layer| layer.areas.iter().find(|area| area.id == node.area))
        .is_some_and(|area| {
            area.groups
                .iter()
                .any(|group| group.children.iter().any(|child| child.id == *id))
        })
}

fn known() -> BTreeMap<LayoutId, Layout> {
    surfaces::layouts::read(|store| store.all().clone()).unwrap_or_default()
}

fn refused(why: layout::OpError) -> EditError {
    EditError::Refused(why.to_string())
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

/// An area an instance can be moved to, drawn the other way there.
#[derive(Clone, Debug, PartialEq)]
struct Destination {
    layer: LayerKind,
    area: AreaId,
    group: GroupId,
    representation: Representation,
    /// What the row calls it.
    place: String,
}

/// Where the instance `node` names can go to be drawn the other way: a chip to every grid and free area on its screen as a widget, a widget or a card to every bar and dock as a chip — as far as its module can be drawn that way, and never onto the lock layer.
fn destinations(
    desktop: &Desktop,
    node: &Node,
    resolved: &ResolvedInstance,
    module: &ModuleDescriptor,
) -> Vec<Destination> {
    let is_chip = resolved.representation == Representation::Chip;
    let draws = |representation: Representation| {
        module
            .input(surfaces::area::representation(representation))
            .is_some()
    };
    let as_widget = [
        Representation::WidgetM,
        Representation::WidgetS,
        Representation::WidgetL,
    ]
    .into_iter()
    .find(|representation| draws(*representation));
    let mut found: Vec<Destination> = Vec::new();
    for layer in LayerKind::SESSION {
        let Some(areas) = desktop.resolved.layer(layer) else {
            continue;
        };
        for area in &areas.areas {
            if layer == node.layer && area.id == node.area {
                continue;
            }
            let (representation, place) = match &area.kind {
                ResolvedAreaKind::Bar { .. } if !is_chip && draws(Representation::Chip) => {
                    (Representation::Chip, telar::t!("editor.menu.place.bar"))
                }
                ResolvedAreaKind::Dock { .. } if !is_chip && draws(Representation::Chip) => {
                    (Representation::Chip, telar::t!("editor.menu.place.dock"))
                }
                ResolvedAreaKind::Grid { .. } | ResolvedAreaKind::Free { .. } if is_chip => {
                    match as_widget {
                        Some(widget) => (widget, place_of(layer)),
                        None => continue,
                    }
                }
                _ => continue,
            };
            let group = area.groups.iter().find(|group| !group.stacked).map_or_else(
                || GroupId::new(resolved.id.as_str()),
                |group| group.id.clone(),
            );
            found.push(Destination {
                layer,
                area: area.id.clone(),
                group,
                representation,
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
        ResolvedAreaKind::Prompt { .. } => telar::t!("editor.menu.kind.prompt"),
    }
}

/// The gesture an area's bound action is on, drawn where a row's shortcut would be.
fn trigger_name(trigger: Trigger) -> String {
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
                MenuEntry::Row { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Picks the row that says `label`, as the panel does: the menu closes, then the row acts.
#[cfg(test)]
pub(crate) fn pick(label: &str) {
    let act = SHOWN.with(|shown| {
        shown
            .borrow()
            .iter()
            .flat_map(|shown| shown.entries.iter())
            .find_map(|entry| match entry {
                MenuEntry::Row {
                    label: said, act, ..
                } if said == label => Some(Rc::clone(act)),
                _ => None,
            })
    });
    let act = act.unwrap_or_else(|| panic!("no row says {label:?}: {:?}", rows()));
    transient::close(ID);
    act();
}
