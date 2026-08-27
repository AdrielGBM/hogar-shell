//! Asking a worker thread for something the frame must not wait for.
//!
//! Cover art was the first of these and wrote the shape by hand: a request goes out, a signal comes back `Loading`, and a worker fills it in later. Two more wanted the same thing for different reasons — a wallpaper thumbnail is a full-resolution decode, a `qalc` answer is a subprocess — and neither is work the UI thread can do between two frames. This is that shape once, so the next one is a `Loader::new` rather than a fourth copy of the borrow rule below.
//!
//! The store is per-thread by construction: a `Loader` holds `Rc` signal handles, so it lives on the driver thread with the surfaces that read it, and only the work itself crosses over.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};

use platform_wayland::{EventSender, watch};
use telar::{ReadSignal, RwSignal, signal};

/// Where one request has got to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    /// The work ran and there is nothing to show. A view draws its fallback rather than waiting longer.
    Missing,
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }
}

type Signals<K, V> = Rc<RefCell<HashMap<K, RwSignal<Load<V>>>>>;

/// A keyed set of in-flight results, one worker thread behind all of them.
pub struct Loader<K: 'static, V: 'static> {
    signals: Signals<K, V>,
    requests: Sender<K>,
}

impl<K, V> Loader<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + 'static,
{
    /// Starts a loader whose worker runs `work` for each distinct key, in the order asked.
    ///
    /// Headless — under a test or an offline render — `watch` is a no-op: no worker runs and every request stays `Loading`, which is what a surface with no platform behind it should show.
    pub fn new(work: impl Fn(&K) -> Option<V> + Send + 'static) -> Self {
        let signals: Signals<K, V> = Rc::new(RefCell::new(HashMap::new()));
        let (requests, incoming) = channel::<K>();
        let delivery = Rc::clone(&signals);
        watch(
            move |sender| serve(incoming, sender, work),
            move |(key, value)| deliver(&delivery, key, value),
        );
        Self { signals, requests }
    }

    /// The state of `key`, starting the work the first time it is asked for.
    ///
    /// `at_hand` is the answer that needs no worker — a cache entry already on disk — so the common case renders the real thing on the frame it is asked for instead of flashing a placeholder.
    pub fn get(&self, key: K, at_hand: impl FnOnce(&K) -> Option<V>) -> ReadSignal<Load<V>> {
        if let Some(existing) = self.signals.borrow().get(&key) {
            return existing.read_only();
        }
        let initial = match at_hand(&key) {
            Some(value) => Load::Ready(value),
            None => Load::Loading,
        };
        let pending = matches!(initial, Load::Loading);
        // Detached: this cache outlives every scope that reads from it, and the first read for a key is somebody's build. Attributed to that build, the entry would be freed when it went away and every later reader would find a dead signal.
        let handle = telar::detached(|| signal(initial));
        self.signals.borrow_mut().insert(key.clone(), handle);
        if pending {
            let _ = self.requests.send(key);
        }
        handle.read_only()
    }
}

fn serve<K, V>(
    incoming: Receiver<K>,
    sender: EventSender<(K, Option<V>)>,
    work: impl Fn(&K) -> Option<V>,
) where
    K: Send + 'static,
    V: Send + 'static,
{
    for key in incoming {
        let value = work(&key);
        if !sender.send((key, value)) {
            return;
        }
    }
}

fn deliver<K: Eq + Hash, V>(signals: &Signals<K, V>, key: K, value: Option<V>) {
    // Clone the handle out and drop the map borrow BEFORE `set`: a signal write flushes effects synchronously, and an effect that asks the same loader for another key would re-enter this borrow and panic.
    let handle = signals.borrow().get(&key).cloned();
    if let Some(handle) = handle {
        handle.set(match value {
            Some(value) => Load::Ready(value),
            None => Load::Missing,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_already_at_hand_never_reaches_the_worker() {
        let loader: Loader<String, u32> = Loader::new(|_| unreachable!("the worker is not needed"));
        let state = loader.get("a".to_string(), |_| Some(7));
        assert_eq!(state.peek(), Load::Ready(7));
    }

    #[test]
    fn one_key_is_one_request_and_one_signal() {
        let loader: Loader<String, u32> = Loader::new(|_| None);
        let first = loader.get("a".to_string(), |_| None);
        let second = loader.get("a".to_string(), |_| None);
        assert_eq!(first.peek(), Load::Loading);
        assert_eq!(
            second.peek(),
            Load::Loading,
            "the second ask joins the first rather than starting another"
        );
        assert_eq!(loader.signals.borrow().len(), 1);
    }

    #[test]
    fn a_delivered_answer_replaces_the_placeholder_and_a_failed_one_says_so() {
        let signals: Signals<String, u32> = Rc::new(RefCell::new(HashMap::new()));
        let handle = signal(Load::Loading);
        signals.borrow_mut().insert("a".to_string(), handle);
        deliver(&signals, "a".to_string(), Some(3));
        assert_eq!(handle.peek(), Load::Ready(3));

        deliver(&signals, "a".to_string(), None);
        assert_eq!(handle.peek(), Load::Missing);

        // A key nobody is waiting on is dropped rather than stored: the request is what creates the signal.
        deliver(&signals, "gone".to_string(), Some(1));
        assert_eq!(signals.borrow().len(), 1);
    }
}
