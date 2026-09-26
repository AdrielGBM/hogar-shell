//! The layouts the shell is holding, and what has been done to them.
//!
//! One owner. Edit modes, popovers, the settings window and `hogar-shell layout …` are all clients of this store rather than writers of the files, which is what lets a single undo stack cover a drag, a scrubbed value and a scripted command alike: each of them commits a [`Transaction`], and the store keeps the operations that take it back.
//!
//! **Persistence is deliberate, not automatic.** A committed transaction marks its layout dirty and nothing more; [`LayoutStore::flush`] is what writes. A drag commits on every frame it moves, and writing a file per frame would put the disk in the middle of a gesture. The caller waits for the change to settle ([`SETTLE`]) and flushes then, and flushes outright when an edit mode exits or the shell shuts down, so nothing is ever lost — only deferred.
//!
//! Writes go through `util::writer`, the shell's one ordered writer, so a flush that overtakes another cannot land out of order. A layout that resolved and validated cleanly is also copied to `layouts/.last-good/`, which is what `--safe-layout` and a fatal load error fall back to.

use std::collections::{BTreeMap, BTreeSet};
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
}

impl LayoutStore {
    /// Reads every layout in `dir`, adding the built-in one. A file that does not parse is reported and skipped, so one broken layout does not cost the user the others.
    pub fn load(dir: impl Into<PathBuf>) -> (Self, Report) {
        let dir = dir.into();
        let mut report = Report::default();
        let mut layouts = BTreeMap::new();
        layouts.insert(LayoutId::new(BUILT_IN), crate::built_in::layout());

        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|it| it.to_str()) != Some("toml") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|it| it.to_str()) else {
                    continue;
                };
                match read_layout(&path, stem) {
                    Ok(layout) => {
                        layouts.insert(layout.id.clone(), layout);
                    }
                    Err(why) => report.error(Finding::new(path.clone(), "", why)),
                }
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
        };
        (store, report)
    }

    /// Reads the directory again, keeping the layout that is active if it is still there.
    ///
    /// For an edit that arrived from outside the shell — a layout file written by hand, or by `layout import-config`. The history goes with the files it described: an undo stack over operations on a layout somebody else has since rewritten would restore bytes that no longer mean what they meant. Nothing is lost by that today, because nothing in the shell writes a layout yet; once the mutating verbs do (T-3.7), a reload has to reconcile against uncommitted work rather than drop it.
    pub fn reload(&mut self) -> Report {
        let active = self.active.clone();
        let (fresh, report) = Self::load(self.dir.clone());
        *self = fresh;
        if self.layouts.contains_key(&active) {
            self.active = active;
        }
        report
    }

    /// A store holding only the built-in layout, for `--safe-layout` and for a startup whose own layout failed fatally.
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
        }
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

    pub fn undo(&mut self) -> Result<String, StoreError> {
        let entry = self.done.pop().ok_or(StoreError::NothingToUndo)?;
        let redo = self.reverse(&entry)?;
        let label = entry.label.clone();
        self.undone.push(redo);
        Ok(label)
    }

    pub fn redo(&mut self) -> Result<String, StoreError> {
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
    pub fn flush(&mut self) -> Report {
        let mut report = Report::default();
        for id in std::mem::take(&mut self.dirty) {
            let Some(layout) = self.layouts.get(&id) else {
                continue;
            };
            let path = self.path_of(&id);
            match toml::to_string_pretty(layout) {
                Ok(text) => {
                    if let Err(why) = util::writer::write(path.clone(), text.into_bytes()) {
                        report.error(Finding::new(path, "", why.to_string()));
                    }
                }
                Err(why) => report.error(Finding::new(path, "", why.to_string())),
            }
        }
        self.changed_at = None;
        report
    }

    /// Records a layout as one that resolved and validated cleanly, so there is something to fall back to when a later edit does not.
    pub fn keep_last_good(&self, id: &LayoutId) -> Result<(), std::io::Error> {
        let Some(layout) = self.layouts.get(id) else {
            return Ok(());
        };
        let text =
            toml::to_string_pretty(layout).map_err(|why| std::io::Error::other(why.to_string()))?;
        let dir = self.dir.join(".last-good");
        std::fs::create_dir_all(&dir)?;
        util::writer::write(dir.join(format!("{id}.toml")), text.into_bytes())
    }

    /// The last copy of `id` that was known to work, if one was ever kept.
    pub fn last_good(&self, id: &LayoutId) -> Option<Layout> {
        let path = self.dir.join(".last-good").join(format!("{id}.toml"));
        read_layout(&path, id.as_str()).ok()
    }

    pub fn path_of(&self, id: &LayoutId) -> PathBuf {
        self.dir.join(format!("{id}.toml"))
    }
}

/// Reads one layout, taking its id from the file name rather than from the file.
///
/// The name on disk is the name everything else addresses — `layout use`, `extends`, the last-good copy — so letting the file disagree with it would give one layout two names and no way to tell which was meant.
fn read_layout(path: &Path, stem: &str) -> Result<Layout, String> {
    let text = std::fs::read_to_string(path).map_err(|why| why.to_string())?;
    let mut layout: Layout = toml::from_str(&text).map_err(|why| why.to_string())?;
    layout.id = LayoutId::new(stem);
    Ok(layout)
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
