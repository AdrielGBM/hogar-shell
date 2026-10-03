//! What a popover edits: its own copy of one area or instance as the layout writes it, and the controls bound to parts of that copy.
//!
//! Every control of a popover writes into the copy, and one effect previews the copy through the popover's [`Edit`]: the layout as it was when the popover opened, with the operations that write the copy back where it came from. So every change is live on the real item, Esc puts the layout back exactly, and however many controls moved, closing any other way records one entry in the history.
//!
//! Where the copy is written back can change while the popover is open: switching "this workspace only" on moves an area's copy into that workspace's rule ([`crate::variant`]). An instance's copy moves the same way, its changes replayed by name. The copy is then made again from what that rule writes, with every control that has moved written into it once more, so the change lands there alone and nothing the popover did not touch is pinned into the rule.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use telar::{OwnerId, ReadSignal, Rect, RwSignal, effect, signal};

use config::Config;
use layout::{
    Area, AreaKind, Expr, Instance, Layout, LayoutOp, Origin, ResolvedArea, ResolvedInstance, Site,
    Unset,
};
use surfaces::reconcile::Desktop;
use surfaces::rects::{self, Node, Part};

use crate::session::Edit;
use crate::written::{Written, WrittenInstance};

use super::value::{self, Path};

/// Writes one control's value into an area, if it moved from where it started.
type Replay = Rc<dyn Fn(&mut Area)>;

/// An area being customized.
#[derive(Clone)]
pub struct AreaDraft {
    pub node: Node,
    /// The area as it was on screen when the popover opened, which is what a control shows for a value the layout leaves unset.
    pub resolved: Rc<ResolvedArea>,
    /// The configuration of the screen the area is on.
    pub config: Arc<Config>,
    /// The size of that screen, in logical pixels.
    pub screen: (f32, f32),
    area: RwSignal<Area>,
    written: Rc<RefCell<Written>>,
    edit: Edit,
    values: Rc<RefCell<HashMap<&'static str, Box<dyn Any>>>>,
    replays: Rc<RefCell<Vec<Replay>>>,
    /// What the shared values belong to: the popover, so they outlive a rebuild of its rows.
    owner: Option<OwnerId>,
}

impl AreaDraft {
    /// A draft of the area `node` names, as `resolved` shows it and `written` writes it, previewing through `edit`.
    pub fn new(
        edit: &Edit,
        node: Node,
        resolved: ResolvedArea,
        config: Arc<Config>,
        screen: (f32, f32),
        written: Written,
    ) -> Self {
        let area = signal(written.area.clone());
        let written = Rc::new(RefCell::new(written));
        let target = Rc::clone(&written);
        previewing(edit, area, move |changed| {
            let written = target.borrow();
            match *changed == written.area {
                true => Vec::new(),
                false => written.ops(changed),
            }
        });
        let draft = Self {
            node,
            resolved: Rc::new(resolved),
            config,
            screen,
            area,
            written,
            edit: edit.clone(),
            values: Rc::default(),
            replays: Rc::default(),
            owner: telar::current_owner(),
        };
        draft.follow_the_variant();
        draft
    }

    /// Writes the copy where the workspace variant being edited says, whenever that changes under the open popover.
    fn follow_the_variant(&self) {
        if !crate::variant::applies_to(&self.node) {
            return;
        }
        let draft = self.clone();
        let written_for = RefCell::new(crate::variant::editing());
        effect(move || {
            let workspace = crate::variant::workspace();
            if *written_for.borrow() == workspace {
                return;
            }
            *written_for.borrow_mut() = workspace.clone();
            let Some(before) = draft.edit.transaction().before() else {
                return;
            };
            match Written::area(
                &before,
                draft.node.output.as_deref(),
                draft.node.layer,
                &draft.node.area,
                workspace.as_ref(),
            ) {
                Ok(written) => draft.retarget(written),
                Err(why) => tracing::info!("the popover's change stays where it was: {why}"),
            }
        });
    }

    /// Writes the copy back where `written` says from now on: the copy made again from what that rule writes, with every control that has moved written into it once more.
    fn retarget(&self, written: Written) {
        let mut area = written.area.clone();
        for replay in self.replays.borrow().iter() {
            replay(&mut area);
        }
        let ops = match area == written.area {
            true => Vec::new(),
            false => written.ops(&area),
        };
        *self.written.borrow_mut() = written;
        if self.area.peek_with(|now| *now != area) {
            self.area.set(area);
        }
        if self.edit.is_open()
            && let Err(why) = self.edit.preview(ops)
        {
            crate::mode::refuse(why);
        }
    }

    /// The area as the popover has it now, as the layout writes it.
    pub fn area(&self) -> ReadSignal<Area> {
        self.area.read_only()
    }

    /// Whether what the popover writes is laid over `writer`, the level that wrote an expression the area shows: only then does taking it back where the popover writes take it off the screen.
    pub fn lays_over(&self, writer: &Origin) -> bool {
        let site = self.written.borrow().site.clone();
        laid_over(&self.edit, &site, &self.node, writer)
    }

    /// Which kind of area this is, as the layout file spells it.
    pub fn kind(&self) -> &'static str {
        self.resolved.kind.name()
    }

    /// Where the area is on its screen now, preview included. Read in a build or an effect, it follows the area as it moves.
    pub fn rect(&self) -> Option<Rect> {
        rects::rect(&self.node)
    }

    /// The value of the area called `name` that every control of it shares — a row, a handle on the item, another tool's row for the same field: the first to ask starts it at `seed`, and each change is written into the area by `write`.
    pub fn value<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        seed: impl FnOnce() -> T,
        write: impl Fn(&mut Area, &T) + 'static,
    ) -> RwSignal<T> {
        if let Some(shared) = self.shared(name) {
            return shared;
        }
        let area = self.area;
        let write = Rc::new(write);
        let writing = Rc::clone(&write);
        let started = seed();
        let from = started.clone();
        let value = telar::with_owner(self.owner, || {
            bound(started, move |value| {
                update(area, |held| writing(held, value))
            })
        });
        self.values.borrow_mut().insert(name, Box::new(value));
        self.replays
            .borrow_mut()
            .push(Rc::new(move |area: &mut Area| {
                let now = value.peek();
                if now != from {
                    write(area, &now);
                }
            }));
        value
    }

    /// The value called `name`, if a control has made one of that type.
    pub fn shared<T: 'static>(&self, name: &str) -> Option<RwSignal<T>> {
        self.values
            .borrow()
            .get(name)
            .and_then(|held| held.downcast_ref::<RwSignal<T>>())
            .copied()
    }

    /// Whether a control of the popover edits the value called `name`, whatever its type.
    pub fn edits(&self, name: &str) -> bool {
        self.values.borrow().contains_key(name)
    }

    /// The area's geometry as it writes it, made a partial entry of its own kind first when it names none: what a control changing one field of the geometry writes into.
    pub fn kind_mut<'a>(area: &'a mut Area, kind: &'static str) -> Option<&'a mut AreaKind> {
        if area.kind.is_none() {
            area.kind = Some(blank(kind)?);
        }
        area.kind.as_mut()
    }
}

/// Sets the field `$field` of an area's `$variant` geometry to `Some($value)`, making the geometry a partial entry of `$kind` first where the area names none ([`AreaDraft::kind_mut`]).
macro_rules! kind_field {
    ($area:expr, $kind:expr, $variant:ident { $field:ident }, $value:expr) => {{
        let value = $value;
        if let Some(::layout::AreaKind::$variant { $field, .. }) =
            $crate::popover::AreaDraft::kind_mut($area, $kind)
        {
            *$field = Some(value);
        }
    }};
}
pub(crate) use kind_field;

/// Writes one change into an instance, again whenever the copy is made afresh.
type Change = Rc<dyn Fn(&mut Instance)>;

/// What a change to an instance in an area of some kind brings with it on its screen, given the layout with the change made: a widget grown on a grid moves what it now covers out of the way.
pub type Settle = fn(&Node, &Desktop, &Layout) -> Vec<LayoutOp>;

/// An instance being customized.
#[derive(Clone)]
pub struct InstanceDraft {
    pub node: Node,
    pub resolved: Rc<ResolvedInstance>,
    /// The kind of area it is placed in, which decides what it may be drawn as.
    pub area_kind: &'static str,
    /// Its options as the screen has them: its module's sections, its presentation and what the layout sets on it, over one another.
    pub shown: Rc<toml::Table>,
    instance: RwSignal<Instance>,
    written: Rc<RefCell<WrittenInstance>>,
    edit: Edit,
    /// Every change made so far, one per thing changed and in the order they were last made.
    changes: Rc<RefCell<Vec<(String, Change)>>>,
}

impl InstanceDraft {
    /// A draft of the instance `node` names, placed in an area of the kind `area_kind`, as `resolved` and `shown` show it and `written` writes it, previewing through `edit` with whatever `settle` adds.
    pub fn new(
        edit: &Edit,
        node: Node,
        resolved: ResolvedInstance,
        area_kind: &'static str,
        shown: toml::Table,
        written: WrittenInstance,
        settle: Option<(Settle, Desktop)>,
    ) -> Self {
        let instance = signal(written.instance.clone());
        let written = Rc::new(RefCell::new(written));
        let (target, settling, previewed) = (Rc::clone(&written), node.clone(), edit.clone());
        previewing(edit, instance, move |changed| {
            let written = target.borrow();
            if *changed == written.instance {
                return Vec::new();
            }
            let mut ops = written.ops(changed);
            if let Some((settle, desktop)) = &settle
                && let Some(mut after) = previewed.transaction().before()
                && layout::ops::apply_all(&mut after, &ops).is_ok()
            {
                ops.extend(settle(&settling, desktop, &after));
            }
            ops
        });
        let draft = Self {
            node,
            resolved: Rc::new(resolved),
            area_kind,
            shown: Rc::new(shown),
            instance,
            written,
            edit: edit.clone(),
            changes: Rc::default(),
        };
        draft.follow_the_variant();
        draft
    }

    /// Writes the copy where the workspace variant being edited says, whenever that changes under the open popover — the same replay an area's draft does.
    fn follow_the_variant(&self) {
        let Part::Instance(group, id) = self.node.part.clone() else {
            return;
        };
        if !crate::variant::applies_to(&self.node) {
            return;
        }
        let draft = self.clone();
        let written_for = RefCell::new(crate::variant::editing());
        effect(move || {
            let workspace = crate::variant::workspace();
            if *written_for.borrow() == workspace {
                return;
            }
            *written_for.borrow_mut() = workspace.clone();
            let Some(before) = draft.edit.transaction().before() else {
                return;
            };
            match Written::area(
                &before,
                draft.node.output.as_deref(),
                draft.node.layer,
                &draft.node.area,
                workspace.as_ref(),
            ) {
                Ok(written) => draft.retarget(written.instance(&group, &id.template())),
                Err(why) => tracing::info!("the popover's change stays where it was: {why}"),
            }
        });
    }

    /// Writes the copy back where `written` says from now on: made again from what that rule writes, with every change made so far written into it once more.
    fn retarget(&self, written: WrittenInstance) {
        *self.written.borrow_mut() = written;
        self.instance.set(self.replayed());
    }

    /// The instance as the layout writes it, with every change made so far written into it.
    fn replayed(&self) -> Instance {
        let mut instance = self.written.borrow().instance.clone();
        for (_, change) in self.changes.borrow().iter() {
            change(&mut instance);
        }
        instance
    }

    /// Takes back the change called `name`, so the copy is what the layout writes there again, every other change kept.
    fn forget(&self, name: &str) {
        let had = {
            let mut changes = self.changes.borrow_mut();
            let before = changes.len();
            changes.retain(|(held, _)| held != name);
            changes.len() != before
        };
        if !had {
            return;
        }
        let instance = self.replayed();
        if self.instance.peek_with(|now| *now != instance) {
            self.instance.set(instance);
        }
    }

    pub fn instance(&self) -> ReadSignal<Instance> {
        self.instance.read_only()
    }

    /// Makes the change called `name` — the instance's size, one option — replacing whatever change of that name was made before it.
    pub fn update(&self, name: impl Into<String>, change: impl Fn(&mut Instance) + 'static) {
        let change: Change = Rc::new(change);
        {
            let mut changes = self.changes.borrow_mut();
            let name = name.into();
            changes.retain(|(held, _)| *held != name);
            changes.push((name, Rc::clone(&change)));
        }
        update(self.instance, |held| change(held));
    }

    /// What the option at `path` is on screen now.
    pub fn shown_at(&self, path: &[value::Step]) -> Option<toml::Value> {
        let own = self.instance.peek();
        value::get(&own.options, path)
            .or_else(|| value::get(&self.shown, path))
            .cloned()
    }

    /// A value of the option at `path` that controls share: it starts at `seed`, and each change is written into the instance's own options as `to_value` makes it.
    pub fn bind_option<T: Clone + PartialEq + 'static>(
        &self,
        path: Path,
        seed: T,
        to_value: impl Fn(&T) -> toml::Value + 'static,
    ) -> RwSignal<T> {
        let draft = self.clone();
        bound(seed, move |value| draft.set_option(&path, to_value(value)))
    }

    /// Writes `whole` at `path` in the instance's own options — a list or a table of names being one value however many elements it has.
    pub fn set_option(&self, path: &[value::Step], whole: toml::Value) {
        let (shown, at) = (Rc::clone(&self.shown), path.to_vec());
        self.update(option_change(path), move |held| {
            value::set(&mut held.options, &shown, &at, whole.clone())
        });
    }

    /// Takes the option at `path` off the instance, so it shows what it inherits there again.
    pub fn unset(&self, path: &[value::Step]) {
        let at = path.to_vec();
        self.update(option_change(path), move |held| {
            value::unset(&mut held.options, &at)
        });
    }

    /// Whether the instance sets the option at `path` itself.
    pub fn sets(&self, path: &[value::Step]) -> bool {
        self.instance
            .with(|held| value::get(&held.options, path).is_some())
    }

    /// Binds the option at `path` (or `accent`) to `expr`, or takes the instance's own binding there off with `None`, replacing what was asked for that path before. An expression of its own makes taking back the one it inherits moot, so binding one drops that.
    pub fn bind(&self, path: &str, expr: Option<Expr>) {
        let (at, unset) = (path.to_string(), Unset::binding(path));
        self.update(binding_change(path), move |held| match &expr {
            Some(expr) => {
                held.bindings.insert(at.clone(), expr.clone());
                held.unset.retain(|taken| *taken != unset);
            }
            None => {
                held.bindings.remove(&at);
            }
        });
    }

    /// Takes the binding at `path` back where the popover writes (DEC-26): its own expression off, and the one a broader level gives it named in its `unset`, so the option shows its value again.
    pub fn take_back(&self, path: &str) {
        let (at, unset) = (path.to_string(), Unset::binding(path));
        self.update(binding_change(path), move |held| {
            held.bindings.remove(&at);
            if !held.unset.contains(&unset) {
                held.unset.push(unset.clone());
            }
        });
    }

    /// Takes back whatever [`InstanceDraft::bind`] asked for at `path`, so the instance binds it as the layout writes it.
    pub fn keep_binding(&self, path: &str) {
        self.forget(&binding_change(path));
    }

    /// What the instance's bindings read besides the shell's names: `$item` and `$index` where its group repeats, as the layout being edited says where the popover writes it — what validation checks them against.
    pub fn locals(&self) -> layout::Locals {
        let Part::Instance(group, _) = &self.node.part else {
            return layout::Locals::default();
        };
        layout::child_locals(
            &crate::session::draft().peek(),
            &surfaces::catalogue::Descriptors::installed(),
            &self.written.borrow().area.site,
            &self.node.area,
            group,
        )
    }

    /// Whether what the popover writes is laid over `writer`, the level that wrote an expression the instance shows: only then does taking it back where the popover writes take it off the screen.
    pub fn lays_over(&self, writer: &Origin) -> bool {
        let site = self.written.borrow().area.site.clone();
        laid_over(&self.edit, &site, &self.node, writer)
    }

    /// Whether the binding at `path` comes from a level this instance's own entry only lies over: one its own entry cannot take away.
    pub fn inherits_binding(&self, path: &str) -> bool {
        self.resolved.bindings.contains_key(path)
            && !self.written.borrow().instance.bindings.contains_key(path)
    }

    /// Every binding the instance has now, by path: what the screen showed when the popover opened, with what the popover has changed since — what it took back gone.
    pub fn bindings(&self) -> BTreeMap<String, Expr> {
        let mut shown: BTreeMap<String, Expr> = self
            .resolved
            .bindings
            .iter()
            .map(|(path, bound)| (path.clone(), bound.expr.clone()))
            .collect();
        let written = self.written.borrow().instance.bindings.clone();
        self.instance.with(|own| {
            for path in written.keys() {
                if !own.bindings.contains_key(path) {
                    shown.remove(path);
                }
            }
            shown.retain(|path, _| !own.unset.contains(&Unset::binding(path.as_str())));
            shown.extend(own.bindings.clone());
        });
        shown
    }
}

/// The name a change to the option at `path` goes by, which a later change to the same option replaces.
fn option_change(path: &[value::Step]) -> String {
    format!("options.{}", value::dotted(path))
}

/// The name a change to the binding at `path` goes by.
fn binding_change(path: &str) -> String {
    format!("bindings.{path}")
}

fn update<T: Clone + PartialEq + 'static>(held: RwSignal<T>, change: impl FnOnce(&mut T)) {
    let mut next = held.peek();
    change(&mut next);
    if held.peek_with(|now| *now != next) {
        held.set(next);
    }
}

/// A signal starting at `seed` whose every later change is handed to `write`.
fn bound<T: Clone + PartialEq + 'static>(seed: T, write: impl Fn(&T) + 'static) -> RwSignal<T> {
    let value = signal(seed);
    let seeded = Cell::new(false);
    effect(move || {
        let now = value.get();
        if seeded.replace(true) {
            write(&now);
        }
    });
    value
}

/// Previews `copy` through `edit` whenever it changes: the operations `ops` make of it, none when it is back as it was.
fn previewing<T: Clone + PartialEq + 'static>(
    edit: &Edit,
    copy: RwSignal<T>,
    ops: impl Fn(&T) -> Vec<LayoutOp> + 'static,
) {
    let edit = edit.clone();
    effect(move || {
        let changed = copy.get();
        if !edit.is_open() {
            return;
        }
        if let Err(why) = edit.preview(ops(&changed)) {
            crate::mode::refuse(why);
        }
    });
}

/// An entry of the kind `kind` naming nothing, which a partial override fills field by field.
fn blank(kind: &str) -> Option<AreaKind> {
    Some(match kind {
        "bar" => AreaKind::Bar {
            edge: None,
            thickness: None,
            length: None,
            offset: None,
            shape: Default::default(),
            autohide: None,
        },
        "grid" => AreaKind::Grid {
            rect: None,
            cell: None,
            gap: None,
            anchor: None,
        },
        "stack" => AreaKind::Stack {
            anchor: None,
            offset: None,
            width: None,
            output_policy: None,
            routes: Vec::new(),
            launcher: None,
        },
        "wallpaper_region" => AreaKind::WallpaperRegion {
            rect: None,
            source: None,
            fit: None,
            transition: None,
        },
        "texture" => AreaKind::Texture {
            rect: None,
            image: None,
            gradient: None,
            tile: None,
            blend: None,
            opacity: None,
        },
        "dock" => AreaKind::Dock {
            edge: None,
            thickness: None,
        },
        "free" => AreaKind::Free { rect: None },
        "prompt" => AreaKind::Prompt { rect: None },
        _ => return None,
    })
}

/// Whether what the level `site` of the layout `edit` changes writes is laid over `writer` on the screen `node` is on ([`layout::lays_over`]).
fn laid_over(edit: &Edit, site: &Site, node: &Node, writer: &Origin) -> bool {
    let Some(layout) = edit.transaction().before() else {
        return false;
    };
    let level = Origin {
        layout: layout.id.clone(),
        output: site.output.clone(),
        workspace: site.workspace.clone(),
    };
    layout::lays_over(
        &layout,
        node.output.as_deref().unwrap_or_default(),
        &level,
        writer,
    )
}
