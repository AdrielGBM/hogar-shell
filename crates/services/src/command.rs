//! The shell's command surface, as a producer sees it.
//!
//! Two services run what the *user* configured rather than what their own code says: `[idle]` fires a request line at each stage, and a bound global shortcut is a request line the desktop portal delivers. Neither knows the command table — it lives with the socket, above here — so both go through the hooks below, installed once at startup by whoever owns that table.
//!
//! [`Request`] lives here rather than beside the socket for the same reason: a shortcut and a `hogar-shell …` invocation must produce the *same* thing, and only one of the two can see the socket.
//!
//! **A command may answer later.** Most answer as they return, but one whose work belongs off the driver thread — reading a bundle somebody else made — calls [`defer`] while it runs and keeps the [`Later`] it gets, the driver thread moves on, and the client goes on waiting, up to the time the command gave, until the command answers through it ([`Deferred::wait`]). What the command returned is what the client is told should that answer not come in time, and an answer that comes too late is refused, so the command can say it another way.

use std::cell::RefCell;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// One request in flight: the raw line, and where its reply goes when somebody waits for one. The socket thread blocks on `reply` while the driver thread runs the handler, which is what makes a command synchronous from the caller's point of view.
pub struct Request {
    line: String,
    reply: Option<mpsc::Sender<Reply>>,
}

/// What the driver thread sends back for a request.
pub enum Reply {
    /// The answer.
    Now(String),
    /// The command answers later: [`Deferred::wait`] is the answer.
    Later(Deferred),
}

impl Request {
    /// A request from a client that is waiting on the answer.
    pub fn attended(line: impl Into<String>, reply: mpsc::Sender<Reply>) -> Self {
        Self {
            line: line.into(),
            reply: Some(reply),
        }
    }

    /// A request nobody is waiting on the answer to — a global shortcut, a keypress. Its command cannot [`defer`], since there is nobody to answer later.
    ///
    /// Constructed here rather than by making the fields public: a `Request` carries a live reply channel the socket path depends on, and the only two ways to make one should be "from a client" and "from nobody".
    pub fn unattended(line: impl Into<String>) -> Self {
        Self {
            line: line.into(),
            reply: None,
        }
    }

    pub fn line(&self) -> &str {
        &self.line
    }

    /// Runs `run` on the request's line and sends back what it answers — or, where the command deferred its answer, that it answers later, with what it returned as what to say should that answer not come in time. Nothing is sent for an unattended request.
    pub fn answer_with(self, run: impl FnOnce(&str) -> String) {
        let Some(reply) = self.reply else {
            run(&self.line);
            return;
        };
        let answering = Answering::begin(Some(Asked::Open));
        let said = run(&self.line);
        let reply_now = match answering.end() {
            Some(Asked::Deferred { within, slot }) => Reply::Later(Deferred {
                within,
                otherwise: said,
                slot,
            }),
            _ => Reply::Now(said),
        };
        let _ = reply.send(reply_now);
    }
}

/// Where the request being answered on this thread stands.
enum Asked {
    /// Somebody waits for its answer, and the command has not deferred it.
    Open,
    /// The command deferred it ([`defer`]).
    Deferred { within: Duration, slot: Arc<Slot> },
}

thread_local! {
    static ASKED: RefCell<Option<Asked>> = const { RefCell::new(None) };
}

/// The request a command runs for, set for as long as it runs and put back as it was afterwards, a panic included: a line of a chain run inside another request is a request of its own, which nobody waits on.
struct Answering {
    outer: Option<Option<Asked>>,
}

impl Answering {
    fn begin(now: Option<Asked>) -> Self {
        Self {
            outer: Some(ASKED.with(|asked| asked.replace(now))),
        }
    }

    fn end(mut self) -> Option<Asked> {
        let outer = self.outer.take().flatten();
        ASKED.with(|asked| asked.replace(outer))
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        if let Some(outer) = self.outer.take() {
            ASKED.with(|asked| asked.replace(outer));
        }
    }
}

/// Defers the answer to the request this thread is running a command for: the client goes on waiting, up to `within`, for what the command sends through the [`Later`] this returns. `None` where nobody waits for an answer — an unattended request, a line of a chain, a second call for one request — and the command answers as it returns.
pub fn defer(within: Duration) -> Option<Later> {
    ASKED.with(|asked| {
        let mut asked = asked.borrow_mut();
        if !matches!(*asked, Some(Asked::Open)) {
            return None;
        }
        let slot = Arc::new(Slot::default());
        *asked = Some(Asked::Deferred {
            within,
            slot: Arc::clone(&slot),
        });
        Some(Later { slot })
    })
}

/// Where a deferred answer is handed from the command to whoever waits for it.
#[derive(Default)]
struct Slot {
    handoff: Mutex<Handoff>,
    ready: Condvar,
}

#[derive(Default)]
enum Handoff {
    #[default]
    Waiting,
    Answered(String),
    /// One side let go — the waiter gave up, or the command dropped its [`Later`] — so nothing more passes.
    Gone,
}

impl Slot {
    fn handoff(&self) -> std::sync::MutexGuard<'_, Handoff> {
        self.handoff.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn let_go(&self) {
        let mut handoff = self.handoff();
        if matches!(*handoff, Handoff::Waiting) {
            *handoff = Handoff::Gone;
            self.ready.notify_all();
        }
    }
}

/// A command's promise to answer a request later, from [`defer`].
pub struct Later {
    slot: Arc<Slot>,
}

impl Later {
    /// Hands `reply` to whoever waits for it: `false` when nobody does any more — they stopped waiting, or were never there — so the command can say it another way.
    pub fn answer(self, reply: String) -> bool {
        let mut handoff = self.slot.handoff();
        if !matches!(*handoff, Handoff::Waiting) {
            return false;
        }
        *handoff = Handoff::Answered(reply);
        self.slot.ready.notify_all();
        true
    }
}

impl Drop for Later {
    fn drop(&mut self) {
        self.slot.let_go();
    }
}

/// A request whose command answers later, as the client's side of it waits.
pub struct Deferred {
    within: Duration,
    otherwise: String,
    slot: Arc<Slot>,
}

impl Deferred {
    /// Blocks until the command answers, at most the time it gave, and answers what it said; past that, or once it dropped its [`Later`], what it said as it returned — and an answer after that is refused.
    pub fn wait(mut self) -> String {
        let deadline = Instant::now() + self.within;
        let otherwise = std::mem::take(&mut self.otherwise);
        let mut handoff = self.slot.handoff();
        loop {
            match std::mem::replace(&mut *handoff, Handoff::Gone) {
                Handoff::Answered(reply) => return reply,
                Handoff::Gone => return otherwise,
                Handoff::Waiting => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return otherwise;
                    }
                    *handoff = Handoff::Waiting;
                    handoff = self
                        .slot
                        .ready
                        .wait_timeout(handoff, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
    }
}

impl Drop for Deferred {
    fn drop(&mut self) {
        self.slot.let_go();
    }
}

/// Performs a request line and renders the reply.
type Runner = Box<dyn Fn(&str) -> String>;
/// Looks a request line up *without* performing it — the half of the table a validator needs.
type Resolver = Box<dyn Fn(&str) -> bool>;

thread_local! {
    static RUN: RefCell<Option<Runner>> = const { RefCell::new(None) };
    static RESOLVES: RefCell<Option<Resolver>> = const { RefCell::new(None) };
}

/// Registers the command table. Set once at startup by whoever owns it.
pub fn set_runner(
    run: impl Fn(&str) -> String + 'static,
    resolves: impl Fn(&str) -> bool + 'static,
) {
    RUN.with(|hook| *hook.borrow_mut() = Some(Box::new(run)));
    RESOLVES.with(|hook| *hook.borrow_mut() = Some(Box::new(resolves)));
}

/// Runs `line` as a request and returns its reply. Nobody waits on it but the caller, who takes the reply as it returns, so its command cannot [`defer`].
pub fn run(line: &str) -> String {
    let _unattended = Answering::begin(None);
    RUN.with(|hook| match hook.borrow().as_ref() {
        Some(run) => run(line),
        None => "err the shell is not accepting commands yet".to_string(),
    })
}

/// Runs `line` as a request: its reply's payload (empty for a bare `ok`), or why the shell refused it. A reply is a refusal when it is `err` or starts with `err `; anything else answered it.
pub fn run_checked(line: &str) -> Result<String, String> {
    let reply = run(line);
    if reply == "err" {
        return Err("the shell refused it".to_string());
    }
    if let Some(why) = reply.strip_prefix("err ") {
        return Err(why.to_string());
    }
    if reply == "ok" {
        return Ok(String::new());
    }
    Ok(match reply.strip_prefix("ok ") {
        Some(payload) => payload.to_string(),
        None => reply,
    })
}

/// A line of a chain the shell refused, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub line: String,
    pub why: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` was refused: {}", self.line, self.why)
    }
}

/// Runs `lines` in order, stopping at the first one the shell refuses, since a later line of a chain is written expecting the earlier ones to have happened. Answers each line's reply payload.
pub fn run_chain(lines: &[String]) -> Result<Vec<String>, Refused> {
    lines
        .iter()
        .map(|line| {
            run_checked(line).map_err(|why| Refused {
                line: line.clone(),
                why,
            })
        })
        .collect()
}

/// Whether `line` names a command the shell answers, **without running it**.
///
/// The distinction is the whole reason this is separate from [`run`]: anything that wants to check a request line — the global-shortcut table, a config validator — must be able to do so without performing it. Half the table changes the machine. Before the table is installed nothing resolves, which is the safe answer: a validator that ran this early would wave every line through.
pub fn resolves(line: &str) -> bool {
    RESOLVES.with(|hook| hook.borrow().as_ref().is_some_and(|check| check(line)))
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static RAN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn answering(reply: fn(&str) -> String) {
        RAN.with(|ran| ran.borrow_mut().clear());
        set_runner(
            move |line| {
                RAN.with(|ran| ran.borrow_mut().push(line.to_string()));
                reply(line)
            },
            |_| true,
        );
    }

    #[test]
    fn a_reply_is_a_refusal_only_when_it_says_err() {
        answering(|line| line.to_string());
        assert_eq!(run_checked("ok"), Ok(String::new()));
        assert_eq!(run_checked("ok 42"), Ok("42".to_string()));
        assert_eq!(run_checked("okay"), Ok("okay".to_string()));
        assert_eq!(
            run_checked("err no such screen"),
            Err("no such screen".to_string())
        );
        assert_eq!(run_checked("err"), Err("the shell refused it".to_string()));
        assert_eq!(run_checked("error-free"), Ok("error-free".to_string()));
    }

    /// What a client waiting on `line` receives when `run` answers it.
    fn asked(line: &str, run: impl FnOnce(&str) -> String) -> Reply {
        let (reply, replies) = mpsc::channel();
        Request::attended(line, reply).answer_with(run);
        replies
            .try_recv()
            .expect("an attended request is always answered")
    }

    fn later(reply: Reply) -> Deferred {
        match reply {
            Reply::Later(deferred) => deferred,
            Reply::Now(said) => panic!("answered at once: {said}"),
        }
    }

    #[test]
    fn a_command_that_does_not_defer_answers_as_it_returns() {
        match asked("shell ping", |_| "ok pong".to_string()) {
            Reply::Now(said) => assert_eq!(said, "ok pong"),
            Reply::Later(_) => panic!("it never deferred"),
        }
    }

    #[test]
    fn a_deferred_answer_reaches_whoever_waits_for_it() {
        let mut kept = None;
        let deferred = later(asked("layout import x", |_| {
            kept = defer(Duration::from_secs(5));
            "ok still reading".to_string()
        }));
        let promise = kept.expect("an attended request can be deferred");
        let answering = std::thread::spawn(move || promise.answer("ok imported".to_string()));
        assert_eq!(deferred.wait(), "ok imported");
        assert!(answering.join().unwrap(), "it reached the client");
    }

    #[test]
    fn an_answer_that_comes_too_late_is_refused_and_the_client_hears_what_was_returned() {
        let mut kept = None;
        let deferred = later(asked("layout import x", |_| {
            kept = defer(Duration::from_millis(20));
            "ok still reading".to_string()
        }));
        assert_eq!(deferred.wait(), "ok still reading");
        assert!(
            !kept.unwrap().answer("ok imported".to_string()),
            "nobody waits any more, so the command says it another way"
        );
    }

    #[test]
    fn a_promise_dropped_unanswered_ends_the_wait_at_once() {
        let mut kept = None;
        let deferred = later(asked("layout import x", |_| {
            kept = defer(Duration::from_secs(60));
            "ok still reading".to_string()
        }));
        drop(kept);
        let started = Instant::now();
        assert_eq!(deferred.wait(), "ok still reading");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_client_that_hung_up_refuses_the_answer() {
        let mut kept = None;
        drop(later(asked("layout import x", |_| {
            kept = defer(Duration::from_secs(60));
            String::new()
        })));
        assert!(!kept.unwrap().answer("ok imported".to_string()));
    }

    /// Only a request somebody waits on can be answered later: not an unattended one, not a line of a chain run while answering another, and not twice.
    #[test]
    fn nothing_but_an_attended_request_defers() {
        assert!(defer(Duration::from_secs(1)).is_none(), "no request at all");
        Request::unattended("layout import x").answer_with(|_| {
            assert!(defer(Duration::from_secs(1)).is_none());
            String::new()
        });
        set_runner(
            |_| match defer(Duration::from_secs(1)) {
                Some(_) => "deferred".to_string(),
                None => "answered".to_string(),
            },
            |_| true,
        );
        let mut inner = String::new();
        let deferred = later(asked("rule run x", |_| {
            inner = run("layout import x");
            assert!(
                defer(Duration::from_secs(1)).is_some(),
                "the outer request still can"
            );
            assert!(defer(Duration::from_secs(1)).is_none(), "but once");
            String::new()
        }));
        assert_eq!(inner, "answered");
        drop(deferred);
    }

    #[test]
    fn a_chain_stops_at_the_first_refused_line() {
        answering(|line| match line.starts_with("nope") {
            true => "err no such command".to_string(),
            false => format!("ok {line}"),
        });
        let lines = ["a", "nope", "b"].map(String::from);
        let refused = run_chain(&lines).unwrap_err();
        assert_eq!(
            refused,
            Refused {
                line: "nope".to_string(),
                why: "no such command".to_string()
            }
        );
        assert_eq!(refused.to_string(), "`nope` was refused: no such command");
        assert_eq!(RAN.with(|ran| ran.borrow().clone()), ["a", "nope"]);

        let lines = ["a", "b"].map(String::from);
        assert_eq!(
            run_chain(&lines),
            Ok(vec!["a".to_string(), "b".to_string()])
        );
    }
}
