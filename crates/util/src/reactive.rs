//! Deriving one value from another, for a card that has to follow a service while it is on screen.
//!
//! Two rules are encoded here, both learned the hard way.
//!
//! **Read the value out before mapping it.** `with` holds the reactive runtime's borrow for as long as its
//! closure runs, so a nested read panics with `RefCell already borrowed`. The nested read is rarely visible:
//! `t!` reads the locale signal, so any closure translating its own string is one. It is not a compile error
//! and it does not fire until the surface is built. Reading the locale still happens inside `map`, which is
//! what makes a derived label re-render on a live language switch.
//!
//! **A derivation is a [`Memo`], never a signal written by an effect.** `telar::effect` hands back a handle whose
//! `Drop` deregisters the effect, so `let _ = effect(…)` runs exactly once and then stops — the derived value is
//! seeded correctly and never moves again, which looks like a working card until you watch it. A `Memo` is
//! `Rc`-backed and lives as long as the closure reading it, so the widget that draws the value is what keeps
//! the derivation alive, with nothing for a caller to remember.

pub use telar::{Source, derive, derive_pair};

use telar::{Memo, memo};

/// A value a surface reads and re-reads: derived from a service, or fixed for the life of the surface. One type
/// for both so a card takes one kind of argument rather than two.
pub type Live<T> = Memo<T>;

/// A value that never changes while the surface is up — a device name, a configured step, a mount point.
pub fn fixed<T: Clone + PartialEq + 'static>(value: T) -> Live<T> {
    memo(move || value.clone())
}

/// [`fixed`] for a literal, saving the `.to_string()` at every call site that labels a row.
pub fn fixed_text(text: impl Into<String>) -> Live<String> {
    fixed(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::signal;

    #[test]
    fn a_derived_value_follows_its_source() {
        telar::reset_runtime();
        let source = signal(2i32);
        let doubled = derive(source.clone(), |n| n * 2);
        assert_eq!(
            doubled.get(),
            4,
            "seeded from the source, not from a default"
        );
        source.set(5);
        assert_eq!(doubled.get(), 10);
    }

    #[test]
    fn a_pair_recomputes_when_either_half_moves() {
        telar::reset_runtime();
        let level = signal(10i32);
        let charging = signal(false);
        let label = derive_pair(
            level.read_only(),
            charging.read_only(),
            |level, charging| format!("{level}{}", if charging { "+" } else { "" }),
        );
        assert_eq!(label.get(), "10");
        charging.set(true);
        assert_eq!(label.get(), "10+");
        level.set(11);
        assert_eq!(label.get(), "11+");
    }

    /// The regression this module exists for. Deriving through a signal written by an effect seeds correctly
    /// and then goes dead the moment the handle drops, which is what every hover popout used to do.
    #[test]
    fn a_derivation_outlives_the_call_that_made_it() {
        telar::reset_runtime();
        let source = signal(1i32);
        let derived = derive(source.clone(), |n| n * 10);
        // Whatever a widget would do: hold the handle in a closure and read it later.
        let read: Box<dyn Fn() -> i32> = Box::new(move || derived.get());
        source.set(7);
        assert_eq!(
            read(),
            70,
            "a widget holding the derivation keeps it subscribed"
        );
    }

    #[test]
    fn a_fixed_value_reads_back_unchanged() {
        telar::reset_runtime();
        assert_eq!(fixed_text("Tctl").get(), "Tctl");
        assert_eq!(fixed(42u32).get(), 42);
    }
}
