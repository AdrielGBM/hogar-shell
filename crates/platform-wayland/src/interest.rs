//! Whether a registration is still wanted, asked without calling it.
//!
//! Every watcher here keeps a list of callbacks and runs a thread and a connection to feed them, and the shell's standing rule is that neither may outlive the last thing asking. The obvious way to express that — a callback returning whether it wants to stay — cannot: a return value is only read when the callback is called, so the registration of a compositor that has gone quiet is exactly the one that never gets to say it is done, and the watcher goes on holding a connection for nobody.
//!
//! So the answer lives beside the registration instead of inside its return value, and a registry can prune before it publishes rather than because it published. `EventSender::alive` answers the same question for a surface's subscription; this is it for a raw callback.
//!
//! **One token per producer run, not per registration.** A producer that registers in two places — the workspaces service reads a Wayland protocol *and* the compositor's event stream — has to retire both at once. Retiring them one at a time leaves a window where the second asks `Broadcast::wanted` again, is told a new subscriber has arrived, stays, and is then joined by the fresh producer's own registration.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A producer's claim on the registries it has registered with, and the one way to give every one of them up.
///
/// Cloning shares the claim rather than copying it, which is what lets the registry hold one end and the producer the other.
#[derive(Clone, Debug)]
pub struct Interest(Arc<AtomicBool>);

impl Interest {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }

    /// Whether whoever registered still wants to be called.
    pub fn alive(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Gives up every registration made with this token. Final, and deliberately not reversible: a producer that could change its mind would need the registries to agree on when it had, and the next subscriber starting a fresh producer is both simpler and what `Broadcast::wanted` already promises.
    pub fn retire(&self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Default for Interest {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the two-registry case rests on: one retirement reaches every copy, so a producer with a registration in two places cannot be half-retired.
    #[test]
    fn retiring_one_copy_retires_every_other() {
        let held_by_producer = Interest::new();
        let held_by_registry = held_by_producer.clone();
        let held_by_second_registry = held_by_producer.clone();
        assert!(held_by_registry.alive());

        held_by_producer.retire();

        assert!(!held_by_registry.alive());
        assert!(!held_by_second_registry.alive());
    }
}
