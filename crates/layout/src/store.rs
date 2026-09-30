//! The layouts the shell is holding, and what has been done to them.
//!
//! One owner. Edit modes, popovers, the settings window and `hogar-shell layout …` are all clients of this store rather than writers of the files, which is what lets a single undo stack cover a drag, a scrubbed value and a scripted command alike: each of them commits a [`Transaction`], and the store keeps the operations that take it back.
//!
//! **Persistence is deliberate, not automatic.** A committed transaction marks its layout dirty and nothing more; [`LayoutStore::flush`] is what writes. A drag commits on every frame it moves, and writing a file per frame would put the disk in the middle of a gesture. The caller waits for the change to settle ([`SETTLE`]) and flushes then, and flushes outright when an edit mode exits or the shell shuts down, so nothing is ever lost — only deferred.
//!
//! Writes go through `util::writer`, the shell's one ordered writer, so a flush that overtakes another cannot land out of order. A layout that resolved and validated cleanly is also copied to `layouts/.last-good/`, which is what a startup whose own file has gone missing or stopped parsing falls back to.
//!
//! **A reload keeps whatever the files do not contradict.** [`LayoutStore::reload`] compares each file against what the store last read from it or last wrote to it, so the shell's own flush coming back through the watcher is not read as somebody else's edit: that layout keeps the edits it has not written yet and the history that would take them back. Only a layout somebody rewrote is taken from the file, and that layout's undo entries go with the bytes they described — operations replayed against content that has since been rewritten would restore something that no longer means what it meant.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use util::report::{Finding, Report};

use crate::model::*;
use crate::ops::{LayoutOp, OpError, apply_all};

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
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::ReadOnly(id) => write!(
                f,
                "`{id}` is the built-in layout and cannot be edited; copy it first"
            ),
            StoreError::Unknown(id) => write!(f, "there is no layout called `{id}`"),
            StoreError::Failed(error) => write!(f, "{error}"),
            StoreError::NothingToUndo => write!(f, "there is nothing to undo"),
            StoreError::NothingToRedo => write!(f, "there is nothing to redo"),
            StoreError::Safe => write!(
                f,
                "this shell was started with --safe-layout, which draws the built-in layout and writes none; restart it without the flag to edit"
            ),
        }
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
    layouts: BTreeMap<LayoutId, Layout>,
    active: LayoutId,
    done: Vec<Done>,
    undone: Vec<Done>,
    dirty: BTreeSet<LayoutId>,
    changed_at: Option<Instant>,
    /// What each layout's file held when the store last read it, or what the store last put there. A reload compares the file against this, which is what tells the shell's own write apart from an edit somebody else made — by content, so two writes inside one tick of the filesystem's clock are still two different things (`config::fingerprint`).
    seen: BTreeMap<LayoutId, u64>,
    /// Whether this is the recovery store [`LayoutStore::safe`] builds. It holds no user layout, so an edit could only fork the built-in one over a file it cannot see; refusing is what keeps `--safe-layout` from writing over the layout it was started to rescue.
    safe: bool,
    /// What each last-good copy holds. [`LayoutStore::keep_last_good`] is called from the pass that plans the screen, which runs on every reload and every monitor change, so without this a copy nobody's layout changed would be rewritten — temp file, fsync and rename — each time.
    kept: BTreeMap<LayoutId, u64>,
}

impl LayoutStore {
    /// Reads every layout in `dir`, adding the built-in one. A file that does not parse is reported and skipped, so one broken layout does not cost the user the others.
    pub fn load(dir: impl Into<PathBuf>) -> (Self, Report) {
        let dir = dir.into();
        let mut report = Report::default();
        let mut layouts = BTreeMap::new();
        let mut seen = BTreeMap::new();
        layouts.insert(LayoutId::new(BUILT_IN), crate::built_in::layout());

        for (path, id) in files_in(&dir) {
            match read_layout(&path, id.as_str()) {
                Ok((layout, held)) => {
                    seen.insert(id, held);
                    layouts.insert(layout.id.clone(), layout);
                }
                Err(why) => report.error(Finding::new(path, "", why)),
            }
        }

        let store = Self {
            dir,
            layouts,
            active: LayoutId::new(BUILT_IN),
            done: Vec::new(),
            undone: Vec::new(),
            dirty: BTreeSet::new(),
            changed_at: None,
            seen,
            safe: false,
            kept: BTreeMap::new(),
        };
        (store, report)
    }

    /// Brings the store in line with the files, keeping everything the files do not contradict.
    ///
    /// For an edit that arrived from outside the shell — a layout file written by hand or by another tool. A layout whose file still holds what the store last read from it or last wrote to it is left exactly as it is, the edits it has not flushed yet included, so the shell's own write coming back through the watcher costs nothing; a layout somebody rewrote is taken from the file, and that layout's history goes with the bytes it described.
    ///
    /// A file that has stopped parsing is reported and the layout in memory stays on screen, which is the same rule a config that stops parsing follows: the last content that worked keeps drawing, and the user is told what is wrong rather than shown an empty screen.
    pub fn reload(&mut self) -> Report {
        let mut report = Report::default();
        let mut on_disk = BTreeSet::new();

        for (path, id) in files_in(&self.dir) {
            on_disk.insert(id.clone());
            match read_layout(&path, id.as_str()) {
                // The store already reflects this file, whichever of them wrote it last.
                Ok((_, held)) if self.seen.get(&id) == Some(&held) => {}
                Ok((layout, held)) => {
                    self.forget(&id);
                    self.dirty.remove(&id);
                    self.seen.insert(id.clone(), held);
                    self.layouts.insert(id, layout);
                }
                Err(why) => report.error(Finding::new(path, "", why)),
            }
        }

        // A layout whose file is gone is gone — unless the store is holding edits nothing has written yet, which the next flush puts back.
        let vanished: Vec<LayoutId> = self
            .layouts
            .keys()
            .filter(|id| !on_disk.contains(*id) && !self.dirty.contains(*id))
            .cloned()
            .collect();
        for id in vanished {
            self.layouts.remove(&id);
            self.seen.remove(&id);
            self.forget(&id);
        }

        // The built-in layout is code, so it can neither be lost nor go stale; a file of that name still speaks for it, which is what `load` does too.
        let built_in = LayoutId::new(BUILT_IN);
        if !on_disk.contains(&built_in) {
            self.layouts
                .insert(built_in.clone(), crate::built_in::layout());
        }
        if !self.layouts.contains_key(&self.active) {
            self.active = built_in;
        }
        report
    }

    /// A store holding only the built-in layout and writing nothing, for `hogar-shell --safe-layout`.
    ///
    /// It refuses every edit rather than forking the built-in layout the way an ordinary store does. A recovery start cannot see the user's layouts, so a fork would pick a name from an empty set and write it over whichever file already has it — over the layout the flag was used to rescue.
    pub fn safe(dir: impl Into<PathBuf>) -> Self {
        let mut layouts = BTreeMap::new();
        layouts.insert(LayoutId::new(BUILT_IN), crate::built_in::layout());
        Self {
            dir: dir.into(),
            layouts,
            active: LayoutId::new(BUILT_IN),
            done: Vec::new(),
            undone: Vec::new(),
            dirty: BTreeSet::new(),
            changed_at: None,
            seen: BTreeMap::new(),
            safe: true,
            kept: BTreeMap::new(),
        }
    }

    /// Whether this store is the recovery one, which is what a reply has to say instead of reporting an edit it never made.
    pub fn is_safe(&self) -> bool {
        self.safe
    }

    pub fn names(&self) -> impl Iterator<Item = &LayoutId> {
        self.layouts.keys()
    }

    pub fn get(&self, id: &LayoutId) -> Option<&Layout> {
        self.layouts.get(id)
    }

    pub fn all(&self) -> &BTreeMap<LayoutId, Layout> {
        &self.layouts
    }

    pub fn active(&self) -> &Layout {
        self.layouts
            .get(&self.active)
            .expect("the active layout is one the store holds")
    }

    pub fn active_id(&self) -> &LayoutId {
        &self.active
    }

    pub fn use_layout(&mut self, id: &LayoutId) -> Result<(), StoreError> {
        if !self.layouts.contains_key(id) {
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
            .layouts
            .get(from)
            .ok_or_else(|| StoreError::Unknown(from.clone()))?
            .clone();
        copy.id = to.clone();
        if copy.name.is_empty() {
            copy.name = to.to_string();
        }
        self.layouts.insert(to.clone(), copy);
        self.mark(&to);
        Ok(())
    }

    /// Applies a transaction and records how to take it back. A new edit clears the redo stack, because redoing after a different edit would replay operations against a layout they no longer describe.
    pub fn commit(&mut self, transaction: Transaction) -> Result<(), StoreError> {
        if self.safe {
            return Err(StoreError::Safe);
        }
        if transaction.layout.as_str() == BUILT_IN {
            return Err(StoreError::ReadOnly(transaction.layout));
        }
        let layout = self
            .layouts
            .get_mut(&transaction.layout)
            .ok_or_else(|| StoreError::Unknown(transaction.layout.clone()))?;

        let back = apply_all(layout, &transaction.ops)?;
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
        !self.dirty.is_empty()
    }

    /// Whether the last change is old enough to be worth writing.
    pub fn is_settled(&self, now: Instant) -> bool {
        match self.changed_at {
            Some(at) => !self.dirty.is_empty() && now.duration_since(at) >= SETTLE,
            None => false,
        }
    }

    /// Writes every layout that has changed. Returns what it could not write.
    ///
    /// What it wrote is recorded as what that file now holds, so the watcher noticing the write finds the store already reflecting it. Recorded only once the bytes are on disk: a write that failed left the file holding something the store did not put there, and reading it back as somebody else's content is then the honest answer.
    pub fn flush(&mut self) -> Report {
        let mut report = Report::default();
        for id in std::mem::take(&mut self.dirty) {
            let Some(layout) = self.layouts.get(&id) else {
                continue;
            };
            let path = self.path_of(&id);
            let text = match toml::to_string_pretty(layout) {
                Ok(text) => text,
                Err(why) => {
                    report.error(Finding::new(path, "", why.to_string()));
                    continue;
                }
            };
            let wrote = held(text.as_bytes());
            match util::writer::write(path.clone(), text.into_bytes()) {
                Ok(()) => {
                    self.seen.insert(id, wrote);
                }
                Err(why) => report.error(Finding::new(path, "", why.to_string())),
            }
        }
        self.changed_at = None;
        report
    }

    /// Records a layout as one that resolved and validated cleanly, so there is something to fall back to when a later edit does not.
    ///
    /// Called from the pass that proves it — which is every reload and every monitor change — so a copy that would hold what it already holds is not written again.
    pub fn keep_last_good(&mut self, id: &LayoutId) -> Result<(), std::io::Error> {
        let Some(layout) = self.layouts.get(id) else {
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
        self.layouts.insert(id.clone(), layout);
        // Forgotten rather than recorded: the file still holds whatever could not be read, so the next reload has to look at it again.
        self.seen.remove(id);
        true
    }

    pub fn path_of(&self, id: &LayoutId) -> PathBuf {
        self.dir.join(format!("{id}.toml"))
    }
}

/// Every layout in `dir` with the id it answers to, in the order the file names sort.
fn files_in(dir: &Path) -> Vec<(PathBuf, LayoutId)> {
    config::fingerprint::layout_files(dir)
        .into_iter()
        .filter_map(|path| {
            let id = path.file_stem().and_then(|it| it.to_str())?;
            Some((path.clone(), LayoutId::new(id)))
        })
        .collect()
}

/// Reads one layout and what its file held, taking its id from the file name rather than from the file.
///
/// The name on disk is the name everything else addresses — `layout use`, `extends`, the last-good copy — so letting the file disagree with it would give one layout two names and no way to tell which was meant.
fn read_layout(path: &Path, stem: &str) -> Result<(Layout, u64), String> {
    let text = std::fs::read_to_string(path).map_err(|why| why.to_string())?;
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

/// The layout the shell is running, published so anything outside the reconcile — a preview, a sweep, a settings page — draws what the user is looking at rather than the shipped default.
static RUNNING: std::sync::Mutex<Option<std::sync::Arc<Layout>>> = std::sync::Mutex::new(None);

/// Publishes the active layout. Called by the pass that plans the screen, so a reader always has the one the windows were last built from.
pub fn set_running(layout: std::sync::Arc<Layout>) {
    if let Ok(mut running) = RUNNING.lock() {
        *running = Some(layout);
    }
}

/// The active layout, or `None` before the shell has planned a screen — a unit test, a CLI invocation, a preview on a machine with no shell running.
pub fn running() -> Option<std::sync::Arc<Layout>> {
    RUNNING.lock().ok().and_then(|running| running.clone())
}
