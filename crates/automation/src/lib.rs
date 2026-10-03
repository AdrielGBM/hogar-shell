//! Data and automation: the readings an expression can name, the variables a user sets, the shell's function families and the rules that act on them, over the `telar-expression` language (TA-6, DEC-22).
//!
//! - [`sources`]: every reading behind one producer each — module readings, the commands and addresses a layout declares, variables and events — with `[automation]`'s limits and backoff.
//! - [`env`]: the [`Environment`] expressions are checked against and the [`Readings`] they evaluate through, for the signed-in user or for the lock screen.
//! - [`bindings`]: what a layout binding drives, and how its value is written as an instance's option.
//! - [`rules`]: `[[rules]]`, a trigger and a `when` running a chain of commands and keeping a value in a variable.
//! - [`failures`]: what is failing as the shell runs — a source, a layout expression, a rule — for the problems notice.
//! - [`theme`]: `$theme.*`, the colours the shell paints with.
//! - [`vars`]: typed variables kept in machine state.
//! - [`scalar`]: text read as a number, a truth value or a colour, one way for a variable and a source alike.
//! - [`functions`]: `df`, `tr`, `bytes` and `rate`, on top of the standard families.

telar::rsx_modules!();

pub use env::{EVENT, Environment, Gate, Local, Readings, ReferenceKind};
pub use sources::{SourceSpec, UserSource, UserSources};
