//! The producers behind command, address and event sources: one thread per spec, one run at a time, stopped once nobody wants the readings.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use config::AutomationConfig;
use platform_wayland::EventSender;
use services::events::{EventKind, ShellEvent};
use telar_expression::Value;
use util::broadcast::Broadcast;
use util::process::{Limits, Outcome, StreamEnd, StreamEvent};
use util::report::Message;

use super::{Parse, SourceSpec, interval_text};
use crate::failures::{self, Site};

/// What a producer may cost and how it backs off, taken from `[automation]` when it starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    pub limits: Limits,
    /// The first wait after a failure, and the longest any wait grows to.
    pub backoff: (Duration, Duration),
}

impl Policy {
    pub fn of(config: &AutomationConfig) -> Self {
        Self {
            limits: config.limits(),
            backoff: config.backoff(),
        }
    }

    /// The running shell's `[automation]`, or its defaults outside one. Read through the cross-thread snapshot, since a producer starts on a thread of its own.
    pub fn current() -> Self {
        config::shared_config()
            .map(|config| Self::of(&config.automation))
            .unwrap_or_else(|| Self::of(&AutomationConfig::default()))
    }
}

/// How long a source that has failed `failures` times in a row waits before it runs again: the first wait, doubled for each failure after the first, never longer than the longest. Zero when it has not failed.
pub fn backoff(policy: &Policy, failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let (first, longest) = policy.backoff;
    first
        .saturating_mul(1u32 << (failures - 1).min(20))
        .min(longest)
}

/// How often a waiting producer looks in on whether anyone still wants its readings, which bounds how long one outlives its last subscriber.
const LOOK_IN: Duration = Duration::from_millis(25);

/// What one producer of a spec leaves for the next, kept by the registry for as long as it keeps the spec.
///
/// A producer stops whenever its consumers go — a widget hidden, the session locked — and a new one starts when they come back. Were the schedule and the failure streak the producer's own, a `10m` source would run on every show, and a failing one would start again as if it had never failed.
#[derive(Default)]
pub(super) struct Pace(Mutex<Paced>);

#[derive(Default)]
struct Paced {
    /// When the last run started, or the last listener ended.
    last: Option<Instant>,
    /// Runs in a row that failed.
    failures: u32,
}

impl Pace {
    fn paced(&self) -> MutexGuard<'_, Paced> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// When a periodic source runs next: `every` after the last run started, or longer while it is failing; now when it has never run.
    fn next_run(&self, every: Duration, policy: &Policy) -> Instant {
        let paced = self.paced();
        let now = Instant::now();
        paced.last.map_or(now, |last| {
            (last + every.max(backoff(policy, paced.failures))).max(now)
        })
    }

    fn ran(&self, started: Instant, succeeded: bool) {
        let mut paced = self.paced();
        paced.last = Some(started);
        paced.failures = match succeeded {
            true => 0,
            false => paced.failures.saturating_add(1),
        };
    }

    /// When a listener starts next: the wait its failure streak has earned after the last one ended; now when none has run.
    fn next_start(&self, policy: &Policy) -> Instant {
        let paced = self.paced();
        let now = Instant::now();
        paced.last.map_or(now, |last| {
            (last + backoff(policy, paced.failures)).max(now)
        })
    }

    /// Records a listener that ran from `started` until now. One that stayed up past the longest wait was healthy, so the failures before it no longer count; one that `exited` is a failure however much it printed first, since a listener is meant to keep running.
    fn listened(&self, started: Instant, exited: bool, policy: &Policy) {
        let mut paced = self.paced();
        let now = Instant::now();
        if now.duration_since(started) >= policy.backoff.1 {
            paced.failures = 0;
        }
        if exited {
            paced.failures = paced.failures.saturating_add(1);
        }
        paced.last = Some(now);
    }
}

pub(super) fn producer(
    spec: SourceSpec,
    policy: Policy,
) -> impl FnOnce(&Arc<Broadcast<Value>>, &Pace) + Send + 'static {
    move |out, pace| {
        match &spec {
            SourceSpec::Poll { cmd, every, parse } => {
                periodic(&spec, out, pace, *every, &policy, parse, || {
                    ran(util::process::run_line(cmd, policy.limits), &policy.limits)
                })
            }
            SourceSpec::Http { url, every, parse } => {
                let agent = agent(&policy.limits);
                periodic(&spec, out, pace, *every, &policy, parse, || {
                    fetch(&agent, url, &policy.limits)
                })
            }
            SourceSpec::Listen { cmd, parse } => listen(&spec, out, pace, cmd, parse, &policy),
            SourceSpec::Event(name) => {
                if let Some(kind) = EventKind::from_name(name) {
                    event(out, kind);
                }
            }
            SourceSpec::Service(_) | SourceSpec::Var(_) => {}
        }
        super::retired(&spec);
    }
}

/// Waits until `until`, answering `false` — after which the producer returns and does not ask again — as soon as nobody wants its readings.
fn wait_wanted(out: &Broadcast<Value>, until: Instant) -> bool {
    loop {
        if !out.wanted() {
            return false;
        }
        let now = Instant::now();
        if now >= until {
            return true;
        }
        std::thread::sleep(LOOK_IN.min(until - now));
    }
}

/// One run every `every`, counted from the start of one to the start of the next, and never two at once: a run that outlasts the interval is followed straight away by the next. A producer started again runs first when the spec's last run says the next is due.
fn periodic(
    spec: &SourceSpec,
    out: &Broadcast<Value>,
    pace: &Pace,
    every: Duration,
    policy: &Policy,
    parse: &Parse,
    run: impl Fn() -> Result<String, Message>,
) {
    loop {
        if !wait_wanted(out, pace.next_run(every, policy)) {
            return;
        }
        let started = Instant::now();
        let reading = run().and_then(|output| parse.read(&output));
        pace.ran(started, reading.is_ok());
        match reading {
            Ok(reading) => {
                out.publish(reading);
                failures::recover(&Site::Source(spec.clone()));
            }
            Err(why) => failures::fail(Site::Source(spec.clone()), why),
        }
    }
}

fn ran(outcome: Outcome, limits: &Limits) -> Result<String, Message> {
    match outcome {
        Outcome::Ok(text) => Ok(text),
        Outcome::TimedOut => Err(util::message!(
            "finding.timed_out",
            limit = interval_text(limits.timeout)
        )),
        Outcome::TooLong => Err(too_long(limits)),
        Outcome::Failed(exit) => Err(util::message!("finding.exited", exit = exit)),
        Outcome::Spawn(why) => Err(util::message!(
            "finding.not_started",
            why = Message::verbatim(why.to_string())
        )),
        Outcome::Wait(why) => Err(util::message!(
            "finding.not_waited_on",
            why = Message::verbatim(why.to_string())
        )),
    }
}

fn too_long(limits: &Limits) -> Message {
    util::message!(
        "finding.too_long",
        run = limits.max_run / 1024,
        line = limits.max_line / 1024
    )
}

fn agent(limits: &Limits) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(limits.timeout))
        .build()
        .into()
}

fn fetch(agent: &ureq::Agent, url: &str, limits: &Limits) -> Result<String, Message> {
    let mut response = agent.get(url).call().map_err(|why| match why {
        ureq::Error::Timeout(_) => util::message!(
            "finding.no_answer",
            url = url,
            limit = interval_text(limits.timeout)
        ),
        why => util::message!(
            "finding.not_fetched",
            url = url,
            why = Message::verbatim(why.to_string())
        ),
    })?;
    let body = response
        .body_mut()
        .with_config()
        .limit(limits.max_run as u64)
        .read_to_string()
        .map_err(|why| match why {
            ureq::Error::BodyExceedsLimit(_) => too_long(limits),
            why => util::message!(
                "finding.not_read",
                url = url,
                why = Message::verbatim(why.to_string())
            ),
        })?;
    if body.lines().any(|line| line.len() > limits.max_line) {
        return Err(too_long(limits));
    }
    Ok(body)
}

/// A command kept running, every line it prints a reading. One that exits is a failure, started again after the backoff its streak has earned.
fn listen(
    spec: &SourceSpec,
    out: &Broadcast<Value>,
    pace: &Pace,
    cmd: &str,
    parse: &Parse,
    policy: &Policy,
) {
    loop {
        if !wait_wanted(out, pace.next_start(policy)) {
            return;
        }
        let started = Instant::now();
        let (tx, rx) = mpsc::channel();
        let ended = match util::process::stream_line(cmd, policy.limits.max_line, move |event| {
            let _ = tx.send(event);
        }) {
            Err(why) => util::message!(
                "finding.not_started",
                why = Message::verbatim(why.to_string())
            ),
            Ok(stream) => match follow(spec, out, &rx, parse) {
                Some(why) => why,
                None => {
                    stream.stop();
                    pace.listened(started, false, policy);
                    return;
                }
            },
        };
        failures::fail(Site::Source(spec.clone()), ended);
        pace.listened(started, true, policy);
    }
}

/// Publishes each line until the command ends, answering why, or until nobody wants the readings, answering `None`.
fn follow(
    spec: &SourceSpec,
    out: &Broadcast<Value>,
    events: &mpsc::Receiver<StreamEvent>,
    parse: &Parse,
) -> Option<Message> {
    let mut looked = Instant::now();
    loop {
        match events.recv_timeout(LOOK_IN) {
            Ok(StreamEvent::Line(line)) => match parse.read(&line) {
                Ok(reading) => {
                    out.publish(reading);
                    failures::recover(&Site::Source(spec.clone()));
                }
                Err(why) => failures::fail(Site::Source(spec.clone()), why),
            },
            Ok(StreamEvent::Ended(end)) => return Some(ended(end)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Some(util::message!("finding.stopped")),
        }
        if looked.elapsed() >= LOOK_IN {
            looked = Instant::now();
            if !out.wanted() {
                return None;
            }
        }
    }
}

fn ended(end: StreamEnd) -> Message {
    match end {
        StreamEnd::Exited(exit) => util::message!("finding.exited", exit = exit),
        StreamEnd::LineTooLong => util::message!("finding.line_too_long"),
        StreamEnd::Wait(why) => util::message!(
            "finding.not_waited_on",
            why = Message::verbatim(why.to_string())
        ),
    }
}

/// The last event of `kind`, as text: what it carried, or its own name for one that carries nothing. Starts from the last one emitted before anyone listened, so a reading begun after the event still has it.
fn event(out: &Broadcast<Value>, kind: EventKind) {
    let woken = wake(kind);
    let events = services::events::listen_holding(&[]);
    if let Some(last) = services::events::last_of(kind) {
        out.publish(Value::text(detail(&last)));
    }
    loop {
        if let Some(delivery) = events.next_within(LOOK_IN)
            && delivery.event.kind() == kind
        {
            out.publish(Value::text(detail(&delivery.event)));
        }
        if let Some(drain) = &woken {
            drain();
        }
        if !out.wanted() {
            return;
        }
    }
}

/// One way of holding a subscription to a service that only runs while someone subscribes to it.
trait Hold {
    type Held;
    fn hold<T: Send + 'static>(self, subscribe: fn(EventSender<T>)) -> Self::Held;
}

/// Subscribes to the service that emits `kind` through `how`, where that service only runs while someone subscribes to it — the radios and the power-profile watcher.
fn awaken<H: Hold>(kind: EventKind, how: H) -> Option<H::Held> {
    match kind {
        EventKind::WifiEnabled | EventKind::WifiDisabled => {
            Some(how.hold(services::network::subscribe_wifi))
        }
        EventKind::BluetoothEnabled | EventKind::BluetoothDisabled => {
            Some(how.hold(services::bluetooth::subscribe))
        }
        EventKind::PowerProfileChanged => Some(how.hold(services::powerprofiles::subscribe)),
        _ => None,
    }
}

/// Held from a producer thread, which drains what the service sends itself.
struct Drained;

impl Hold for Drained {
    type Held = Box<dyn Fn()>;

    fn hold<T: Send + 'static>(self, subscribe: fn(EventSender<T>)) -> Box<dyn Fn()> {
        let (tx, subscription) = platform_wayland::detached();
        subscribe(tx);
        Box::new(move || while subscription.try_recv().is_some() {})
    }
}

/// Held on the driver thread by the reactive owner current at the time, so the service may stop once that owner goes.
struct Watched;

impl Hold for Watched {
    type Held = ();

    fn hold<T: Send + 'static>(self, subscribe: fn(EventSender<T>)) {
        platform_wayland::watch(subscribe, |_: T| {});
    }
}

/// Keeps running the service that emits `kind` for an event source's producer, answering how to drain what it sends.
fn wake(kind: EventKind) -> Option<Box<dyn Fn()>> {
    awaken(kind, Drained)
}

/// Keeps running the service that emits `kind` for as long as the reactive owner current now lives: what a rule triggered by `kind` does.
pub(crate) fn keep_awake(kind: EventKind) {
    awaken(kind, Watched);
}

pub(crate) fn detail(event: &ShellEvent) -> String {
    match event {
        ShellEvent::WallpaperChanged { path, .. } => path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        ShellEvent::ThemeModeChanged { mode } => mode.id().to_string(),
        ShellEvent::BatteryStateChanged { charging, .. } => match charging {
            true => "charging".to_string(),
            false => "discharging".to_string(),
        },
        ShellEvent::BatteryUnderThreshold { threshold, .. } => threshold.to_string(),
        ShellEvent::PowerProfileChanged { profile } => profile.clone(),
        other => other.name().to_string(),
    }
}
