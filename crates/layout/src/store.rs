//! The layouts the shell is holding, and what has been done to them.
//!
//! The komponents their groups use live here too, read from `components/` beside `layouts/`, reloaded and written on the same terms as a layout, though outside the undo history: a komponent is saved, not edited.
//!
//! One owner. Edit modes, popovers, the settings window and `hogar-shell layout …` are all clients of this store rather than writers of the files, which is what lets a single undo stack cover a drag, a scrubbed value and a scripted command alike: each of them commits a [`Transaction`], and the store keeps the operations that take it back.
//!
//! **Persistence is deliberate, not automatic.** A committed transaction marks its layout dirty and nothing more; [`LayoutStore::flush`] is what writes. A drag commits on every frame it moves, and writing a file per frame would put the disk in the middle of a gesture. The caller waits for the change to settle ([`SETTLE`]) and flushes then, and flushes outright when an edit mode exits or the shell shuts down, so nothing is ever lost — only deferred.
//!
//! Writes go through `util::writer`, the shell's one ordered writer, so a flush that overtakes another cannot land out of order. A layout that resolved and validated cleanly is also copied to `layouts/.last-good/`, which is what a startup whose own file has gone missing or stopped parsing falls back to.
//!
//! **Trust follows the text.** A write that would put in a file of the user's own a line, a command or an address that a bundle brought and the user has not trusted is refused whole, whatever made it — a gesture, a menu row, a komponent saved or detached, an IPC verb — since that file runs whatever it holds ([`crate::trust::carried`], DEC-30).
//!
//! **A reload keeps whatever the files do not contradict.** [`LayoutStore::reload`] compares each file against what the store last read from it or last wrote to it, so the shell's own flush coming back through the watcher is not read as somebody else's edit: that layout keeps the edits it has not written yet and the history that would take them back. Only a layout somebody rewrote is taken from the file, and that layout's undo entries go with the bytes they described — operations replayed against content that has since been rewritten would restore something that no longer means what it meant.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use util::report::{Finding, Message, Report};

use crate::library::{Library, komponent_path};
use crate::model::*;
use crate::ops::{LayoutOp, OpError, apply_all};
use crate::resolve::layout_path;
use crate::trust::{
    Carried, Item, granting, komponent_chains, komponent_items, layout_chains, layout_items,
};

/// How long a layout has to stop changing before it is worth writing. A drag commits a transaction per frame; this is what keeps the disk out of the gesture.
pub const SETTLE: Duration = Duration::from_millis(250);

/// One committed edit, whatever made it.
#[derive(Clone, Debug)]
pub struct Transaction {
    /// What the user would call this edit, for the undo entry.
    pub label: String,
    pub layout: LayoutId,
    pub ops: Vec<LayoutOp>,
}

impl Transaction {
    pub fn new(label: impl Into<String>, layout: LayoutId, ops: Vec<LayoutOp>) -> Self {
        Self {
            label: label.into(),
            layout,
            ops,
        }
    }
}

/// The labels of the undo history: what each undo would take back, the oldest first, so the last is where the layout is now; and what each redo would put back, the next one first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    pub undo: Vec<String>,
    pub redo: Vec<String>,
}

impl History {
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }
}

/// A committed transaction and the operations that undo it.
#[derive(Clone, Debug)]
struct Done {
    label: String,
    layout: LayoutId,
    back: Vec<LayoutOp>,
}

#[derive(Debug)]
pub enum StoreError {
    ReadOnly(LayoutId),
    Unknown(LayoutId),
    Failed(OpError),
    NothingToUndo,
    NothingToRedo,
    /// The store is the recovery one: it holds the built-in layout alone and writes nothing.
    Safe,
    /// A komponent of that name exists already.
    KomponentExists(KomponentId),
    /// The edit would copy what a bundle runs, and the user has not trusted, into a file of the user's own ([`crate::trust::carried`]).
    Carried(Box<Carried>),
    /// The edit would write a line that grants trust into an action chain, where nothing may run it ([`crate::trust::grants_trust`]): what the key is and what the line says.
    GrantsTrust {
        key: String,
        line: String,
    },
}

impl StoreError {
    /// Why the store refused, in no language yet.
    pub fn message(&self) -> Message {
        match self {
            StoreError::ReadOnly(id) => util::message!("finding.built_in_read_only", id = id),
            StoreError::Unknown(id) => util::message!("finding.unknown_layout", id = id),
            StoreError::Failed(error) => error.message(),
            StoreError::NothingToUndo => util::message!("finding.nothing_to_undo"),
            StoreError::NothingToRedo => util::message!("finding.nothing_to_redo"),
            StoreError::Safe => util::message!("finding.safe_layout"),
            StoreError::KomponentExists(id) => {
                util::message!("finding.komponent_exists", komponent = id)
            }
            StoreError::Carried(carried) => carried.message(),
            StoreError::GrantsTrust { key, line } => {
                util::message!("finding.trust_in_action_written", key = key, line = line)
            }
        }
    }
}

/// In English, as the command line says it.
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message().english())
    }
}

impl std::error::Error for StoreError {}

impl From<OpError> for StoreError {
    fn from(error: OpError) -> Self {
        StoreError::Failed(error)
    }
}

/// The name of the layout that ships with the shell. It is never written to, so a user who has broken theirs always has one that works.
pub const BUILT_IN: &str = "default";

pub struct LayoutStore {
    dir: PathBuf,
    /// Where the komponents live: `components/` beside `dir`.
    components: PathBuf,
    library: Library,
    active: LayoutId,
    done: Vec<Done>,
    undone: Vec<Done>,
    dirty: BTreeSet<LayoutId>,
    /// Komponents saved and not written yet.
    dirty_komponents: BTreeSet<KomponentId>,
    changed_at: Option<Instant>,
    /// What each layout's file held when the store last read it, or what the store last put there. A reload compares the file against this, which is what tells the shell's own write apart from an edit somebody else made — by content, so two writes inside one tick of the filesystem's clock are still two different things (`config::fingerprint`).
    seen: BTreeMap<LayoutId, u64>,
    /// The same for each komponent's file.
    seen_komponents: BTreeMap<KomponentId, u64>,
    /// Whether this is the recovery store [`LayoutStore::safe`] builds. It holds no user layout, so an edit could only fork the built-in one over a file it cannot see; refusing is what keeps `--safe-layout` from writing over the layout it was started to rescue.
    safe: bool,
    /// What each last-good copy holds. [`LayoutStore::keep_last_good`] is called from the pass that plans the screen, which runs on every reload and every monitor change, so without this a copy nobody's layout changed would be rewritten — temp file, fsync and rename — each time.
    kept: BTreeMap<LayoutId, u64>,
}

impl LayoutStore {
    /// Reads every layout in `dir`, adding the built-in one, and every komponent in `components/` beside it. A file that does not parse is reported and skipped, so one broken layout or komponent does not cost the user the others.
    pub fn load(dir: impl Into<PathBuf>) -> (Self, Report) {
        let dir = dir.into();
        let components = components_beside(&dir);
        let mut report = Report::default();
        let mut library = Library::default();
        let mut seen = BTreeMap::new();
        library
            .layouts
            .insert(LayoutId::new(BUILT_IN), crate::built_in::layout());

        for (path, id) in layout_files(&dir, &mut report) {
            match read_layout(&path, id.as_str()) {
                Ok((layout, held)) => {
                    seen.insert(id, held);
                    library.layouts.insert(layout.id.clone(), layout);
                }
                Err(why) => report.error(Finding::new(path, "", Message::verbatim(why))),
            }
        }
        let mut seen_komponents = BTreeMap::new();
        for (path, id) in komponent_files(&components) {
            match read_komponent(&path) {
                Ok((komponent, held)) => {
                    seen_komponents.insert(id.clone(), held);
                    library.komponents.insert(id, komponent);
                }
                Err(unreadable) => report.error(*unreadable),
            }
        }

        let store = Self {
            dir,
            components,
            library,
            active: LayoutId::new(BUILT_IN),
            done: Vec::new(),
            undone: Vec::new(),
            dirty: BTreeSet::new(),
            dirty_komponents: BTreeSet::new(),
            changed_at: None,
            seen,
            seen_komponents,
            safe: false,
            kept: BTreeMap::new(),
        };
        (store, report)
    }

    /// Brings the store in line with the files, keeping everything the files do not contradict.
    ///
    /// For an edit that arrived from outside the shell — a layout or komponent file written by hand or by another tool. A layout whose file still holds what the store last read from it or last wrote to it is left exactly as it is, the edits it has not flushed yet included, so the shell's own write coming back through the watcher costs nothing; a layout somebody rewrote is taken from the file, and that layout's history goes with the bytes it described. A komponent is taken from its file on the same terms.
    ///
    /// A file that has stopped parsing is reported and the layout or komponent in memory stays on screen, which is the same rule a config that stops parsing follows: the last content that worked keeps drawing, and the user is told what is wrong rather than shown an empty screen.
    pub fn reload(&mut self) -> Report {
        let mut report = Report::default();
        let mut on_disk = BTreeSet::new();

        for (path, id) in layout_files(&self.dir, &mut report) {
            on_disk.insert(id.clone());
            match read_layout(&path, id.as_str()) {
                // The store already reflects this file, whichever of them wrote it last.
                Ok((_, held)) if self.seen.get(&id) == Some(&held) => {}
                Ok((layout, held)) => {
                    self.forget(&id);
                    self.dirty.remove(&id);
                    self.seen.insert(id.clone(), held);
                    self.library.layouts.insert(id, layout);
                }
                Err(why) => report.error(Finding::new(path, "", Message::verbatim(why))),
            }
        }

        // A layout whose file is gone is gone — unless the store is holding edits nothing has written yet, which the next flush puts back.
        let vanished: Vec<LayoutId> = self
            .library
            .layouts
            .keys()
            .filter(|id| !on_disk.contains(*id) && !self.dirty.contains(*id))
            .cloned()
            .collect();
        for id in vanished {
            self.library.layouts.remove(&id);
            self.seen.remove(&id);
            self.forget(&id);
        }

        let built_in = LayoutId::new(BUILT_IN);
        self.library
            .layouts
            .insert(built_in.clone(), crate::built_in::layout());
        if !self.library.layouts.contains_key(&self.active) {
            self.active = built_in;
        }
        if !self.safe {
            report.merge(self.reload_komponents());
        }
        report
    }

    fn reload_komponents(&mut self) -> Report {
        let mut report = Report::default();
        let mut on_disk = BTreeSet::new();
        for (path, id) in komponent_files(&self.components) {
            on_disk.insert(id.clone());
            match read_komponent(&path) {
                Ok((_, held)) if self.seen_komponents.get(&id) == Some(&held) => {}
                Ok((komponent, held)) => {
                    self.dirty_komponents.remove(&id);
                    self.seen_komponents.insert(id.clone(), held);
                    self.library.komponents.insert(id, komponent);
                }
                Err(unreadable) => report.error(*unreadable),
            }
        }
        self.library
            .komponents
            .retain(|id, _| on_disk.contains(id) || self.dirty_komponents.contains(id));
        self.seen_komponents.retain(|id, _| on_disk.contains(id));
        report
    }

    /// A store holding only the built-in layout and writing nothing, for `hogar-shell --safe-layout`.
    ///
    /// It refuses every edit rather than forking the built-in layout the way an ordinary store does. A recovery start cannot see the user's layouts, so a fork would pick a name from an empty set and write it over whichever file already has it — over the layout the flag was used to rescue. It holds no komponent either, since the built-in layout uses none.
    pub fn safe(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let mut library = Library::default();
        library
            .layouts
            .insert(LayoutId::new(BUILT_IN), crate::built_in::layout());
        Self {
            components: components_beside(&dir),
            dir,
            library,
            active: LayoutId::new(BUILT_IN),
            done: Vec::new(),
            undone: Vec::new(),
            dirty: BTreeSet::new(),
            dirty_komponents: BTreeSet::new(),
            changed_at: None,
            seen: BTreeMap::new(),
            seen_komponents: BTreeMap::new(),
            safe: true,
            kept: BTreeMap::new(),
        }
    }

    /// Whether this store is the recovery one, which is what a reply has to say instead of reporting an edit it never made.
    pub fn is_safe(&self) -> bool {
        self.safe
    }

    pub fn names(&self) -> impl Iterator<Item = &LayoutId> {
        self.library.layouts.keys()
    }

    pub fn get(&self, id: &LayoutId) -> Option<&Layout> {
        self.library.layouts.get(id)
    }

    /// Whether a layout or a file of the store's directory already has `id`, a file that does not read included: it is still the user's.
    pub fn is_taken(&self, id: &LayoutId) -> bool {
        self.get(id).is_some() || self.path_of(id).exists()
    }

    /// `base`, or else the first of `base-2`, `base-3` and on that [`Self::is_taken`] does not answer for.
    pub fn free_id(&self, base: &str) -> LayoutId {
        std::iter::once(base.to_string())
            .chain((2..).map(|nth| format!("{base}-{nth}")))
            .map(LayoutId::new)
            .find(|id| !self.is_taken(id))
            .expect("the counting runs out long after the names do")
    }

    /// Every layout and komponent the store holds: what a layout is resolved against.
    pub fn all(&self) -> &Library {
        &self.library
    }

    pub fn komponent(&self, id: &KomponentId) -> Option<&Komponent> {
        self.library.komponent(id)
    }

    /// Adds `komponent` under `id`, to be written with the next flush. Refused where a komponent of that name exists already, since the groups that use it would change under the user, and on the recovery store, which writes nothing.
    pub fn add_komponent(
        &mut self,
        id: KomponentId,
        komponent: Komponent,
    ) -> Result<(), StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        if self.library.komponents.contains_key(&id) {
            return Err(StoreError::KomponentExists(id));
        }
        self.admit_komponent(&id, &komponent)?;
        self.library.komponents.insert(id.clone(), komponent);
        self.dirty_komponents.insert(id);
        self.changed_at = Some(Instant::now());
        Ok(())
    }

    /// Puts `layout` in the store under its id, in place of one of that name, to be written with the next flush: a layout a bundle brought in. Its history goes, since the operations in it describe what it replaces. Refused for the built-in name and on the recovery store.
    pub fn put_layout(&mut self, layout: Layout) -> Result<(), StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        if layout.id.as_str() == BUILT_IN {
            return Err(StoreError::ReadOnly(layout.id));
        }
        let id = layout.id.clone();
        self.admit_layout(self.library.layouts.get(&id), &layout)?;
        self.forget(&id);
        self.library.layouts.insert(id.clone(), layout);
        self.mark(&id);
        Ok(())
    }

    /// Puts `komponent` in the store as `id`, in place of one of that name, to be written with the next flush: a komponent a bundle brought in. Refused on the recovery store.
    pub fn put_komponent(
        &mut self,
        id: KomponentId,
        komponent: Komponent,
    ) -> Result<(), StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        self.admit_komponent(&id, &komponent)?;
        self.library.komponents.insert(id.clone(), komponent);
        self.dirty_komponents.insert(id);
        self.changed_at = Some(Instant::now());
        Ok(())
    }

    /// Refuses a write that would leave the layout file running something it must not: a line that grants trust where it held none at that key, or, as the user's own, a text a bundle brought that the user has not trusted ([`crate::trust::carried`]). Every write but undo and redo passes here — those put back what the file held before, which it ran already.
    fn admit_layout(&self, before: Option<&Layout>, after: &Layout) -> Result<(), StoreError> {
        refuse_new_grants(
            before.map(layout_chains).unwrap_or_default(),
            layout_chains(after),
        )?;
        let trust = &self.library.trust;
        let had = before.map(|it| layout_items(it, trust)).unwrap_or_default();
        self.admit(&layout_path(&after.id), &had, &layout_items(after, trust))
    }

    /// The same for the komponent file `id`.
    fn admit_komponent(&self, id: &KomponentId, komponent: &Komponent) -> Result<(), StoreError> {
        let before = self.library.komponent(id);
        refuse_new_grants(
            before.map(komponent_chains).unwrap_or_default(),
            komponent_chains(komponent),
        )?;
        let trust = &self.library.trust;
        let had = before
            .map(|held| komponent_items(id, held, trust))
            .unwrap_or_default();
        self.admit(
            &komponent_path(id),
            &had,
            &komponent_items(id, komponent, trust),
        )
    }

    fn admit(&self, into: &str, before: &[Item], after: &[Item]) -> Result<(), StoreError> {
        match crate::trust::carried(&self.library, into, before, after) {
            Some(carried) => Err(StoreError::Carried(Box::new(carried))),
            None => Ok(()),
        }
    }

    /// Makes `trust` what resolution against this store holds back.
    pub fn set_trust(&mut self, trust: crate::trust::Trust) {
        self.library.trust = trust;
    }

    /// Takes back a komponent added since the last flush, for an edit that was to use it and was refused: one already written is the user's and stays. Answers whether there was one to take back.
    pub fn discard_komponent(&mut self, id: &KomponentId) -> bool {
        let unwritten = self.dirty_komponents.remove(id);
        if unwritten {
            self.library.komponents.remove(id);
        }
        unwritten
    }

    pub fn active(&self) -> &Layout {
        self.library
            .layouts
            .get(&self.active)
            .expect("the active layout is one the store holds")
    }

    pub fn active_id(&self) -> &LayoutId {
        &self.active
    }

    pub fn use_layout(&mut self, id: &LayoutId) -> Result<(), StoreError> {
        if !self.library.layouts.contains_key(id) {
            return Err(StoreError::Unknown(id.clone()));
        }
        self.active = id.clone();
        Ok(())
    }

    /// Copies a layout under a new name. The built-in one is read-only, so this is how the first edit to it becomes the user's own.
    pub fn fork(&mut self, from: &LayoutId, to: LayoutId) -> Result<(), StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        let mut copy = self
            .library
            .layouts
            .get(from)
            .ok_or_else(|| StoreError::Unknown(from.clone()))?
            .clone();
        copy.id = to.clone();
        if copy.name.is_empty() {
            copy.name = to.to_string();
        }
        self.admit_layout(None, &copy)?;
        self.library.layouts.insert(to.clone(), copy);
        self.mark(&to);
        Ok(())
    }

    /// Applies a transaction and records how to take it back. A new edit clears the redo stack, because redoing after a different edit would replay operations against a layout they no longer describe.
    ///
    /// A transaction the layout's file must not take — a line granting trust, a held text copied into a file of the user's own — leaves the layout exactly as it was before: it is put back from a copy taken first rather than by replaying the operations backwards, so nothing of the refused edit stays in memory for a later flush to write.
    pub fn commit(&mut self, transaction: Transaction) -> Result<(), StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        if transaction.layout.as_str() == BUILT_IN {
            return Err(StoreError::ReadOnly(transaction.layout));
        }
        let layout = self
            .library
            .layouts
            .get_mut(&transaction.layout)
            .ok_or_else(|| StoreError::Unknown(transaction.layout.clone()))?;
        let before = layout.clone();
        let back = apply_all(layout, &transaction.ops)?;
        let after = &self.library.layouts[&transaction.layout];
        if let Err(refused) = self.admit_layout(Some(&before), after) {
            self.library.layouts.insert(transaction.layout, before);
            return Err(refused);
        }
        self.done.push(Done {
            label: transaction.label,
            layout: transaction.layout.clone(),
            back,
        });
        self.undone.clear();
        self.mark(&transaction.layout);
        Ok(())
    }

    /// Takes back the last committed transaction. Refused on the recovery store like every other edit (F-10.35), rather than answered as an empty history.
    pub fn undo(&mut self) -> Result<String, StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        let entry = self.done.pop().ok_or(StoreError::NothingToUndo)?;
        let redo = self.reverse(&entry)?;
        let label = entry.label.clone();
        self.undone.push(redo);
        Ok(label)
    }

    pub fn redo(&mut self) -> Result<String, StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        let entry = self.undone.pop().ok_or(StoreError::NothingToRedo)?;
        let undo = self.reverse(&entry)?;
        let label = entry.label.clone();
        self.done.push(undo);
        Ok(label)
    }

    fn reverse(&mut self, entry: &Done) -> Result<Done, StoreError> {
        let layout = self
            .library
            .layouts
            .get_mut(&entry.layout)
            .ok_or_else(|| StoreError::Unknown(entry.layout.clone()))?;
        let back = apply_all(layout, &entry.back)?;
        self.mark(&entry.layout);
        Ok(Done {
            label: entry.label.clone(),
            layout: entry.layout.clone(),
            back,
        })
    }

    /// What an undo would take back, and what a redo would put again, for a menu entry that says so.
    pub fn undo_label(&self) -> Option<&str> {
        self.done.last().map(|entry| entry.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.undone.last().map(|entry| entry.label.as_str())
    }

    /// Every entry an undo and a redo would walk through, for a history list that jumps several at once.
    pub fn history(&self) -> History {
        History {
            undo: self.done.iter().map(|entry| entry.label.clone()).collect(),
            redo: self
                .undone
                .iter()
                .rev()
                .map(|entry| entry.label.clone())
                .collect(),
        }
    }

    fn mark(&mut self, id: &LayoutId) {
        self.dirty.insert(id.clone());
        self.changed_at = Some(Instant::now());
    }

    /// Drops one layout's history, for when the file it described was rewritten under it or taken away.
    fn forget(&mut self, id: &LayoutId) {
        self.done.retain(|entry| &entry.layout != id);
        self.undone.retain(|entry| &entry.layout != id);
    }

    pub fn has_unsaved(&self) -> bool {
        !self.dirty.is_empty() || !self.dirty_komponents.is_empty()
    }

    /// Whether the last change is old enough to be worth writing.
    pub fn is_settled(&self, now: Instant) -> bool {
        match self.changed_at {
            Some(at) => self.has_unsaved() && now.duration_since(at) >= SETTLE,
            None => false,
        }
    }

    /// Writes every layout that has changed, and every komponent saved since the last flush. Returns what it could not write.
    ///
    /// What it wrote is recorded as what that file now holds, so the watcher noticing the write finds the store already reflecting it. Recorded only once the bytes are on disk: a write that failed left the file holding something the store did not put there, and reading it back as somebody else's content is then the honest answer.
    pub fn flush(&mut self) -> Report {
        let mut report = Report::default();
        for id in std::mem::take(&mut self.dirty) {
            if let Err(why) = self.write_layout(&id) {
                report.error(Finding::new(self.path_of(&id), "", why));
            }
        }
        for id in std::mem::take(&mut self.dirty_komponents) {
            if let Err(why) = self.write_komponent(&id) {
                report.error(Finding::new(self.komponent_path(&id), "", why));
            }
        }
        self.changed_at = None;
        report
    }

    /// Writes the layout `id` as the store holds it, recording what the file now holds.
    fn write_layout(&mut self, id: &LayoutId) -> Result<(), Message> {
        let Some(layout) = self.library.layouts.get(id) else {
            return Ok(());
        };
        let path = self.path_of(id);
        let wrote = write_toml(&path, layout)?;
        self.dirty.remove(id);
        self.seen.insert(id.clone(), wrote);
        Ok(())
    }

    /// The same for the komponent `id`.
    fn write_komponent(&mut self, id: &KomponentId) -> Result<(), Message> {
        let Some(komponent) = self.library.komponents.get(id) else {
            return Ok(());
        };
        let path = self.komponent_path(id);
        let wrote = write_toml(&path, komponent)?;
        self.dirty_komponents.remove(id);
        self.seen_komponents.insert(id.clone(), wrote);
        Ok(())
    }

    /// Puts what a bundle brought in the store and writes it to disk there and then, file by file, rather than at the next flush: an import has to know which of its files are written, since what it records as the bundle's must be exactly those (a file the store held unrecorded would run as the user's own; a record claiming a file nobody wrote would hold a file of the user's).
    ///
    /// A refusal ([`LayoutStore::put_layout`], [`LayoutStore::put_komponent`]) puts nothing at all. A file that cannot be written is put back to what the store held under that name before, which is what its file still holds, and is reported; the files that were written stay.
    pub fn put_bundle(
        &mut self,
        layouts: Vec<Layout>,
        komponents: Vec<(KomponentId, Komponent)>,
    ) -> Result<Written, StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        let before_layouts: Vec<(LayoutId, Option<Layout>, bool)> = layouts
            .iter()
            .map(|layout| {
                let id = layout.id.clone();
                let held = self.library.layouts.get(&id).cloned();
                let dirty = self.dirty.contains(&id);
                (id, held, dirty)
            })
            .collect();
        let before_komponents: Vec<(KomponentId, Option<Komponent>, bool)> = komponents
            .iter()
            .map(|(id, _)| {
                let held = self.library.komponents.get(id).cloned();
                (id.clone(), held, self.dirty_komponents.contains(id))
            })
            .collect();
        let put = komponents
            .into_iter()
            .try_for_each(|(id, komponent)| self.put_komponent(id, komponent))
            .and_then(|()| {
                layouts
                    .into_iter()
                    .try_for_each(|layout| self.put_layout(layout))
            });
        if let Err(refused) = put {
            for (id, held, dirty) in before_komponents {
                self.restore_komponent(id, held, dirty);
            }
            for (id, held, dirty) in before_layouts {
                self.restore_layout(id, held, dirty);
            }
            return Err(refused);
        }
        let mut written = Written::default();
        for (id, held, dirty) in before_komponents {
            match self.write_komponent(&id) {
                Ok(()) => {
                    written.files.insert(komponent_path(&id));
                }
                Err(why) => {
                    written
                        .failed
                        .error(Finding::new(self.komponent_path(&id), "", why));
                    self.restore_komponent(id, held, dirty);
                }
            }
        }
        for (id, held, dirty) in before_layouts {
            match self.write_layout(&id) {
                Ok(()) => {
                    written.files.insert(layout_path(&id));
                }
                Err(why) => {
                    written
                        .failed
                        .error(Finding::new(self.path_of(&id), "", why));
                    self.restore_layout(id, held, dirty);
                }
            }
        }
        Ok(written)
    }

    fn restore_layout(&mut self, id: LayoutId, held: Option<Layout>, dirty: bool) {
        match held {
            Some(layout) => self.library.layouts.insert(id.clone(), layout),
            None => self.library.layouts.remove(&id),
        };
        match dirty {
            true => self.dirty.insert(id),
            false => self.dirty.remove(&id),
        };
    }

    fn restore_komponent(&mut self, id: KomponentId, held: Option<Komponent>, dirty: bool) {
        match held {
            Some(komponent) => self.library.komponents.insert(id.clone(), komponent),
            None => self.library.komponents.remove(&id),
        };
        match dirty {
            true => self.dirty_komponents.insert(id),
            false => self.dirty_komponents.remove(&id),
        };
    }

    /// Records a layout as one that resolved and validated cleanly, so there is something to fall back to when a later edit does not.
    ///
    /// Called from the pass that proves it — which is every reload and every monitor change — so a copy that would hold what it already holds is not written again.
    pub fn keep_last_good(&mut self, id: &LayoutId) -> Result<(), std::io::Error> {
        let Some(layout) = self.library.layouts.get(id) else {
            return Ok(());
        };
        let text =
            toml::to_string_pretty(layout).map_err(|why| std::io::Error::other(why.to_string()))?;
        let holds = held(text.as_bytes());
        if self.kept.get(id) == Some(&holds) {
            return Ok(());
        }
        let dir = self.dir.join(".last-good");
        std::fs::create_dir_all(&dir)?;
        util::writer::write(dir.join(format!("{id}.toml")), text.into_bytes())?;
        self.kept.insert(id.clone(), holds);
        Ok(())
    }

    /// The last copy of `id` that was known to work, if one was ever kept.
    pub fn last_good(&self, id: &LayoutId) -> Option<Layout> {
        let path = self.dir.join(".last-good").join(format!("{id}.toml"));
        read_layout(&path, id.as_str())
            .ok()
            .map(|(layout, _)| layout)
    }

    /// Puts the last copy of `id` that was known to work back in the store, for a startup whose own file has gone missing or stopped parsing. Answers whether there was one.
    ///
    /// **The file on disk is left alone.** A broken layout is the user's to fix, and a shell that quietly wrote an older copy over it would take the evidence away with it — so this restores what is *drawn* and says nothing about what is stored. Nothing is marked dirty either, so no flush can turn the rescue into a write, and the next start makes it again until the file itself is dealt with.
    pub fn restore_last_good(&mut self, id: &LayoutId) -> bool {
        let Some(layout) = self.last_good(id) else {
            return false;
        };
        self.library.layouts.insert(id.clone(), layout);
        // Forgotten rather than recorded: the file still holds whatever could not be read, so the next reload has to look at it again.
        self.seen.remove(id);
        true
    }

    pub fn path_of(&self, id: &LayoutId) -> PathBuf {
        self.dir.join(format!("{id}.toml"))
    }

    pub fn komponent_path(&self, id: &KomponentId) -> PathBuf {
        self.components.join(format!("{id}.toml"))
    }
}

/// Refuses a write that adds a line granting trust to an action chain: `layout trust` is the user's to run, never something a gesture runs for them, which is what validation says of a file at load too.
fn refuse_new_grants(
    before: Vec<(String, &Action)>,
    after: Vec<(String, &Action)>,
) -> Result<(), StoreError> {
    let had = granting(before);
    match granting(after).into_iter().find(|it| !had.contains(it)) {
        Some((key, line)) => Err(StoreError::GrantsTrust { key, line }),
        None => Ok(()),
    }
}

/// Where the komponents live for layouts kept in `layouts`: `components/` beside it, as TA-2 lays the config directory out.
pub fn components_beside(layouts: &Path) -> PathBuf {
    layouts.with_file_name("components")
}

/// Every komponent in `dir` with the name it is used by, in the order the file names sort.
fn komponent_files(dir: &Path) -> Vec<(PathBuf, KomponentId)> {
    config::fingerprint::layout_files(dir)
        .into_iter()
        .filter_map(|path| {
            let id = path.file_stem().and_then(|it| it.to_str())?;
            Some((path.clone(), KomponentId::new(id)))
        })
        .collect()
}

/// Reads one komponent and what its file held, or why it cannot be read.
pub fn read_komponent(path: &Path) -> Result<(Komponent, u64), Box<Finding>> {
    let unreadable = |why: String| Box::new(Finding::new(path, "", Message::verbatim(why)));
    let text =
        config::fingerprint::read_layout_text(path).map_err(|why| unreadable(why.to_string()))?;
    let komponent: Komponent = toml::from_str(&text).map_err(|why| {
        crate::validate::retired_in_komponent(&text, path)
            .map(Box::new)
            .unwrap_or_else(|| unreadable(why.to_string()))
    })?;
    Ok((komponent, held(text.as_bytes())))
}

/// Every layout in `dir` with the id it answers to, in the order the file names sort, and a finding for the one file the store never reads: one named after the built-in layout, which is code and read-only (F-10.2), so no file stands in for it — `extends = "default"`, `layout use default` and `--safe-layout` always mean the layout that ships, and the first edit to it forks a copy under another name.
fn layout_files(dir: &Path, report: &mut Report) -> Vec<(PathBuf, LayoutId)> {
    let mut files = Vec::new();
    for path in config::fingerprint::layout_files(dir) {
        let Some(id) = path
            .file_stem()
            .and_then(|it| it.to_str())
            .map(LayoutId::new)
        else {
            continue;
        };
        if id.as_str() == BUILT_IN {
            report.warn(Finding::new(
                &path,
                "",
                util::message!(
                    "finding.built_in_file",
                    file = path.display(),
                    id = BUILT_IN
                ),
            ));
            continue;
        }
        files.push((path, id));
    }
    files
}

/// Reads one layout and what its file held, taking its id from the file name rather than from the file.
///
/// The name on disk is the name everything else addresses — `layout use`, `extends`, the last-good copy — so letting the file disagree with it would give one layout two names and no way to tell which was meant.
fn read_layout(path: &Path, stem: &str) -> Result<(Layout, u64), String> {
    let text = config::fingerprint::read_layout_text(path).map_err(|why| why.to_string())?;
    let mut layout: Layout = toml::from_str(&text).map_err(|why| why.to_string())?;
    layout.id = LayoutId::new(stem);
    Ok((layout, held(text.as_bytes())))
}

/// What a file holds, as one number. It never leaves the process, so `DefaultHasher` answering differently between Rust versions costs nothing — the same trade `config::fingerprint` makes, and the same question: content, not a modification time.
fn held(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

thread_local! {
    /// The layout the shell is running, published so anything outside the reconcile — a preview, a sweep, a settings page — draws what the user is looking at rather than the shipped default. Kept on the driver thread that plans the screen and draws from it, so a test planning its own screen on its own thread reads its own.
    static RUNNING: std::cell::RefCell<Option<std::sync::Arc<Layout>>> = const { std::cell::RefCell::new(None) };
}

/// Publishes the active layout. Called by the pass that plans the screen, so a reader always has the one the windows were last built from.
pub fn set_running(layout: std::sync::Arc<Layout>) {
    RUNNING.with(|running| *running.borrow_mut() = Some(layout));
}

/// The active layout, or `None` before the shell has planned a screen on this thread — a unit test, a CLI invocation, a preview on a machine with no shell running.
pub fn running() -> Option<std::sync::Arc<Layout>> {
    RUNNING.with(|running| running.borrow().clone())
}

/// What [`LayoutStore::put_bundle`] wrote.
#[derive(Debug, Default)]
pub struct Written {
    /// Each file it put in the store and on disk, as a finding names it.
    pub files: BTreeSet<String>,
    /// Each file it could not write, which the store holds as it did before.
    pub failed: Report,
}

/// Writes `value` as TOML to `path`, answering a hash of what the file now holds.
fn write_toml<T: serde::Serialize>(path: &Path, value: &T) -> Result<u64, Message> {
    let text = toml::to_string_pretty(value).map_err(|why| Message::verbatim(why.to_string()))?;
    let wrote = held(text.as_bytes());
    util::writer::write(path.to_path_buf(), text.into_bytes())
        .map_err(|why| Message::verbatim(why.to_string()))?;
    Ok(wrote)
}
