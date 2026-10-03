//! Every reading an expression or a rule can name, behind one producer each.
//!
//! A [`SourceSpec`] is the normalised identity of a reading — the command, the way its output is read and how often it runs, with the spelling the user chose taken out — so two places that mean the same source share one producer however they wrote it. Command, address and event sources are filed by spec in one [`util::broadcast::Keyed`] registry: the first subscriber starts the producer, every later one is handed its current reading, and the producer retires once nobody listens (TA-6). Service sources are the module descriptors' own `SourceDef`s, fed by their services; `var` sources are machine state ([`crate::vars`]).
//!
//! What each producer may cost is `[automation]`'s: a run is killed at the timeout or when it prints past a cap, one run per source is in flight at a time, and a source that keeps failing waits twice as long before each retry. The schedule and the failure streak belong to the spec rather than to one producer, so a source whose consumers come and go runs no more often than one that is always shown. A failure is a [`crate::failures`] site, reported under the name the layout gave the source.

mod parse;
mod run;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use config::AutomationConfig;
use layout::While;
use platform_wayland::EventSender;
use services::events::EventKind;
use telar_expression::{Type, Value};
use util::broadcast::Keyed;
use util::report::{Finding, Message, Report};

use crate::failures::{self, Site};

pub use parse::{
    Parse, Pattern, Step, Unreadable, clamp_interval, coerce, interval, interval_text,
};
pub(crate) use run::keep_awake;
pub use run::{Policy, backoff};

/// The identity of a reading, normalised: what decides whether two subscribers share a producer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceSpec {
    /// A module's own reading, by its `SourceDef` id.
    Service(&'static str),
    Poll {
        cmd: String,
        every: Duration,
        parse: Parse,
    },
    Listen {
        cmd: String,
        parse: Parse,
    },
    Http {
        url: String,
        every: Duration,
        parse: Parse,
    },
    /// A variable from machine state, by name.
    Var(String),
    /// The last event of one kind, by its stable name.
    Event(&'static str),
}

impl SourceSpec {
    /// A `poll` source, normalised: the command trimmed.
    pub fn poll(cmd: &str, every: Duration, parse: Parse) -> Self {
        SourceSpec::Poll {
            cmd: cmd.trim().to_string(),
            every,
            parse,
        }
    }

    pub fn listen(cmd: &str, parse: Parse) -> Self {
        SourceSpec::Listen {
            cmd: cmd.trim().to_string(),
            parse,
        }
    }

    pub fn http(url: &str, every: Duration, parse: Parse) -> Self {
        SourceSpec::Http {
            url: url.trim().to_string(),
            every,
            parse,
        }
    }

    pub fn event(kind: EventKind) -> Self {
        SourceSpec::Event(kind.as_str())
    }

    /// Whether this runs something the user wrote — a command or an address — which is what the lock screen refuses unless the source says `lock_safe`.
    pub fn is_user_command(&self) -> bool {
        matches!(
            self,
            SourceSpec::Poll { .. } | SourceSpec::Listen { .. } | SourceSpec::Http { .. }
        )
    }

    /// Whether readings come from a producer in the [`subscribe`] registry.
    fn is_produced(&self) -> bool {
        self.is_user_command() || matches!(self, SourceSpec::Event(_))
    }
}

impl fmt::Display for SourceSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceSpec::Service(id) => write!(f, "service {id}"),
            SourceSpec::Poll { cmd, every, parse } => {
                write!(f, "poll {} {parse} {cmd}", interval_text(*every))
            }
            SourceSpec::Listen { cmd, parse } => write!(f, "listen {parse} {cmd}"),
            SourceSpec::Http { url, every, parse } => {
                write!(f, "http {} {parse} {url}", interval_text(*every))
            }
            SourceSpec::Var(name) => write!(f, "var {name}"),
            SourceSpec::Event(kind) => write!(f, "event {kind}"),
        }
    }
}

/// A source a layout declares, ready to read: what runs, the type every reading is read as, and who may read it.
#[derive(Clone, Debug, PartialEq)]
pub struct UserSource {
    pub spec: SourceSpec,
    pub ty: Type,
    /// What the source reads before its first reading, already of [`UserSource::ty`].
    pub initial: Option<Value>,
    pub running_while: While,
    pub lock_safe: bool,
}

impl UserSource {
    /// A layout's source, once every level of its `extends` chain has been laid over the others. `Err` lists, as `(key, why)`, everything that keeps it from running. An `every` under `[automation]`'s minimum is not one of them: the source runs at the minimum, which [`clamp_interval`] warns about.
    pub fn from_layout(
        source: &layout::Source,
        config: &AutomationConfig,
    ) -> Result<Self, Vec<(String, Message)>> {
        let mut wrong = Vec::new();
        let parse = Parse::from_spec(source.parse()).unwrap_or_else(|why| {
            wrong.push(("parse".to_string(), why));
            Parse::default()
        });
        if matches!(source, layout::Source::Listen { .. }) && parse == Parse::Lines {
            wrong.push(("parse".to_string(), util::message!("finding.listen_lines")));
        }
        let every = match source.every() {
            None => DEFAULT_EVERY,
            Some(every) => match clamp_interval(every, config.min_interval()) {
                Ok((every, _)) => every,
                Err(why) => {
                    wrong.push(("every".to_string(), why));
                    DEFAULT_EVERY
                }
            },
        };
        let (ty, initial) = typed_initial(source.initial(), &parse).unwrap_or_else(|why| {
            wrong.push(("initial".to_string(), why));
            (Type::Text, None)
        });
        let kind = source.kind_name();
        let spec = match source {
            layout::Source::Poll { cmd, .. } => command(cmd.as_deref(), kind)
                .map(|cmd| SourceSpec::poll(cmd, every, parse.clone()))
                .map_err(|why| ("cmd", why)),
            layout::Source::Listen { cmd, .. } => command(cmd.as_deref(), kind)
                .map(|cmd| SourceSpec::listen(cmd, parse.clone()))
                .map_err(|why| ("cmd", why)),
            layout::Source::Http { url, .. } => address(url.as_deref(), kind)
                .map(|url| SourceSpec::http(url, every, parse.clone()))
                .map_err(|why| ("url", why)),
        };
        match spec {
            Ok(spec) if wrong.is_empty() => Ok(UserSource {
                spec,
                ty,
                initial,
                running_while: source.running_while(),
                lock_safe: source.lock_safe(),
            }),
            Ok(_) => Err(wrong),
            Err((key, why)) => {
                wrong.push((key.to_string(), why));
                Err(wrong)
            }
        }
    }
}

/// How often a `poll` or `http` source that does not say runs.
const DEFAULT_EVERY: Duration = Duration::from_secs(10);

/// The command line a source runs, or why it cannot run it. A line may span lines and carry tabs, since `sh -c` reads a script; any other control character is refused, and a NUL is one no process can be handed at all.
fn command<'a>(cmd: Option<&'a str>, kind: &str) -> Result<&'a str, Message> {
    let cmd =
        cmd.ok_or_else(|| util::message!("finding.source_needs", kind = kind, key = "cmd"))?;
    if cmd.trim().is_empty() {
        return Err(util::message!("finding.command_empty"));
    }
    if let Some(control) = cmd
        .chars()
        .find(|c| c.is_control() && !matches!(c, '\n' | '\t'))
    {
        return Err(util::message!(
            "finding.command_control",
            character = control.escape_default()
        ));
    }
    Ok(cmd)
}

/// The address a source fetches, or why it cannot fetch it.
fn address<'a>(url: Option<&'a str>, kind: &str) -> Result<&'a str, Message> {
    let url =
        url.ok_or_else(|| util::message!("finding.source_needs", kind = kind, key = "url"))?;
    if url.chars().any(char::is_control) {
        return Err(util::message!(
            "finding.url_control",
            url = url.escape_default()
        ));
    }
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err(util::message!("finding.url_not_http", url = url));
    }
    Ok(url)
}

/// The type a source's `initial` declares, and that initial value as one of it.
fn typed_initial(
    initial: Option<&toml::Value>,
    parse: &Parse,
) -> Result<(Type, Option<Value>), Message> {
    let Some(initial) = initial else {
        let ty = match parse.yields_list() {
            true => Type::list(Type::Text),
            false => Type::Text,
        };
        return Ok((ty, None));
    };
    let value = toml_value(initial)?;
    let ty = value.type_of();
    let ty = match ty {
        Type::List(element) if *element == Type::Never => Type::list(Type::Text),
        ty => ty,
    };
    if parse.yields_list() && !matches!(ty, Type::List(_)) {
        return Err(util::message!("finding.initial_lines"));
    }
    if !value.conforms_to(&ty) {
        return Err(util::message!("finding.initial_mixed"));
    }
    Ok((ty, Some(value)))
}

fn toml_value(value: &toml::Value) -> Result<Value, Message> {
    Ok(match value {
        toml::Value::String(text) => Value::text(text.as_str()),
        toml::Value::Integer(n) => Value::Number(*n as f64),
        toml::Value::Float(n) if n.is_finite() => Value::Number(*n),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Array(items) => {
            let items = items
                .iter()
                .map(toml_value)
                .collect::<Result<Vec<_>, _>>()?;
            if items.iter().any(|item| matches!(item, Value::List(_))) {
                return Err(util::message!("finding.initial_nested"));
            }
            Value::list(items)
        }
        _ => {
            return Err(util::message!("finding.initial_kind"));
        }
    })
}

/// The sources one layout declares, by the name an expression reads each as.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UserSources {
    file: String,
    by_name: BTreeMap<String, UserSource>,
}

impl UserSources {
    /// Reads a layout's merged sources (`layout::sources`), reporting under `file` everything wrong with them and leaving out each one that cannot run.
    ///
    /// The one place a source's problems are said: `layout check` and the running shell both read a layout's sources through here, so neither has a second checker to agree with.
    pub fn of(
        file: impl Into<String>,
        sources: &BTreeMap<String, layout::Source>,
        config: &AutomationConfig,
    ) -> (Self, Report) {
        let file = file.into();
        let mut report = Report::default();
        let mut by_name = BTreeMap::new();
        for (name, source) in sources {
            let at = |key: &str| format!("sources.{name}.{key}");
            if let Some(Ok((_, Some(why)))) = source
                .every()
                .map(|every| clamp_interval(every, config.min_interval()))
            {
                report.warn(Finding::new(&file, at("every"), why));
            }
            match UserSource::from_layout(source, config) {
                Ok(user) => {
                    by_name.insert(name.clone(), user);
                }
                Err(wrong) => {
                    for (key, why) in wrong {
                        report.error(Finding::new(&file, at(&key), why));
                    }
                }
            }
        }
        (Self { file, by_name }, report)
    }

    pub fn get(&self, name: &str) -> Option<&UserSource> {
        self.by_name.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &UserSource)> {
        self.by_name.iter()
    }

    pub fn file(&self) -> &str {
        &self.file
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// The sources `layout` declares once every level of its `extends` chain is laid over the others, ready to read, with what is wrong with any it leaves out.
pub fn of_layout(
    layout: &layout::Layout,
    known: &BTreeMap<layout::LayoutId, layout::Layout>,
    config: &AutomationConfig,
) -> (UserSources, Report) {
    let (merged, mut report) = layout::sources(layout, known);
    let (sources, problems) =
        UserSources::of(format!("layouts/{}.toml", layout.id), &merged, config);
    report.merge(problems);
    (sources, report)
}

/// Producers for every command, address and event source, by spec, each spec with the pace its producers keep.
static PRODUCERS: Keyed<SourceSpec, Value, run::Pace> = Keyed::new("hogar-shell-source");

/// The sources the running layout declares: what a failure is reported under, and which specs the registry keeps a reading for once their producer stops.
static DECLARED: Mutex<Option<Arc<UserSources>>> = Mutex::new(None);

fn declared_slot() -> MutexGuard<'static, Option<Arc<UserSources>>> {
    DECLARED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Makes `sources` the set the running layout declares. A spec it no longer names loses its last reading as soon as nothing is running it, so a source an edit removed does not hold one for the life of the shell.
pub fn declare(sources: UserSources) {
    *declared_slot() = Some(Arc::new(sources));
    forget_undeclared();
}

fn declared() -> Option<Arc<UserSources>> {
    declared_slot().clone()
}

/// The sources the running layout declares; none before the shell has declared any. The same `Arc` until the next [`declare`], which is what lets a caller tell whether they changed.
pub fn declared_sources() -> Arc<UserSources> {
    static NONE: LazyLock<Arc<UserSources>> = LazyLock::new(Arc::default);
    declared().unwrap_or_else(|| Arc::clone(&NONE))
}

fn is_declared(spec: &SourceSpec) -> bool {
    matches!(spec, SourceSpec::Event(_))
        || declared().is_some_and(|sources| sources.iter().any(|(_, source)| source.spec == *spec))
}

fn forget_undeclared() {
    PRODUCERS.retain(is_declared);
    failures::retain(|site| !matches!(site, Site::Source(spec) if !is_declared(spec)));
}

/// Registers `tx` for `spec`'s readings, starting its producer under `[automation]`'s limits unless one is running. A surface passes this to `platform_wayland::watch`.
///
/// Only command, address and event specs have a producer here: a service source is fed by its descriptor, and a var by machine state, so for those this does nothing.
pub fn subscribe(spec: SourceSpec, tx: EventSender<Value>) {
    subscribe_with(spec, tx, Policy::current());
}

fn subscribe_with(spec: SourceSpec, tx: EventSender<Value>, policy: Policy) {
    if !spec.is_produced() {
        return;
    }
    let producer = run::producer(spec.clone(), policy);
    PRODUCERS.subscribe(spec, tx, producer);
}

/// The last reading `spec`'s producer published, without starting it.
pub fn current(spec: &SourceSpec) -> Option<Value> {
    PRODUCERS.current(spec)
}

/// Called by a producer as it retires, so a spec no layout declares any more is forgotten once nothing runs it.
fn retired(spec: &SourceSpec) {
    if !is_declared(spec) {
        forget_undeclared();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use services::events::{self, ShellEvent};
    use util::process::Limits;

    use super::*;

    /// Tests that make a set of sources the declared one take turns: the set is the process's, and two of them at once would each forget the other's.
    static DECLARING: Mutex<()> = Mutex::new(());

    fn declaring() -> std::sync::MutexGuard<'static, ()> {
        DECLARING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn policy(timeout_ms: u64, backoff_ms: (u64, u64)) -> Policy {
        Policy {
            limits: Limits {
                timeout: Duration::from_millis(timeout_ms),
                max_line: 64 * 1024,
                max_run: 1 << 20,
            },
            backoff: (
                Duration::from_millis(backoff_ms.0),
                Duration::from_millis(backoff_ms.1),
            ),
        }
    }

    /// A file of this test's own, which a fake command appends a line to on every run: the count of lines is the count of runs.
    fn tally(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = util::paths::state_dir().join("source-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn runs(path: &PathBuf) -> usize {
        std::fs::read_to_string(path)
            .map(|text| text.lines().count())
            .unwrap_or(0)
    }

    fn poll(cmd: &str, every_ms: u64) -> SourceSpec {
        SourceSpec::poll(cmd, Duration::from_millis(every_ms), Parse::Text)
    }

    fn eventually(what: &str, patience: Duration, check: impl Fn() -> bool) {
        let deadline = Instant::now() + patience;
        while Instant::now() < deadline {
            if check() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn producing(spec: &SourceSpec) -> usize {
        let name = format!("hogar-shell-source:{spec}");
        util::broadcast::running_services()
            .iter()
            .filter(|running| **running == name)
            .count()
    }

    fn process_exists(pid: i32) -> bool {
        // SAFETY: signal 0 only asks whether the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn reported(file: &str) -> Vec<Finding> {
        failures::report()
            .errors
            .into_iter()
            .filter(|finding| finding.file == std::path::Path::new(file))
            .collect()
    }

    fn declared_as(file: &str, name: &str, spec: &SourceSpec) {
        let mut by_name = BTreeMap::new();
        by_name.insert(
            name.to_string(),
            UserSource {
                spec: spec.clone(),
                ty: Type::Text,
                initial: None,
                running_while: While::Visible,
                lock_safe: false,
            },
        );
        declare(UserSources {
            file: file.to_string(),
            by_name,
        });
    }

    /// The problems notice follows the registry rather than a reload: a source that starts failing while the shell runs is published as it fails, and published again as it recovers.
    #[test]
    fn a_source_that_starts_failing_while_running_is_published_and_so_is_its_recovery() {
        let _declaring = declaring();
        let file = "layouts/flaky.toml";
        let flag = tally("flaky-flag");
        let spec = poll(
            &format!(
                "if [ -e '{}' ]; then exit 3; else echo ok; fi",
                flag.display()
            ),
            100,
        );
        declared_as(file, "flaky", &spec);
        let (heard, notices) = platform_wayland::detached::<Report>();
        failures::subscribe(heard);
        let says_flaky = |want: bool| {
            eventually(
                "the notice to follow the source",
                Duration::from_secs(5),
                || {
                    std::iter::from_fn(|| notices.try_recv()).any(|report| {
                        report
                            .findings()
                            .any(|finding| finding.key == "sources.flaky")
                            == want
                    })
                },
            )
        };

        let (tx, _subscription) = platform_wayland::detached::<Value>();
        subscribe_with(spec.clone(), tx, policy(2000, (50, 100)));
        eventually("a first reading", Duration::from_secs(5), || {
            current(&spec) == Some(Value::text("ok"))
        });
        assert!(reported(file).is_empty());

        std::fs::write(&flag, "").unwrap();
        says_flaky(true);
        assert_eq!(
            reported(file)[0]
                .message
                .argument("why")
                .and_then(|why| match why {
                    util::report::Arg::Message(why) => why.key(),
                    util::report::Arg::Text(_) => None,
                }),
            Some("finding.exited")
        );

        std::fs::remove_file(&flag).unwrap();
        says_flaky(false);
        assert!(reported(file).is_empty());
        declare(UserSources::default());
    }

    /// T-8.1's acceptance: twenty instances reading one `poll` are one process per interval, not twenty.
    #[test]
    fn twenty_consumers_of_one_poll_spawn_one_process_per_interval() {
        let file = tally("shared");
        let spec = poll(&format!("echo run >> '{}'; echo ok", file.display()), 200);
        let subscriptions: Vec<_> = (0..20)
            .map(|_| {
                let (tx, subscription) = platform_wayland::detached();
                subscribe_with(spec.clone(), tx, policy(2000, (100, 400)));
                subscription
            })
            .collect();

        eventually("the first reading", Duration::from_secs(5), || {
            current(&spec) == Some(Value::text("ok"))
        });
        let started = runs(&file);
        std::thread::sleep(Duration::from_millis(1000));
        let ran = runs(&file) - started;
        assert!(
            (3..=7).contains(&ran),
            "a second at a 200 ms interval is about five runs, and twenty consumers each running it would be a hundred: {ran}"
        );
        assert_eq!(producing(&spec), 1, "one producer serves all twenty");
        assert!(
            subscriptions.iter().all(|s| s.try_recv().is_some()),
            "and every consumer is fed by it"
        );
    }

    #[test]
    fn dropping_every_consumer_stops_the_command_within_one_interval() {
        let file = tally("dropped");
        let every = Duration::from_millis(300);
        let spec = poll(&format!("echo run >> '{}'", file.display()), 300);
        let subscriptions: Vec<_> = (0..3)
            .map(|_| {
                let (tx, subscription) = platform_wayland::detached::<Value>();
                subscribe_with(spec.clone(), tx, policy(2000, (100, 400)));
                subscription
            })
            .collect();
        eventually("a run", Duration::from_secs(5), || runs(&file) > 0);

        let hidden = Instant::now();
        drop(subscriptions);
        eventually("the producer to retire", every * 3, || {
            producing(&spec) == 0
        });
        assert!(
            hidden.elapsed() < every + Duration::from_millis(200),
            "the producer outlived its consumers by {:?}",
            hidden.elapsed()
        );
        let idle = runs(&file);
        std::thread::sleep(every * 2);
        assert_eq!(runs(&file), idle, "nothing runs for nobody");
    }

    #[test]
    fn a_hung_command_is_killed_at_the_timeout_and_reported_under_its_name() {
        let _declaring = declaring();
        let pid_file = tally("hung-pid");
        let spec = poll(
            &format!("echo $$ > '{}'; exec sleep 30", pid_file.display()),
            5000,
        );
        declared_as("layouts/test.toml", "stuck", &spec);

        let (tx, _subscription) = platform_wayland::detached::<Value>();
        let started = Instant::now();
        subscribe_with(spec.clone(), tx, policy(200, (1000, 1000)));
        eventually("the failure to be reported", Duration::from_secs(5), || {
            !reported("layouts/test.toml").is_empty()
        });
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the run was stopped at its 200 ms timeout, not left to sleep"
        );
        let reported = reported("layouts/test.toml");
        assert_eq!(reported[0].key, "sources.stuck");
        assert!(
            reported[0]
                .message
                .english()
                .contains("did not finish within 200ms"),
            "{}",
            reported[0].message.english()
        );
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        eventually("the command to be gone", Duration::from_secs(2), || {
            !process_exists(pid)
        });
        declare(UserSources::default());
    }

    #[test]
    fn a_failing_source_waits_twice_as_long_before_each_retry() {
        let quick = policy(2000, (100, 400));
        let waits: Vec<u128> = (0..6)
            .map(|failures| backoff(&quick, failures).as_millis())
            .collect();
        assert_eq!(waits, [0, 100, 200, 400, 400, 400]);

        let file = tally("failing");
        let spec = poll(&format!("echo run >> '{}'; exit 1", file.display()), 50);
        let (tx, _subscription) = platform_wayland::detached::<Value>();
        subscribe_with(spec, tx, policy(2000, (200, 1000)));
        eventually("a run", Duration::from_secs(5), || runs(&file) > 0);
        std::thread::sleep(Duration::from_millis(1000));
        let ran = runs(&file);
        assert!(
            (2..=5).contains(&ran),
            "at 50 ms a second is twenty runs; backing off from 200 ms it is about four: {ran}"
        );
    }

    fn written(toml: &str) -> layout::Source {
        toml::from_str(toml).expect("the source parses")
    }

    #[test]
    fn two_spellings_of_one_source_are_one_producer() {
        let config = AutomationConfig::default();
        let sources = BTreeMap::from([
            (
                "one".to_string(),
                written("kind = 'poll'\ncmd = '  echo same  '\nevery = '60s'\nparse = 'text'"),
            ),
            (
                "two".to_string(),
                written("kind = 'poll'\ncmd = 'echo same'\nevery = '1m'"),
            ),
            (
                "three".to_string(),
                written("kind = 'poll'\ncmd = 'echo same'\nevery = '1m'\ninitial = 0"),
            ),
        ]);
        let (declared, report) = UserSources::of("layouts/t.toml", &sources, &config);
        assert!(report.is_clean(), "{}", report.render());
        let specs: Vec<&SourceSpec> = declared.iter().map(|(_, source)| &source.spec).collect();
        assert!(
            specs.windows(2).all(|pair| pair[0] == pair[1]),
            "a trimmed command, `60s` against `1m`, `text` against nothing and a different type are one source: {specs:?}"
        );
        assert_eq!(
            declared.get("three").unwrap().ty,
            Type::Number,
            "the type is the reader's, not the producer's"
        );

        let (first, first_sub) = platform_wayland::detached::<Value>();
        let (second, second_sub) = platform_wayland::detached::<Value>();
        subscribe_with(specs[0].clone(), first, policy(2000, (100, 400)));
        subscribe_with(specs[1].clone(), second, policy(2000, (100, 400)));
        eventually("a reading", Duration::from_secs(5), || {
            current(specs[0]).is_some()
        });
        assert_eq!(producing(specs[0]), 1);
        drop((first_sub, second_sub));
    }

    #[test]
    fn a_spec_a_layout_stopped_declaring_is_forgotten_once_its_producer_retires() {
        let _declaring = declaring();
        let kept = poll("echo kept", 100);
        let dropped = poll("echo dropped", 100);
        let declared_only = |spec: &SourceSpec| UserSources {
            file: "layouts/test.toml".to_string(),
            by_name: BTreeMap::from([(
                "kept".to_string(),
                UserSource {
                    spec: spec.clone(),
                    ty: Type::Text,
                    initial: None,
                    running_while: While::Visible,
                    lock_safe: false,
                },
            )]),
        };
        declare(declared_only(&kept));

        let subscriptions: Vec<_> = [&kept, &dropped]
            .into_iter()
            .map(|spec| {
                let (tx, subscription) = platform_wayland::detached::<Value>();
                subscribe_with(spec.clone(), tx, policy(2000, (100, 400)));
                subscription
            })
            .collect();
        eventually("both readings", Duration::from_secs(5), || {
            current(&kept).is_some() && current(&dropped).is_some()
        });
        drop(subscriptions);
        eventually("both producers to retire", Duration::from_secs(2), || {
            producing(&kept) == 0 && producing(&dropped) == 0
        });
        eventually(
            "the undeclared one to be forgotten",
            Duration::from_secs(1),
            || !PRODUCERS.keys().contains(&dropped),
        );
        assert!(
            PRODUCERS.keys().contains(&kept),
            "a declared source keeps its last reading for the consumer that comes back"
        );

        declare(UserSources::default());
        assert!(
            !PRODUCERS.keys().contains(&kept),
            "and loses it once no layout declares it"
        );
    }

    #[test]
    fn a_listener_publishes_each_line_and_stops_with_its_last_consumer() {
        let spec = SourceSpec::listen("printf 'one\\ntwo\\n'; sleep 30", Parse::Text);
        let (tx, subscription) = platform_wayland::detached::<Value>();
        subscribe_with(spec.clone(), tx, policy(2000, (100, 400)));
        eventually("the second line", Duration::from_secs(5), || {
            current(&spec) == Some(Value::text("two"))
        });
        drop(subscription);
        eventually("the listener to stop", Duration::from_secs(2), || {
            producing(&spec) == 0
        });
    }

    #[test]
    fn an_event_source_reads_the_last_event_of_its_kind() {
        let spec = SourceSpec::event(EventKind::ThemeModeChanged);
        let (tx, _subscription) = platform_wayland::detached::<Value>();
        subscribe(spec.clone(), tx);
        eventually("the event to arrive", Duration::from_secs(5), || {
            events::emit(ShellEvent::ThemeModeChanged {
                mode: config::scheme::Mode::Light,
            });
            current(&spec) == Some(Value::text("light"))
        });
    }

    /// `$event.<kind>` is the last event of its kind, whether or not anything was reading when it happened.
    #[test]
    fn an_event_source_starts_from_the_last_event_emitted_before_it() {
        let path = std::path::PathBuf::from("/sources-test/before.png");
        events::emit(ShellEvent::WallpaperChanged {
            output: None,
            path: Some(path.clone()),
        });
        let spec = SourceSpec::event(EventKind::WallpaperChanged);
        let (tx, _subscription) = platform_wayland::detached::<Value>();
        subscribe(spec.clone(), tx);
        eventually("the last event to be read", Duration::from_secs(5), || {
            current(&spec) == Some(Value::text(path.display().to_string()))
        });
    }

    #[test]
    fn a_service_or_a_var_has_no_producer_here() {
        let (tx, _subscription) = platform_wayland::detached::<Value>();
        subscribe(SourceSpec::Var("x".to_string()), tx);
        assert!(!PRODUCERS.keys().contains(&SourceSpec::Var("x".to_string())));
    }

    fn declare_only(specs: &[&SourceSpec]) {
        declare(UserSources {
            file: "layouts/test.toml".to_string(),
            by_name: specs
                .iter()
                .enumerate()
                .map(|(at, spec)| {
                    (
                        format!("s{at}"),
                        UserSource {
                            spec: (*spec).clone(),
                            ty: Type::Text,
                            initial: None,
                            running_while: While::Visible,
                            lock_safe: false,
                        },
                    )
                })
                .collect(),
        });
    }

    /// A `1m` source on a widget that is hidden and shown again runs once a minute, not on every show: the schedule is the spec's, not the producer's.
    #[test]
    fn a_source_shown_again_runs_when_it_is_due_rather_than_when_it_is_shown() {
        let _declaring = declaring();
        let file = tally("shown-again");
        let spec = poll(
            &format!("echo run >> '{}'; echo ok", file.display()),
            60_000,
        );
        declare_only(&[&spec]);
        for _ in 0..4 {
            let (tx, subscription) = platform_wayland::detached::<Value>();
            subscribe_with(spec.clone(), tx, policy(2000, (100, 400)));
            eventually("a reading", Duration::from_secs(5), || {
                current(&spec).is_some()
            });
            drop(subscription);
            eventually("the producer to retire", Duration::from_secs(2), || {
                producing(&spec) == 0
            });
        }
        assert_eq!(runs(&file), 1, "shown four times within its minute");
        declare(UserSources::default());
    }

    /// A source that fails while its consumer flickers in and out of view keeps the streak it earned, rather than starting each new producer as if it had never failed.
    #[test]
    fn a_failing_source_keeps_backing_off_however_often_it_is_shown() {
        let _declaring = declaring();
        let file = tally("flapping");
        let spec = poll(&format!("echo run >> '{}'; exit 1", file.display()), 50);
        declare_only(&[&spec]);
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(900) {
            let (tx, subscription) = platform_wayland::detached::<Value>();
            subscribe_with(spec.clone(), tx, policy(2000, (300, 2000)));
            std::thread::sleep(Duration::from_millis(30));
            drop(subscription);
            std::thread::sleep(Duration::from_millis(40));
        }
        let ran = runs(&file);
        assert!(
            (1..=3).contains(&ran),
            "shown every 70 ms, a source backing off from 300 ms runs at 0, 300 and 900 ms at most: {ran}"
        );
        declare(UserSources::default());
    }

    /// A listener that prints a line and exits is failing, however much it printed: if each line wiped the streak, it would be restarted at the first wait for ever.
    #[test]
    fn a_listener_that_prints_and_exits_backs_off_like_any_failure() {
        let file = tally("listener-exits");
        let spec = SourceSpec::listen(
            &format!("echo run >> '{}'; echo line", file.display()),
            Parse::Text,
        );
        let (tx, _subscription) = platform_wayland::detached::<Value>();
        subscribe_with(spec, tx, policy(2000, (100, 1000)));
        eventually("a run", Duration::from_secs(5), || runs(&file) > 0);
        std::thread::sleep(Duration::from_millis(1000));
        let ran = runs(&file);
        assert!(
            (3..=6).contains(&ran),
            "restarted after 100, 200, 400 and 800 ms it runs about four times in a second; at a steady 100 ms it would be ten: {ran}"
        );
    }

    /// What reaches the registry is keyed by what the user wrote, so a NUL in a command must cost that command its run and nothing else — not the thread that subscribed, and not every source subscribed after it.
    #[test]
    fn a_spec_with_a_nul_in_it_fails_its_own_run_and_nothing_else() {
        let broken = poll("echo \u{0}", 100);
        let fine = poll("echo after-nul", 100);
        let (first, _first_sub) = platform_wayland::detached::<Value>();
        let (second, _second_sub) = platform_wayland::detached::<Value>();
        subscribe_with(broken.clone(), first, policy(2000, (100, 400)));
        subscribe_with(fine.clone(), second, policy(2000, (100, 400)));
        eventually("the next source's reading", Duration::from_secs(5), || {
            current(&fine) == Some(Value::text("after-nul"))
        });
        assert_eq!(current(&broken), None);
    }

    #[test]
    fn what_a_source_says_wrong_is_located_by_key() {
        let config = AutomationConfig::default();
        let keys = |toml: &str| -> Vec<String> {
            match UserSource::from_layout(&written(toml), &config) {
                Ok(_) => Vec::new(),
                Err(wrong) => wrong.into_iter().map(|(key, _)| key).collect(),
            }
        };
        assert!(keys("kind = 'poll'\ncmd = 'date'\nevery = '5s'").is_empty());
        assert!(
            keys("kind = 'poll'\ncmd = 'date'\nevery = '500ms'").is_empty(),
            "an interval under the minimum runs at the minimum"
        );
        assert_eq!(keys("kind = 'poll'\nevery = '5s'"), ["cmd"]);
        assert_eq!(
            keys("kind = 'poll'\ncmd = 'date'\nevery = 'often'"),
            ["every"]
        );
        assert_eq!(
            keys("kind = 'poll'\ncmd = 'date'\nparse = 'xml'"),
            ["parse"]
        );
        assert_eq!(
            keys("kind = 'poll'\ncmd = 'date'\nparse = 'regex:('"),
            ["parse"]
        );
        assert_eq!(
            keys("kind = 'poll'\ncmd = 'date'\nparse = 'lines'\ninitial = 'a'"),
            ["initial"]
        );
        assert_eq!(
            keys("kind = 'poll'\ncmd = 'date'\ninitial = [1, 'a']"),
            ["initial"]
        );
        assert_eq!(
            keys("kind = 'listen'\ncmd = 'date'\nparse = 'lines'"),
            ["parse"]
        );
        assert_eq!(keys("kind = 'http'\nurl = 'ftp://x'"), ["url"]);
        assert_eq!(keys("kind = 'poll'\ncmd = '  '"), ["cmd"]);
        assert_eq!(
            keys("kind = 'poll'\nparse = 'xml'"),
            ["parse", "cmd"],
            "everything that keeps it from running, not only the first"
        );
    }

    /// A NUL can be written in TOML as `\u0000`, and no process can be handed one; an escape sequence in a command line is no more what a user meant. Both are refused where the source is read.
    #[test]
    fn a_command_or_an_address_with_a_control_character_in_it_is_refused() {
        let config = AutomationConfig::default();
        let refused = |toml: &str| -> Vec<String> {
            UserSource::from_layout(&written(toml), &config)
                .err()
                .into_iter()
                .flatten()
                .map(|(key, _)| key)
                .collect()
        };
        assert_eq!(refused("kind = 'poll'\ncmd = \"echo \\u0000\""), ["cmd"]);
        assert_eq!(
            refused("kind = 'listen'\ncmd = \"tail\\u001b[2J\""),
            ["cmd"]
        );
        assert_eq!(
            refused("kind = 'http'\nurl = \"https://example.org/\\u0000\""),
            ["url"]
        );
        assert!(
            refused("kind = 'poll'\ncmd = \"\"\"\nif true; then\n\\techo yes\nfi\n\"\"\"")
                .is_empty(),
            "a script may span lines and be indented with tabs"
        );
    }

    #[test]
    fn a_problem_with_a_source_is_reported_once() {
        let sources = BTreeMap::from([(
            "odd".to_string(),
            written("kind = 'poll'\ncmd = 'date'\nparse = 'xml'"),
        )]);
        let (declared, report) =
            UserSources::of("layouts/t.toml", &sources, &AutomationConfig::default());
        assert!(declared.is_empty());
        assert_eq!(report.errors.len(), 1, "{}", report.render());
        assert_eq!(report.errors[0].key, "sources.odd.parse");
    }

    #[test]
    fn an_interval_under_the_minimum_is_raised_to_it_with_a_warning() {
        let min = Duration::from_secs(1);
        assert_eq!(
            clamp_interval("5s", min),
            Ok((Duration::from_secs(5), None))
        );
        let (every, warning) = clamp_interval("200ms", min).expect("an interval");
        assert_eq!(every, min);
        assert!(
            warning
                .as_ref()
                .is_some_and(|why| why.english().contains("runs every 1s")),
            "{warning:?}"
        );
        assert!(clamp_interval("often", min).is_err());
    }

    #[test]
    fn a_source_with_nothing_to_run_is_reported_and_left_out() {
        let sources = BTreeMap::from([("bare".to_string(), written("kind = 'http'"))]);
        let (declared, report) =
            UserSources::of("layouts/t.toml", &sources, &AutomationConfig::default());
        assert!(declared.is_empty());
        assert_eq!(report.errors[0].key, "sources.bare.url");
    }

    #[test]
    fn output_is_read_the_way_the_spec_says() {
        let parse = |spec: &str| Parse::from_spec(Some(spec)).unwrap();
        assert_eq!(parse("text").read("  42\n"), Ok(Value::text("42")));
        assert_eq!(
            parse("lines").read("a\nb\n"),
            Ok(Value::list([Value::text("a"), Value::text("b")]))
        );
        let json = r#"{"current": {"temp": 21.5, "tags": ["a", "b"]}, "items": [{"name": "x"}]}"#;
        assert_eq!(
            parse("json:.current.temp").read(json),
            Ok(Value::Number(21.5))
        );
        assert_eq!(parse("json:items[0].name").read(json), Ok(Value::text("x")));
        assert_eq!(
            parse("json:.current.tags").read(json),
            Ok(Value::list([Value::text("a"), Value::text("b")]))
        );
        assert!(parse("json:.missing").read(json).is_err());
        assert!(
            parse("json:.current").read(json).is_err(),
            "an object is not a reading"
        );
        assert!(parse("json:.a").read("not json").is_err());
        assert_eq!(
            parse(r"regex:temp: (\d+)").read("cpu temp: 54 C"),
            Ok(Value::text("54"))
        );
        assert_eq!(parse(r"regex:\d+").read("v12"), Ok(Value::text("12")));
        assert!(parse(r"regex:\d+").read("none").is_err());
        assert_eq!(parse("json:.a[1]").to_string(), "json:.a[1]");
    }

    #[test]
    fn a_reading_is_read_as_the_type_the_source_declares() {
        assert_eq!(
            coerce(&Value::text(" 21.5 "), &Type::Number),
            Ok(Value::Number(21.5))
        );
        assert!(coerce(&Value::text("warm"), &Type::Number).is_err());
        assert_eq!(
            coerce(&Value::text("on"), &Type::Bool),
            Ok(Value::Bool(true))
        );
        assert_eq!(
            coerce(&Value::Number(3.0), &Type::Text),
            Ok(Value::text("3"))
        );
        assert_eq!(
            coerce(&Value::text("1\n2"), &Type::list(Type::Number)),
            Ok(Value::list([Value::Number(1.0), Value::Number(2.0)]))
        );
    }

    #[test]
    fn intervals_are_a_whole_number_and_a_unit() {
        assert_eq!(interval("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(interval("5s"), Ok(Duration::from_secs(5)));
        assert_eq!(interval("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(interval("1h"), Ok(Duration::from_secs(3600)));
        assert!(interval("5").is_err());
        assert!(interval("1.5s").is_err());
        assert_eq!(interval_text(Duration::from_secs(60)), "1m");
        assert_eq!(interval_text(Duration::from_millis(1500)), "1500ms");
    }

    #[test]
    fn an_interval_under_the_minimum_is_a_warning_and_runs_at_the_minimum() {
        let sources = BTreeMap::from([(
            "eager".to_string(),
            written("kind = 'poll'\ncmd = 'date'\nevery = '200ms'"),
        )]);
        let (declared, report) =
            UserSources::of("layouts/t.toml", &sources, &AutomationConfig::default());
        assert!(report.errors.is_empty(), "{}", report.render());
        assert_eq!(report.warnings.len(), 1, "{}", report.render());
        assert_eq!(report.warnings[0].key, "sources.eager.every");
        let spec = &declared.get("eager").expect("it still runs").spec;
        assert!(
            matches!(spec, SourceSpec::Poll { every, .. } if *every == Duration::from_secs(1)),
            "{spec:?}"
        );
    }
}
