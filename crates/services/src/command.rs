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

/// Whether `line` names a command the shell answers, **without running it**.
///
/// The distinction is the whole reason this is separate from [`run`]: anything that wants to check a request line — the global-shortcut table, a config validator — must be able to do so without performing it. Half the table changes the machine. Before the table is installed nothing resolves, which is the safe answer: a validator that ran this early would wave every line through.
pub fn resolves(line: &str) -> bool {
    RESOLVES.with(|hook| hook.borrow().as_ref().is_some_and(|check| check(line)))
}
