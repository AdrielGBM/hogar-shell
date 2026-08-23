//! Open windows over `ext-foreign-toplevel-list-v1`: what exists, what it is called, and a handle on it.
//!
//! The enumeration a dock, a window switcher and an open-window launcher mode all rest on, read from the
//! compositor rather than from one compositor's IPC.
//!
//! **The identifier is the join.** Each toplevel carries an opaque, stable string the compositor promises is
//! unique for that window's whole life. On Hyprland it is exactly the `stableId` of `hyprctl clients` —
//! verified against a live 0.56.1 session — which is what lets a reading taken here be matched against one
//! taken over IPC by equality rather than by guessing from titles.
//!
//! **What this protocol does not carry.** A toplevel handle reports a title, an app id and that identifier. It
//! does *not* report which workspace or output the window is on, whether it is focused, minimised or
//! fullscreen, and it offers no way to act on the window — `zwlr-foreign-toplevel-management-v1` is the only
//! portable route to any of that. A list built from this alone is a list, not a switcher.
//!
//! It is also the only way to *name* a window as a capture source: `ext-image-copy-capture-v1` will capture a
//! toplevel, but only one identified by the `ext_foreign_toplevel_handle_v1` that only this protocol hands out.
//!
//! **The watcher stops when the last registration is retired**, dropping its connection with it, and the next
//! [`watch`] starts a fresh one. Unlike the other three watchers here this one is read-only — the protocol has
//! no requests — so there is nothing to talk to it over and the running flag is the whole of its slot. Teardown
//! is bounded by the next event either way: until the compositor says a window opened or closed, the thread is
//! asleep in `poll`, resident but not running.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

use crate::interest::Interest;

use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};

/// The global a compositor advertises when it can list windows at all.
pub const TOPLEVEL_LIST_INTERFACE: &str = "ext_foreign_toplevel_list_v1";

/// Names one window for as long as it is open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToplevelId(u32);

/// An open window, as the compositor describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toplevel {
    pub id: ToplevelId,
    /// Opaque, unique and stable for the window's life. Hyprland's `stableId`, and the field to match on when
    /// the same window is also being read over compositor IPC.
    pub identifier: String,
    pub title: String,
    /// The application's own id — what `class` is called everywhere except Hyprland.
    pub app_id: String,
}

type Handler = Box<dyn FnMut(&[Toplevel]) + Send>;

/// One reader of the list, and the claim that says whether it still wants to be one.
struct Registration {
    interest: Interest,
    handler: Handler,
}

static HANDLERS: Mutex<Vec<Registration>> = Mutex::new(Vec::new());
static LATEST: Mutex<Vec<Toplevel>> = Mutex::new(Vec::new());
/// Whether a watcher is running. Not a `OnceLock`, because it has to be able to say "no" again: the thread
/// gives it back when the last registration goes, and a later [`watch`] starts a fresh one.
static WATCHING: Mutex<bool> = Mutex::new(false);
/// Set when starting finds no compositor or no list protocol, so a caller on one that cannot answer does not
/// open a connection on every `watch` — no watcher is left behind to say it already failed.
static UNSUPPORTED: AtomicBool = AtomicBool::new(false);

/// Whether the compositor lists windows at all, asked over a connection of its own so it answers outside a
/// running shell. `None` means no compositor could be reached.
pub fn toplevels_supported() -> Option<bool> {
    crate::globals::advertises(TOPLEVEL_LIST_INTERFACE)
}

/// Registers `on_change` for the window list, starting the watcher on first use and keeping it for as long as
/// `interest` is alive.
///
/// Returns false when the compositor does not implement the protocol, in which case `on_change` is never
/// called. A handler registered after the watcher is running is handed the current list immediately.
///
/// The running flag is held across both the start and the registration, and taken again by [`retire`]: that
/// overlap is what stops a `watch` landing on a watcher already on its way out and never being called.
pub fn watch(interest: &Interest, on_change: impl FnMut(&[Toplevel]) + Send + 'static) -> bool {
    let mut handler: Handler = Box::new(on_change);
    let mut running = WATCHING.lock().unwrap();
    if !*running {
        if UNSUPPORTED.load(Ordering::Relaxed) {
            return false;
        }
        if !start() {
            UNSUPPORTED.store(true, Ordering::Relaxed);
            return false;
        }
        *running = true;
    }
    let latest = LATEST.lock().unwrap().clone();
    if !latest.is_empty() {
        handler(&latest);
    }
    HANDLERS.lock().unwrap().push(Registration {
        interest: interest.clone(),
        handler,
    });
    true
}

/// The last list published, without waiting for a window to open or close.
pub fn current() -> Vec<Toplevel> {
    LATEST.lock().unwrap().clone()
}

/// Drops the registrations whose owner has retired, and says whether any are left.
fn anyone_listening() -> bool {
    let mut handlers = HANDLERS.lock().unwrap();
    handlers.retain(|registration| registration.interest.alive());
    !handlers.is_empty()
}

/// Gives up the running flag, for a thread about to return. `false` is a `watch` having landed since the last
/// registration went: it is already in the list, and retiring now would leave it waiting on nothing.
fn retire() -> bool {
    let mut running = WATCHING.lock().unwrap();
    if !HANDLERS.lock().unwrap().is_empty() {
        return false;
    }
    *running = false;
    true
}

/// Gives the flag up whatever is registered, for a watcher whose connection has failed under it: every
/// registration is waiting on a thread that is not coming back, and leaving the flag set would stop any later
/// caller from starting one that works.
fn forget() {
    let mut running = WATCHING.lock().unwrap();
    HANDLERS.lock().unwrap().clear();
    *running = false;
}

/// Connects and binds here rather than on the watcher thread, so the answer to "does this compositor list
/// windows" is known by the time [`watch`] returns.
fn start() -> bool {
    let Ok(connection) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, queue)) = registry_queue_init::<Watcher>(&connection) else {
        return false;
    };
    let qh = queue.handle();
    if let Err(e) = globals.bind::<ExtForeignToplevelListV1, _, _>(&qh, 1..=1, ()) {
        tracing::debug!("no ext-foreign-toplevel-list-v1: {e}");
        return false;
    }

    std::thread::Builder::new()
        .name("hogar-shell-ext-toplevels".to_string())
        .spawn(move || run(Watcher::default(), connection, queue))
        .is_ok()
}

fn run(mut watcher: Watcher, connection: Connection, queue: EventQueue<Watcher>) {
    let Ok(mut event_loop) = EventLoop::<Watcher>::try_new() else {
        return forget();
    };
    if WaylandSource::new(connection, queue)
        .insert(event_loop.handle())
        .is_err()
    {
        return forget();
    }
    while !watcher.finished {
        if event_loop.dispatch(None, &mut watcher).is_err() {
            break;
        }
        // Asked after a dispatch rather than after a publish: a registration is retired by whoever made it,
        // which is not something this thread is told about, so the only sound moment to look is every time it
        // wakes.
        if !anyone_listening() && retire() {
            return;
        }
    }
    forget();
}

#[derive(Default)]
struct Entry {
    identifier: String,
    title: String,
    app_id: String,
}

/// The accumulated list, with no protocol object of its own — which is what lets the reading be checked
/// without a compositor.
#[derive(Default)]
struct State {
    windows: HashMap<u32, Entry>,
    /// Announcement order, which is the only order this protocol offers: a `HashMap` would reshuffle a window
    /// list on every publish.
    order: Vec<u32>,
}

#[derive(Default)]
struct Watcher {
    state: State,
    finished: bool,
}

impl State {
    fn add(&mut self, key: u32) {
        if self.windows.insert(key, Entry::default()).is_none() {
            self.order.push(key);
        }
    }

    fn remove(&mut self, key: u32) {
        self.windows.remove(&key);
        self.order.retain(|open| *open != key);
    }

    fn snapshot(&self) -> Vec<Toplevel> {
        self.order
            .iter()
            .filter_map(|key| {
                let entry = self.windows.get(key)?;
                Some(Toplevel {
                    id: ToplevelId(*key),
                    identifier: entry.identifier.clone(),
                    title: entry.title.clone(),
                    app_id: entry.app_id.clone(),
                })
            })
            .collect()
    }
}

impl Watcher {
    /// Publishes the whole list. The protocol batches a window's changes behind its own `done`, so a title
    /// being retyped one keystroke at a time is one publish per commit rather than one per event.
    fn publish(&self) {
        let snapshot = self.state.snapshot();
        *LATEST.lock().unwrap() = snapshot.clone();
        let mut handlers = HANDLERS.lock().unwrap();
        handlers.retain_mut(|registration| {
            if !registration.interest.alive() {
                return false;
            }
            (registration.handler)(&snapshot);
            true
        });
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for Watcher {
    wayland_client::event_created_child!(Watcher, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);

    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } => {
                state.state.add(toplevel.id().protocol_id())
            }
            ext_foreign_toplevel_list_v1::Event::Finished => state.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for Watcher {
    fn event(
        state: &mut Self,
        proxy: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = proxy.id().protocol_id();
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => {
                state.state.windows.entry(key).or_default().identifier = identifier
            }
            ext_foreign_toplevel_handle_v1::Event::Title { title } => {
                state.state.windows.entry(key).or_default().title = title
            }
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                state.state.windows.entry(key).or_default().app_id = app_id
            }
            ext_foreign_toplevel_handle_v1::Event::Done => state.publish(),
            // The window is gone and the handle is inert; destroying it is this client's half of the exchange.
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                state.state.remove(key);
                proxy.destroy();
                state.publish();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Watcher {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The half of "nothing runs unless something is asking for it" that a lazy start does not give: the
    /// watcher has to *stop* when the last registration is retired, and say so again so the next [`watch`]
    /// starts a fresh one rather than registering with a thread on its way out.
    ///
    /// Skipped under `HOGAR_SHELL_WAYLAND_LIVE`, where the registry is not this test's to reason about: the live
    /// tests here and in `capture` register with a real watcher, so "nothing is registered" is false through no
    /// fault of the code, and emptying the registry to make it true would retire the watcher out from under
    /// them.
    #[test]
    fn the_watcher_lives_exactly_as_long_as_its_registrations() {
        if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_ok() {
            eprintln!("a real watcher holds the registry in a live run; skipping");
            return;
        }
        *WATCHING.lock().unwrap() = true;

        let interest = Interest::new();
        HANDLERS.lock().unwrap().push(Registration {
            interest: interest.clone(),
            handler: Box::new(|_| {}),
        });
        assert!(anyone_listening(), "a live registration is listening");
        assert!(!retire(), "something is registered, so the watcher stays");

        // Retired by whoever registered, and dropped without ever being called again — which is the whole
        // reason the answer lives beside the handler instead of in what it returns.
        interest.retire();
        assert!(!anyone_listening(), "a retired registration was kept");
        assert!(retire(), "nothing is registered, so nothing needs it");
        assert!(
            !*WATCHING.lock().unwrap(),
            "the flag has to say 'no' again, or no later watch could start one"
        );
    }

    /// Captured from a live Hyprland 0.56.1 session, identifiers included: they are the `stableId` values
    /// `hyprctl clients` reported for the same windows at the same moment.
    fn open_windows() -> State {
        let mut state = State::default();
        for (key, identifier, app_id, title) in [
            (20, "1800000d", "kitty", "nvim"),
            (21, "1800000b", "code", "README.md - Visual Studio Code"),
            (22, "18000008", "helium", "Chat - Helium"),
        ] {
            state.add(key);
            let entry = state.windows.get_mut(&key).unwrap();
            entry.identifier = identifier.to_string();
            entry.app_id = app_id.to_string();
            entry.title = title.to_string();
        }
        state
    }

    #[test]
    fn the_list_keeps_the_order_the_compositor_announced() {
        let state = open_windows();
        assert_eq!(
            state
                .snapshot()
                .iter()
                .map(|w| w.app_id.as_str())
                .collect::<Vec<_>>(),
            vec!["kitty", "code", "helium"]
        );
    }

    /// A window closing must leave the rest in place — the bug a `HashMap` order hides until the day a list
    /// reorders itself under the pointer.
    #[test]
    fn a_closed_window_leaves_the_others_where_they_were() {
        let mut state = open_windows();
        state.remove(21);
        assert_eq!(
            state
                .snapshot()
                .iter()
                .map(|w| w.app_id.as_str())
                .collect::<Vec<_>>(),
            vec!["kitty", "helium"]
        );
        // Removing something that is not there is what a second `closed` would be, and it must not panic.
        state.remove(21);
        assert_eq!(state.snapshot().len(), 2);
    }

    /// Reopening the same protocol id — the compositor reuses them — must not double an entry in the order.
    #[test]
    fn a_reused_handle_is_one_entry() {
        let mut state = open_windows();
        state.add(20);
        assert_eq!(state.snapshot().len(), 3);
    }

    /// The identifier is what a reading taken here is matched against one taken over IPC. If it ever arrives
    /// empty the join silently becomes "everything matches everything".
    #[test]
    fn every_window_carries_the_identifier_the_join_is_made_on() {
        assert!(
            open_windows()
                .snapshot()
                .iter()
                .all(|w| !w.identifier.is_empty())
        );
    }

    /// The half no fixture can prove: that this reads a real compositor, and that what it reads agrees with
    /// what that compositor says about itself.
    ///
    /// `HOGAR_SHELL_WAYLAND_LIVE=1 cargo test -p platform-wayland toplevels -- --nocapture --test-threads=1`
    #[test]
    fn the_watcher_lists_the_windows_that_are_open() {
        use std::sync::mpsc;
        use std::time::Duration;

        if std::env::var("HOGAR_SHELL_WAYLAND_LIVE").is_err() {
            eprintln!(
                "set HOGAR_SHELL_WAYLAND_LIVE to list the real compositor's windows; skipping"
            );
            return;
        }

        let (published, changes) = mpsc::channel();
        let interest = Interest::new();
        assert!(
            watch(&interest, move |windows: &[Toplevel]| {
                let _ = published.send(windows.to_vec());
            }),
            "the compositor advertises ext-foreign-toplevel-list-v1 but the watcher would not start"
        );

        // One publish per window as the compositor announces them; the last one to arrive is the whole list.
        let mut listed = Vec::new();
        while let Ok(windows) = changes.recv_timeout(Duration::from_millis(500)) {
            listed = windows;
        }
        eprintln!("{} windows: {listed:#?}", listed.len());

        assert!(
            !listed.is_empty(),
            "this test is running in a terminal, which is itself a window"
        );
        assert!(
            listed.iter().all(|w| !w.identifier.is_empty()),
            "an empty identifier would make every match against IPC succeed"
        );
        let identifiers: std::collections::HashSet<&str> =
            listed.iter().map(|w| w.identifier.as_str()).collect();
        assert_eq!(
            identifiers.len(),
            listed.len(),
            "the protocol promises the identifier is unique, and the join depends on it"
        );
    }
}
