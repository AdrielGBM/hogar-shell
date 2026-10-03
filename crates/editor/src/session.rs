//! What an edit mode works on: the selection, the draft the screen previews an undecided edit in, and the one history every committed edit goes into.
//!
//! **The draft.** A signal holding the active layout as the store holds it. An [`Edit`] previews into it through telar's [`Transaction`], so a drag ([`StyledContainer::drag_transaction`](telar::StyledContainer::drag_transaction)) and a popover ([`telar::register_transaction`]) decide it by the one convention every gesture shares (F-7): release, Enter or a click outside commits, Esc or the other button reverts. Whatever the draft holds is what the windows draw ([`surfaces::layouts::preview`]); nothing previewed is written or recorded, and a commit is one transaction in the store and one undo entry. There is one draft, so there is one open edit at a time: a second is refused rather than nested (no savepoints, F-5.10).
//!
//! **The selection** is what the tools act on: an area, a group or an instance on the layer and screen being edited. It follows what it names through an edit and clears when that is gone, or when the mode changes.

use std::cell::RefCell;
use std::fmt;
use std::rc::{Rc, Weak};

use telar::{
    Key, ModifiersState, ReadSignal, Rect, RwSignal, Transaction, TransactionError, detached,
    effect, signal,
};

use layout::{AreaId, Layout, LayoutOp, ResolvedLayer, StoreError};
use surfaces::rects::{Node, Part};
use surfaces::{layouts, reconcile};

use crate::mode;

/// What the tools of an edit mode act on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    Area(Node),
    Group(Node),
    Instance(Node),
}

impl Selection {
    /// The area, group or instance `node` names.
    pub fn of(node: Node) -> Self {
        match node.part {
            Part::Area => Self::Area(node),
            Part::Group(_) => Self::Group(node),
            Part::Instance(..) => Self::Instance(node),
        }
    }

    pub fn node(&self) -> Option<&Node> {
        match self {
            Self::None => None,
            Self::Area(node) | Self::Group(node) | Self::Instance(node) => Some(node),
        }
    }
}

thread_local! {
    static SELECTION: RwSignal<Selection> = detached(|| signal(Selection::None));
    static DRAFT: RwSignal<Layout> = detached(|| signal(stored().unwrap_or_else(layout::built_in)));
    static EDITS: RefCell<Vec<Weak<Pending>>> = const { RefCell::new(Vec::new()) };
}

/// Keeps the selection, the draft and the screen in step with the mode, the store and each other. Installed once, on the driver thread.
pub(crate) fn install() {
    detached(|| {
        effect(|| {
            mode::active().with(|_| ());
            SELECTION.with(|selection| selection.set(Selection::None));
        });
        effect(follow_the_layout);
        effect(follow_the_store);
        effect(|| layouts::preview(&DRAFT.with(RwSignal::get)));
    });
}

/// The selection, as a signal: read inside an effect or a build, it runs again whenever something else is selected.
pub fn selection() -> ReadSignal<Selection> {
    SELECTION.with(|selection| selection.read_only())
}

/// What is selected right now, without subscribing.
pub fn selected() -> Selection {
    SELECTION.with(|selection| selection.peek())
}

/// Selects `selection`, answering whether it did: only what is on the layer and screen being edited can be, and nothing can while no mode is up.
pub fn select(selection: Selection) -> bool {
    let in_scope = selection.node().is_none_or(mode::editing);
    if in_scope {
        SELECTION.with(|current| {
            if current.peek() != selection {
                current.set(selection);
            }
        });
    }
    in_scope
}

pub fn clear_selection() {
    select(Selection::None);
}

/// What a click at `point` selects among `placed`, given what is selected already: the smallest thing under the pointer, and on a click at the same thing again, the one around it — instance, then its group, then its area — until it comes back round to the smallest. A click on nothing selects nothing.
pub fn pick(current: &Selection, point: (f32, f32), placed: &[(Node, Rect)]) -> Selection {
    let mut under: Vec<&(Node, Rect)> = placed
        .iter()
        .filter(|(_, rect)| rect.contains(point.0, point.1))
        .collect();
    under.sort_by(|(a, at), (b, bt)| {
        (at.width * at.height)
            .total_cmp(&(bt.width * bt.height))
            .then_with(|| depth(b).cmp(&depth(a)))
    });
    under.dedup_by(|(a, _), (b, _)| a == b);
    let next = match current
        .node()
        .and_then(|node| under.iter().position(|(it, _)| it == node))
    {
        Some(at) => (at + 1) % under.len(),
        None => 0,
    };
    under
        .get(next)
        .map_or(Selection::None, |(node, _)| Selection::of(node.clone()))
}

fn depth(node: &Node) -> u8 {
    match node.part {
        Part::Area => 0,
        Part::Group(_) => 1,
        Part::Instance(..) => 2,
    }
}

/// Keeps the selection on what it names as the edited screen changes under it: an instance moved to another group is still the one selected, and one that is gone is not. While a preview is showing, something the preview takes away stays selected, since the edit may yet be reverted.
fn follow_the_layout() {
    let Some(mode) = mode::active().get() else {
        return;
    };
    let layer = reconcile::desktop(Some(&mode.output))
        .and_then(|desktop| desktop.resolved.layer(mode.layer).cloned())
        .unwrap_or_default();
    let current = selected();
    let Some(node) = current.node() else {
        return;
    };
    let followed = match found(node, &layer) {
        Some(node) => Selection::of(node),
        None if reconcile::previewing().is_some() => current,
        None => Selection::None,
    };
    SELECTION.with(|selection| {
        if selection.peek() != followed {
            selection.set(followed);
        }
    });
}

/// Where what `node` names is in `layer` now: an instance by its id wherever it went in the layer, a group within its area, an area by its id.
fn found(node: &Node, layer: &ResolvedLayer) -> Option<Node> {
    let area = |id: &layout::AreaId| layer.areas.iter().find(|area| area.id == *id);
    let at = |area: &layout::AreaId| Node::area(node.output.as_deref(), node.layer, area);
    match &node.part {
        Part::Area => area(&node.area).map(|_| node.clone()),
        Part::Group(group) => area(&node.area)?
            .groups
            .iter()
            .any(|it| it.id == *group)
            .then(|| node.clone()),
        Part::Instance(_, instance) => layer.areas.iter().find_map(|area| {
            area.groups.iter().find_map(|group| {
                group
                    .children
                    .iter()
                    .any(|child| child.id == instance.template())
                    .then(|| at(&area.id).instance(&group.id, instance))
            })
        }),
    }
}

/// The draft is the store's active layout whenever no edit is open. A change to the store under an open edit — a reload, a script, `layout use` — reverts it, since what it previewed was planned against a layout that is no longer there; a store that did not actually change leaves it alone.
fn follow_the_store() {
    layouts::revision().with(|_| ());
    let Some(stored) = stored() else {
        return;
    };
    if let Some(edit) = open() {
        if edit.0.transaction.before().as_ref() == Some(&stored) {
            return;
        }
        let _ = edit.revert();
    }
    DRAFT.with(|draft| {
        if !draft.peek_with(|draft| *draft == stored) {
            draft.set(stored);
        }
    });
}

fn stored() -> Option<Layout> {
    layouts::read(|store| store.active().clone())
}

/// The layout an edit previews into, as a signal: the store's active layout, or that with the open edit's operations applied. What a tool that draws its own view of the layout — the lock preview — reads to show an undecided edit as well.
pub fn draft() -> ReadSignal<Layout> {
    DRAFT.with(|draft| draft.read_only())
}

fn draft_signal() -> RwSignal<Layout> {
    DRAFT.with(|draft| *draft)
}

/// The edit that is open, if one is.
pub fn open() -> Option<Edit> {
    EDITS.with(|edits| {
        edits
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|pending| pending.transaction.is_open())
            .map(Edit)
    })
}

/// Why an edit did nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditError {
    /// Another edit is open. Edits do not nest: the one open owns the draft until it is committed or reverted.
    Nested,
    NotOpen,
    /// The shell was started with `--safe-layout`, which refuses every edit (F-10.35).
    Safe,
    /// The layout refused the operations, or the store refused the transaction.
    Refused(String),
}

impl EditError {
    pub fn refused(why: impl Into<String>) -> Self {
        Self::Refused(why.into())
    }

    /// Nothing is selected that the step acts on.
    pub fn nothing() -> Self {
        Self::refused(telar::t!("editor.refused.nothing"))
    }

    /// The area `id` is no longer on its screen.
    pub fn gone(id: &AreaId) -> Self {
        Self::refused(telar::t!("editor.refused.gone", id = id.to_string()))
    }

    /// The screen the edit is for is no longer drawn on.
    pub fn no_output() -> Self {
        Self::refused(telar::t!("editor.refused.no_output"))
    }

    /// What `name` names cannot go any further the way it was asked to.
    pub fn no_way(name: impl fmt::Display) -> Self {
        Self::refused(telar::t!("editor.refused.no_way", name = name.to_string()))
    }
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nested => f.write_str(&telar::t!("editor.draft.nested")),
            Self::NotOpen => f.write_str(&telar::t!("editor.draft.not_open")),
            Self::Safe => f.write_str(&StoreError::Safe.message().render()),
            Self::Refused(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for EditError {}

impl From<layout::OpError> for EditError {
    fn from(error: layout::OpError) -> Self {
        Self::Refused(error.message().render())
    }
}

impl From<TransactionError> for EditError {
    fn from(error: TransactionError) -> Self {
        match error {
            TransactionError::AlreadyOpen => Self::Nested,
            TransactionError::NotOpen | TransactionError::Disposed => Self::NotOpen,
        }
    }
}

/// One undecided change to the layout, previewed live and then committed as one undo entry or reverted exactly.
///
/// Made where the gesture or popover driving it is built, and reverted if that owner goes before it is decided. Opened by [`Edit::begin`], or by telar when handed over as [`Edit::transaction`] to `drag_transaction` or `register_transaction`.
#[derive(Clone)]
pub struct Edit(Rc<Pending>);

struct Pending {
    label: String,
    transaction: Transaction<Layout>,
    ops: RefCell<Vec<LayoutOp>>,
    refused: RefCell<Option<String>>,
}

impl Edit {
    /// An edit called `label` in the undo history, not yet begun.
    pub fn new(label: impl Into<String>) -> Self {
        let transaction = Transaction::new(draft_signal());
        let pending = Rc::new(Pending {
            label: label.into(),
            transaction,
            ops: RefCell::default(),
            refused: RefCell::default(),
        });
        let settling = Rc::downgrade(&pending);
        transaction.on_commit(move |before, after| {
            if let Some(pending) = settling.upgrade() {
                pending.settle(before, after);
            }
        });
        EDITS.with(|edits| {
            let mut edits = edits.borrow_mut();
            edits.retain(|edit| edit.strong_count() > 0);
            edits.push(Rc::downgrade(&pending));
        });
        Self(pending)
    }

    /// The same edit as telar drives it, for `drag_transaction` and `register_transaction`.
    pub fn transaction(&self) -> Transaction<Layout> {
        self.0.transaction
    }

    pub fn label(&self) -> &str {
        &self.0.label
    }

    pub fn is_open(&self) -> bool {
        self.0.transaction.is_open()
    }

    /// The operations the last preview applied, which a commit records.
    pub fn ops(&self) -> Vec<LayoutOp> {
        self.0.ops.borrow().clone()
    }

    pub fn begin(&self) -> Result<(), EditError> {
        refuse_under_safe_layout()?;
        self.0.transaction.begin()?;
        self.0.ops.borrow_mut().clear();
        Ok(())
    }

    /// Shows the layout as it was when the edit began with `ops` applied, in place of whatever the last preview showed: a drag previews where it is now, not a step from where it was a frame ago. Operations the layout refuses leave the last preview on screen, and so do operations that would make a locked screen fall back to the minimal lock ([`crate::modes::lock::kept`]).
    pub fn preview(&self, ops: Vec<LayoutOp>) -> Result<(), EditError> {
        refuse_under_safe_layout()?;
        let before = self.0.transaction.before().ok_or(EditError::NotOpen)?;
        let mut next = before.clone();
        layout::ops::apply_all(&mut next, &ops).map_err(EditError::from)?;
        crate::modes::lock::kept(&before, &next).map_err(EditError::Refused)?;
        self.0.transaction.preview(move |draft| *draft = next)?;
        *self.0.ops.borrow_mut() = ops;
        Ok(())
    }

    /// Records what was last previewed as one transaction in the store, and one undo entry. An edit that previewed nothing records nothing.
    pub fn commit(&self) -> Result<(), EditError> {
        self.0.transaction.commit()?;
        match self.0.refused.take() {
            Some(why) => Err(EditError::Refused(why)),
            None => Ok(()),
        }
    }

    /// Puts the draft back exactly as it was when the edit began, and the screen with it.
    pub fn revert(&self) -> Result<(), EditError> {
        self.0.transaction.revert()?;
        self.0.ops.borrow_mut().clear();
        Ok(())
    }
}

impl Pending {
    /// What a commit does however it was asked for — a release, a popover closing, [`Edit::commit`].
    fn settle(&self, before: &Layout, after: &Layout) {
        let ops = self.ops.take();
        if ops.is_empty() || before == after {
            return;
        }
        let committed = match layouts::read(|store| store.active_id().clone()) {
            Some(active) => {
                layouts::commit(layout::Transaction::new(self.label.clone(), active, ops))
            }
            None => Err(layouts::no_store()),
        };
        if let Err(why) = committed {
            tracing::warn!(
                edit = self.label,
                "the edit was not recorded: {}",
                why.english()
            );
            *self.refused.borrow_mut() = Some(why.render());
            if let Some(stored) = stored() {
                DRAFT.with(|draft| draft.set(stored));
            }
        }
    }
}

fn refuse_under_safe_layout() -> Result<(), EditError> {
    match layouts::read(layout::LayoutStore::is_safe) {
        Some(true) => Err(EditError::Safe),
        _ => Ok(()),
    }
}

/// Opens an edit called `label`: [`Edit::new`] and [`Edit::begin`] at once, for an edit driven by the keyboard or by code rather than by a gesture.
pub fn begin(label: impl Into<String>) -> Result<Edit, EditError> {
    let edit = Edit::new(label);
    edit.begin()?;
    Ok(edit)
}

/// Takes back the last change, answering with what it was called: the edit still open if there is one — which was never recorded, so reverting it is the whole of the undo — else the last one committed, whichever part of the shell committed it.
pub fn undo() -> Result<String, String> {
    if let Some(edit) = open() {
        edit.revert().map_err(|why| why.to_string())?;
        return Ok(edit.label().to_string());
    }
    layouts::undo().map_err(|why| why.render())
}

/// Puts back what the last undo took, reverting an edit still open first: the redo was planned against the layout without it.
pub fn redo() -> Result<String, String> {
    if let Some(edit) = open() {
        edit.revert().map_err(|why| why.to_string())?;
    }
    layouts::redo().map_err(|why| why.render())
}

/// Which way through the history a key asks to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum History {
    Undo,
    Redo,
}

/// Ctrl+Z undoes; Ctrl+Shift+Z and Ctrl+Y redo.
pub fn history_key(key: &Key, modifiers: ModifiersState) -> Option<History> {
    if !modifiers.is_ctrl || modifiers.is_alt || modifiers.is_meta {
        return None;
    }
    match key {
        Key::Char(ch) if ch.eq_ignore_ascii_case(&'z') => Some(match modifiers.is_shift {
            true => History::Redo,
            false => History::Undo,
        }),
        Key::Char(ch) if ch.eq_ignore_ascii_case(&'y') && !modifiers.is_shift => {
            Some(History::Redo)
        }
        _ => None,
    }
}
