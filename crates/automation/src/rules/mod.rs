//! Rules: when something happens — an event, an expression turning true, a time of day, an interval — check `when`, run a chain of commands, and keep a value in a variable (TA-6, T-8.5). They are written in `config.toml` as `[[rules]]`.
//!
//! **Where they run.** On the driver thread, beside the surfaces. A rule's expressions read module readings, which their services feed only to a subscriber on that thread, and its commands go through the command table, which only that thread holds; reading through the same [`Environment`] a binding does means one implementation of every name an expression can write. A rule's readings are held whatever is on screen and while the session is locked ([`Environment::for_rules`]): rules are session automation, not lock content (TA-8).
//!
//! **Once per crossing.** An `edge` rule fires when its expression turns from `false` to `true` — never for a reading that leaves it true, and never for the first answer, which only records where it stands. An evaluation that fails moves nothing. `when` is read as the rule fires, not when the trigger was armed. Firing is deferred out of the reactive pass that noticed the crossing, so the commands it runs never run inside one.
//!
//! **Reloading.** A reload hands the whole set over again. A rule whose config, and what each name it reads means, are both unchanged keeps running as it was — its edge state, its interval's phase — so a reload neither fires it again nor forgets a crossing it is waiting on. Every other rule is stopped, and started again if it is still written. The set is checked again the same way as a variable it names appears, goes or changes type ([`vars::declared`]), so a rule that reads `$name` is invalid until `name` exists and starts then.
//!
//! **Failures.** What keeps a rule from loading is `config check`'s ([`check`]); what goes wrong as one fires — a command the shell refused, a `when` or `store` that could not be read — is a [`crate::failures`] site, under the rule's place in the file, until it next fires cleanly. A `when` waiting on a reading that has not arrived does not hold, which is no failure.
//!
//! **Loops.** A rule's commands run while it fires, so a rule whose commands run it again would recurse until the stack gives out: `config check` refuses `rule run` in a rule, and a rule asked to fire while it is already firing — however that came about — is refused and reported. A loop across turns of the driver loop — two rules setting each other off through a variable — never deepens the stack, so it is caught by rate instead: a rule that fires more than [`RuleConfig::FIRINGS_PER_SECOND`] times within a second is suspended and reported until the next [`Rules::load`].

mod check;
mod schedule;

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use config::{AutomationConfig, RuleConfig};
use platform_wayland::EventSender;
use services::events::{Delivery, Edge, SESSION_ENDING, ShellEvent};
use telar::{Memo, OwnerId};
use telar_expression::{Error, Type, Value};
use util::report::Message;

use crate::env::{self, Environment, Gate};
use crate::failures::{self, Site};
use crate::sources::keep_awake;
use crate::vars;

pub use check::{Problem, Trigger, check};
pub use schedule::{Alarm, Metronome, Schedule, ScheduleError};

use check::Prepared;

/// How often a timer thread looks at the clock and at whether its rule is still loaded: the most a scheduled time is noticed late by, and the longest a stopped rule's thread outlives it.
const LOOK: Duration = Duration::from_secs(1);

/// The rules engine. Cheap to clone: every clone is the same engine.
#[derive(Clone)]
pub struct Rules(Rc<RefCell<Engine>>);

/// Runs a task once whatever is running now has finished: the driver loop's next turn, in the shell.
pub type Defer = Rc<dyn Fn(Box<dyn FnOnce()>)>;

struct Engine {
    defer: Defer,
    file: PathBuf,
    written: Vec<RuleConfig>,
    env: Option<Environment>,
    automation: AutomationConfig,
    running: BTreeMap<String, Running>,
    fired: BTreeMap<String, DateTime<Local>>,
    firing: BTreeSet<String>,
    budgets: BTreeMap<String, Budget>,
    suspended: BTreeSet<String>,
    events: Option<OwnerId>,
    checking: Option<OwnerId>,
    serial: u64,
}

/// How many times a rule has fired since the second it is counting from.
struct Budget {
    since: Instant,
    fired: u32,
}

impl Budget {
    /// Counts one firing at `now`, answering whether the rule is still within [`RuleConfig::FIRINGS_PER_SECOND`].
    fn spend(&mut self, now: Instant) -> bool {
        if now.duration_since(self.since) >= Duration::from_secs(1) {
            *self = Budget {
                since: now,
                fired: 0,
            };
        }
        self.fired += 1;
        self.fired <= RuleConfig::FIRINGS_PER_SECOND
    }
}

/// Marks a rule as firing until dropped, which is what lets a nested firing of it be refused.
struct Entered {
    engine: Rc<RefCell<Engine>>,
    id: String,
}

impl Drop for Entered {
    fn drop(&mut self) {
        self.engine.borrow_mut().firing.remove(&self.id);
    }
}

/// A rule that is loaded: what it was loaded from, the reactive owner its readings and timers belong to, and what it reads as it fires.
struct Running {
    serial: u64,
    rule: RuleConfig,
    prepared: Prepared,
    owner: OwnerId,
    when: Option<Memo<Result<Value, Error>>>,
    store: Option<Stored>,
}

#[derive(Clone)]
struct Stored {
    var: String,
    ty: Type,
    value: Memo<Result<Value, Error>>,
}

/// What firing a rule does, taken out of the engine so nothing of it is borrowed while its commands run, one of which may reload the rules.
struct Firing {
    run: Vec<String>,
    when: Option<Memo<Result<Value, Error>>>,
    store: Option<Stored>,
}

/// One rule as `hogar-shell rule list` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    /// The trigger as written: `event colors_changed`, `edge $battery.level < 15`, `schedule 07:30 mon-fri`, `every 5m`.
    pub trigger: String,
    pub state: State,
    pub last_fired: Option<DateTime<Local>>,
}

impl Listed {
    /// When the rule last fired, to the second, as `rule list` and the settings page both show it.
    pub fn fired_at(&self) -> Option<String> {
        self.last_fired
            .map(|at| at.format("%Y-%m-%d %H:%M:%S").to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Loaded, and firing when its trigger does.
    On,
    /// `enabled = false`.
    Off,
    /// Written with a mistake that keeps it from loading; `config check` says which.
    Invalid,
    /// Fired too often to be anything but a loop, and kept from firing until the config is loaded again.
    Suspended,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::On => "on",
            State::Off => "off",
            State::Invalid => "invalid",
            State::Suspended => "suspended",
        }
    }
}

impl Rules {
    /// An engine whose failures are reported against `file`, the `config.toml` its rules are written in.
    pub fn new(defer: Defer, file: PathBuf) -> Self {
        Self(Rc::new(RefCell::new(Engine {
            defer,
            file,
            written: Vec::new(),
            env: None,
            automation: AutomationConfig::default(),
            running: BTreeMap::new(),
            fired: BTreeMap::new(),
            firing: BTreeSet::new(),
            budgets: BTreeMap::new(),
            suspended: BTreeSet::new(),
            events: None,
            checking: None,
            serial: 0,
        })))
    }

    /// Makes `rules` the set that runs, read through `env`. An unchanged rule keeps running as it was; see the module docs. A rule suspended as caught in a loop starts again.
    pub fn load(&self, rules: &[RuleConfig], env: Environment, automation: &AutomationConfig) {
        let (suspended, checking) = {
            let mut engine = self.0.borrow_mut();
            engine.written = rules.to_vec();
            engine.env = Some(env);
            engine.automation = *automation;
            engine.budgets.clear();
            (
                std::mem::take(&mut engine.suspended),
                engine.checking.take(),
            )
        };
        self.refile_failures(&suspended);
        if let Some(owner) = checking {
            telar::dispose_owner(owner);
        }
        let owner = telar::detached(|| telar::owner_scope().id());
        let engine = Rc::downgrade(&self.0);
        telar::with_owner(Some(owner), || {
            telar::effect(move || {
                if let Some(engine) = engine.upgrade() {
                    Rules(engine).reconcile();
                }
            })
        });
        self.0.borrow_mut().checking = Some(owner);
    }

    /// Run by the effect [`Rules::load`] makes, so checking the set reads each variable it names through [`vars::declared`] and runs again when one appears, goes or changes type.
    fn reconcile(&self) {
        let (written, env, automation) = {
            let engine = self.0.borrow();
            let Some(env) = engine.env.clone() else {
                return;
            };
            (engine.written.clone(), env, engine.automation)
        };
        let prepared =
            check::prepare_all(&written, &env, &services::command::resolves, &automation);
        let mut wanted: BTreeMap<String, (RuleConfig, Prepared)> = BTreeMap::new();
        for (rule, (prepared, _)) in written.iter().zip(prepared) {
            if let Some(prepared) = prepared.filter(|_| rule.enabled) {
                wanted
                    .entry(rule.id.clone())
                    .or_insert((rule.clone(), prepared));
            }
        }

        let stopped: Vec<OwnerId> = {
            let mut engine = self.0.borrow_mut();
            let unchanged = |running: &Running| {
                wanted
                    .get(&running.rule.id)
                    .is_some_and(|(rule, prepared)| {
                        *rule == running.rule && prepared.meanings == running.prepared.meanings
                    })
            };
            let gone: Vec<String> = engine
                .running
                .values()
                .filter(|running| !unchanged(running))
                .map(|running| running.rule.id.clone())
                .collect();
            gone.iter()
                .filter_map(|id| engine.running.remove(id))
                .map(|running| running.owner)
                .collect()
        };
        for owner in stopped {
            telar::dispose_owner(owner);
        }

        for (id, (rule, prepared)) in wanted {
            if self.0.borrow().running.contains_key(&id) {
                continue;
            }
            let serial = {
                let mut engine = self.0.borrow_mut();
                engine.serial += 1;
                engine.serial
            };
            let running = self.start(serial, rule, prepared, &env);
            self.0.borrow_mut().running.insert(id, running);
        }

        self.listen_for_events();
        {
            let mut engine = self.0.borrow_mut();
            let ids: Vec<String> = engine.written.iter().map(|rule| rule.id.clone()).collect();
            engine.fired.retain(|id, _| ids.contains(id));
        }
        self.refile_failures(&BTreeSet::new());
    }

    /// Loads one rule under a reactive owner of its own, which stopping it disposes along with every reading and timer it holds.
    fn start(
        &self,
        serial: u64,
        rule: RuleConfig,
        prepared: Prepared,
        env: &Environment,
    ) -> Running {
        let owner = telar::detached(|| telar::owner_scope().id());
        let id = rule.id.clone();
        let bind = |compiled: &telar_expression::Compiled| {
            telar_expression::bind(compiled.clone(), env.readings(Gate::always()))
        };
        let (when, store) = telar::with_owner(Some(owner), || {
            let when = prepared.when.as_ref().map(bind);
            let store = prepared.store.as_ref().map(|(var, value)| Stored {
                var: var.clone(),
                ty: value.ty().clone(),
                value: bind(value),
            });
            match &prepared.trigger {
                Trigger::Edge(edge) => self.watch_edge(&id, serial, bind(edge)),
                Trigger::Event(kind) => keep_awake(*kind),
                Trigger::Schedule(schedule) => self.tick(&id, serial, alarm(schedule.clone())),
                Trigger::Every(every) => self.tick(&id, serial, metronome(*every)),
            }
            (when, store)
        });
        Running {
            serial,
            rule,
            prepared,
            owner,
            when,
            store,
        }
    }

    /// Fires `id` each time `edge` turns from `false` to `true`.
    fn watch_edge(&self, id: &str, serial: u64, edge: Memo<Result<Value, Error>>) {
        let crossing = Rc::new(Edge::<bool>::new());
        let rules = Rc::downgrade(&self.0);
        let id = id.to_string();
        telar::effect(move || {
            let Ok(Value::Bool(now)) = edge.get() else {
                return;
            };
            if crossing.observe(now) != Some(true) {
                return;
            }
            let Some(engine) = rules.upgrade() else {
                return;
            };
            let defer = Rc::clone(&engine.borrow().defer);
            let (rules, id) = (Rules(engine), id.clone());
            defer(Box::new(move || rules.fire(&id, serial)));
        });
    }

    /// Fires `id` each time `producer`, a timer on a thread of its own, says it is due.
    fn tick(&self, id: &str, serial: u64, producer: impl FnOnce(EventSender<()>) + Send + 'static) {
        let rules = Rc::downgrade(&self.0);
        let id = id.to_string();
        platform_wayland::watch(producer, move |()| {
            if let Some(engine) = rules.upgrade() {
                Rules(engine).fire(&id, serial);
            }
        });
    }

    /// Keeps one listener on the event stream while any loaded rule is triggered by an event, and none while no rule is.
    fn listen_for_events(&self) {
        let wanted = self
            .0
            .borrow()
            .running
            .values()
            .any(|running| matches!(running.prepared.trigger, Trigger::Event(_)));
        let held = self.0.borrow().events;
        match (wanted, held) {
            (true, None) => {
                let owner = telar::detached(|| telar::owner_scope().id());
                let rules: Weak<RefCell<Engine>> = Rc::downgrade(&self.0);
                telar::with_owner(Some(owner), || {
                    platform_wayland::watch(listen_to_events, move |delivery: Delivery| {
                        if let Some(engine) = rules.upgrade() {
                            Rules(engine).on_event(&delivery.event);
                        }
                    });
                });
                self.0.borrow_mut().events = Some(owner);
            }
            (false, Some(owner)) => {
                self.0.borrow_mut().events = None;
                telar::dispose_owner(owner);
            }
            _ => {}
        }
    }

    /// Fires every loaded rule `event` triggers.
    pub fn on_event(&self, event: &ShellEvent) {
        let kind = event.kind();
        let due: Vec<(String, u64)> = self
            .0
            .borrow()
            .running
            .values()
            .filter(|running| matches!(running.prepared.trigger, Trigger::Event(on) if on == kind))
            .map(|running| (running.rule.id.clone(), running.serial))
            .collect();
        for (id, serial) in due {
            self.fire(&id, serial);
        }
    }

    /// Fires the rule loaded as `id`, if it is still the one loaded as `serial` and is not suspended: checks `when`, runs the commands, keeps the value.
    fn fire(&self, id: &str, serial: u64) {
        let firing = {
            let engine = self.0.borrow();
            if engine.suspended.contains(id) {
                return;
            }
            let Some(running) = engine
                .running
                .get(id)
                .filter(|running| running.serial == serial)
            else {
                return;
            };
            Firing {
                run: running.rule.run.clone(),
                when: running.when,
                store: running.store.clone(),
            }
        };
        if let Some(when) = firing.when {
            match when.try_get() {
                Some(Ok(Value::Bool(true))) => {}
                Some(Ok(_)) | None => return,
                Some(Err(error)) if env::awaits_reading(&error) => return,
                Some(Err(error)) => {
                    self.failed(
                        id,
                        util::message!("finding.when_unreadable", why = env::describe(&error.code)),
                    );
                    return;
                }
            }
        }
        let _ = self.perform(id, &firing.run, firing.store.as_ref());
    }

    /// Runs `run` ([`services::command::run_chain`]), then keeps `store`'s value, answering each command's reply. Refused while `id` is already firing, and once it has fired too often to be anything but a loop.
    fn perform(
        &self,
        id: &str,
        run: &[String],
        store: Option<&Stored>,
    ) -> Result<Vec<String>, Message> {
        let _entered = self.enter(id)?;
        let replies = services::command::run_chain(run)
            .map_err(|refused| self.failed(id, Message::verbatim(refused.to_string())))?;
        if let Some(store) = store {
            let value = match store.value.try_get() {
                Some(Ok(value)) => value,
                Some(Err(error)) => {
                    return Err(self.failed(
                        id,
                        util::message!(
                            "finding.store_unreadable",
                            why = env::describe(&error.code)
                        ),
                    ));
                }
                None => return Ok(replies),
            };
            vars::store(&store.var, &value, &store.ty).map_err(|why| {
                self.failed(
                    id,
                    util::message!("finding.not_kept", var = &store.var, why = why),
                )
            })?;
        }
        self.0
            .borrow_mut()
            .fired
            .insert(id.to_string(), Local::now());
        self.recovered(id);
        Ok(replies)
    }

    /// Marks `id` as firing, or says why it may not fire now: it is suspended, it is firing already — its own commands ran it again — or this firing is one too many for its budget, which suspends it.
    fn enter(&self, id: &str) -> Result<Entered, Message> {
        let refused = {
            let mut engine = self.0.borrow_mut();
            let now = Instant::now();
            if engine.suspended.contains(id) {
                return Err(suspension(id));
            }
            if engine.firing.contains(id) {
                Some(util::message!("finding.ran_itself", id = id))
            } else if !engine
                .budgets
                .entry(id.to_string())
                .or_insert(Budget {
                    since: now,
                    fired: 0,
                })
                .spend(now)
            {
                engine.suspended.insert(id.to_string());
                Some(suspension(id))
            } else {
                engine.firing.insert(id.to_string());
                None
            }
        };
        match refused {
            Some(why) => Err(self.failed(id, why)),
            None => Ok(Entered {
                engine: Rc::clone(&self.0),
                id: id.to_string(),
            }),
        }
    }

    /// `hogar-shell rule run <id>`: the rule's commands, now, whatever its trigger, `when` and `enabled` say. Answers each command and its reply, one tab-separated row each — the reply's payload, or `ok` where there is none — or the first refusal.
    pub fn run_now(&self, id: &str) -> Result<String, String> {
        let run = {
            let engine = self.0.borrow();
            let Some(rule) = engine.written.iter().find(|rule| rule.id == id) else {
                let known: Vec<&str> = engine.written.iter().map(|rule| rule.id.as_str()).collect();
                return Err(match known.is_empty() {
                    true => {
                        format!("there is no rule called `{id}`, and `config.toml` writes none")
                    }
                    false => format!("there is no rule called `{id}` (try: {})", known.join(", ")),
                });
            };
            rule.run.clone()
        };
        let replies = self.perform(id, &run, None).map_err(|why| why.english())?;
        Ok(run
            .iter()
            .zip(replies)
            .map(|(line, reply)| match reply.is_empty() {
                true => format!("{line}\tok"),
                false => format!("{line}\t{reply}"),
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Every rule `config.toml` writes, in its order.
    pub fn list(&self) -> Vec<Listed> {
        let engine = self.0.borrow();
        engine
            .written
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                let first =
                    engine.written.iter().position(|other| other.id == rule.id) == Some(index);
                let state = match engine.running.get(&rule.id) {
                    _ if first && engine.suspended.contains(&rule.id) => State::Suspended,
                    Some(running) if first && running.rule == *rule => State::On,
                    _ if !rule.enabled => State::Off,
                    _ => State::Invalid,
                };
                Listed {
                    id: rule.id.clone(),
                    trigger: trigger_text(rule),
                    state,
                    last_fired: first.then(|| engine.fired.get(&rule.id).copied()).flatten(),
                }
            })
            .collect()
    }

    /// Records why `id` did not finish, answering it.
    fn failed(&self, id: &str, why: Message) -> Message {
        if let Some(site) = self.site(id) {
            failures::fail(site, why.clone());
        }
        why
    }

    fn recovered(&self, id: &str) {
        if let Some(site) = self.site(id) {
            failures::recover(&site);
        }
    }

    fn site(&self, id: &str) -> Option<Site> {
        let engine = self.0.borrow();
        let index = engine.written.iter().position(|rule| rule.id == id)?;
        Some(Site::Rule {
            file: engine.file.clone(),
            index,
            id: id.to_string(),
        })
    }

    /// Moves this engine's failures to where their rules are written now, forgetting those of rules no longer written and of `forget`.
    fn refile_failures(&self, forget: &BTreeSet<String>) {
        let engine = self.0.borrow();
        failures::refile(|site| match site {
            Site::Rule { file, id, .. } if *file == engine.file => {
                let index = engine.written.iter().position(|rule| rule.id == *id)?;
                (!forget.contains(id)).then(|| Site::Rule {
                    file: file.clone(),
                    index,
                    id: id.clone(),
                })
            }
            other => Some(other.clone()),
        });
    }
}

fn suspension(id: &str) -> Message {
    util::message!(
        "finding.suspended",
        id = id,
        count = RuleConfig::FIRINGS_PER_SECOND
    )
}

fn trigger_text(rule: &RuleConfig) -> String {
    match rule.trigger.written().first() {
        Some((key, value)) => format!("{key} {}", value.trim()),
        None => "-".to_string(),
    }
}

/// The event stream, forwarded to the driver thread until nobody is listening there.
///
/// The events that end the session are held, so the session action waits until the driver thread has run the rules they trigger and dropped the delivery; a delivery that never reaches the driver thread is dropped with the channel, which lets go too.
fn listen_to_events(tx: EventSender<Delivery>) {
    let events = services::events::listen_holding(&SESSION_ENDING);
    while tx.alive() {
        if let Some(delivery) = events.next_within(LOOK)
            && !tx.send(delivery)
        {
            return;
        }
    }
}

fn alarm(schedule: Schedule) -> impl FnOnce(EventSender<()>) + Send + 'static {
    move |tx| {
        let now = || Local::now().naive_local();
        let mut alarm = Alarm::new(schedule, now());
        while tx.alive() {
            if alarm.ring(now()) && !tx.send(()) {
                return;
            }
            std::thread::sleep(alarm.wait(now()).min(LOOK));
        }
    }
}

fn metronome(every: Duration) -> impl FnOnce(EventSender<()>) + Send + 'static {
    move |tx| {
        let mut metronome = Metronome::new(every, Instant::now());
        while tx.alive() {
            if metronome.beat(Instant::now()) && !tx.send(()) {
                return;
            }
            std::thread::sleep(metronome.wait(Instant::now()).min(LOOK));
        }
    }
}

thread_local! {
    static SHELL: OnceCell<Rules> = const { OnceCell::new() };
}

/// The shell's own engine, on the driver thread: rules fire on the loop's next turn after the pass that noticed them, and fail against the `config.toml` the shell reads.
pub fn shell() -> Rules {
    SHELL.with(|shell| {
        shell
            .get_or_init(|| {
                let defer: Defer = Rc::new(|task: Box<dyn FnOnce()>| {
                    platform_wayland::timeout(Duration::ZERO, task)
                });
                Rules::new(defer, config::Config::default_path())
            })
            .clone()
    })
}

/// Loads `rules` into the shell's engine on the loop's next turn, outside whatever surface asked for it: what a rule holds belongs to no surface, so it must not be filed against one and taken down with it.
pub fn install(rules: Vec<RuleConfig>, env: Environment, automation: AutomationConfig) {
    platform_wayland::timeout(Duration::ZERO, move || {
        shell().load(&rules, env, &automation)
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use config::{RuleStore, RuleTrigger};
    use services::events::EventKind;
    use services::state::Var;
    use ui::descriptor::{FieldDef, FieldType, Privacy, Reading, Sink, SourceDef};

    use super::*;
    use crate::UserSources;

    thread_local! {
        static SINKS: RefCell<Vec<Sink>> = RefCell::default();
        static RAN: RefCell<Vec<String>> = RefCell::default();
        static LATER: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::default();
    }

    fn battery_feed(sink: Sink) {
        SINKS.with(|sinks| sinks.borrow_mut().push(sink));
    }

    static BATTERY: SourceDef = SourceDef {
        id: "battery",
        fields: &[
            FieldDef {
                name: "level",
                privacy: Privacy::Public,
                ty: FieldType::Number,
            },
            FieldDef {
                name: "charging",
                privacy: Privacy::Public,
                ty: FieldType::Bool,
            },
        ],
        feed: battery_feed,
    };

    fn no_feed(_: Sink) {}

    static NOTES: SourceDef = SourceDef {
        id: "notes",
        fields: &[FieldDef {
            name: "summary",
            privacy: Privacy::Private,
            ty: FieldType::Text,
        }],
        feed: no_feed,
    };

    /// The battery service publishing a reading, to every rule reading it.
    fn battery(level: f64, charging: bool) {
        let mut sinks = SINKS.with(|sinks| std::mem::take(&mut *sinks.borrow_mut()));
        for sink in &mut sinks {
            sink(Reading::from([Value::Number(level), Value::Bool(charging)]));
        }
        SINKS.with(|held| held.borrow_mut().extend(sinks));
        settle();
    }

    /// The driver loop's next turn: runs what the engine deferred.
    fn settle() {
        loop {
            let later = LATER.with(|later| std::mem::take(&mut *later.borrow_mut()));
            if later.is_empty() {
                return;
            }
            for task in later {
                task();
            }
        }
    }

    /// An engine over the probe battery, with a command table that records each line and refuses the ones starting with `nope`.
    fn engine() -> Rules {
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                match line.starts_with("nope") {
                    true => "err no such command".to_string(),
                    false => "ok".to_string(),
                }
            },
            |line| !line.starts_with("typo"),
        );
        RAN.with(|ran| ran.borrow_mut().clear());
        Rules::new(
            Rc::new(|task: Box<dyn FnOnce()>| LATER.with(|later| later.borrow_mut().push(task))),
            config_file(),
        )
    }

    /// A `config.toml` of this test's own, since every engine reports into the one registry.
    fn config_file() -> PathBuf {
        let test = std::thread::current().name().unwrap_or("rules").to_string();
        PathBuf::from(format!("{test}.toml"))
    }

    fn failing() -> util::report::Report {
        let file = config_file();
        let mut report = util::report::Report::default();
        for finding in failures::report().errors {
            if finding.file == file {
                report.error(finding);
            }
        }
        report
    }

    fn environment() -> Environment {
        Environment::new([&BATTERY, &NOTES], UserSources::default()).for_rules()
    }

    fn ran() -> Vec<String> {
        RAN.with(|ran| ran.borrow().clone())
    }

    fn rule(id: &str, trigger: RuleTrigger, run: &[&str]) -> RuleConfig {
        RuleConfig {
            id: id.to_string(),
            trigger,
            run: run.iter().map(|line| line.to_string()).collect(),
            ..RuleConfig::default()
        }
    }

    fn edge(expression: &str) -> RuleTrigger {
        RuleTrigger {
            edge: Some(expression.to_string()),
            ..RuleTrigger::default()
        }
    }

    fn event(name: &str) -> RuleTrigger {
        RuleTrigger {
            event: Some(name.to_string()),
            ..RuleTrigger::default()
        }
    }

    fn load(rules: &Rules, written: &[RuleConfig]) {
        rules.load(written, environment(), &AutomationConfig::default());
        settle();
    }

    fn low_battery() -> RuleConfig {
        rule(
            "low-battery",
            edge("$battery.level < 15 && !$battery.charging"),
            &[
                "toast show 'Battery low'",
                "var set rules_test_low true --type bool",
            ],
        )
    }

    #[test]
    fn a_battery_under_fifteen_percent_fires_once_per_crossing_not_every_reading() {
        let rules = engine();
        load(&rules, &[low_battery()]);

        battery(40.0, false);
        battery(14.0, false);
        assert_eq!(
            ran(),
            [
                "toast show 'Battery low'",
                "var set rules_test_low true --type bool"
            ],
            "the drop under 15 % fires the chain, in order"
        );

        for level in [13.0, 12.0, 12.0, 11.0, 9.0] {
            battery(level, false);
        }
        assert_eq!(
            ran().len(),
            2,
            "readings that stay under it fire nothing more"
        );

        battery(9.0, true);
        battery(16.0, true);
        battery(16.0, false);
        assert_eq!(
            ran().len(),
            2,
            "charging, and climbing back, are not crossings down"
        );

        battery(14.0, false);
        assert_eq!(ran().len(), 4, "the next crossing fires again");
        assert!(rules.list()[0].last_fired.is_some());
    }

    #[test]
    fn an_edge_that_already_holds_when_loaded_waits_for_the_next_crossing() {
        let rules = engine();
        load(&rules, &[low_battery()]);

        battery(10.0, false);
        battery(8.0, false);
        assert!(
            ran().is_empty(),
            "the first answer only records where it stands"
        );

        battery(50.0, false);
        battery(10.0, false);
        assert_eq!(ran().len(), 2);
    }

    #[test]
    fn an_edge_flickering_back_and_forth_fires_once_per_rise() {
        let rules = engine();
        load(
            &rules,
            &[rule(
                "rise",
                edge("$battery.level < 15"),
                &["toast show rise"],
            )],
        );
        battery(20.0, false);
        for _ in 0..3 {
            battery(14.0, false);
            battery(14.0, false);
            battery(15.0, false);
        }
        assert_eq!(ran(), ["toast show rise"; 3]);
    }

    #[test]
    fn an_event_rule_fires_on_its_event_and_no_other() {
        let rules = engine();
        load(
            &rules,
            &[rule(
                "colours",
                event("colors_changed"),
                &["shell run makoctl reload"],
            )],
        );

        rules.on_event(&ShellEvent::WallpaperChanged {
            output: None,
            path: None,
        });
        assert!(ran().is_empty());

        rules.on_event(&ShellEvent::ColorsChanged);
        rules.on_event(&ShellEvent::ColorsChanged);
        assert_eq!(
            ran(),
            ["shell run makoctl reload", "shell run makoctl reload"],
            "every event of its kind fires it: an event is not an edge"
        );
    }

    #[test]
    fn when_is_read_as_the_rule_fires() {
        let rules = engine();
        let mut gated = rule("gated", event("session_locked"), &["toast show locked"]);
        gated.when = Some("$battery.level < 50".to_string());
        load(&rules, &[gated]);

        rules.on_event(&ShellEvent::SessionLocked);
        assert!(
            ran().is_empty(),
            "with no reading yet, `when` does not hold"
        );

        battery(80.0, false);
        rules.on_event(&ShellEvent::SessionLocked);
        assert!(ran().is_empty());

        battery(30.0, false);
        assert!(ran().is_empty(), "`when` turning true is not a trigger");
        rules.on_event(&ShellEvent::SessionLocked);
        assert_eq!(ran(), ["toast show locked"]);
    }

    #[test]
    fn store_keeps_the_value_as_the_rule_fires_in_a_typed_variable() {
        let rules = engine();
        let mut keeping = rule("keep", event("battery_state_changed"), &[]);
        keeping.store = Some(RuleStore {
            var: "rules_test_level".to_string(),
            value: "$battery.level".to_string(),
        });
        load(&rules, &[keeping]);

        battery(42.0, true);
        rules.on_event(&ShellEvent::BatteryStateChanged {
            level: 42,
            charging: true,
        });
        assert_eq!(vars::get("rules_test_level"), Some(Var::Number(42.0)));

        battery(41.0, false);
        assert_eq!(
            vars::get("rules_test_level"),
            Some(Var::Number(42.0)),
            "kept as it fired, not followed"
        );
        vars::remove("rules_test_level");
    }

    #[test]
    fn a_refused_command_stops_the_chain_and_is_reported_until_the_rule_fires_cleanly() {
        let rules = engine();
        load(
            &rules,
            &[rule(
                "broken",
                event("started"),
                &["toast show a", "nope", "toast show b"],
            )],
        );
        rules.on_event(&ShellEvent::Started);
        assert_eq!(ran(), ["toast show a", "nope"]);
        let report = failing();
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert_eq!(report.errors[0].key, "rules[0]");
        assert!(
            report.errors[0]
                .message
                .english()
                .contains("`nope` was refused"),
            "{}",
            report.render()
        );

        load(
            &rules,
            &[rule("broken", event("started"), &["toast show a"])],
        );
        rules.on_event(&ShellEvent::Started);
        assert!(failing().is_clean());
    }

    #[test]
    fn a_reload_keeps_an_unchanged_rules_edge_and_restarts_a_changed_one_without_firing() {
        let rules = engine();
        let other = rule("other", event("started"), &["toast show other"]);
        load(&rules, &[low_battery()]);
        battery(40.0, false);
        battery(10.0, false);
        assert_eq!(ran().len(), 2);

        load(&rules, &[low_battery(), other.clone()]);
        battery(9.0, false);
        assert_eq!(
            ran().len(),
            2,
            "the reload neither fires it again nor forgets it is under"
        );
        battery(40.0, false);
        battery(10.0, false);
        assert_eq!(ran().len(), 4, "and it still sees the next crossing");

        let mut changed = low_battery();
        changed.run = vec!["toast show changed".to_string()];
        load(&rules, &[changed, other]);
        battery(9.0, false);
        assert_eq!(
            ran().len(),
            4,
            "a changed rule starts over, recording where it stands"
        );
        battery(40.0, false);
        battery(10.0, false);
        assert_eq!(ran().last().map(String::as_str), Some("toast show changed"));
    }

    #[test]
    fn a_rule_taken_out_or_switched_off_stops() {
        let rules = engine();
        load(&rules, &[low_battery()]);
        battery(40.0, false);

        let mut off = low_battery();
        off.enabled = false;
        load(&rules, &[off]);
        battery(10.0, false);
        assert!(ran().is_empty());
        assert_eq!(rules.list()[0].state, State::Off);

        assert_eq!(
            rules.run_now("low-battery"),
            Ok(
                "toast show 'Battery low'\tok\nvar set rules_test_low true --type bool\tok"
                    .to_string()
            ),
            "`rule run` runs a rule that is switched off"
        );
        assert_eq!(ran().len(), 2);
        assert!(
            rules
                .run_now("nobody")
                .unwrap_err()
                .contains("try: low-battery")
        );

        load(&rules, &[]);
        assert!(rules.list().is_empty());
    }

    #[test]
    fn the_list_says_each_rules_trigger_and_state() {
        let rules = engine();
        let mut every = rule("tick", RuleTrigger::default(), &["toast show tick"]);
        every.trigger.every = Some("5m".to_string());
        let broken = rule("broken", edge("$battery.levle < 15"), &["toast show x"]);
        load(&rules, &[low_battery(), every, broken]);
        let listed: Vec<(String, String, State)> = rules
            .list()
            .into_iter()
            .map(|rule| (rule.id, rule.trigger, rule.state))
            .collect();
        assert_eq!(
            listed,
            [
                (
                    "low-battery".to_string(),
                    "edge $battery.level < 15 && !$battery.charging".to_string(),
                    State::On
                ),
                ("tick".to_string(), "every 5m".to_string(), State::On),
                (
                    "broken".to_string(),
                    "edge $battery.levle < 15".to_string(),
                    State::Invalid
                ),
            ]
        );
    }

    #[test]
    fn validation_names_each_mistake_where_it_is_written() {
        let no_trigger = rule("quiet", RuleTrigger::default(), &[]);
        let mut two = rule("two", event("started"), &["toast show x"]);
        two.trigger.every = Some("5m".to_string());
        let mut when = rule("when", event("started"), &["typo here"]);
        when.when = Some("$battery.level + 1".to_string());
        let mut stored = rule("stored", event("started"), &[]);
        stored.store = Some(RuleStore {
            var: "2fast".to_string(),
            value: "$battery.lvl".to_string(),
        });
        let mut fast = rule("fast", RuleTrigger::default(), &["toast show x"]);
        fast.trigger.every = Some("100ms".to_string());
        let mut late = rule("late", RuleTrigger::default(), &["toast show x"]);
        late.trigger.schedule = Some("7:30 mon-fry".to_string());
        let written = [
            rule("ok", event("colors_changed"), &["toast show x"]),
            rule("ok", event("colours_changed"), &["toast show x"]),
            no_trigger,
            two,
            when,
            rule("bad id", edge("$battery.level <"), &["toast show x"]),
            stored,
            fast,
            late,
        ];

        let problems = check(
            &written,
            &environment(),
            &|line: &str| !line.starts_with("typo"),
            &AutomationConfig::default(),
        );
        let found: Vec<(&str, Option<Range>, bool)> = problems
            .iter()
            .map(|problem| {
                (
                    problem.key.as_str(),
                    problem.within.clone(),
                    problem.warning,
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("rules[1].trigger.event", None, false),
                ("rules[1].id", None, false),
                ("rules[2].trigger", None, false),
                ("rules[2].run", None, true),
                ("rules[3].trigger.every", None, false),
                ("rules[4].when", Some(0..18), false),
                ("rules[4].run[0]", None, false),
                ("rules[5].id", None, false),
                ("rules[5].trigger.edge", Some(16..16), false),
                ("rules[6].store.var", None, false),
                ("rules[6].store.value", Some(0..12), false),
                ("rules[7].trigger.every", None, true),
                ("rules[8].trigger.schedule", Some(5..12), false),
            ],
            "{problems:#?}"
        );
        let message = |key: &str| {
            problems
                .iter()
                .find(|problem| problem.key == key)
                .map(|problem| problem.message.english())
                .unwrap_or_default()
        };
        assert!(message("rules[1].trigger.event").contains("`colors_changed`"));
        assert!(message("rules[1].id").contains("called `ok` already"));
        assert!(
            message("rules[4].when").contains("expected bool"),
            "{}",
            message("rules[4].when")
        );
        assert!(message("rules[4].run[0]").contains("`typo here`"));
        assert!(message("rules[6].store.value").contains("lvl"));
        assert!(message("rules[7].trigger.every").contains("runs every 1s"));
        assert!(message("rules[8].trigger.schedule").contains("`mon-fry`"));
    }

    type Range = std::ops::Range<usize>;

    #[test]
    fn an_interval_under_the_limit_is_reported_and_runs_at_the_limit() {
        let rules = engine();
        let mut fast = rule("fast", RuleTrigger::default(), &["toast show x"]);
        fast.trigger.every = Some("100ms".to_string());
        load(&rules, &[fast]);
        assert_eq!(rules.list()[0].state, State::On);
    }

    #[test]
    fn a_rule_reading_a_variable_loads_once_the_variable_is_set() {
        vars::remove("rules_test_armed");
        let rules = engine();
        load(
            &rules,
            &[rule(
                "armed",
                edge("$rules_test_armed && $battery.level < 15"),
                &["toast show armed"],
            )],
        );
        assert_eq!(
            rules.list()[0].state,
            State::Invalid,
            "nothing is called `$rules_test_armed` yet"
        );

        vars::set("rules_test_armed", Var::Bool(true)).unwrap();
        assert_eq!(rules.list()[0].state, State::On);
        vars::remove("rules_test_armed");
    }

    #[test]
    fn the_reference_names_exactly_the_events_a_rule_can_trigger_on() {
        let (_, _, doc) = config::schema::CONFIG_DOCS
            .iter()
            .find(|(owner, field, _)| *owner == "RuleTrigger" && *field == "event")
            .expect("`trigger.event` is documented");
        let named: Vec<&str> = doc.split('`').skip(1).step_by(2).collect();
        assert_eq!(named, EventKind::names().collect::<Vec<_>>());
    }

    thread_local! {
        static UNDER_TEST: RefCell<Option<Rules>> = RefCell::default();
        static LEVELS: RefCell<Vec<f64>> = RefCell::default();
    }

    /// An engine whose command table answers `rule run <id>` by running that rule of this engine, as the shell's does, and `probe level <n>` by queuing a battery reading for the test's next turn.
    fn engine_that_runs_rules() -> Rules {
        let rules = engine();
        UNDER_TEST.with(|under| *under.borrow_mut() = Some(rules.clone()));
        services::command::set_runner(
            |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                if let Some(level) = line.strip_prefix("probe level ") {
                    LEVELS.with(|levels| levels.borrow_mut().push(level.parse().unwrap()));
                    return "ok".to_string();
                }
                let Some(id) = line.strip_prefix("rule run ") else {
                    return "ok".to_string();
                };
                let rules = UNDER_TEST.with(|under| under.borrow().clone()).unwrap();
                match rules.run_now(id) {
                    Ok(reply) => format!("ok {reply}"),
                    Err(why) => format!("err {why}"),
                }
            },
            |_| true,
        );
        rules
    }

    #[test]
    fn a_rule_that_runs_itself_is_refused_rather_than_recursing() {
        let rules = engine_that_runs_rules();
        load(
            &rules,
            &[
                rule(
                    "again",
                    event("started"),
                    &["toast show a", "rule run again"],
                ),
                rule("ping", event("started"), &["rule run pong"]),
                rule("pong", event("started"), &["rule run ping"]),
            ],
        );
        assert!(
            rules.list().iter().all(|rule| rule.state == State::Invalid),
            "`config check` refuses `rule run` in a rule, so none of them loads"
        );

        for _ in 0..2 {
            let refused = rules.run_now("again").unwrap_err();
            assert!(refused.contains("ran itself again"), "{refused}");
        }
        assert_eq!(
            ran(),
            [
                "toast show a",
                "rule run again",
                "toast show a",
                "rule run again"
            ],
            "the inner run stops at once, and the rule is not left marked as firing"
        );

        let refused = rules.run_now("ping").unwrap_err();
        assert!(refused.contains("`rule run pong` was refused"), "{refused}");
        let report = failing();
        assert!(
            report.errors.iter().any(|finding| finding.key == "rules[0]"
                && finding.message.english().contains("ran itself")),
            "{}",
            report.render()
        );
    }

    #[test]
    fn config_check_refuses_a_rule_that_runs_a_rule() {
        let problems = check(
            &[rule(
                "loop",
                event("started"),
                &["toast show x", "rule  run loop"],
            )],
            &environment(),
            &|_: &str| true,
            &AutomationConfig::default(),
        );
        assert_eq!(problems.len(), 1, "{problems:#?}");
        assert_eq!(problems[0].key, "rules[0].run[1]");
        assert!(!problems[0].warning);
        assert_eq!(problems[0].message.key(), Some("finding.rule_runs_rule"));
    }

    #[test]
    fn two_rules_setting_each_other_off_are_suspended_until_the_config_loads_again() {
        let rules = engine_that_runs_rules();
        let written = [
            rule("low", edge("$battery.level < 15"), &["probe level 50"]),
            rule("high", edge("$battery.level > 40"), &["probe level 10"]),
        ];
        load(&rules, &written);
        battery(30.0, false);
        battery(10.0, false);

        let mut turns = 0;
        while let Some(level) = LEVELS.with(|levels| levels.borrow_mut().pop()) {
            battery(level, false);
            turns += 1;
            assert!(turns < 100, "the loop never stops");
        }
        let budget = RuleConfig::FIRINGS_PER_SECOND as usize;
        assert_eq!(ran().len(), 2 * budget, "each fires its budget and no more");

        let suspended: Vec<String> = rules
            .list()
            .into_iter()
            .filter(|rule| rule.state == State::Suspended)
            .map(|rule| rule.id)
            .collect();
        assert_eq!(suspended, ["low"]);
        let report = failing();
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert!(
            report.errors[0]
                .message
                .english()
                .contains(&format!("fired more than {budget} times within a second")),
            "{}",
            report.render()
        );
        assert!(
            rules.run_now("low").unwrap_err().contains("suspended"),
            "`rule run` cannot restart the loop either"
        );

        load(&rules, &written);
        assert!(rules.list().iter().all(|rule| rule.state == State::On));
        assert!(failing().is_clean());
        LEVELS.with(|levels| levels.borrow_mut().clear());
    }

    #[test]
    fn storing_what_the_lock_screen_hides_is_a_warning() {
        let mut keeping = rule("keep", event("started"), &[]);
        keeping.store = Some(RuleStore {
            var: "last_summary".to_string(),
            value: "\"> \" + $notes.summary".to_string(),
        });
        let mut level = rule("level", event("started"), &[]);
        level.store = Some(RuleStore {
            var: "last_level".to_string(),
            value: "$battery.level".to_string(),
        });
        let problems = check(
            &[keeping, level],
            &environment(),
            &|_: &str| true,
            &AutomationConfig::default(),
        );
        assert_eq!(problems.len(), 1, "{problems:#?}");
        assert_eq!(problems[0].key, "rules[0].store.value");
        assert!(problems[0].warning);
        assert_eq!(problems[0].within, Some(7..21));
        assert_eq!(
            problems[0].message.key(),
            Some("finding.store_shows_hidden")
        );
        assert!(problems[0].message.english().contains("`$last_summary`"));
    }
}
