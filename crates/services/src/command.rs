//! The shell's command surface, as a producer sees it.
//!
//! Two services run what the *user* configured rather than what their own code says: `[idle]` fires a request line at each stage, and a bound global shortcut is a request line the desktop portal delivers. Neither knows the command table — it lives with the socket, above here — so both go through the hooks below, installed once at startup by whoever owns that table.
//!
//! [`Request`] lives here rather than beside the socket for the same reason: a shortcut and a `hogar-shell …` invocation must produce the *same* thing, and only one of the two can see the socket.

use std::cell::RefCell;
use std::sync::mpsc;

/// One request in flight: the raw line, and where its reply goes. The socket thread blocks on `reply` while the driver thread runs the handler, which is what makes a command synchronous from the caller's point of view.
pub struct Request {
    line: String,
    reply: mpsc::Sender<String>,
}

impl Request {
    /// A request from a client that is waiting on the answer.
    pub fn attended(line: impl Into<String>, reply: mpsc::Sender<String>) -> Self {
        Self {
            line: line.into(),
            reply,
        }
    }

    /// A request nobody is waiting on the answer to — a global shortcut, a keypress. The handler still sends its reply, into a receiver that has already been dropped, which is a no-op.
    ///
    /// Constructed here rather than by making the fields public: a `Request` carries a live reply channel the socket path depends on, and the only two ways to make one should be "from a client" and "from nobody".
    pub fn unattended(line: impl Into<String>) -> Self {
        let (reply, _) = mpsc::channel();
        Self {
            line: line.into(),
            reply,
        }
    }

    pub fn line(&self) -> &str {
        &self.line
    }

    /// Answers the request. Nothing is listening for an unattended one, which is why this cannot fail.
    pub fn answer(&self, reply: String) {
        let _ = self.reply.send(reply);
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

/// Runs `line` as a request and returns its reply.
pub fn run(line: &str) -> String {
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
