//! What a popover edits: its own copy of one area or instance as the layout writes it, and the controls bound to parts of that copy.
//!
//! Every control of a popover writes into the copy, and one effect previews the copy through the popover's [`Edit`]: the layout as it was when the popover opened, with the operations that write the copy back where it came from. So every change is live on the real item, Esc puts the layout back exactly, and however many controls moved, closing any other way records one entry in the history.
//!
//! Where the copy is written back can change while the popover is open: switching "this workspace only" on moves an area's copy into that workspace's rule ([`crate::variant`]). An instance's copy moves the same way, its changes replayed by name. The copy is then made again from what that rule writes, with every control that has moved written into it once more, so the change lands there alone and nothing the popover did not touch is pinned into the rule.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use telar::{LayoutItem, Memo, OwnerId, ReadSignal, Rect, RwSignal, effect, signal};

use config::Config;
use layout::{
    Action, Area, AreaKind, Expr, Group, GroupId, Holder, Instance, Layout, LayoutOp, Origin,
    ResolvedArea, ResolvedGroup, ResolvedInstance, Trigger, Unset,
};
use surfaces::reconcile::Desktop;
use surfaces::rects::{self, Node, Part};

use crate::session::Edit;
use crate::written::{Written, WrittenInstance};

use super::actions::Actions;
use super::origin::{self, Measure, Provenance};
use super::value::{self, Path};

/// Writes one control's value into an area, if it moved from where it started.
type Replay = Rc<dyn Fn(&mut Area)>;

/// Reads one control's value again from the area as drawn, once Reset has taken the key it writes back off.
type Reseed = Rc<dyn Fn(&ResolvedArea)>;

type Take = Rc<dyn Fn(&mut Area)>;

type Capture = Rc<dyn Fn() -> Box<dyn Fn()>>;

/// One value the controls of an area share.
struct Shared<T: 'static> {
    value: RwSignal<T>,
    /// Where it started, or where Reset last put it: it has moved only once it differs from this.
    from: Rc<RefCell<T>>,
}

/// The Reset under way, if one is: the names of the values it has put where the area now draws them, which are not written back as the area's own.
type Resetting = Rc<RefCell<Option<Vec<&'static str>>>>;

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
    /// The copy as the layout file writes it, without its groups: what a key is looked up in to tell whether the popover's level writes it.
    entry: Memo<Option<toml::Table>>,
    written: Rc<RefCell<Written>>,
    edit: Edit,
    measure: Measure,
    values: Rc<RefCell<HashMap<&'static str, Box<dyn Any>>>>,
    replays: Rc<RefCell<Vec<Replay>>>,
    /// What each key Reset takes back reads again, by key: an area's own as the file spells it, a group's under `groups.<id>.`.
    reseeds: Rc<RefCell<Vec<(String, Reseed)>>>,
    /// The keys Reset has taken back so far, which a change of where the popover writes takes back there too.
    taken_off: Rc<RefCell<Vec<(String, Take)>>>,
    resetting: Resetting,
    /// Set while a cancelled drag puts the copy and every value back as it found them, so no value writes itself into the copy on the way.
    putting_back: Rc<Cell<bool>>,
    captures: Rc<RefCell<Vec<Capture>>>,
    /// Moved once a Reset or a put-back has made its writes, which ends it.
    reset_done: RwSignal<u64>,
    /// What the shared values belong to: the popover, so they outlive a rebuild of its rows.
    owner: Option<OwnerId>,
}

impl AreaDraft {
    /// A draft of the area `node` names — or of the area holding the group it names — as `resolved` shows it and `written` writes it, previewing through `edit` with whatever `settle` adds.
    pub fn new(
        edit: &Edit,
        node: Node,
        resolved: ResolvedArea,
        config: Arc<Config>,
        screen: (f32, f32),
        written: Written,
        settle: Option<(Settle, Desktop)>,
    ) -> Self {
        let area = signal(written.area.clone());
        let written = Rc::new(RefCell::new(written));
        let (target, settling, previewed) = (Rc::clone(&written), node.clone(), edit.clone());
        previewing(edit, area, move |changed| {
            let written = target.borrow();
            if *changed == written.area {
                return Vec::new();
            }
            settled(written.ops(changed), settle.as_ref(), &settling, &previewed)
        });
        let entry = telar::memo(move || {
            area.with(|area| {
                let mut entry = origin::table_of(area)?;
                entry.remove("groups");
                Some(entry)
            })
        });
        let draft = Self {
            measure: Measure::new(edit, &node),
            node,
            resolved: Rc::new(resolved),
            config,
            screen,
            area,
            entry,
            written,
            edit: edit.clone(),
            values: Rc::default(),
            replays: Rc::default(),
            reseeds: Rc::default(),
            taken_off: Rc::default(),
            resetting: Rc::default(),
            putting_back: Rc::default(),
            captures: Rc::default(),
            reset_done: signal(0),
            owner: telar::current_owner(),
        };
        draft.end_each_reset();
        draft.follow_the_variant();
        draft
    }

    fn end_each_reset(&self) {
        let (resetting, putting_back, done) = (
            Rc::clone(&self.resetting),
            Rc::clone(&self.putting_back),
            self.reset_done,
        );
        // A flush runs effects in the order they were scheduled, so this runs after everything a Reset's own writes set off directly, which still see the Reset under way, however the writes were batched.
        effect(move || {
            done.with(|_| ());
            resetting.borrow_mut().take();
            putting_back.set(false);
        });
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
                Err(why) => {
                    tracing::info!("the popover's change stays where it was: {}", why.english())
                }
            }
        });
    }

    /// Writes the copy back where `written` says from now on: the copy made again from what that rule writes, with every control that has moved written into it once more.
    fn retarget(&self, written: Written) {
        let mut area = written.area.clone();
        for (_, take) in self.taken_off.borrow().iter() {
            take(&mut area);
        }
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
        self.measure.lays_over(&site, writer)
    }

    /// Where the value written at any of `keys` comes from: of keys that exclude one another, such as a texture's `image` and `gradient`, the one some level writes. Reactive.
    pub fn provenance(&self, keys: &[&str]) -> Provenance {
        let site = self.written.borrow().site.clone();
        keys.iter()
            .map(|key| {
                self.measure
                    .provenance(&site, Holder::Area(&self.node.area), key)
            })
            .find(|found| *found != Provenance::Default)
            .unwrap_or(Provenance::Default)
    }

    /// Whether the level the popover writes into writes any of `keys`, with what the popover has changed so far. Reactive.
    pub fn writes(&self, keys: &[&str]) -> bool {
        self.entry
            .get()
            .is_some_and(|entry| keys.iter().any(|key| origin::holds(&entry, key)))
    }

    /// Takes `keys` off where the popover writes, so they show what they inherit there, and puts every value tied to them ([`AreaDraft::setting`]) where the area now draws it.
    pub fn reset(&self, keys: &[&str]) {
        let takes = keys
            .iter()
            .map(|key| {
                let key = (*key).to_string();
                let taking = key.clone();
                let take: Take = Rc::new(move |area: &mut Area| {
                    if let Some(without) = origin::without(area, &taking) {
                        *area = without;
                    }
                });
                (key, take)
            })
            .collect();
        self.take_back(takes);
    }

    /// Each take is remembered by name, so a change of where the popover writes takes the key back there too.
    fn take_back(&self, takes: Vec<(String, Take)>) {
        let names: Vec<String> = takes.iter().map(|(name, _)| name.clone()).collect();
        for (name, take) in takes {
            update(self.area, |area| take(area));
            let mut taken_off = self.taken_off.borrow_mut();
            taken_off.retain(|(taken, _)| *taken != name);
            taken_off.push((name, take));
        }
        self.resetting.borrow_mut().get_or_insert_with(Vec::new);
        self.reseed(&names);
        self.reset_done.update(|count| *count += 1);
    }

    fn reseed(&self, keys: &[String]) {
        let Some(drawn) = self.drawn() else {
            return;
        };
        let reseeds: Vec<Reseed> = self
            .reseeds
            .borrow()
            .iter()
            .filter(|(tied, _)| keys.contains(tied))
            .map(|(_, reseed)| Rc::clone(reseed))
            .collect();
        for reseed in reseeds {
            reseed(&drawn);
        }
    }

    /// Whether a Reset is putting values where the area now draws them, or a cancelled drag where it found them, read by what keeps one value in step with another so it does not take that for a change of the other. It stays so until whatever its writes set off directly has run.
    pub fn is_resetting(&self) -> bool {
        self.resetting.borrow().is_some() || self.putting_back.get()
    }

    /// `row` with what [`super::origin::marked`] adds for a value written at any of `keys`: where it comes from, and a Reset while the popover's level writes it.
    pub fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> ui::descriptor::Built {
        let keys: Rc<[&'static str]> = Rc::from(keys);
        let (standing, writing, resetting) = (self.clone(), self.clone(), self.clone());
        let (seen, written, taken) = (Rc::clone(&keys), Rc::clone(&keys), keys);
        origin::marked(
            row,
            move || standing.provenance(&seen),
            move || writing.writes(&written),
            move || resetting.reset(&taken),
        )
    }

    /// The area as the screen draws it with the copy as it is now.
    fn drawn(&self) -> Option<ResolvedArea> {
        let copy = self.area.peek();
        let ops = {
            let written = self.written.borrow();
            match copy == written.area {
                true => Vec::new(),
                false => written.ops(&copy),
            }
        };
        self.measure
            .drawn_with(&ops)?
            .resolved
            .area(self.node.layer, &self.node.area)
            .cloned()
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
        let from = Rc::new(RefCell::new(started.clone()));
        let (resetting, putting_back, put) = (
            Rc::clone(&self.resetting),
            Rc::clone(&self.putting_back),
            Rc::clone(&from),
        );
        let value = telar::with_owner(self.owner, || {
            bound(started, move |value| {
                let put_back = putting_back.get()
                    || resetting
                        .borrow()
                        .as_ref()
                        .is_some_and(|names| names.contains(&name))
                        && *value == *put.borrow();
                if !put_back {
                    update(area, |held| writing(held, value))
                }
            })
        });
        let moved_from = Rc::clone(&from);
        self.values
            .borrow_mut()
            .insert(name, Box::new(Shared { value, from }));
        self.captures.borrow_mut().push(Rc::new(move || {
            let then = value.peek();
            Box::new(move || {
                if value.peek_with(|now| *now != then) {
                    value.set(then.clone());
                }
            })
        }));
        self.replays
            .borrow_mut()
            .push(Rc::new(move |area: &mut Area| {
                let now = value.peek();
                if now != *moved_from.borrow() {
                    write(area, &now);
                }
            }));
        value
    }

    /// [`AreaDraft::value`] for a value the area writes at the key `key` as the layout file spells it (`style.fill`, `thickness`, `rect`): started at what `read` finds in the area as it was drawn when the popover opened, and read again from the area as drawn once Reset takes the key back off ([`AreaDraft::reset`]).
    pub fn setting<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&ResolvedArea) -> T + 'static,
        write: impl Fn(&mut Area, &T) + 'static,
    ) -> RwSignal<T> {
        self.setting_tied(name, key.to_string(), read, write)
    }

    fn setting_tied<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        tied: String,
        read: impl Fn(&ResolvedArea) -> T + 'static,
        write: impl Fn(&mut Area, &T) + 'static,
    ) -> RwSignal<T> {
        if let Some(shared) = self.shared(name) {
            return shared;
        }
        let read = Rc::new(read);
        let (seeding, resolved) = (Rc::clone(&read), Rc::clone(&self.resolved));
        let value = self.value(name, move || seeding(&resolved), write);
        let (values, resetting) = (Rc::clone(&self.values), Rc::clone(&self.resetting));
        let reseed: Reseed = Rc::new(move |drawn: &ResolvedArea| {
            let Some((value, from)) = values
                .borrow()
                .get(name)
                .and_then(|held| held.downcast_ref::<Shared<T>>())
                .map(|shared| (shared.value, Rc::clone(&shared.from)))
            else {
                return;
            };
            let next = read(drawn);
            *from.borrow_mut() = next.clone();
            if value.peek_with(|now| *now != next) {
                if let Some(names) = resetting.borrow_mut().as_mut() {
                    names.push(name);
                }
                value.set(next);
            }
        });
        self.reseeds.borrow_mut().push((tied, reseed));
        value
    }

    pub fn grip(&self) -> Grip {
        Grip {
            draft: self.clone(),
            found: Rc::default(),
        }
    }

    /// The value called `name`, if a control has made one of that type.
    pub fn shared<T: 'static>(&self, name: &str) -> Option<RwSignal<T>> {
        self.values
            .borrow()
            .get(name)
            .and_then(|held| held.downcast_ref::<Shared<T>>())
            .map(|shared| shared.value)
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

    pub fn actions(&self) -> Actions {
        let seed = self.area.peek().actions;
        let written = self.value(
            "actions",
            || seed,
            |area, actions: &BTreeMap<Trigger, Action>| area.actions = actions.clone(),
        );
        let unwritten = self.value("actions.unwritten", BTreeSet::new, |_, _| {});
        let (measure, node) = (self.measure.clone(), self.node.clone());
        let drawn = Rc::new(move || {
            measure
                .screen()
                .and_then(|screen| {
                    screen
                        .area(node.layer, &node.area)
                        .map(|area| area.actions.clone())
                })
                .unwrap_or_default()
        });
        let standing = self.clone();
        Actions::new(
            written,
            unwritten,
            drawn,
            Rc::new(move |key: &str| standing.provenance(&[key])),
        )
    }
}

/// The copy and every value of an area's popover as a handle's drag found them, from its first move until it is kept or cancelled. Putting each value back where it was would write it into the copy, pinning a key the layout left unset, so a cancelled drag puts the copy back whole instead.
#[derive(Clone)]
pub struct Grip {
    draft: AreaDraft,
    found: Rc<RefCell<Option<Found>>>,
}

struct Found {
    area: Area,
    restores: Vec<Box<dyn Fn()>>,
}

impl Grip {
    /// Takes hold of the copy and the values as they are now, unless the drag already has: called on each move, before the move writes anything.
    pub fn hold(&self) {
        let mut found = self.found.borrow_mut();
        if found.is_none() {
            let restores = self
                .draft
                .captures
                .borrow()
                .iter()
                .map(|capture| capture())
                .collect();
            *found = Some(Found {
                area: self.draft.area.peek(),
                restores,
            });
        }
    }

    pub fn release(&self) {
        self.found.borrow_mut().take();
    }

    pub fn put_back(&self) {
        let Some(Found { area, restores }) = self.found.borrow_mut().take() else {
            return;
        };
        let draft = &self.draft;
        draft.putting_back.set(true);
        for restore in &restores {
            restore();
        }
        if draft.area.peek_with(|now| *now != area) {
            draft.area.set(area);
        }
        draft.reset_done.update(|count| *count += 1);
    }
}

pub(crate) fn group_entry<'a>(area: &'a mut Area, id: &GroupId) -> &'a mut Group {
    let at = match area.groups.iter().position(|group| group.id == *id) {
        Some(at) => at,
        None => {
            area.groups.push(Group {
                id: id.clone(),
                ..Group::default()
            });
            area.groups.len() - 1
        }
    };
    &mut area.groups[at]
}

/// Written through a draft of the area that holds it, so it previews, reverts and records as the area's would.
#[derive(Clone)]
pub struct GroupDraft {
    pub area: AreaDraft,
    pub id: GroupId,
    /// The group as it was on screen when the popover opened.
    pub resolved: Rc<ResolvedGroup>,
}

impl GroupDraft {
    pub fn new(area: AreaDraft, resolved: ResolvedGroup) -> Self {
        Self {
            area,
            id: resolved.id.clone(),
            resolved: Rc::new(resolved),
        }
    }

    pub fn group(&self) -> Option<Group> {
        self.area.area.with(|area| {
            area.groups
                .iter()
                .find(|group| group.id == self.id)
                .cloned()
        })
    }

    fn tied(&self, key: &str) -> String {
        format!("groups.{}.{key}", self.id)
    }

    pub fn setting<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        key: &'static str,
        read: impl Fn(&ResolvedGroup) -> T + 'static,
        write: impl Fn(&mut Group, &T) + 'static,
    ) -> RwSignal<T> {
        let (reading, writing, fallback) =
            (self.id.clone(), self.id.clone(), Rc::clone(&self.resolved));
        self.area.setting_tied(
            name,
            self.tied(key),
            move |area| {
                let drawn = area.groups.iter().find(|group| group.id == reading);
                read(drawn.unwrap_or(&fallback))
            },
            move |area, value| write(group_entry(area, &writing), value),
        )
    }

    pub fn provenance(&self, keys: &[&str]) -> Provenance {
        let site = self.area.written.borrow().site.clone();
        keys.iter()
            .map(|key| {
                self.area.measure.provenance(
                    &site,
                    Holder::Group(&self.area.node.area, &self.id),
                    key,
                )
            })
            .find(|found| *found != Provenance::Default)
            .unwrap_or(Provenance::Default)
    }

    pub fn writes(&self, keys: &[&str]) -> bool {
        self.group()
            .and_then(|group| origin::table_of(&group))
            .is_some_and(|entry| keys.iter().any(|key| origin::holds(&entry, key)))
    }

    /// An entry left naming nothing but its id goes with the keys.
    pub fn reset(&self, keys: &[&str]) {
        let takes = keys
            .iter()
            .map(|key| {
                let (id, taking) = (self.id.clone(), (*key).to_string());
                let take: Take = Rc::new(move |area: &mut Area| {
                    let Some(at) = area.groups.iter().position(|group| group.id == id) else {
                        return;
                    };
                    if let Some(without) = origin::without(&area.groups[at], &taking) {
                        area.groups[at] = without;
                    }
                    let bare = Group {
                        id: id.clone(),
                        ..Group::default()
                    };
                    if area.groups[at] == bare {
                        area.groups.remove(at);
                    }
                });
                (self.tied(key), take)
            })
            .collect();
        self.area.take_back(takes);
    }

    pub fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> ui::descriptor::Built {
        let keys: Rc<[&'static str]> = Rc::from(keys);
        let (standing, writing, resetting) = (self.clone(), self.clone(), self.clone());
        let (seen, written, taken) = (Rc::clone(&keys), Rc::clone(&keys), keys);
        origin::marked(
            row,
            move || standing.provenance(&seen),
            move || writing.writes(&written),
            move || resetting.reset(&taken),
        )
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

/// Reads the field `$field` of an area's resolved `$variant` geometry, or `$fallback` where the area is drawn as another kind: what a control's value starts at, and is read again at after a Reset ([`AreaDraft::setting`]).
macro_rules! kind_read {
    ($variant:ident { $field:ident }, $fallback:expr) => {
        move |area: &::layout::ResolvedArea| match &area.kind {
            ::layout::ResolvedAreaKind::$variant { $field, .. } => {
                ::std::clone::Clone::clone($field)
            }
            _ => ::std::clone::Clone::clone(&$fallback),
        }
    };
}
pub(crate) use kind_read;

type Gestures = RwSignal<BTreeMap<Trigger, Action>>;

/// The gestures given a row that runs nothing yet, which are not written until it does.
type Unwritten = RwSignal<BTreeSet<Trigger>>;

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
    measure: Measure,
    /// Its options as the screen has them once Reset has taken one back, which is what `shown` says until then.
    reshown: Rc<RefCell<Option<Rc<toml::Table>>>>,
    /// Every change made so far, one per thing changed and in the order they were last made.
    changes: Rc<RefCell<Vec<(String, Change)>>>,
    /// Moved by every Reset, which builds the rows it touches again from the instance as now drawn.
    generation: RwSignal<u64>,
    values: Rc<RefCell<HashMap<&'static str, Box<dyn Any>>>>,
    actions: Rc<RefCell<Option<(Gestures, Unwritten)>>>,
    /// What the values its rows share belong to: the popover, so they outlive a rebuild of its rows.
    owner: Option<OwnerId>,
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
            settled(written.ops(changed), settle.as_ref(), &settling, &previewed)
        });
        let draft = Self {
            measure: Measure::new(edit, &node),
            node,
            resolved: Rc::new(resolved),
            area_kind,
            shown: Rc::new(shown),
            instance,
            written,
            edit: edit.clone(),
            reshown: Rc::default(),
            changes: Rc::default(),
            generation: signal(0),
            values: Rc::default(),
            actions: Rc::default(),
            owner: telar::current_owner(),
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
                Err(why) => {
                    tracing::info!("the popover's change stays where it was: {}", why.english())
                }
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
        if let Some(set) = value::get(&own.options, path) {
            return Some(set.clone());
        }
        value::get(&self.shown_now(), path).cloned()
    }

    fn shown_now(&self) -> Rc<toml::Table> {
        match self.reshown.borrow().as_ref() {
            Some(reshown) => Rc::clone(reshown),
            None => Rc::clone(&self.shown),
        }
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
        let (shown, at) = (self.shown_now(), path.to_vec());
        self.update(option_change(path), move |held| {
            value::set(&mut held.options, &shown, &at, whole.clone())
        });
    }

    /// Takes the option at `path` off the instance, so it shows what it inherits there again — a level under the popover's, its presentation or its module's section — which is what a row built for it afterwards starts at.
    pub fn reset_option(&self, path: &[value::Step]) {
        let at = path.to_vec();
        self.update(option_change(path), move |held| {
            value::unset(&mut held.options, &at)
        });
        self.reshow();
    }

    fn reshow(&self) {
        if let Some(reshown) = self.drawn_options() {
            *self.reshown.borrow_mut() = Some(Rc::new(reshown));
        }
    }

    /// The instance's options as the screen shows them with the copy as it is now.
    fn drawn_options(&self) -> Option<toml::Table> {
        let (config, drawn) = self.drawn_on_screen()?;
        Some(super::instance::shown(
            &config,
            &self.resolved.module,
            &drawn.options,
        ))
    }

    fn drawn_on_screen(&self) -> Option<(Arc<Config>, ResolvedInstance)> {
        let Part::Instance(group, id) = &self.node.part else {
            return None;
        };
        let copy = self.instance.peek();
        let ops = {
            let written = self.written.borrow();
            match copy == written.instance {
                true => Vec::new(),
                false => written.ops(&copy),
            }
        };
        let screen = self.measure.drawn_with(&ops)?;
        let drawn = screen
            .resolved
            .area(self.node.layer, &self.node.area)?
            .groups
            .iter()
            .find(|held| held.id == *group)?
            .children
            .iter()
            .find(|child| child.id == id.template())?
            .clone();
        Some((screen.config, drawn))
    }

    pub fn drawn(&self) -> ResolvedInstance {
        self.drawn_on_screen()
            .map_or_else(|| (*self.resolved).clone(), |(_, drawn)| drawn)
    }

    /// Where the option at `path` comes from on the screen as drawn now: an option inside a list is the list's, which a level writes whole. Reactive.
    pub fn option_provenance(&self, path: &[value::Step]) -> Provenance {
        let written: Vec<&str> = path
            .iter()
            .map_while(|step| match step {
                value::Step::Key(key) => Some(key.as_str()),
                value::Step::Index(_) => None,
            })
            .collect();
        self.provenance(&[&format!("options.{}", written.join("."))])
    }

    /// Whether the instance writes the option at `path` itself. Reactive.
    pub fn writes_option(&self, path: &[value::Step]) -> bool {
        self.instance
            .with(|held| value::get(&held.options, path).is_some())
    }

    pub fn provenance(&self, keys: &[&str]) -> Provenance {
        let Part::Instance(group, id) = &self.node.part else {
            return Provenance::Default;
        };
        let site = self.written.borrow().area.site.clone();
        let id = id.template();
        keys.iter()
            .map(|key| {
                self.measure
                    .provenance(&site, Holder::Instance(&self.node.area, group, &id), key)
            })
            .find(|found| *found != Provenance::Default)
            .unwrap_or(Provenance::Default)
    }

    pub fn writes(&self, keys: &[&str]) -> bool {
        self.instance.with(|held| {
            origin::table_of(held)
                .is_some_and(|entry| keys.iter().any(|key| origin::holds(&entry, key)))
        })
    }

    /// The rows are built again rather than reseeded, so they start at what is now inherited.
    pub fn reset(&self, keys: &[&str]) {
        for key in keys {
            let taking = (*key).to_string();
            self.update(format!("reset.{key}"), move |held| {
                if let Some(without) = origin::without(held, &taking) {
                    *held = without;
                }
            });
        }
        self.generation.update(|built| *built += 1);
    }

    pub fn reset_all(&self) {
        self.update("reset", |held| {
            held.options = toml::Table::new();
            held.style = layout::Style::default();
        });
        self.reshow();
        self.generation.update(|built| *built += 1);
    }

    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    pub fn marked(&self, keys: &[&'static str], row: Box<dyn LayoutItem>) -> ui::descriptor::Built {
        let keys: Rc<[&'static str]> = Rc::from(keys);
        let (standing, writing, resetting) = (self.clone(), self.clone(), self.clone());
        let (seen, written, taken) = (Rc::clone(&keys), Rc::clone(&keys), keys);
        origin::marked(
            row,
            move || standing.provenance(&seen),
            move || writing.writes(&written),
            move || resetting.reset(&taken),
        )
    }

    pub fn setting<T: Clone + PartialEq + 'static>(
        &self,
        name: &'static str,
        read: impl Fn(&ResolvedInstance) -> T,
        write: impl Fn(&mut Instance, &T) + 'static,
    ) -> RwSignal<T> {
        let seed = read(&self.drawn());
        let (draft, write) = (self.clone(), Rc::new(write));
        let value = bound(seed, move |value| {
            let (value, write) = (value.clone(), Rc::clone(&write));
            draft.update(name, move |held| write(held, &value));
        });
        self.values.borrow_mut().insert(name, Box::new(value));
        value
    }

    pub fn view<T: 'static>(&self, name: &'static str, seed: T) -> RwSignal<T> {
        let value = signal(seed);
        self.values.borrow_mut().insert(name, Box::new(value));
        value
    }

    pub fn shared<T: 'static>(&self, name: &str) -> Option<RwSignal<T>> {
        self.values
            .borrow()
            .get(name)
            .and_then(|held| held.downcast_ref::<RwSignal<T>>())
            .copied()
    }

    pub fn actions(&self) -> Actions {
        let (written, unwritten) = *self.actions.borrow_mut().get_or_insert_with(|| {
            let seed = self.instance.peek().actions;
            let draft = self.clone();
            telar::with_owner(self.owner, || {
                let written = bound(seed, move |actions: &BTreeMap<Trigger, Action>| {
                    let actions = actions.clone();
                    draft.update("actions", move |held| held.actions = actions.clone());
                });
                (written, signal(BTreeSet::new()))
            })
        });
        let (measure, node) = (self.measure.clone(), self.node.clone());
        let drawn = Rc::new(move || {
            let Part::Instance(group, id) = &node.part else {
                return BTreeMap::new();
            };
            measure
                .screen()
                .and_then(|screen| {
                    screen
                        .area(node.layer, &node.area)?
                        .groups
                        .iter()
                        .find(|held| held.id == *group)?
                        .children
                        .iter()
                        .find(|child| child.id == id.template())
                        .map(|child| child.actions.clone())
                })
                .unwrap_or_default()
        });
        let standing = self.clone();
        Actions::new(
            written,
            unwritten,
            drawn,
            Rc::new(move |key: &str| standing.provenance(&[key])),
        )
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
        self.measure.lays_over(&site, writer)
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

fn settled(
    mut ops: Vec<LayoutOp>,
    settle: Option<&(Settle, Desktop)>,
    node: &Node,
    edit: &Edit,
) -> Vec<LayoutOp> {
    if let Some((settle, desktop)) = settle
        && let Some(mut after) = edit.transaction().before()
        && layout::ops::apply_all(&mut after, &ops).is_ok()
    {
        ops.extend(settle(node, desktop, &after));
    }
    ops
}

/// A signal starting at `seed` whose every later change is handed to `write`: setting it to the value it already holds is no change.
fn bound<T: Clone + PartialEq + 'static>(seed: T, write: impl Fn(&T) + 'static) -> RwSignal<T> {
    let value = signal(seed);
    let held: RefCell<Option<T>> = RefCell::new(None);
    effect(move || {
        let now = value.get();
        let before = held.borrow_mut().replace(now.clone());
        if before.is_some_and(|before| before != now) {
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
            flow: None,
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
        "free" => AreaKind::Free {
            rect: None,
            anchor: None,
        },
        "panel" => AreaKind::Panel {
            owner: None,
            along: None,
            cols: None,
            rows: None,
            cell: None,
            gap: None,
        },
        "prompt" => AreaKind::Prompt { rect: None },
        _ => return None,
    })
}
