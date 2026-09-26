//! The layout store the running shell owns, and the one way anything outside the surface pass reaches it.
//!
//! The store is created by `setup_shell` and lives on the driver thread, because that is where the windows that draw from it are built and where the commands that change it run. Published here so the two callers that are not that function can reach it: the `layout` IPC verbs, and the lock session opener, which resolves the lock layer the moment a lock is taken.
//!
//! **Only the shell's driver thread has one.** A `hogar-shell layout …` invocation that arrives in the CLI process finds nothing here, which is why the verbs that change a layout are sent to the shell rather than answered locally (see `main.rs`) while the ones that only read answer from the files. That distinction is the whole reason this is a thread-local rather than a global: a second process cannot borrow a store that is not its own, and the absence is an error that says so rather than a silently empty answer (F-10.29).
//!
//! **Every change is one transaction, redrawn and then written.** [`edit`] commits, brings the screen in line with the result and asks for the write; the write itself waits out [`SETTLE`], so a burst of edits — a drag committing per frame, a script running ten lines — costs one file write rather than one each.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use layout::{BUILT_IN, Layout, LayoutId, LayoutOp, LayoutStore, SETTLE, StoreError, Transaction};

/// The name the first edit to the built-in layout forks it under.
const FORKED: &str = "custom";

/// The store and the pass that draws from it, as `setup_shell` leaves them.
struct Live {
    store: Rc<RefCell<LayoutStore>>,
    /// Brings the screen in line with the store again. Every edit calls it: a layout whose effect the user cannot see is one they have no way to edit.
    redraw: Rc<dyn Fn()>,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    /// Whether a write is already waiting for the edits to stop, so a burst arms one timer rather than one per edit.
    static WAITING: Cell<bool> = const { Cell::new(false) };
}

/// Publishes the store the shell draws from, and the pass that redraws from it. Called once, on the driver thread.
pub(crate) fn install(store: Rc<RefCell<LayoutStore>>, redraw: Rc<dyn Fn()>) {
    LIVE.with(|live| *live.borrow_mut() = Some(Live { store, redraw }));
}

/// Where the user's layouts live. One directory, one file per layout, beside `config.toml`.
pub(crate) fn dir() -> PathBuf {
    util::paths::config_dir().join("layouts")
}

/// Reads the store the shell is drawing from. `None` outside the shell process, where there is none.
pub(crate) fn read<R>(f: impl FnOnce(&LayoutStore) -> R) -> Option<R> {
    let store = LIVE.with(|live| live.borrow().as_ref().map(|it| Rc::clone(&it.store)))?;
    Some(f(&store.borrow()))
}

/// Makes one edit to the layout being drawn: `plan` says what to do to the layout it will be done to, and hands back whatever the caller needs to describe what it did.
///
/// The plan is where the layout is in hand, so it is also the only place that can answer what an edit chose — the id a new instance was given, which of two things an ambiguous name turned out to name. That answer comes back beside the operations rather than through a cell the caller reads afterwards.
///
/// The transaction is committed whole or not at all, so a plan that cannot be carried out leaves the layout as it was rather than half changed.
pub(crate) fn edit<R>(
    label: &str,
    plan: impl FnOnce(&Layout, &LayoutId) -> Result<(Vec<LayoutOp>, R), String>,
) -> Result<R, String> {
    change(|store| {
        let id = editable(store)?;
        let (ops, said) = {
            let layout = store
                .get(&id)
                .ok_or_else(|| StoreError::Unknown(id.clone()).to_string())?;
            plan(layout, &id)?
        };
        store
            .commit(Transaction::new(label, id.clone(), ops))
            .map_err(|why| why.to_string())?;
        Ok(said)
    })
}

/// Takes back the last committed transaction, whatever made it, and answers with what it was called.
pub(crate) fn undo() -> Result<String, String> {
    change(|store| store.undo().map_err(|why| why.to_string()))
}

pub(crate) fn redo() -> Result<String, String> {
    change(|store| store.redo().map_err(|why| why.to_string()))
}

/// Writes whatever the store is holding, without waiting for it to settle. For the way out: a shell shutting down must not take an edit with it.
pub(crate) fn flush() {
    let Some(store) = LIVE.with(|live| live.borrow().as_ref().map(|it| Rc::clone(&it.store)))
    else {
        return;
    };
    if !store.borrow().has_unsaved() {
        return;
    }
    crate::report_layout_problems(&store.borrow_mut().flush());
}

/// Runs one change against the store, then brings the screen in line with it and asks for the write.
fn change<R>(f: impl FnOnce(&mut LayoutStore) -> Result<R, String>) -> Result<R, String> {
    let live = LIVE.with(|live| {
        live.borrow()
            .as_ref()
            .map(|it| (Rc::clone(&it.store), Rc::clone(&it.redraw)))
    });
    let Some((store, redraw)) = live else {
        return Err("no running shell owns a layout store".to_string());
    };
    // The borrow ends with the statement, so the redraw below — which reads the store to plan the screen — is not inside it.
    let done = f(&mut store.borrow_mut())?;
    redraw();
    write_once_settled();
    Ok(done)
}

/// The layout an edit lands in: the one being drawn, or a copy of the built-in one under a name of the user's own.
///
/// The shipped layout is read-only (TA-7), so the first edit to it forks. The copy becomes the active layout in the same breath, `state.json` included — which layout is drawn is a decision about this installation rather than a description of one, and `layout use` writes it the same way.
fn editable(store: &mut LayoutStore) -> Result<LayoutId, String> {
    let active = store.active_id().clone();
    if active.as_str() != BUILT_IN {
        return Ok(active);
    }
    let name = free_name(store);
    store.fork(&active, name.clone()).map_err(as_message)?;
    store.use_layout(&name).map_err(as_message)?;
    services::state::update(|state| state.layout = Some(name.to_string()));
    tracing::info!("the built-in layout is read-only, so this edit forked it to `{name}`");
    Ok(name)
}

fn as_message(why: StoreError) -> String {
    why.to_string()
}

/// A name no layout has yet, for the copy the first edit forks. The store knows every layout file in the directory, so a free name here is a free file there.
fn free_name(store: &LayoutStore) -> LayoutId {
    let taken = |name: &str| store.get(&LayoutId::new(name)).is_some();
    if !taken(FORKED) {
        return LayoutId::new(FORKED);
    }
    let mut nth = 2;
    loop {
        let name = format!("{FORKED}-{nth}");
        if !taken(&name) {
            return LayoutId::new(name);
        }
        nth += 1;
    }
}

/// Asks for the store to be written once the edits have stopped for [`SETTLE`].
///
/// One timer at a time, re-armed while the layout is still moving: a drag commits a transaction per frame, and a write per frame would put the disk in the middle of the gesture (F-10.5). Outside a driver loop — a test, a CLI process — the timer never runs and [`flush`] on the way out is what writes.
fn write_once_settled() {
    if WAITING.get() {
        return;
    }
    WAITING.set(true);
    platform_wayland::timeout(SETTLE, on_settled);
}

fn on_settled() {
    WAITING.set(false);
    let Some(store) = LIVE.with(|live| live.borrow().as_ref().map(|it| Rc::clone(&it.store)))
    else {
        return;
    };
    if !store.borrow().has_unsaved() {
        return;
    }
    if !store.borrow().is_settled(Instant::now()) {
        write_once_settled();
        return;
    }
    crate::report_layout_problems(&store.borrow_mut().flush());
}
