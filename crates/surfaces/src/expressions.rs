//! Where a layout's expressions meet the screen: an area's `visible` and an instance's `bindings`, bound to live readings for as long as what they drive is built.
//!
//! **When they read.** An expression subscribes to what it reads only while its window is on screen — a hidden window keeps its tree (F-2.16), so the tree going away cannot be what stops it — while its area's own `visible` holds, and, for anything but the lock screen, while the session is unlocked ([`automation::Gate`]).
//!
//! **What moves.** A `visible` that flips takes its area out of layout and back, which neither rebuilds it nor touches the window or the areas beside it (F-10.42). A binding whose value changes builds its instance again, alone ([`Expressions::bound_instance`]): the area, the window and every other instance keep their nodes, and what the instance keeps in an `InstanceStore` survives because the store is keyed by its id (F-3.4).
//!
//! **Copies.** A group with `repeat` draws its children once per item of a list ([`Repeat`]): copy `<id>#<index>` reads its item as `$item` and its place as `$index`, keyed by index, so a list that grows or shrinks adds or drops copies at its end and leaves the others and the group's siblings as they were. An item that changes in place moves only what reads it. What a drawn copy reads is there for the editor too ([`drawn_item`]).
//!
//! **Komponents.** What a komponent holds reads the parameters of the use it is drawn in before any of the shell's names ([`layout::ResolvedExpr::within`]): each parameter's value — what the use sets it to, else the komponent's default — is bound where the group is drawn, on the same gate, and read as a local the way a copy reads `$item`. One that does not check reads as its type's empty value, so what the komponent holds still draws, and is reported where its value is written.
//!
//! **Variables.** An expression is checked against the variables there are, and checked again — its old binding taken down, a new one made — as one it names appears, goes or changes type, so a binding to a variable the user sets later comes alive then, without a reload ([`automation::vars::declared`]).
//!
//! **What an error means.** An expression that does not check here — a layout `layout check` would have refused, or warned of for a variable not set yet — is left out until it does: an area with no `visible` it can read is shown, an instance draws its written options. One that compiles keeps its last good value through an evaluation error, and a `visible` that has never answered is shown, since hiding what the user placed over a reading that has not arrived yet is the worse mistake. A `repeat` is different, since there is nothing to draw a copy of but an item: one that cannot be read, or has not answered yet, draws no copies. Every failure is an [`automation::failures`] site for as long as it lasts, in the file and under the key `layout check` names the expression by — the level that wrote it, as resolution recorded ([`layout::Origin`], [`site`]). A reading that has not arrived yet and a variable not set yet are waits, not failures.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use automation::bindings::{self, Target, Written};
use automation::env::{awaits_reading, describe};
use automation::failures::{self, Drawn, Site};
use automation::{Environment, Gate, Local};
use config::fields::OptionField;
use layout::{KomponentUse, Origin, ResolvedExpr, ResolvedInstance, ResolvedParameter};
use telar::{Color, LayoutStyle, Memo, OwnerId, ReactiveList, ReadSignal, RwSignal};
use telar_expression::{Compiled, Errors, Held, Type, Value};
use ui::descriptor::Built;
use ui::host::Audience;
use util::report::Message;

use crate::layer_window::LayerWindowContext;
use crate::rects::{Node, Part};

/// Whether the area being built is shown by its own `visible`, for what is built inside it.
#[derive(Clone, Copy)]
struct AreaShown(Memo<bool>);

/// The copy of a repeated group's children being built, for what is built inside it.
#[derive(Clone)]
struct CopyOf {
    repeat: Repeat,
    index: usize,
}

/// The names an expression drawn here reads, for whom, and while what is on screen.
#[derive(Clone)]
pub struct Expressions {
    env: Environment,
    copy: Option<CopyOf>,
    gate: Gate,
}

impl Expressions {
    /// What an expression built here reads: the running shell's names, shown to `audience`, gated on the window it is drawn in being on screen and on the area around it being shown.
    pub fn here(audience: Audience) -> Self {
        let window: Option<ReadSignal<bool>> =
            LayerWindowContext::current().map(|window| window.mapped);
        let area = telar::context::<AreaShown>();
        Self {
            env: Environment::running().on_layer(audience == Audience::Anyone),
            copy: telar::context::<CopyOf>(),
            gate: Gate::new(move || {
                window.is_none_or(|mapped| mapped.get())
                    && area.is_none_or(|AreaShown(shown)| shown.get())
            }),
        }
    }

    /// The names as they are now, with the copy's own `$item` and `$index` before them inside a copy.
    fn env(&self) -> Environment {
        self.env_within(None, None)
    }

    /// [`Expressions::env`] for an expression drawn `within` a komponent use, which reads the use's parameters before the shell's names too; `output` is the screen it is drawn on.
    fn env_within(&self, within: Option<&KomponentUse>, output: Option<&str>) -> Environment {
        let mut locals = match &self.copy {
            Some(CopyOf { repeat, index }) => repeat.locals(*index),
            None => Vec::new(),
        };
        if let Some(used) = within {
            locals.extend(
                used.parameters
                    .iter()
                    .map(|parameter| self.parameter(used, parameter, output)),
            );
        }
        self.env.clone().with_locals(locals)
    }

    /// One parameter of the komponent use `used` as a local: what it reads at the use, bound while this is on screen, waited for until it first answers. One that does not check reads as its type's empty value, so what the komponent holds still draws, and its failure is reported where its value is written.
    fn parameter(
        &self,
        used: &KomponentUse,
        parameter: &ResolvedParameter,
        output: Option<&str>,
    ) -> Local {
        let site = parameter_site(used, parameter, output);
        let (name, ty) = (parameter.name.as_str(), parameter.ty.clone());
        let empty = Value::empty(&ty);
        match checked(&self.env, &parameter.expr().0, &ty) {
            Ok(compiled) => {
                let held = self.env.bind(compiled, self.gate.clone());
                report(held, site);
                Local::live(name, ty, move || held.get().value)
            }
            Err(unready) => {
                unready.report(&site);
                Local::live(name, ty, move || empty.clone())
            }
        }
    }

    /// Whether the area `node` names is shown by its `visible`, as it changes. Made available to what is built under the current owner afterwards, so the instances inside stop reading while their area is hidden.
    pub fn visible(&self, node: &Node, expr: &ResolvedExpr) -> Memo<bool> {
        let site = site(node, Slot::Visible, &expr.origin);
        let (this, source) = (self.clone(), expr.expr.0.clone());
        let held = remade(move || {
            let env = this.env();
            match checked(&env, &source, &Type::Bool) {
                Ok(compiled) => {
                    let held = env.bind(compiled, this.gate.clone());
                    report(held, site.clone());
                    Some(held)
                }
                Err(unready) => {
                    unready.report(&site);
                    None
                }
            }
        });
        let shown = telar::memo(move || {
            held.get()
                .is_none_or(|held| !matches!(held.get().value, Some(Value::Bool(false))))
        });
        telar::set_context(AreaShown(shown));
        shown
    }

    /// The list the group `node` names repeats its children over, as it changes; no items while it cannot be read here. Through an evaluation error it keeps its last list. The copies drawn from it are what [`drawn_item`] reads for as long as the current owner lives.
    pub fn repeat(&self, node: &Node, expr: &ResolvedExpr) -> Repeat {
        let (this, within, output) = (self.clone(), expr.within.clone(), node.output.clone());
        let repeat = Repeat::made(
            move || this.env_within(within.as_deref(), output.as_deref()),
            expr.expr.0.clone(),
            self.gate.clone(),
            site(node, Slot::Repeat, &expr.origin),
        );
        drawn(node.clone(), &repeat);
        repeat
    }

    /// `instance`, placed at `node`, built by `build` under what its bindings say over its written options. One with no bindings this shell can read is built once, laid out by `style`, with no overlay. One with bindings is built again — alone — each time they say something else, inside a box laid out by `style` that each build is told to fill.
    pub fn bound_instance(
        &self,
        instance: &ResolvedInstance,
        node: &Node,
        style: LayoutStyle,
        build: impl Fn(LayoutStyle, Option<&Overlay>) -> Built + 'static,
    ) -> Built {
        match self.overlay(instance, node) {
            None => build(style, None),
            Some(bound) => rebuilt(style, bound, move |overlay| {
                build(
                    LayoutStyle::new().flex_column().flex_grow(1.0),
                    Some(overlay),
                )
            }),
        }
    }

    /// What `instance`'s bindings say over its written options, as it changes; `None` where it has none, or shows a module this shell does not have. `node` is where it is placed.
    ///
    /// Every binding of one instance reads through one set of readings, so a reading several of them follow rebuilds the instance once.
    fn overlay(&self, instance: &ResolvedInstance, node: &Node) -> Option<Memo<Overlay>> {
        if instance.bindings.is_empty() {
            return None;
        }
        let fields = ui::descriptor::find(&instance.module)?.option_fields();
        let (this, node, module, bindings) = (
            self.clone(),
            node.clone(),
            instance.module.clone(),
            instance.bindings.clone(),
        );
        let written = remade(move || this.writes(&fields, &module, &bindings, &node));
        let options = instance.options.clone();
        Some(telar::memo(move || {
            let mut overlay = Overlay {
                options: options.clone(),
                accent: None,
                style: BoundStyle::default(),
            };
            for (path, value) in written.get() {
                match value.get() {
                    Some(Written::Option(value)) => {
                        bindings::overlay(&mut overlay.options, &path, value)
                    }
                    Some(Written::Accent(accent)) => overlay.accent = Some(accent),
                    Some(Written::Fill(fill)) => overlay.style.fill = Some(fill),
                    Some(Written::Opacity(opacity)) => overlay.style.opacity = Some(opacity),
                    Some(Written::BorderColor(color)) => overlay.style.border_color = Some(color),
                    None => {}
                }
            }
            overlay
        }))
    }

    /// What each of `bindings` on an instance of `module`, whose options are `fields`, writes as it changes: those that check here, all bound through one set of readings.
    fn writes(
        &self,
        fields: &[OptionField],
        module: &str,
        bindings: &BTreeMap<String, ResolvedExpr>,
        node: &Node,
    ) -> Vec<(String, Memo<Option<Written>>)> {
        let within = bindings.values().find_map(|bound| bound.within.clone());
        let env = self.env_within(within.as_deref(), node.output.as_deref());
        let mut drives = Vec::new();
        let mut compiled = Vec::new();
        for (path, expr) in bindings {
            let site = site(node, Slot::Binding(path), &expr.origin);
            let ready = Target::of(fields, path)
                .map_err(|why| {
                    Unready::Failed(util::message!(
                        "finding.of_module",
                        module = module,
                        why = why
                    ))
                })
                .and_then(|target| {
                    let ty = target.ty();
                    checked(&env, &expr.expr.0, &ty).map(|ok| (target, ok))
                });
            match ready {
                Ok((target, ok)) => {
                    drives.push((path.clone(), target, site));
                    compiled.push(ok);
                }
                Err(unready) => unready.report(&site),
            }
        }
        if compiled.is_empty() {
            return Vec::new();
        }
        let held = env.bind_together(compiled, self.gate.clone());
        drives
            .into_iter()
            .zip(held)
            .map(|((path, target, site), held)| (path, written(held, target, site)))
            .collect()
    }
}

/// `source` checked against `env` to give `ty`, or why it does not yet.
fn checked(env: &Environment, source: &str, ty: &Type) -> Result<Compiled, Unready> {
    env.compile(source)
        .map_err(|errors| Unready::of(env, source, errors))?
        .require(ty)
        .map_err(|error| Unready::Failed(describe(&error.code)))
}

/// Why an expression is not drawn.
enum Unready {
    /// It reads a variable not set yet and nothing else is wrong with it, which `layout check` warns of rather than refuses: a wait, which ends as the variable is set and the expression checks again.
    Waiting,
    Failed(Message),
}

impl Unready {
    /// What compiling `source` against `env` failed with: a wait where every error is a variable not set yet ([`Environment::awaits_variable`]), else the other errors.
    fn of(env: &Environment, source: &str, errors: Errors) -> Self {
        let failed = errors
            .into_iter()
            .filter(|error| !env.awaits_variable(source, error))
            .map(|error| describe(&error.code))
            .reduce(|first, rest| util::message!("finding.and_more", first = first, rest = rest));
        match failed {
            None => Unready::Waiting,
            Some(failed) => Unready::Failed(failed),
        }
    }

    /// Reports a failure at `site` for as long as what the expression was written on is drawn. A wait is no failure.
    fn report(&self, site: &Site) {
        if let Unready::Failed(why) = self {
            failures::fail(site.clone(), why.clone());
            let site = site.clone();
            telar::on_cleanup(move || failures::recover(&site));
        }
    }
}

/// What `make` answers, made again — what it made last time disposed with its owner — whenever something it read while making moves: a variable an expression names appearing, going or changing type, or the type of the items of the copy it is drawn in.
fn remade<T: Clone + 'static>(make: impl Fn() -> T + 'static) -> Remade<T> {
    let made: RwSignal<Option<T>> = telar::signal(None);
    let held: Rc<Cell<Option<OwnerId>>> = Rc::default();
    telar::effect(move || {
        if let Some(owner) = held.take() {
            telar::dispose_owner(owner);
        }
        let scope = telar::owner_scope();
        let now = make();
        held.set(Some(scope.id()));
        drop(scope);
        made.set(Some(now));
    });
    Remade(made)
}

/// What [`remade`] made last, read reactively.
struct Remade<T: 'static>(RwSignal<Option<T>>);

impl<T: 'static> Clone for Remade<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for Remade<T> {}

impl<T: Clone + 'static> Remade<T> {
    fn get(&self) -> T {
        self.0
            .get()
            .expect("made as soon as the effect that makes it is")
    }
}

/// The most copies of its children a group with `repeat` draws: far more than any bar, dock or free area has room to show, and few enough that building them stays inside a frame. A longer list draws its first `MAX_COPIES` items and is reported, so a source that answers with a hundred thousand lines cannot stall the shell.
pub const MAX_COPIES: usize = 256;

/// What a group with `repeat` is drawn from: the list, as it changes, and the type of its items.
#[derive(Clone)]
pub struct Repeat {
    items: Memo<Arc<[Value]>>,
    item: Memo<Type>,
}

impl Repeat {
    /// The list `expr` gives, read through `env` while `gate` holds, and checked again as the names it reads change. A failure is reported at `site`.
    fn made(
        env: impl Fn() -> Environment + 'static,
        source: String,
        gate: Gate,
        site: Site,
    ) -> Self {
        let made = remade(move || {
            let env = env();
            let ready = env
                .compile(&source)
                .map_err(|errors| Unready::of(&env, &source, errors))
                .and_then(|compiled| {
                    layout::repeat_item(compiled.ty())
                        .map(|item| (compiled, item))
                        .map_err(Unready::Failed)
                });
            match ready {
                Ok((compiled, item)) => {
                    let held = env.bind(compiled, gate.clone());
                    report_repeat(held, site.clone());
                    Some((held, item))
                }
                Err(unready) => {
                    unready.report(&site);
                    None
                }
            }
        });
        Self {
            items: telar::memo(move || match made.get() {
                Some((held, _)) => match held.get().value {
                    Some(Value::List(items)) => items,
                    _ => Arc::from([]),
                },
                None => Arc::from([]),
            }),
            item: telar::memo(move || made.get().map_or(Type::Never, |(_, item)| item)),
        }
    }

    /// `children` drawn once per item now, up to [`MAX_COPIES`] items, in order: each copy `<id>#<index>`, the children of one item together.
    pub fn copies(&self, children: &[ResolvedInstance]) -> Vec<ResolvedInstance> {
        let count = self.items.with(|items| items.len().min(MAX_COPIES));
        (0..count)
            .flat_map(|index| {
                children.iter().map(move |child| ResolvedInstance {
                    id: child.id.copy(index),
                    ..child.clone()
                })
            })
            .collect()
    }

    /// Makes what is built under the current owner afterwards the copy `copy` — its item as `$item` and its index as `$index` — where `copy` is one of [`Repeat::copies`].
    pub fn enter(&self, copy: &ResolvedInstance) {
        if let Some(index) = copy.id.copy_index() {
            telar::set_context(CopyOf {
                repeat: self.clone(),
                index,
            });
        }
    }

    /// What copy `index` reads besides the shell's names: its item, as the list moves — so a copy whose item changes in place is not built again for it — and its index.
    fn locals(&self, index: usize) -> Vec<Local> {
        let items = self.items;
        let item = telar::memo(move || items.with(|items| items.get(index).cloned()));
        vec![
            Local::live(layout::ITEM, self.item.get(), move || item.get()),
            Local::live(layout::INDEX, Type::Number, move || {
                Some(Value::Number(index as f64))
            }),
        ]
    }

    fn item_at(&self, index: usize) -> Option<Value> {
        self.items
            .is_alive()
            .then(|| self.items.with(|items| items.get(index).cloned()))
            .flatten()
    }
}

thread_local! {
    static DRAWN: RefCell<HashMap<Node, (u64, Repeat)>> = RefCell::default();
    static DRAWN_MOVED: RwSignal<u64> = telar::detached(|| telar::signal(0));
}

/// Records `repeat` as what the group at `node` draws its copies from, until the current owner goes.
fn drawn(node: Node, repeat: &Repeat) {
    let serial = DRAWN_MOVED.with(|moved| moved.peek()) + 1;
    DRAWN.with(|drawn| {
        drawn
            .borrow_mut()
            .insert(node.clone(), (serial, repeat.clone()))
    });
    DRAWN_MOVED.with(|moved| moved.set(serial));
    telar::on_cleanup(move || {
        let gone = DRAWN.with(|drawn| {
            let mut drawn = drawn.borrow_mut();
            let ours = drawn.get(&node).is_some_and(|(held, _)| *held == serial);
            if ours {
                drawn.remove(&node);
            }
            ours
        });
        if gone {
            DRAWN_MOVED.with(|moved| {
                if moved.is_alive() {
                    moved.set(moved.peek() + 1);
                }
            });
        }
    });
}

/// What copy `index` of the repeated group at `group` reads as `$item` where the screen draws it, read reactively: `None` while no such group or copy is drawn. What an editor shows a copy's bindings reading.
pub fn drawn_item(group: &Node, index: usize) -> Option<Value> {
    let _moved = DRAWN_MOVED.with(|moved| moved.get());
    let repeat = DRAWN.with(|drawn| drawn.borrow().get(group).map(|(_, repeat)| repeat.clone()));
    repeat?.item_at(index)
}

/// An instance's options with what its bindings say laid over them, the colour `accent` binds it to, and the style keys its bindings drive.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    pub options: toml::Table,
    pub accent: Option<Color>,
    pub style: BoundStyle,
}

/// What an instance's `style.*` bindings say now, over the `style` it is written with.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoundStyle {
    pub fill: Option<Color>,
    pub opacity: Option<f32>,
    pub border_color: Option<Color>,
}

impl Overlay {
    /// The options an instance written with `written` is built with: what its bindings say over them under `overlay`, or `written` itself.
    pub fn options_or(overlay: Option<&Self>, written: &toml::Table) -> toml::Table {
        overlay.map_or(written, |overlay| &overlay.options).clone()
    }

    /// The accent an instance is drawn in: the colour its `accent` binding gives under `overlay`, or `fallback`'s where nothing binds it.
    pub fn accent_or(overlay: Option<&Self>, fallback: impl FnOnce() -> Color) -> Color {
        overlay
            .and_then(|overlay| overlay.accent)
            .unwrap_or_else(fallback)
    }
}

/// What `held` writes as `target`: its last value that could be written, through an evaluation error and through a value the target does not take.
fn written(held: Memo<Held>, target: Target, site: Site) -> Memo<Option<Written>> {
    let last: RefCell<Option<Written>> = RefCell::new(None);
    let now = telar::memo(move || {
        let held = held.get();
        let wrote = held.value.as_ref().map(|value| target.write(value));
        let why = match (&held.error, &wrote) {
            (Some(error), _) if awaits_reading(error) => None,
            (Some(error), _) => Some(describe(&error.code)),
            (None, Some(Err(why))) => Some(why.clone()),
            _ => None,
        };
        if let Some(Ok(value)) = wrote {
            last.replace(Some(value));
        }
        (last.borrow().clone(), why)
    });
    let gone = site.clone();
    telar::effect(move || match now.get().1 {
        Some(why) => failures::fail(site.clone(), why),
        None => failures::recover(&site),
    });
    telar::on_cleanup(move || failures::recover(&gone));
    telar::memo(move || now.get().0)
}

/// Keeps `held`'s evaluation errors in the failure report for as long as they last and the expression is drawn.
fn report(held: Memo<Held>, site: Site) {
    let gone = site.clone();
    telar::effect(move || match held.get().error {
        Some(error) if !awaits_reading(&error) => {
            failures::fail(site.clone(), describe(&error.code))
        }
        _ => failures::recover(&site),
    });
    telar::on_cleanup(move || failures::recover(&gone));
}

/// [`report`] for a `repeat`, which also reports a list longer than the copies drawn of it ([`MAX_COPIES`]).
fn report_repeat(held: Memo<Held>, site: Site) {
    let gone = site.clone();
    telar::effect(move || {
        let held = held.get();
        let longer = match &held.value {
            Some(Value::List(items)) if items.len() > MAX_COPIES => Some(items.len()),
            _ => None,
        };
        match (held.error, longer) {
            (Some(error), _) if !awaits_reading(&error) => {
                failures::fail(site.clone(), describe(&error.code))
            }
            (_, Some(count)) => failures::fail(
                site.clone(),
                util::message!("finding.repeat_truncated", count = count, max = MAX_COPIES),
            ),
            _ => failures::recover(&site),
        }
    });
    telar::on_cleanup(move || failures::recover(&gone));
}

/// Which expression of a placed thing: an area's `visible`, a group's `repeat`, or an instance's binding at a path.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Slot<'a> {
    Visible,
    Repeat,
    Binding(&'a str),
}

impl Slot<'_> {
    fn key(&self) -> String {
        match self {
            Slot::Visible => "visible".to_string(),
            Slot::Repeat => "repeat".to_string(),
            Slot::Binding(path) => format!("bindings.{path}"),
        }
    }
}

/// Where a failure of `slot` of `node`, written by `origin`, is reported: the file of the level that wrote it and the key `layout check` names the same expression by there, on the output that draws it; a copy of a repeated child under the child it is made from, with which copy it is.
fn site(node: &Node, slot: Slot, origin: &Origin) -> Site {
    let (file, key) = expression_key(origin, node, slot);
    let copy = match &node.part {
        Part::Instance(_, instance) => instance.copy_index(),
        _ => None,
    };
    Site::Expression(Drawn {
        file,
        key,
        output: node.output.clone(),
        copy,
    })
}

/// The file and key validation names `slot` of `node` by, where `origin` wrote it: a level of the layout, or the komponent the group uses, whose file names a child by the id it has there.
pub(crate) fn expression_key(origin: &Origin, node: &Node, slot: Slot) -> (String, String) {
    let level = match origin {
        Origin::Level(level) => level,
        Origin::Komponent(_) => {
            let key = match &node.part {
                Part::Instance(_, instance) => format!(
                    "children.{}.{}",
                    instance
                        .komponent_child()
                        .unwrap_or_else(|| instance.template()),
                    slot.key()
                ),
                _ => slot.key(),
            };
            return (origin.file(), key);
        }
    };
    let area = format!("layers.{}.areas.{}", node.layer, node.area);
    let below = match &node.part {
        Part::Area => format!("{area}.{}", slot.key()),
        Part::Group(group) => format!("{area}.groups.{group}.{}", slot.key()),
        Part::Instance(group, instance) => format!(
            "{area}.groups.{group}.children.{}.{}",
            instance.template(),
            slot.key()
        ),
    };
    (level.file(), format!("{}.{below}", level.rule()))
}

/// Where a failure of `parameter` of the komponent use `used`, drawn on `output`, is reported: where the use sets it, or the komponent's own default.
fn parameter_site(
    used: &KomponentUse,
    parameter: &ResolvedParameter,
    output: Option<&str>,
) -> Site {
    let (file, key) = match parameter.value.as_ref().map(|value| &value.origin) {
        Some(Origin::Level(level)) => (
            level.file(),
            format!(
                "{}.layers.{}.areas.{}.groups.{}.parameters.{}",
                level.rule(),
                used.layer,
                used.area,
                used.group,
                parameter.name
            ),
        ),
        _ => (
            layout::komponent_path(&used.id),
            format!("parameters.{}.default", parameter.name),
        ),
    };
    Site::Expression(Drawn {
        file,
        key,
        output: output.map(str::to_string),
        copy: None,
    })
}

/// `build` under what `bound` says now, laid out by `style`, and built again — this alone — each time `bound` says something else.
fn rebuilt(
    style: LayoutStyle,
    bound: Memo<Overlay>,
    build: impl Fn(&Overlay) -> Built + 'static,
) -> Built {
    let seen: Rc<RefCell<(u64, Option<Overlay>)>> = Rc::default();
    let list = ReactiveList::with_style(
        style,
        move || {
            let now = bound.get();
            let mut seen = seen.borrow_mut();
            if seen.1.as_ref() != Some(&now) {
                seen.0 = seen.0.wrapping_add(1);
                seen.1 = Some(now.clone());
            }
            vec![(seen.0, now)]
        },
        |(generation, _): &(u64, Overlay)| *generation,
        move |(_, now): (u64, Overlay)| build(&now),
    )?;
    Ok(Box::new(list))
}
