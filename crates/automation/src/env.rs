//! The shell's expression environment: what `$name` and `$source.field` mean, of which type, and how a binding reads them live.
//!
//! **Names.** `$name` is a source the layout declares, else a variable; `$source.field` is a module's reading (`$battery.level`); `$event.<kind>` is the last event of a kind. A layout source may not take a module source's name (validation says so), and shadows a variable of the same name. In a copy of a repeated group, `$item` and `$index` are read before any of them ([`Local`]). A `$name` nothing answers to yet is a variable not set yet: it does not check until one is ([`Environment::awaits_variable`]), and whatever checked it then checks it again ([`vars::declared`]).
//!
//! **Lock views.** An [`Environment`] is shown to an [`Audience`]. For [`Audience::Anyone`] — the lock screen — a field whose `Privacy` does not allow anyone reads as its type's empty value without its service ever being asked, an `OnLock` field asks `[lock]`, and a source that runs a command is refused unless it says `lock_safe = true` (TA-8). Variables are readable.
//!
//! **Live readings.** [`Readings`] is the resolver a binding evaluates through. Each source a binding actually reads gets signals of its own and a subscription to its producer, made the first time the evaluation reads it and owned by the reactive owner the binding was made under, so the subscription goes when that tree goes (F-10.43) and a branch never taken starts nothing. The subscription is held only while the binding's [`Gate`] says it is on screen — and, for anything but the lock screen, while the session is unlocked — so a hidden consumer stops counting and its producer retires once nobody else reads it (TA-6). A rule's readings ([`Environment::for_rules`]) are held for as long as the rule is loaded. A field is written only when its value changes, so a binding re-evaluates only when something it read moved.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;
use std::sync::Arc;

use config::{Config, LockConfig};
use layout::While;
use services::events::EventKind;
use services::state::{ShellState, Var};
use telar::{Memo, OwnerId, RwSignal};
use telar_expression::{
    Compiled, Error, ErrorCode, ErrorKind, Errors, Held, HostError, Reference, Registry, Type,
    Value,
};
use ui::descriptor::{ModuleDescriptor, Reading, SourceDef};
use ui::host::Audience;
use util::report::Message;

use crate::sources::{self, SourceSpec, Unreadable, UserSource, UserSources, coerce};
use crate::vars;

/// The name event readings live under: `$event.session_locked`.
pub const EVENT: &str = "event";

/// What a name in an expression reads, and who for.
#[derive(Clone)]
pub struct Environment {
    services: Arc<BTreeMap<&'static str, &'static SourceDef>>,
    user: Arc<UserSources>,
    audience: Audience,
    lock: Arc<LockConfig>,
    registry: Arc<Registry>,
    for_rules: bool,
    locals: Rc<[Local]>,
}

/// A name a binding reads besides the shell's own — `$item` or `$index` in a copy of a repeated group — with its type and, where the copy is drawn, its value as it changes. It is read before any source, module reading or variable of the same name.
#[derive(Clone)]
pub struct Local {
    name: &'static str,
    ty: Type,
    value: Option<Rc<dyn Fn() -> Option<Value>>>,
}

impl Local {
    /// A local known only by its type: enough to compile against, not to read.
    pub fn typed(name: &'static str, ty: Type) -> Self {
        Self {
            name,
            ty,
            value: None,
        }
    }

    /// A local read as `value` answers, reactively. `None` is a value the copy does not have yet, or no longer has.
    pub fn live(name: &'static str, ty: Type, value: impl Fn() -> Option<Value> + 'static) -> Self {
        Self {
            name,
            ty,
            value: Some(Rc::new(value)),
        }
    }

    /// Each of `locals` known by its type alone.
    pub fn typed_all(locals: &layout::Locals) -> Vec<Self> {
        locals
            .iter()
            .map(|(name, ty)| Self::typed(name, ty.clone()))
            .collect()
    }
}

/// What a reference reads, as an editor groups the names it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReferenceKind {
    /// `$item` or `$index`, in a copy of a repeated group.
    Local,
    /// A source the layout declares.
    Source,
    /// A module's reading.
    Field,
    /// One of the colours the shell paints with.
    Theme,
    Var,
    Event,
}

enum Target<'a> {
    Local(&'a Local),
    User(&'a UserSource),
    Field(&'static SourceDef, usize),
    Var,
    Event(EventKind),
    /// A bare `$name` nothing answers to: a variable not set yet.
    Unset,
}

impl Target<'_> {
    fn kind(&self) -> Option<ReferenceKind> {
        Some(match self {
            Target::Local(_) => ReferenceKind::Local,
            Target::User(_) => ReferenceKind::Source,
            Target::Field(def, _) if def.id == crate::theme::THEME_SOURCE => ReferenceKind::Theme,
            Target::Field(..) => ReferenceKind::Field,
            Target::Var => ReferenceKind::Var,
            Target::Event(_) => ReferenceKind::Event,
            Target::Unset => return None,
        })
    }
}

impl Environment {
    /// The module readings `services`, the theme's colours and the layout's own `user` sources, for the signed-in user, with the shell's functions.
    pub fn new(
        services: impl IntoIterator<Item = &'static SourceDef>,
        user: impl Into<Arc<UserSources>>,
    ) -> Self {
        Self {
            services: Arc::new(
                services
                    .into_iter()
                    .chain([&crate::theme::THEME])
                    .map(|def| (def.id, def))
                    .collect(),
            ),
            user: user.into(),
            audience: Audience::Owner,
            lock: Arc::new(LockConfig::default()),
            registry: Arc::new(crate::functions::registry()),
            for_rules: false,
            locals: Rc::from([]),
        }
    }

    /// [`Environment::new`] over every reading a module in `modules` gives.
    pub fn of_modules(
        modules: &'static [ModuleDescriptor],
        user: impl Into<Arc<UserSources>>,
    ) -> Self {
        Self::new(
            modules.iter().flat_map(|module| module.sources.iter()),
            user,
        )
    }

    /// What a rule reads: every reading `modules` gives, the theme, variables and events, held whatever is on screen ([`Environment::for_rules`]). A rule belongs to no layout, so no layout source is read; `lock` is the `[lock]` that decides what [`Environment::hidden_on_lock`] answers of what it reads.
    pub fn of_rules(modules: &'static [ModuleDescriptor], lock: &LockConfig) -> Self {
        Self::of_modules(modules, UserSources::default())
            .shown_to(Audience::Owner, lock.clone())
            .for_rules()
    }

    /// What the running shell's expressions read: every installed module's readings, the theme, and the sources the running layout declares (`sources::declare`). Made again only when one of those changes.
    pub fn running() -> Self {
        let modules = ui::descriptor::installed();
        let user = sources::declared_sources();
        RUNNING.with(|running| {
            let mut running = running.borrow_mut();
            if let Some((made_for, made_with, env)) = running.as_ref()
                && std::ptr::eq(*made_for, modules)
                && Arc::ptr_eq(made_with, &user)
            {
                return env.clone();
            }
            let env = Self::of_modules(modules, Arc::clone(&user));
            *running = Some((modules, user, env.clone()));
            env
        })
    }

    /// The same names, read for `audience` under the `[lock]` rules that decide what anyone may see.
    pub fn shown_to(mut self, audience: Audience, lock: LockConfig) -> Self {
        self.audience = audience;
        self.lock = Arc::new(lock);
        self
    }

    /// The same names, read for whoever is in front of a layer: anyone on the lock layer (`on_lock`), the signed-in user on any other, under the `[lock]` of the config the expression is drawn under — the area's own where it is built in one, since a monitor may set its own, else the running config's.
    pub fn on_layer(self, on_lock: bool) -> Self {
        let lock = area_config()
            .or_else(config::config)
            .map(|config| config.lock.clone())
            .unwrap_or_default();
        let audience = match on_lock {
            true => Audience::Anyone,
            false => Audience::Owner,
        };
        self.shown_to(audience, lock)
    }

    /// The same names, read by a rule rather than by something on screen: what it reads keeps being read whatever is shown and while the session is locked, since a rule is session automation rather than lock content (TA-8).
    pub fn for_rules(mut self) -> Self {
        self.for_rules = true;
        self
    }

    /// The same names with `locals` read before them: what a copy of a repeated group's children reads.
    pub fn with_locals(mut self, locals: Vec<Local>) -> Self {
        self.locals = locals.into();
        self
    }

    /// The same names with other functions — an editor's, or a test's.
    pub fn with_registry(mut self, registry: Registry) -> Self {
        self.registry = Arc::new(registry);
        self
    }

    pub fn audience(&self) -> Audience {
        self.audience
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Parses and checks `source` against these names and functions. Checked inside an effect or a memo, it is checked again as a variable it names appears, goes or changes type.
    pub fn compile(&self, source: &str) -> Result<Compiled, Errors> {
        telar_expression::compile(source, self, &self.registry)
    }

    /// Whether `error`, from compiling `source` here, is only that a `$name` it reads names nothing yet: a variable set later answers it, and whatever checked `source` reactively checks it again then.
    pub fn awaits_variable(&self, source: &str, error: &Error) -> bool {
        source
            .get(error.span.range())
            .and_then(|written| written.strip_prefix('$'))
            .filter(|name| vars::check_name(name).is_ok())
            .is_some_and(|name| {
                matches!(
                    self.target(&Reference::new(name, Vec::<String>::new()), &declared),
                    Ok(Target::Unset)
                )
            })
    }

    /// Every reference an expression may write here, with its type and what it reads, for an editor to offer. Fields this audience may not see are offered too: they read as empty, which is what the lock screen shows. A name something else of the same name shadows is offered once, as what it reads.
    pub fn references(&self) -> Vec<(Reference, Type, ReferenceKind)> {
        let bare = |name: &str| Reference::new(name, Vec::<String>::new());
        let named = self
            .locals
            .iter()
            .map(|local| bare(local.name))
            .chain(self.user.iter().map(|(name, _)| bare(name)))
            .chain(self.services.values().flat_map(|def| {
                def.fields
                    .iter()
                    .map(|field| Reference::new(def.id, [field.name]))
            }))
            .chain(vars::list().into_keys().map(|name| bare(&name)))
            .chain(EventKind::names().map(|kind| Reference::new(EVENT, [kind])));
        let mut seen = BTreeSet::new();
        let mut offered = Vec::new();
        for reference in named {
            if !seen.insert(reference.clone()) {
                continue;
            }
            let Ok(target) = self.target(&reference, &declared) else {
                continue;
            };
            if let (Some(kind), Ok(ty)) = (target.kind(), self.type_of(&target, &reference)) {
                offered.push((reference, ty, kind));
            }
        }
        offered
    }

    /// The resolver a binding made now evaluates through, owned by the current reactive owner. `gate` says whether what the binding feeds is on screen.
    pub fn readings(&self, gate: Gate) -> Readings {
        Readings {
            env: self.clone(),
            owner: telar::current_owner(),
            gate,
            produced: RefCell::default(),
            services: RefCell::default(),
            vars: RefCell::default(),
        }
    }

    /// `compiled` bound to live readings under the current reactive owner, keeping its last good value through an error.
    pub fn bind(&self, compiled: Compiled, gate: Gate) -> Memo<Held> {
        telar_expression::bind_held(compiled, self.readings(gate))
    }

    /// Each of `compiled` bound like [`Environment::bind`], all through one set of readings: one subscription per source they read between them, so a reading several of them follow moves them in one step rather than one after another.
    pub fn bind_together(&self, compiled: Vec<Compiled>, gate: Gate) -> Vec<Memo<Held>> {
        let readings = Rc::new(self.readings(gate));
        compiled
            .into_iter()
            .map(|compiled| telar_expression::bind_held(compiled, Shared(Rc::clone(&readings))))
            .collect()
    }

    /// Whether the lock screen reads `reference` as its type's empty value: a module's field whose `Privacy` keeps it from anyone under this environment's `[lock]`.
    pub fn hidden_on_lock(&self, reference: &Reference) -> bool {
        matches!(
            self.target(reference, &|_| false),
            Ok(Target::Field(def, index)) if !def.fields[index].shown_to(Audience::Anyone, &self.lock)
        )
    }

    /// What `reference` names. `is_var` answers whether a variable of that name exists: as its type when checking, which follows the variable being declared, and as its value when reading.
    fn target(
        &self,
        reference: &Reference,
        is_var: &dyn Fn(&str) -> bool,
    ) -> Result<Target<'_>, HostError> {
        let name = reference.name.as_str();
        if let Some(local) = self.locals.iter().find(|local| local.name == name) {
            return match reference.path.is_empty() {
                true => Ok(Target::Local(local)),
                false => Err(refusal(util::message!(
                    "expression.local_has_no_fields",
                    name = name
                ))),
            };
        }
        match reference.path.as_slice() {
            [] => {
                if let Some(source) = self.user.get(name) {
                    if self.audience == Audience::Anyone
                        && source.spec.is_user_command()
                        && !source.lock_safe
                    {
                        return Err(refusal(util::message!(
                            "expression.lock_reads_lock_safe",
                            name = name
                        )));
                    }
                    return Ok(Target::User(source));
                }
                if is_var(name) {
                    return Ok(Target::Var);
                }
                if let Some(def) = self.services.get(name) {
                    return Err(refusal(util::message!(
                        "expression.reading_has_fields",
                        name = name,
                        fields = field_names(def),
                        first = def.fields.first().map_or("", |field| field.name)
                    )));
                }
                if name == EVENT {
                    return Err(refusal(util::message!("expression.name_an_event")));
                }
                if name == layout::ITEM || name == layout::INDEX {
                    return Err(refusal(util::message!(
                        "expression.local_outside_copy",
                        name = name
                    )));
                }
                Ok(Target::Unset)
            }
            [field] => {
                if name == EVENT {
                    return EventKind::from_name(field)
                        .map(Target::Event)
                        .ok_or_else(|| {
                            refusal(util::message!(
                                "expression.not_an_event",
                                field = field,
                                events = event_names()
                            ))
                        });
                }
                if let Some(def) = self.services.get(name) {
                    return def
                        .field(field)
                        .map(|(index, _)| Target::Field(def, index))
                        .ok_or_else(|| {
                            refusal(util::message!(
                                "expression.no_such_field",
                                name = name,
                                field = field,
                                fields = field_names(def)
                            ))
                        });
                }
                if self.user.get(name).is_some() || is_var(name) {
                    return Err(refusal(util::message!(
                        "expression.reading_has_no_fields",
                        name = name
                    )));
                }
                Err(refusal(util::message!(
                    "expression.nothing_called",
                    name = name
                )))
            }
            _ => Err(refusal(util::message!(
                "expression.goes_too_deep",
                reference = reference
            ))),
        }
    }

    fn type_of(&self, target: &Target<'_>, reference: &Reference) -> Result<Type, HostError> {
        match target {
            Target::Local(local) => Ok(local.ty.clone()),
            Target::User(source) => Ok(source.ty.clone()),
            Target::Field(def, index) => Ok(def.fields[*index].ty.ty()),
            Target::Var => vars::declared(&reference.name)
                .map(|ty| vars::type_of(&ty))
                .ok_or_else(|| unset(reference)),
            Target::Event(_) => Ok(Type::Text),
            Target::Unset => Err(unset(reference)),
        }
    }
}

/// Whether a variable called `name` is declared, read reactively.
fn declared(name: &str) -> bool {
    vars::declared(name).is_some()
}

/// A failure of a name as `telar-expression` carries it: the entry of this crate's catalogue that says it, which [`describe`] reads back.
fn refusal(message: Message) -> HostError {
    message.host_error()
}

fn no_reading_yet(reference: &Reference) -> HostError {
    refusal(util::message!(
        "expression.no_reading_yet",
        reference = reference
    ))
}

/// What a copy's `$item` or `$index` says where that copy is not drawn — a popover on a repeated child no screen draws, a copy its list has just dropped: a wait, like a reading that has not arrived.
fn no_copy_drawn(reference: &Reference) -> HostError {
    refusal(util::message!(
        "expression.no_copy_drawn",
        reference = reference
    ))
}

/// The failures that are a wait rather than a failure, by their keys.
const WAITS: [&str; 2] = ["expression.no_reading_yet", "expression.no_copy_drawn"];

/// Whether `error` is only that a reading has not arrived yet: a source that has not answered, an event that has not happened, a copy not drawn yet. That is a wait rather than a failure; when the source itself cannot answer, that is its own failure.
pub fn awaits_reading(error: &Error) -> bool {
    error.kind == ErrorKind::Evaluation
        && matches!(&error.code, ErrorCode::Host(host) if WAITS.contains(&host.key.as_str()))
}

/// What `code` says: a failure of one of the shell's names in this crate's words, any other in the expression language's own.
pub fn describe(code: &ErrorCode) -> Message {
    match code {
        ErrorCode::Host(host) => Message::from_host_error(&crate::__rsx_i18n::CATALOG, host),
        other => Message::expression(other),
    }
}

fn unset(reference: &Reference) -> HostError {
    refusal(util::message!(
        "expression.not_set_yet",
        reference = reference
    ))
}

/// The config of the area being built, where one is being built: a monitor may set its own `[lock]` and `[theme]`. Elsewhere — a rule, a check — the running config is the one to read.
pub(crate) fn area_config() -> Option<Arc<Config>> {
    ui::chrome::Chrome::current().map(|chrome| chrome.config)
}

fn event_names() -> String {
    EventKind::names()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn field_names(def: &SourceDef) -> String {
    def.fields
        .iter()
        .map(|field| format!("`{}`", field.name))
        .collect::<Vec<_>>()
        .join(", ")
}

impl telar_expression::Environment for Environment {
    fn reference(&self, reference: &Reference) -> Result<Type, HostError> {
        let target = self.target(reference, &declared)?;
        self.type_of(&target, reference)
    }
}

/// Whether what a binding feeds is on screen, read reactively: a binding whose consumer is hidden stops counting as a subscriber to what it reads.
#[derive(Clone)]
pub struct Gate(Rc<dyn Fn() -> bool>);

impl Gate {
    pub fn new(shown: impl Fn() -> bool + 'static) -> Self {
        Self(Rc::new(shown))
    }

    pub fn always() -> Self {
        Self::new(|| true)
    }

    fn shown(&self) -> bool {
        (self.0)()
    }
}

/// One signal per field of a module source, in the order the source declares them.
type Fields = Rc<[RwSignal<Option<Value>>]>;

/// The live values a binding reads, made as it first reads each one.
pub struct Readings {
    env: Environment,
    owner: Option<OwnerId>,
    gate: Gate,
    produced: RefCell<HashMap<SourceSpec, RwSignal<Option<Value>>>>,
    services: RefCell<HashMap<&'static str, Fields>>,
    vars: RefCell<Option<Rc<VarCells>>>,
}

impl telar_expression::Resolver for Readings {
    fn read(&self, reference: &Reference) -> Result<Value, HostError> {
        let is_var = |name: &str| self.vars().read(name).is_some();
        match self.env.target(reference, &is_var)? {
            Target::Local(local) => match &local.value {
                Some(value) => value().ok_or_else(|| no_copy_drawn(reference)),
                None => Err(refusal(util::message!(
                    "expression.local_not_drawn",
                    reference = reference
                ))),
            },
            Target::User(source) => match self.produced(&source.spec, source.running_while).get() {
                Some(raw) => match coerce(&raw, &source.ty) {
                    Ok(value) => Ok(value),
                    Err(Unreadable::NotNumber(text)) => Err(refusal(util::message!(
                        "expression.reading_not_number",
                        reference = reference,
                        text = text
                    ))),
                    Err(Unreadable::NotTruth(text)) => Err(refusal(util::message!(
                        "expression.reading_not_truth",
                        reference = reference,
                        text = text
                    ))),
                    // Handed on as it is, so the evaluator says which type it is and which was declared.
                    Err(Unreadable::OtherType) => Ok(raw),
                },
                None => source
                    .initial
                    .clone()
                    .ok_or_else(|| no_reading_yet(reference)),
            },
            Target::Field(def, index) => {
                let field = &def.fields[index];
                if !field.shown_to(self.env.audience, &self.env.lock) {
                    return Value::empty(&field.ty.ty()).ok_or_else(|| {
                        refusal(util::message!(
                            "expression.no_empty_value",
                            reference = reference
                        ))
                    });
                }
                self.service(def)[index]
                    .get()
                    .ok_or_else(|| no_reading_yet(reference))
            }
            Target::Var => self
                .vars()
                .read(&reference.name)
                .map(|var| vars::value_of(&var))
                .ok_or_else(|| {
                    refusal(util::message!(
                        "expression.no_variable",
                        reference = reference
                    ))
                }),
            Target::Event(kind) => self
                .produced(&SourceSpec::event(kind), While::Visible)
                .get()
                .ok_or_else(|| no_reading_yet(reference)),
            Target::Unset => Err(unset(reference)),
        }
    }
}

/// One [`Readings`] that several bindings evaluate through.
struct Shared(Rc<Readings>);

impl telar_expression::Resolver for Shared {
    fn read(&self, reference: &Reference) -> Result<Value, HostError> {
        telar_expression::Resolver::read(&*self.0, reference)
    }
}

impl Readings {
    fn gate_for(&self, running: While) -> Gate {
        if self.env.for_rules {
            return Gate::always();
        }
        let shown = self.gate.clone();
        let session = self.env.audience == Audience::Owner;
        Gate::new(move || {
            (running == While::Always || shown.shown()) && !(session && services::lock::locked())
        })
    }

    fn produced(&self, spec: &SourceSpec, running: While) -> RwSignal<Option<Value>> {
        if let Some(cell) = self.produced.borrow().get(spec) {
            return *cell;
        }
        let cell = telar::with_owner(self.owner, || {
            let cell = telar::signal(sources::current(spec));
            let spec = spec.clone();
            hold(self.gate_for(running), move || {
                let spec = spec.clone();
                platform_wayland::watch(
                    move |tx| sources::subscribe(spec, tx),
                    move |value: Value| store(cell, Some(value)),
                );
            });
            cell
        });
        self.produced.borrow_mut().insert(spec.clone(), cell);
        cell
    }

    fn service(&self, def: &'static SourceDef) -> Fields {
        if let Some(cells) = self.services.borrow().get(def.id) {
            return Rc::clone(cells);
        }
        let cells: Fields = telar::with_owner(self.owner, || {
            let cells: Fields = def.fields.iter().map(|_| telar::signal(None)).collect();
            let fed = Rc::clone(&cells);
            let feed = def.feed;
            hold(self.gate_for(While::Visible), move || {
                let fed = Rc::clone(&fed);
                feed(Box::new(move |reading: Reading| {
                    telar::batch(|| {
                        for (cell, value) in fed.iter().zip(reading.iter()) {
                            store(*cell, Some(value.clone()));
                        }
                    })
                }));
            });
            cells
        });
        self.services.borrow_mut().insert(def.id, Rc::clone(&cells));
        cells
    }

    fn vars(&self) -> Rc<VarCells> {
        if let Some(cells) = self.vars.borrow().as_ref() {
            return Rc::clone(cells);
        }
        let cells = Rc::new(VarCells {
            owner: self.owner,
            cells: RefCell::default(),
        });
        let fed = Rc::clone(&cells);
        telar::with_owner(self.owner, || {
            platform_wayland::watch(services::state::subscribe, move |state: ShellState| {
                fed.apply(&state.vars)
            });
        });
        *self.vars.borrow_mut() = Some(Rc::clone(&cells));
        cells
    }
}

/// One signal per variable a binding has read, so setting one variable moves only the bindings that read it.
pub(crate) struct VarCells {
    owner: Option<OwnerId>,
    cells: RefCell<HashMap<String, RwSignal<Option<Var>>>>,
}

impl VarCells {
    fn read(&self, name: &str) -> Option<Var> {
        let held = self.cells.borrow().get(name).copied();
        let cell = held.unwrap_or_else(|| {
            let cell = telar::with_owner(self.owner, || telar::signal(vars::get(name)));
            self.cells.borrow_mut().insert(name.to_string(), cell);
            cell
        });
        cell.get()
    }

    pub(crate) fn apply(&self, vars: &BTreeMap<String, Var>) {
        let cells: Vec<(String, RwSignal<Option<Var>>)> = self
            .cells
            .borrow()
            .iter()
            .map(|(name, cell)| (name.clone(), *cell))
            .collect();
        telar::batch(|| {
            for (name, cell) in cells {
                store(cell, vars.get(&name).cloned());
            }
        });
    }
}

/// Writes `value` only when it differs: a write moves every reader whether or not the value changed.
fn store<T: Clone + PartialEq + 'static>(cell: RwSignal<T>, value: T) {
    if cell.is_alive() && cell.peek() != value {
        cell.set(value);
    }
}

/// Runs `subscribe` under an owner of its own while `gate` holds, and disposes that owner — taking the subscription with it — while it does not.
fn hold(gate: Gate, subscribe: impl Fn() + 'static) {
    let held: Rc<Cell<Option<OwnerId>>> = Rc::default();
    telar::effect(move || match (gate.shown(), held.get()) {
        (true, None) => {
            let scope = telar::owner_scope();
            subscribe();
            held.set(Some(scope.id()));
        }
        (false, Some(owner)) => {
            held.set(None);
            telar::dispose_owner(owner);
        }
        _ => {}
    });
}

thread_local! {
    static RUNNING: RefCell<Option<RunningEnvironment>> = const { RefCell::new(None) };
}

/// [`Environment::running`] as last made, with the module table and the declared sources it was made from.
type RunningEnvironment = (&'static [ModuleDescriptor], Arc<UserSources>, Environment);

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use config::NotificationDetail;
    use telar_expression::{Pattern, Resolver, Signature};
    use ui::descriptor::{FieldDef, FieldType, Privacy, Sink};

    use super::*;

    thread_local! {
        static SINKS: RefCell<Vec<(Rc<Cell<bool>>, Sink)>> = RefCell::default();
        static SUBSCRIBED: Cell<usize> = const { Cell::new(0) };
        static UNSUBSCRIBED: Cell<usize> = const { Cell::new(0) };
        static EVALUATIONS: Cell<usize> = const { Cell::new(0) };
    }

    /// A service source fed by hand: each subscription is counted, and so is each one taken back.
    fn probe_feed(sink: Sink) {
        SUBSCRIBED.with(|n| n.set(n.get() + 1));
        let alive = Rc::new(Cell::new(true));
        let ended = Rc::clone(&alive);
        telar::on_cleanup(move || {
            ended.set(false);
            UNSUBSCRIBED.with(|n| n.set(n.get() + 1));
        });
        SINKS.with(|sinks| sinks.borrow_mut().push((alive, sink)));
    }

    static PROBE: SourceDef = SourceDef {
        id: "probe",
        fields: &[
            FieldDef {
                name: "level",
                privacy: Privacy::Public,
                ty: FieldType::Number,
            },
            FieldDef {
                name: "other",
                privacy: Privacy::Public,
                ty: FieldType::Text,
            },
            FieldDef {
                name: "secret",
                privacy: Privacy::Private,
                ty: FieldType::Text,
            },
            FieldDef {
                name: "asked",
                privacy: Privacy::OnLock(|lock| {
                    lock.notification_detail == NotificationDetail::Apps
                }),
                ty: FieldType::Text,
            },
        ],
        feed: probe_feed,
    };

    fn publish(level: f64, other: &str) {
        let mut sinks = SINKS.with(|sinks| std::mem::take(&mut *sinks.borrow_mut()));
        for (alive, sink) in &mut sinks {
            if alive.get() {
                sink(Reading::from([
                    Value::Number(level),
                    Value::text(other),
                    Value::text("the code is 4242"),
                    Value::text("Bank"),
                ]));
            }
        }
        SINKS.with(|held| held.borrow_mut().extend(sinks));
    }

    fn subscribed() -> usize {
        SUBSCRIBED.with(Cell::get)
    }

    fn unsubscribed() -> usize {
        UNSUBSCRIBED.with(Cell::get)
    }

    fn evaluations() -> usize {
        EVALUATIONS.with(Cell::get)
    }

    /// The shell's functions and `seen(x)`, which counts the evaluations that reach it.
    fn counting() -> Registry {
        crate::functions::registry().with(|registry| {
            registry.function(
                "seen",
                Signature::new([Pattern::Any], Type::Bool),
                "Counts an evaluation",
                |_| {
                    EVALUATIONS.with(|n| n.set(n.get() + 1));
                    Ok(Value::Bool(true))
                },
            );
        })
    }

    fn user(toml: &str) -> UserSources {
        let sources: BTreeMap<String, layout::Source> =
            toml::from_str(toml).expect("the sources parse");
        let (sources, report) = UserSources::of(
            "layouts/t.toml",
            &sources,
            &config::AutomationConfig::default(),
        );
        assert!(report.is_clean(), "{}", report.render());
        sources
    }

    fn environment() -> Environment {
        Environment::new([&PROBE], UserSources::default()).with_registry(counting())
    }

    fn reference(dotted: &str) -> Reference {
        let mut parts = dotted.split('.');
        let name = parts.next().unwrap();
        Reference::new(name, parts)
    }

    #[test]
    fn a_binding_re_evaluates_only_when_a_field_it_read_changes() {
        let scope = telar::owner_scope();
        let env = environment();
        let bound = env.bind(
            env.compile("seen(0) && $probe.level > 1").unwrap(),
            Gate::always(),
        );
        assert!(bound.get().error.is_some(), "no reading yet");
        let before = evaluations();

        publish(2.0, "a");
        assert_eq!(bound.get().value, Some(Value::Bool(true)));
        assert_eq!(evaluations(), before + 1);

        publish(2.0, "b");
        assert_eq!(bound.get().value, Some(Value::Bool(true)));
        assert_eq!(
            evaluations(),
            before + 1,
            "`other` moved, and the binding never read it"
        );

        publish(0.0, "b");
        assert_eq!(bound.get().value, Some(Value::Bool(false)));
        assert_eq!(evaluations(), before + 2);
        let owner = scope.id();
        drop(scope);
        telar::dispose_owner(owner);
    }

    #[test]
    fn a_binding_re_evaluates_only_when_a_variable_it_read_changes() {
        vars::set("env_test_watched", Var::Number(1.0)).unwrap();
        vars::set("env_test_other", Var::Number(1.0)).unwrap();
        let env = environment();
        let compiled = env.compile("seen(0) && $env_test_watched > 1").unwrap();
        let readings = Rc::new(env.readings(Gate::always()));
        let resolver = Rc::clone(&readings);
        let bound = telar::memo(move || compiled.evaluate(&*resolver));
        assert_eq!(bound.get(), Ok(Value::Bool(false)));
        let before = evaluations();

        let mut state = vars::list();
        state.insert("env_test_other".to_string(), Var::Number(5.0));
        readings.vars().apply(&state);
        assert_eq!(bound.get(), Ok(Value::Bool(false)));
        assert_eq!(evaluations(), before, "a variable it never read moved");

        state.insert("env_test_watched".to_string(), Var::Number(5.0));
        readings.vars().apply(&state);
        assert_eq!(bound.get(), Ok(Value::Bool(true)));
        assert_eq!(evaluations(), before + 1);
    }

    #[test]
    fn a_bound_colour_changes_when_var_set_runs_with_no_reload() {
        vars::set_text(
            "env_test_tint",
            "#112233",
            Some(&services::state::VarType::Color),
        )
        .unwrap();
        let env = environment();
        let compiled = env.compile("alpha($env_test_tint, 1)").unwrap();
        let readings = Rc::new(env.readings(Gate::always()));
        let resolver = Rc::clone(&readings);
        let bound = telar::memo(move || compiled.evaluate(&*resolver));
        let before = bound.get().expect("it reads the variable");

        vars::set_text("env_test_tint", "#ff8800", None).unwrap();
        readings.vars().apply(&vars::list());
        let after = bound.get().expect("and reads it again");
        assert_ne!(before, after);
        assert_eq!(
            after,
            Value::Color(telar::Color::from_hex("#ff8800").unwrap())
        );
    }

    #[test]
    fn a_branch_never_taken_subscribes_to_nothing() {
        let env = environment();
        let compiled = env.compile("if(1 > 2, $probe.level, 0)").unwrap();
        let readings = env.readings(Gate::always());
        assert_eq!(compiled.evaluate(&readings), Ok(Value::Number(0.0)));
        assert_eq!(subscribed(), 0, "the source was never started");
    }

    /// F-10.43: a binding's subscription is its owner's, and goes when the tree that made it goes.
    #[test]
    fn disposing_the_owner_takes_the_subscription_with_it() {
        let scope = telar::owner_scope();
        let owner = scope.id();
        let env = environment();
        let readings = env.readings(Gate::always());
        let _ = readings.read(&reference("probe.level"));
        drop(scope);
        assert_eq!((subscribed(), unsubscribed()), (1, 0));
        telar::dispose_owner(owner);
        assert_eq!(unsubscribed(), 1);
    }

    #[test]
    fn a_hidden_consumer_stops_counting_and_a_shown_one_counts_again() {
        let shown = telar::signal(true);
        let env = environment();
        let readings = env.readings(Gate::new(move || shown.get()));
        let _ = readings.read(&reference("probe.level"));
        let _ = readings.read(&reference("probe.other"));
        assert_eq!(
            subscribed(),
            1,
            "one subscription per source, however many of its fields are read"
        );

        shown.set(false);
        assert_eq!(
            unsubscribed(),
            1,
            "hiding the consumer gave the subscription back"
        );
        shown.set(true);
        assert_eq!(subscribed(), 2, "showing it took one out again");
        publish(7.0, "x");
        assert_eq!(
            readings.read(&reference("probe.level")),
            Ok(Value::Number(7.0))
        );
    }

    #[test]
    fn on_the_lock_screen_a_private_field_is_empty_and_its_service_is_never_asked() {
        let env = environment().shown_to(Audience::Anyone, LockConfig::default());
        let compiled = env.compile("$probe.secret").unwrap();
        let readings = env.readings(Gate::always());
        assert_eq!(compiled.evaluate(&readings), Ok(Value::text("")));
        assert_eq!(subscribed(), 0);

        assert_eq!(
            env.compile("$probe.asked").unwrap().evaluate(&readings),
            Ok(Value::text("")),
            "the application names stay off a locked screen by default"
        );

        let _ = readings.read(&reference("probe.level"));
        publish(3.0, "x");
        assert_eq!(
            readings.read(&reference("probe.level")),
            Ok(Value::Number(3.0)),
            "a public field reads as it does for the owner"
        );
    }

    #[test]
    fn a_field_the_lock_config_decides_follows_that_config() {
        let lock = LockConfig {
            notification_detail: NotificationDetail::Apps,
            ..LockConfig::default()
        };
        let env = environment().shown_to(Audience::Anyone, lock);
        let readings = env.readings(Gate::always());
        let _ = readings.read(&reference("probe.asked"));
        publish(1.0, "x");
        assert_eq!(
            readings.read(&reference("probe.asked")),
            Ok(Value::text("Bank"))
        );
        assert_eq!(
            readings.read(&reference("probe.secret")),
            Ok(Value::text("")),
            "and a private one stays empty whatever it says"
        );
    }

    #[test]
    fn the_owner_sees_every_field() {
        let env = environment();
        let readings = env.readings(Gate::always());
        let _ = readings.read(&reference("probe.secret"));
        publish(1.0, "x");
        assert_eq!(
            readings.read(&reference("probe.secret")),
            Ok(Value::text("the code is 4242"))
        );
    }

    #[test]
    fn the_lock_screen_refuses_a_command_source_that_is_not_lock_safe() {
        let sources = user(
            "[temp]\nkind = 'poll'\ncmd = 'sensors'\ninitial = 0\n[load]\nkind = 'poll'\ncmd = 'uptime'\nlock_safe = true",
        );
        let owner = Environment::new([&PROBE], sources.clone());
        assert!(owner.compile("$temp + 1").is_ok());

        let anyone = owner.shown_to(Audience::Anyone, LockConfig::default());
        let Err(refused) = anyone.compile("$temp + 1") else {
            panic!("a command source that is not lock_safe compiled for the lock screen");
        };
        assert!(refused.to_string().contains("lock_safe"), "{refused}");
        assert!(anyone.compile("$load").is_ok());
    }

    #[test]
    fn a_user_source_reads_its_initial_value_until_it_has_a_reading() {
        let env = Environment::new(
            [&PROBE],
            user(
                "[temp]\nkind = 'poll'\ncmd = 'sensors'\ninitial = 21\n[raw]\nkind = 'poll'\ncmd = 'date'",
            ),
        );
        let readings = env.readings(Gate::always());
        assert_eq!(env.compile("$temp * 2").unwrap().ty(), &Type::Number);
        assert_eq!(
            env.compile("$temp * 2").unwrap().evaluate(&readings),
            Ok(Value::Number(42.0))
        );
        assert!(
            env.compile("$raw").unwrap().evaluate(&readings).is_err(),
            "no initial and no reading yet is an error the binding holds through"
        );
    }

    #[test]
    fn names_resolve_to_sources_variables_and_events_or_say_what_is_there() {
        vars::set("env_test_accent", Var::Color("#88c0d0".to_string())).unwrap();
        let env = environment();
        let ty = |source: &str| env.compile(source).map(|compiled| compiled.ty().clone());
        assert_eq!(ty("$probe.level"), Ok(Type::Number));
        assert_eq!(ty("$env_test_accent"), Ok(Type::Color));
        assert_eq!(ty("$event.session_locked"), Ok(Type::Text));

        let said = |source: &str| match env.compile(source) {
            Ok(_) => panic!("`{source}` compiled"),
            Err(errors) => errors.to_string(),
        };
        assert!(said("$probe").contains("`level`"), "{}", said("$probe"));
        assert!(
            said("$probe.nope").contains("`other`"),
            "{}",
            said("$probe.nope")
        );
        assert!(said("$event.nope").contains("session_locked"));
        assert!(said("$nothing_here").contains("nothing is called"));
        assert!(said("$probe.level.deeper").contains("deeper"));

        let offered = env.references();
        assert!(
            offered
                .iter()
                .any(|(r, ty, kind)| r.dotted() == "probe.level"
                    && *ty == Type::Number
                    && *kind == ReferenceKind::Field)
        );
        assert!(
            offered
                .iter()
                .any(|(r, _, kind)| r.dotted() == "event.colors_changed"
                    && *kind == ReferenceKind::Event)
        );
    }

    /// `$theme.*` is a public reading every expression may name, on the lock screen too, so an accent can be mixed from the theme's own.
    #[test]
    fn the_theme_s_colours_are_a_reading_anyone_may_see() {
        let _scope = telar::owner_scope();
        let theme = config::theme::NordTheme::default();
        for env in [
            environment(),
            environment().shown_to(Audience::Anyone, LockConfig::default()),
        ] {
            let compiled = env
                .compile("mix($theme.accent, #ff0000, 0)")
                .expect("it compiles");
            assert_eq!(compiled.ty(), &Type::Color);
            let held = env.bind(compiled, Gate::always()).get();
            assert_eq!(held.error, None);
            assert_eq!(
                env.compile("$theme.fg")
                    .unwrap()
                    .evaluate(&env.readings(Gate::always())),
                Ok(Value::Color(theme.text))
            );
            assert!(held.value.is_some());
        }
    }

    /// A copy of a repeated group reads its item and index before any other name, live: its item moving re-evaluates it like any reading. Outside a copy the two names say where they can be read.
    #[test]
    fn a_copy_reads_its_item_and_index_before_any_other_name() {
        let scope = telar::owner_scope();
        let item = telar::signal(Some(Value::text("first")));
        let env = environment().with_locals(vec![
            Local::live(layout::ITEM, Type::Text, move || item.get()),
            Local::live(layout::INDEX, Type::Number, || Some(Value::Number(2.0))),
        ]);
        let bound = env.bind(
            env.compile("fmt('{} {}', $index, $item)").unwrap(),
            Gate::always(),
        );
        assert_eq!(bound.get().value, Some(Value::text("2 first")));
        item.set(Some(Value::text("second")));
        assert_eq!(bound.get().value, Some(Value::text("2 second")));
        assert!(
            env.references()
                .iter()
                .any(|(r, ty, kind)| r.dotted() == "item"
                    && *ty == Type::Text
                    && *kind == ReferenceKind::Local)
        );

        let typed =
            environment().with_locals(Local::typed_all(&layout::Locals::of_copy(Type::Number)));
        assert_eq!(
            typed.compile("$item + $index").map(|c| c.ty().clone()),
            Ok(Type::Number)
        );
        let said = |env: &Environment, source: &str| match env.compile(source) {
            Ok(_) => panic!("`{source}` compiled"),
            Err(errors) => errors.to_string(),
        };
        assert!(said(&typed, "$item.name").contains("no fields"));
        assert!(said(&environment(), "$item").contains("`repeat`"));
        let owner = scope.id();
        drop(scope);
        telar::dispose_owner(owner);
    }

    /// What an editor offers says what each name reads in this environment — a layout's own source even where it is not the running layout's, and a local before a source of the same name — rather than what the running shell would make of it.
    #[test]
    fn each_offered_reference_says_what_it_reads_here() {
        vars::set("env_test_offered", Var::Bool(true)).unwrap();
        let sources =
            user("[uptime]\nkind = 'poll'\ncmd = 'uptime'\n[item]\nkind = 'poll'\ncmd = 'date'");
        let env = Environment::new([&PROBE], sources)
            .with_locals(Local::typed_all(&layout::Locals::of_copy(Type::Number)));
        let offered = env.references();
        let kind_of = |dotted: &str| {
            let found: Vec<_> = offered
                .iter()
                .filter(|(reference, _, _)| reference.dotted() == dotted)
                .map(|(_, ty, kind)| (ty.clone(), *kind))
                .collect();
            assert_eq!(found.len(), 1, "`${dotted}` is offered once: {found:?}");
            found[0].clone()
        };
        assert_eq!(kind_of("uptime"), (Type::Text, ReferenceKind::Source));
        assert_eq!(kind_of("item"), (Type::Number, ReferenceKind::Local));
        assert_eq!(kind_of("probe.level"), (Type::Number, ReferenceKind::Field));
        assert_eq!(
            kind_of("theme.highlight_low"),
            (Type::Color, ReferenceKind::Theme)
        );
        assert_eq!(
            kind_of("env_test_offered"),
            (Type::Bool, ReferenceKind::Var)
        );
        assert_eq!(kind_of("event.started"), (Type::Text, ReferenceKind::Event));
        vars::remove("env_test_offered");
    }

    /// A variable is set while the shell runs, so `$name` naming nothing yet waits for one: whatever checked it reactively checks it again once it is set, and comes alive then.
    #[test]
    fn a_check_made_reactively_runs_again_once_a_variable_it_names_is_set() {
        vars::remove("env_test_later");
        let scope = telar::owner_scope();
        let env = environment();
        let source = "$env_test_later + 1";
        let checking = env.clone();
        let checked = telar::memo(move || {
            checking
                .compile(source)
                .map(|compiled| compiled.ty().clone())
                .map_err(|errors| errors.to_string())
        });
        assert!(checked.get().is_err());
        let errors = env.compile(source).unwrap_err();
        assert!(
            errors
                .iter()
                .all(|error| env.awaits_variable(source, error))
        );
        for wrong in ["$probe.nope", "$probe", "$event", "$item"] {
            let errors = env.compile(wrong).unwrap_err();
            assert!(
                !errors.iter().any(|error| env.awaits_variable(wrong, error)),
                "`{wrong}` is no variable to wait for"
            );
        }

        vars::set("env_test_later", Var::Number(2.0)).unwrap();
        assert_eq!(checked.get(), Ok(Type::Number));
        assert!(!env.awaits_variable(source, &errors.iter().next().unwrap().clone()));
        vars::remove("env_test_later");
        assert!(checked.get().is_err(), "and gone again once it is removed");
        let owner = scope.id();
        drop(scope);
        telar::dispose_owner(owner);
    }

    /// The lock layer's view asks the `[lock]` of the area it is drawn in, which a monitor may set for itself, and the running config's where there is no area.
    #[test]
    fn a_layer_s_view_asks_the_lock_config_of_where_it_is_drawn() {
        let with = |detail: NotificationDetail| {
            let mut config = config::Config::default();
            config.lock.notification_detail = detail;
            Arc::new(config)
        };
        let asked = reference("probe.asked");
        config::set_config(with(NotificationDetail::Apps));
        let anyone = environment().on_layer(true);
        assert_eq!(anyone.audience(), Audience::Anyone);
        assert!(
            !anyone.hidden_on_lock(&asked),
            "the running config shows it"
        );
        assert_eq!(environment().on_layer(false).audience(), Audience::Owner);

        let scope = telar::owner_scope();
        ui::chrome::Chrome::global(with(NotificationDetail::Count), None).provide();
        assert!(
            environment().on_layer(true).hidden_on_lock(&asked),
            "the area's own config hides it"
        );
        let owner = scope.id();
        drop(scope);
        telar::dispose_owner(owner);
        config::clear_config();
    }

    /// A name's failure travels through the expression language as a key and its arguments, and reads in the language of whoever shows it.
    #[test]
    fn a_name_s_failure_reads_in_the_language_it_is_shown_in() {
        let env = environment();
        let error = env.compile("$probe.levle").unwrap_err().into_first();
        let said = describe(&error.code);
        assert_eq!(said.key(), Some("expression.no_such_field"));
        assert_eq!(
            said.english(),
            "`$probe` has no `levle`: it has `level`, `other`, `secret`, `asked`"
        );
        assert_eq!(
            said.render_in("es"),
            "`$probe` no tiene `levle`: tiene `level`, `other`, `secret`, `asked`"
        );
        assert_eq!(
            error.message(),
            said.english(),
            "the language's own English is the catalogue's"
        );
        let typed = env.compile("$probe.level + 'a'").unwrap_err().into_first();
        assert_eq!(
            describe(&typed.code).render_in("es"),
            Message::expression(&typed.code).render_in("es"),
            "a failure of the language itself is the language's to word"
        );
    }

    #[test]
    fn every_failure_and_finding_has_words_in_every_language_the_shell_speaks() {
        assert_eq!(
            util::report::untranslated(&crate::__rsx_i18n::CATALOG, &["finding.", "expression."]),
            Vec::<String>::new()
        );
    }
}
