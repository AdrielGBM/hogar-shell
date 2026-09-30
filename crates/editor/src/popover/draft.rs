//! What a popover edits: its own copy of one area or instance as the layout writes it, and the controls bound to parts of that copy.
//!
//! Every control of a popover writes into the copy, and one effect previews the copy through the popover's [`Edit`]: the layout as it was when the popover opened, with the one operation that writes the copy back where it came from. So every change is live on the real item, Esc puts the layout back exactly, and however many controls moved, closing any other way records one entry in the history.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use telar::{OwnerId, ReadSignal, Rect, RwSignal, effect, signal};

use config::Config;
use layout::{Area, AreaKind, Instance, LayoutOp, ResolvedArea, ResolvedInstance};
use surfaces::rects::{self, Node};

use crate::session::Edit;
use crate::written::{Written, WrittenInstance};

use super::value::{self, Path};

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
    values: Rc<RefCell<HashMap<&'static str, Box<dyn Any>>>>,
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
        previewing(edit, area, move |changed| {
            (*changed != written.area).then(|| written.op(changed))
        });
        Self {
            node,
            resolved: Rc::new(resolved),
            config,
            screen,
            area,
            values: Rc::default(),
            owner: telar::current_owner(),
        }
    }

    /// The area as the popover has it now, as the layout writes it.
    pub fn area(&self) -> ReadSignal<Area> {
        self.area.read_only()
    }

    /// Changes the area; a change that leaves it as it was is no change.
    pub fn update(&self, change: impl FnOnce(&mut Area)) {
        update(self.area, change);
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
        let value = telar::with_owner(self.owner, || {
            bound(seed(), move |value| update(area, |held| write(held, value)))
        });
        self.values.borrow_mut().insert(name, Box::new(value));
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

    /// The area's geometry as it writes it, made a partial entry of its own kind first when it names none: what a control changing one field of the geometry writes into.
    pub fn kind_mut<'a>(area: &'a mut Area, kind: &'static str) -> Option<&'a mut AreaKind> {
        if area.kind.is_none() {
            area.kind = Some(blank(kind)?);
        }
        area.kind.as_mut()
    }
}

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
}

impl InstanceDraft {
    /// A draft of the instance `node` names, placed in an area of the kind `area_kind`, as `resolved` and `shown` show it and `written` writes it, previewing through `edit`.
    pub fn new(
        edit: &Edit,
        node: Node,
        resolved: ResolvedInstance,
        area_kind: &'static str,
        shown: toml::Table,
        written: WrittenInstance,
    ) -> Self {
        let instance = signal(written.instance.clone());
        previewing(edit, instance, move |changed| {
            (*changed != written.instance).then(|| written.op(changed))
        });
        Self {
            node,
            resolved: Rc::new(resolved),
            area_kind,
            shown: Rc::new(shown),
            instance,
        }
    }

    pub fn instance(&self) -> ReadSignal<Instance> {
        self.instance.read_only()
    }

    pub fn update(&self, change: impl FnOnce(&mut Instance)) {
        update(self.instance, change);
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
        let (instance, shown) = (self.instance, Rc::clone(&self.shown));
        bound(seed, move |value| {
            update(instance, |held| {
                value::set(&mut held.options, &shown, &path, to_value(value));
            })
        })
    }

    /// Takes the option at `path` off the instance, so it shows what it inherits there again.
    pub fn unset(&self, path: &[value::Step]) {
        self.update(|held| value::unset(&mut held.options, path));
    }

    /// Whether the instance sets the option at `path` itself.
    pub fn sets(&self, path: &[value::Step]) -> bool {
        self.instance
            .with(|held| value::get(&held.options, path).is_some())
    }
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

/// Previews `copy` through `edit` whenever it changes: the operation `op` makes of it, or nothing when it is back as it was.
fn previewing<T: Clone + PartialEq + 'static>(
    edit: &Edit,
    copy: RwSignal<T>,
    op: impl Fn(&T) -> Option<LayoutOp> + 'static,
) {
    let edit = edit.clone();
    effect(move || {
        let changed = copy.get();
        if !edit.is_open() {
            return;
        }
        if let Err(why) = edit.preview(op(&changed).into_iter().collect()) {
            tracing::info!("the popover's change was not previewed: {why}");
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
            width: None,
            output_policy: None,
            routes: Vec::new(),
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
