//! The fallback evaluator: whatever `qalc` can answer that the in-house one cannot.
//!
//! Qalculate knows currencies, physical constants, date arithmetic and a units table far larger than the one next door. It is also a process, and starting one per keystroke on the UI thread would stall the frame for as long as it takes to load — so this goes through `shared::asset`: ask, get a signal, and let the worker fill it in.
//!
//! Asked *only* when the query is explicitly a calculation (the `=` prefix) and the in-house evaluator has already declined. An app search must never spawn a process, and a question with a local answer must never wait for one.

use std::cell::RefCell;
use std::time::Duration;

use telar::ReadSignal;

use crate::asset::{Load, Loader};
use crate::deps::{self, Dep};

/// Long enough for a cold start of a program that loads a units database, short enough that a wedged one is not mistaken for a hard question.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Anything longer is not an answer to a one-line sum, and the row can only show a line of it anyway.
const MAX_LEN: usize = 200;

thread_local! {
    static ANSWERS: RefCell<Option<Loader<String, String>>> = const { RefCell::new(None) };
}

/// `qalc`'s answer to `query`, starting one the first time it is asked for.
///
/// `None` covers all three of "still running", "no qalc installed" and "it had nothing to say" — none of which a launcher row can usefully distinguish for the user, who is typing and will see the answer or will not.
pub fn answer(query: &str) -> Option<String> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    match state(query).get() {
        Load::Ready(answer) => Some(answer),
        Load::Loading | Load::Missing => None,
    }
}

fn state(query: &str) -> ReadSignal<Load<String>> {
    ensure_store();
    ANSWERS.with(|store| {
        let borrow = store.borrow();
        let Some(store) = borrow.as_ref() else {
            return telar::signal(Load::Missing).read_only();
        };
        store.get(query.to_string(), |_| None)
    })
}

fn ensure_store() {
    if ANSWERS.with(|store| store.borrow().is_some()) {
        return;
    }
    let store = Loader::new(|query: &String| run(query));
    ANSWERS.with(|cell| *cell.borrow_mut() = Some(store));
}

/// Runs `qalc` once. Blocking — only ever called on the worker thread, and never without the deadline `shared::process` puts on it: one wedged child would otherwise park the worker for the life of the shell.
fn run(query: &str) -> Option<String> {
    // `-t` is terse (the result alone, no echo of the question), and the expression goes after `--` so a query starting with a dash is an expression rather than an unknown flag.
    let stdout = deps::output(Dep::Qalc, &["-t", "--", query], TIMEOUT)?;
    clean(&stdout)
}

/// The answer out of qalc's terse output, or `None` when it did not really answer.
///
/// Qalculate answers an expression it cannot parse by echoing it back, so an app name typed after `=` would come back as itself and read as a result. An answer identical to the question is therefore treated as no answer.
fn clean(stdout: &str) -> Option<String> {
    let answer = stdout.trim();
    if answer.is_empty() || answer.len() > MAX_LEN {
        return None;
    }
    // Terse output is one line; a warning or an error explanation is not what the row is for.
    let first = answer.lines().next()?.trim();
    (!first.is_empty() && !first.starts_with("error")).then(|| first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_that_is_only_the_question_back_is_not_an_answer() {
        // What qalc does with something it cannot parse, which is exactly what an app name typed after `=` is.
        assert_eq!(clean("42\n").as_deref(), Some("42"));
        assert_eq!(clean("1.8641 mi").as_deref(), Some("1.8641 mi"));
        assert_eq!(clean("  1 EUR  \n").as_deref(), Some("1 EUR"));
        assert_eq!(clean(""), None);
        assert_eq!(clean("   \n  "), None);
        assert_eq!(clean("error: nope"), None);
        assert_eq!(
            clean(&"9".repeat(MAX_LEN + 1)),
            None,
            "a page of output is not a row"
        );
        assert_eq!(
            clean("12\nwarning: assuming something").as_deref(),
            Some("12"),
            "the first line is the answer; the rest is commentary"
        );
    }

    /// The store must not spawn anything just by being asked, which is what makes it safe to call from a build.
    #[test]
    fn asking_for_nothing_runs_nothing() {
        assert_eq!(answer(""), None);
        assert_eq!(answer("   "), None);
        // Headless there is no platform loop, so the worker never runs and the request stays pending.
        assert_eq!(answer("2+2"), None);
    }
}
