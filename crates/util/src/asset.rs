//! Asking a worker thread for something the frame must not wait for: a request goes out, a signal comes back `Loading`, and a worker fills it in later; the store is per-thread since a `Loader` holds `Rc` signal handles.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use platform_wayland::{EventSender, WatchToken, app_watch, timeout, unwatch};
use telar::{OwnerId, ReadSignal, RwSignal, dispose_owner, owner_scope, signal, with_owner};

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

/// How often a key whose work found nothing is asked again before it settles as [`Load::Missing`], for work that fails for a while and then succeeds — a download attempted before the network is up.
pub struct Retry<K> {
    pub attempts: u32,
    pub delay: Duration,
    /// Told about a key that has run out of attempts.
    pub gave_up: fn(&K),
}

impl<K> Clone for Retry<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Retry<K> {}

type Signals<K, V> = Rc<RefCell<HashMap<K, RwSignal<Load<V>>>>>;

/// A keyed set of in-flight results, one worker thread behind all of them.
pub struct Loader<K: 'static, V: 'static> {
    signals: Signals<K, V>,
    requests: Sender<K>,
    /// Owns every signal handed out, so [`Loader::retire`] frees them in one call.
    owner: OwnerId,
    worker: Option<WatchToken>,
}

impl<K, V> Loader<K, V>
where
    K: Clone + Eq + Hash + Send + 'static,
    V: Clone + Send + 'static,
{
    /// Starts a loader whose worker runs `work` for each distinct key, in the order asked; a key it finds nothing for is [`Load::Missing`] at once. The worker and signals belong to the process, not to the first requesting surface, so a loader keeps answering after that surface closes.
    pub fn new(work: impl Fn(&K) -> Option<V> + Send + 'static) -> Self {
        Self::start(work, None)
    }

    /// [`Loader::new`], asking a key whose work found nothing again as `retry` says before it is `Missing`.
    pub fn retrying(retry: Retry<K>, work: impl Fn(&K) -> Option<V> + Send + 'static) -> Self {
        Self::start(work, Some(retry))
    }

    fn start(work: impl Fn(&K) -> Option<V> + Send + 'static, retry: Option<Retry<K>>) -> Self {
        let signals: Signals<K, V> = Rc::new(RefCell::new(HashMap::new()));
        let (requests, incoming) = channel::<K>();
        let mut delivery = Delivery {
            signals: Rc::clone(&signals),
            requests: requests.clone(),
            retry,
            attempts: HashMap::new(),
        };
        let worker = app_watch(
            move |sender| serve(incoming, sender, work),
            move |(key, value)| delivery.deliver(key, value),
        );
        Self {
            signals,
            requests,
            owner: telar::detached(owner_scope).id(),
            worker,
        }
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
        let handle = with_owner(Some(self.owner), || signal(initial));
        self.signals.borrow_mut().insert(key.clone(), handle);
        if pending {
            let _ = self.requests.send(key);
        }
        handle.read_only()
    }

    /// Whether `key` has been asked for.
    pub fn has(&self, key: &K) -> bool {
        self.signals.borrow().contains_key(key)
    }

    /// Stops the worker and frees every signal this loader handed out, for a loader being replaced; one that lives as long as the process has nothing to free.
    pub fn retire(self) {
        if let Some(worker) = self.worker {
            unwatch(worker);
        }
        dispose_owner(self.owner);
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

struct Delivery<K: 'static, V: 'static> {
    signals: Signals<K, V>,
    requests: Sender<K>,
    retry: Option<Retry<K>>,
    attempts: HashMap<K, u32>,
}

impl<K: Clone + Eq + Hash + Send + 'static, V> Delivery<K, V> {
    fn deliver(&mut self, key: K, value: Option<V>) {
        let state = match value {
            Some(value) => {
                self.attempts.remove(&key);
                Load::Ready(value)
            }
            None => match self.retry {
                Some(retry) => {
                    let attempts = self.attempts.entry(key.clone()).or_insert(0);
                    *attempts += 1;
                    if *attempts < retry.attempts {
                        let requests = self.requests.clone();
                        timeout(retry.delay, move || {
                            let _ = requests.send(key);
                        });
                        return;
                    }
                    self.attempts.remove(&key);
                    (retry.gave_up)(&key);
                    Load::Missing
                }
                None => Load::Missing,
            },
        };
        // Clone the handle out and drop the map borrow BEFORE `set`: a signal write flushes effects synchronously, and an effect that asks the same loader for another key would re-enter this borrow and panic.
        let handle = self.signals.borrow().get(&key).cloned();
        if let Some(handle) = handle {
            handle.set(state);
        }
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

    fn delivery(retry: Option<Retry<String>>) -> (Delivery<String, u32>, RwSignal<Load<u32>>) {
        let signals: Signals<String, u32> = Rc::new(RefCell::new(HashMap::new()));
        let handle = signal(Load::Loading);
        signals.borrow_mut().insert("a".to_string(), handle);
        let delivery = Delivery {
            signals,
            requests: channel().0,
            retry,
            attempts: HashMap::new(),
        };
        (delivery, handle)
    }

    #[test]
    fn a_delivered_answer_replaces_the_placeholder_and_a_failed_one_says_so() {
        let (mut delivery, handle) = delivery(None);
        delivery.deliver("a".to_string(), Some(3));
        assert_eq!(handle.peek(), Load::Ready(3));

        delivery.deliver("a".to_string(), None);
        assert_eq!(handle.peek(), Load::Missing);

        // A key nobody is waiting on is dropped rather than stored: the request is what creates the signal.
        delivery.deliver("gone".to_string(), Some(1));
        assert_eq!(delivery.signals.borrow().len(), 1);
    }

    thread_local! {
        static GAVE_UP: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    /// A key whose work found nothing stays `Loading` while it has attempts left, and settles as `Missing` — saying so once — when it runs out.
    #[test]
    fn a_retried_key_stays_loading_until_its_attempts_run_out() {
        let retry = Retry {
            attempts: 3,
            delay: Duration::ZERO,
            gave_up: |key: &String| GAVE_UP.with(|gave_up| gave_up.borrow_mut().push(key.clone())),
        };
        let (mut delivery, handle) = delivery(Some(retry));
        for _ in 0..2 {
            delivery.deliver("a".to_string(), None);
            assert_eq!(handle.peek(), Load::Loading, "an attempt was left");
        }
        delivery.deliver("a".to_string(), None);
        assert_eq!(handle.peek(), Load::Missing);
        assert_eq!(GAVE_UP.with(|gave_up| gave_up.borrow().clone()), ["a"]);
    }

    /// What a retired loader handed out is freed, rather than left in the runtime for as long as the process runs.
    #[test]
    fn retiring_a_loader_frees_every_signal_it_handed_out() {
        let loader: Loader<String, u32> = Loader::new(|_| None);
        let handle = loader.get("a".to_string(), |_| Some(7));
        assert!(handle.is_alive());
        loader.retire();
        assert!(!handle.is_alive());
    }

    /// The first ask for a key is somebody's build, and the entry has to survive that build being torn down: the next surface to ask is handed the cached signal, and a handle freed with the first one panicked the UI thread on its first read.
    #[test]
    fn an_entry_outlives_the_build_that_first_asked_for_it() {
        let loader: Loader<String, u32> = Loader::new(|_| None);
        let panel = telar::owner_scope();
        let owner = panel.id();
        let first = loader.get("a".to_string(), |_| Some(7));
        drop(panel);
        telar::dispose_owner(owner);

        let again = loader.get("a".to_string(), |_| unreachable!("the entry is cached"));
        assert_eq!(again.get(), Load::Ready(7));
        assert_eq!(first.get(), Load::Ready(7));

        telar::reset_layout_runtime();
        assert_eq!(
            loader.get("a".to_string(), |_| None).get(),
            Load::Ready(7),
            "and a layout reset between builds leaves it alone too"
        );
    }
}
