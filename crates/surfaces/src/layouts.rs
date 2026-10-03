//! The layout store the running shell owns, and the one way anything outside the surface pass reaches it.
//!
//! The store is created at startup and lives on the driver thread, because that is where the windows that draw from it are built and where the commands that change it run. Published here so everything that is not the startup code can reach it: the `layout` IPC verbs, the lock session opener, which resolves the lock layer the moment a lock is taken, and an edit mode.
//!
//! **Only the shell's driver thread has one.** A `hogar-shell layout …` invocation that arrives in the CLI process finds nothing here, which is why the verbs that change a layout are sent to the shell rather than answered locally while the ones that only read answer from the files. That distinction is the whole reason this is a thread-local rather than a global: a second process cannot borrow a store that is not its own, and the absence is an error that says so rather than a silently empty answer (F-10.29).
//!
//! **Every change is one transaction, redrawn and then written.** [`edit`] and [`commit`] commit, bring the screen in line with the result and ask for the write; the write itself waits out [`SETTLE`], so a burst of edits — a drag committing per frame, a script running ten lines — costs one file write rather than one each. [`revision`] moves with every change, so a reactive reader of the store runs again after each one.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use telar::{ReadSignal, RwSignal, signal};

use layout::{
    BUILT_IN, Layout, LayoutId, LayoutOp, LayoutStore, Placed, SETTLE, StoreError, Transaction,
};
use util::report::{Finding, Message, Report};

/// The name the first edit to the built-in layout forks it under.
const FORKED: &str = "custom";

/// The store and the pass that draws from it, as startup leaves them.
struct Live {
    store: Rc<RefCell<LayoutStore>>,
    /// Brings the screen in line with the store again. Every edit calls it: a layout whose effect the user cannot see is one they have no way to edit.
    redraw: Rc<dyn Fn()>,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    /// Whether a write is already waiting for the edits to stop, so a burst arms one timer rather than one per edit.
    static WAITING: Cell<bool> = const { Cell::new(false) };
    static REVISION: RwSignal<u64> = telar::detached(|| signal(0));
}

/// Publishes the store the shell draws from, and the pass that redraws from it. Called once, on the driver thread.
pub fn install(store: Rc<RefCell<LayoutStore>>, redraw: Rc<dyn Fn()>) {
    LIVE.with(|live| *live.borrow_mut() = Some(Live { store, redraw }));
    bump();
}

/// Moves with every change to the store: a commit, an undo, a redo, and a reload, which is also how `layout use` arrives. Read inside an effect or a build, it runs that again after each one.
///
/// It says the store changed, not that the screen did: the windows follow once the redraw has run, and [`crate::reconcile::desktops`] is the reactive answer to what they show.
pub fn revision() -> ReadSignal<u64> {
    REVISION.with(|revision| revision.read_only())
}

/// Where the user's layouts live. One directory, one file per layout, beside `config.toml`.
pub fn dir() -> PathBuf {
    util::paths::config_dir().join("layouts")
}

/// The layout file an edit written by hand belongs in: the active layout's, or the one the first edit of the built-in layout forks to. Named without reading the store, so `config check` can say it with no shell running.
pub fn file_for_edits() -> PathBuf {
    let name = services::state::get()
        .layout
        .filter(|name| name != BUILT_IN)
        .unwrap_or_else(|| FORKED.to_string());
    dir().join(format!("{name}.toml"))
}

/// Reads the store the shell is drawing from. `None` outside the shell process, where there is none.
pub fn read<R>(f: impl FnOnce(&LayoutStore) -> R) -> Option<R> {
    let store = store()?;
    Some(f(&store.borrow()))
}

/// Makes one edit to the layout being drawn: `plan` says what to do to the layout it will be done to, and hands back whatever the caller needs to describe what it did.
///
/// The plan is where the layout is in hand, so it is also the only place that can answer what an edit chose — the id a new instance was given, which of two things an ambiguous name turned out to name. That answer comes back beside the operations rather than through a cell the caller reads afterwards.
///
/// The transaction is committed whole or not at all, so a plan that cannot be carried out leaves the layout as it was rather than half changed.
pub fn edit<R>(
    label: &str,
    plan: impl FnOnce(&Layout, &LayoutId) -> Result<(Vec<LayoutOp>, R), String>,
) -> Result<R, String> {
    change(|store| {
        let id = editable(store)?;
        let (ops, said) = {
            let layout = store
                .get(&id)
                .ok_or_else(|| StoreError::Unknown(id.clone()).message())?;
            plan(layout, &id).map_err(Message::verbatim)?
        };
        store
            .commit(Transaction::new(label, id, ops))
            .map_err(|why| why.message())?;
        Ok(said)
    })
    .map_err(|why| why.english())
}

/// Commits a transaction prepared elsewhere — an edit mode's gesture, built while it previewed. One made for the layout being drawn lands where [`edit`] would put it, so while that is the built-in layout the first commit forks it.
pub fn commit(transaction: Transaction) -> Result<(), Message> {
    change(|store| {
        let layout = match transaction.layout == *store.active_id() {
            true => editable(store)?,
            false => transaction.layout.clone(),
        };
        store
            .commit(Transaction {
                layout,
                ..transaction
            })
            .map_err(|why| why.message())
    })
}

/// Takes back the last committed transaction, whatever made it, and answers with what it was called.
pub fn undo() -> Result<String, Message> {
    change(|store| store.undo().map_err(|why| why.message()))
}

pub fn redo() -> Result<String, Message> {
    change(|store| store.redo().map_err(|why| why.message()))
}

/// Shows `draft` in place of the active layout while it differs from what the store holds, and the store's own layout again once it does not: how an edit mode's undecided gesture reaches the screen without being committed ([`crate::reconcile::preview`]).
pub fn preview(draft: &Layout) {
    let previewed = read(|store| {
        let differs = store.active() != draft;
        if differs {
            crate::reconcile::preview(draft, store.all());
        }
        differs
    });
    if previewed != Some(true) {
        crate::reconcile::end_preview();
    }
}

/// Reads the layout files again, and which one is active with it, keeping every layout its file agrees with (F-10.34). The watcher fingerprints the layout files and `layout use` asks for a reload, so this is how both reach the store; the caller redraws.
pub fn reload() {
    let Some(store) = store() else {
        return;
    };
    let report = {
        let mut store = store.borrow_mut();
        let mut report = store.reload();
        select_active(&mut store, &mut report);
        report
    };
    report_problems(&report);
    bump();
}

/// Writes whatever the store is holding, without waiting for it to settle. For the way out: a shell shutting down must not take an edit with it.
pub fn flush() {
    let Some(store) = store() else {
        return;
    };
    if !store.borrow().has_unsaved() {
        return;
    }
    report_problems(&store.borrow_mut().flush());
}

/// Makes the store draw the layout this installation chose, or say why it cannot.
///
/// The name lives in `state.json` because it is a decision about this machine rather than a description of one (TA-2), and `layout use` is the only thing that writes it. A name nothing answers to is reported and the built-in layout stands in — the same shape as every other unknown id in this shell: say what was asked for, draw something anyway.
///
/// **The copy that last worked comes first, though.** A layout whose file has gone missing or stopped parsing is exactly what `layouts/.last-good/` is kept for, so it is drawn from there and said so, rather than the user losing their whole desktop to one bad save. Nothing is written back: the file is theirs to fix, and the next start rescues it again until they do.
pub fn select_active(store: &mut LayoutStore, report: &mut Report) {
    let Some(name) = services::state::get().layout else {
        return;
    };
    let path = dir().join(format!("{name}.toml"));
    let id = LayoutId::new(&name);
    if store.use_layout(&id).is_ok() {
        return;
    }
    if store.restore_last_good(&id) && store.use_layout(&id).is_ok() {
        report.warn(Finding::new(
            path,
            "layout",
            util::message!("finding.last_good"),
        ));
        return;
    }
    report.error(Finding::new(
        path,
        "layout",
        util::message!("finding.no_layout", name = name),
    ));
}

/// Says what a layout could not answer, in the one live notice the config's own problems already use — a layout that does not resolve is the same kind of news as a config that does not parse, and a user reading one place should see both.
pub fn report_problems(report: &Report) {
    if report.is_clean() {
        return;
    }
    tracing::warn!("the layout was not fully applied:\n{}", report.render());
}

fn store() -> Option<Rc<RefCell<LayoutStore>>> {
    LIVE.with(|live| live.borrow().as_ref().map(|it| Rc::clone(&it.store)))
}

/// Why a change to the layouts could not be made: no running shell owns a store to make it in.
pub fn no_store() -> Message {
    util::message!("finding.no_store")
}

fn bump() {
    REVISION.with(|revision| revision.update(|n| *n = n.wrapping_add(1)));
}

/// Runs one change against the store, then brings the screen in line with it and asks for the write.
///
/// A change that failed after the built-in layout forked still changed the store — the copy is the active layout now, and `state.json` says so — so it is redrawn and written like one that went through. Whatever the change stopped placing is forgotten before the redraw, wherever state is kept by id ([`forget_gone`]).
fn change<R>(f: impl FnOnce(&mut LayoutStore) -> Result<R, Message>) -> Result<R, Message> {
    let live = LIVE.with(|live| {
        live.borrow()
            .as_ref()
            .map(|it| (Rc::clone(&it.store), Rc::clone(&it.redraw)))
    });
    let Some((store, redraw)) = live else {
        return Err(no_store());
    };
    // The borrow ends with the block, so the redraw below — which reads the store to plan the screen — is not inside it.
    let (done, changed, gone) = {
        let mut store = store.borrow_mut();
        let before = store.active_id().clone();
        let placed = Placed::of(store.active(), store.all());
        let done = f(&mut store);
        let changed = done.is_ok() || *store.active_id() != before;
        let gone = placed.gone(&Placed::of(store.active(), store.all()));
        (done, changed, gone)
    };
    if changed {
        forget_gone(&gone);
        redraw();
        bump();
        write_once_settled();
    }
    done
}

/// Forgets what every owner of state kept by id holds for what the layout no longer places: each instance's stores, the copies of a repeated child's included, and what the areas keep across rebuilds (F-3.4).
pub(crate) fn forget_gone(gone: &Placed) {
    if gone.is_empty() {
        return;
    }
    ui::host::forget_where(|held| {
        gone.instances
            .contains(&layout::InstanceId::new(held.as_str()).template())
    });
    crate::area::forget_gone(gone);
}

/// The layout an edit lands in: the one being drawn, or a copy of the built-in one under a name of the user's own.
///
/// The shipped layout is read-only (TA-7), so the first edit to it forks. The copy becomes the active layout in the same breath, `state.json` included — which layout is drawn is a decision about this installation rather than a description of one, and `layout use` writes it the same way.
fn editable(store: &mut LayoutStore) -> Result<LayoutId, Message> {
    let active = store.active_id().clone();
    if active.as_str() != BUILT_IN {
        return Ok(active);
    }
    let name = free_name(store);
    store
        .fork(&active, name.clone())
        .map_err(|why| why.message())?;
    store.use_layout(&name).map_err(|why| why.message())?;
    services::state::update(|state| state.layout = Some(name.to_string()));
    tracing::info!("the built-in layout is read-only, so this edit forked it to `{name}`");
    Ok(name)
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
    let Some(store) = store() else {
        return;
    };
    if !store.borrow().has_unsaved() {
        return;
    }
    if !store.borrow().is_settled(Instant::now()) {
        write_once_settled();
        return;
    }
    report_problems(&store.borrow_mut().flush());
}

#[cfg(test)]
mod tests {
    use layout::{Area, AreaId, AreaKind, LayerKind, Site};

    use super::*;

    /// A store of the built-in layout saved as `mine`, installed as the one this thread's shell owns, and how many times it was redrawn.
    fn shell_with(test: &str, active: &str) -> (Rc<RefCell<LayoutStore>>, Rc<Cell<usize>>) {
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join(format!("live-layouts-{test}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a layouts directory");
        std::fs::write(
            dir.join("mine.toml"),
            toml::to_string_pretty(&layout::built_in()).expect("the layout serializes"),
        )
        .expect("a layout to edit");
        let (mut store, report) = LayoutStore::load(&dir);
        assert!(report.is_clean(), "{}", report.render());
        store
            .use_layout(&LayoutId::new(active))
            .expect("the store holds it");
        let store = Rc::new(RefCell::new(store));
        let redrawn = Rc::new(Cell::new(0));
        install(Rc::clone(&store), {
            let redrawn = Rc::clone(&redrawn);
            Rc::new(move || redrawn.set(redrawn.get() + 1))
        });
        (store, redrawn)
    }

    fn empty_grid(id: &str) -> LayoutOp {
        LayoutOp::InsertArea {
            site: Site::everywhere(LayerKind::Desktop),
            index: 0,
            area: Box::new(Area {
                id: AreaId::new(id),
                kind: Some(AreaKind::Grid {
                    rect: None,
                    cell: None,
                    gap: None,
                    anchor: None,
                }),
                ..Area::default()
            }),
        }
    }

    fn areas_on_desktop(store: &LayoutStore) -> usize {
        store.active().outputs[0].layers.desktop.areas.len()
    }

    /// Whatever reads the store reactively — an edit mode's outlines, its undo button — runs again after every change to it, whoever made the change.
    #[test]
    fn every_change_to_the_store_moves_its_revision() {
        let (store, redrawn) = shell_with("revision", "mine");
        let seen: Rc<RefCell<Vec<u64>>> = Rc::default();
        let _watching = telar::effect({
            let seen = Rc::clone(&seen);
            move || seen.borrow_mut().push(revision().get())
        });
        let moved = |seen: &Rc<RefCell<Vec<u64>>>, what: &str, count: usize| {
            assert_eq!(seen.borrow().len(), count, "{what} moved the revision once");
        };
        let before = areas_on_desktop(&store.borrow());

        commit(Transaction::new(
            "add a grid",
            LayoutId::new("mine"),
            vec![empty_grid("grid")],
        ))
        .expect("it commits");
        moved(&seen, "a commit", 2);
        assert_eq!(areas_on_desktop(&store.borrow()), before + 1);

        edit("add another", |_, _| Ok((vec![empty_grid("grid-2")], ()))).expect("it edits");
        moved(&seen, "an edit", 3);

        assert_eq!(undo().as_deref(), Ok("add another"));
        moved(&seen, "an undo", 4);
        assert_eq!(redo().as_deref(), Ok("add another"));
        moved(&seen, "a redo", 5);

        reload();
        moved(&seen, "a reload", 6);
        assert_eq!(
            areas_on_desktop(&store.borrow()),
            before + 2,
            "a reload keeps the edits no file contradicts"
        );
        assert_eq!(
            redrawn.get(),
            4,
            "every change but the reload redraws itself"
        );

        assert!(
            edit::<()>("refused", |_, _| Err("no".to_string())).is_err(),
            "the plan refused"
        );
        moved(&seen, "a refused edit never", 6);
        assert_eq!(redrawn.get(), 4);
    }

    /// A transaction an edit mode built while the built-in layout was drawn lands in the user's own copy, the way an edit through the verbs does.
    #[test]
    fn a_transaction_for_the_built_in_layout_forks_it() {
        let (store, _) = shell_with("commit-fork", BUILT_IN);

        commit(Transaction::new(
            "add a grid",
            LayoutId::new(BUILT_IN),
            vec![empty_grid("grid")],
        ))
        .expect("it commits to a copy");

        let active = store.borrow().active_id().clone();
        assert_ne!(active.as_str(), BUILT_IN);
        assert_eq!(
            store
                .borrow()
                .get(&active)
                .map(|it| it.outputs[0].layers.desktop.areas.len()),
            Some(layout::built_in().outputs[0].layers.desktop.areas.len() + 1)
        );
    }

    static TAPS: ui::host::InstanceStore<u32> = ui::host::InstanceStore::new(|| 0);

    fn remove_clock() -> LayoutOp {
        LayoutOp::DeleteInstance {
            spot: layout::Spot {
                site: Site::everywhere(LayerKind::Top),
                area: AreaId::new("bar-top"),
                group: layout::GroupId::new("center"),
            },
            id: layout::InstanceId::new("clock"),
        }
    }

    /// F-3.4: an instance a commit, an undo or a redo takes out of the layout leaves nothing in the stores kept by id, and one that comes back or stays keeps what it had.
    #[test]
    fn whatever_takes_an_instance_out_of_the_layout_forgets_it() {
        let _shell = shell_with("forget", "mine");
        let (clock, notes) = (
            ui::host::InstanceId::new("clock"),
            ui::host::InstanceId::new("notes"),
        );
        TAPS.set(&clock, 3);
        TAPS.set(&notes, 5);

        edit("remove the clock", |_, _| Ok((vec![remove_clock()], ()))).expect("it edits");
        assert_eq!(TAPS.get(&clock), 0, "the commit forgot it");
        assert_eq!(TAPS.get(&notes), 5, "and nothing else");

        TAPS.set(&clock, 7);
        undo().expect("it undoes");
        assert_eq!(TAPS.get(&clock), 7, "putting it back forgets nothing");

        redo().expect("it redoes");
        assert_eq!(TAPS.get(&clock), 0, "the redo took it out again");
    }
}
