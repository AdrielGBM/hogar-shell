//! The keyboard path through an edit mode (TA-4, WCAG 2.5.7): everything a pointer does there, a key does too.
//!
//! **What the keys do.** The arrows (and `h` `j` `k` `l` under `[keynav] vim`) move the selection to the nearest thing of the same depth that way, the way [`ui::keynav`] reads a grid; Alt+Down and Alt+Up go into what is selected and out to what holds it; Tab and Shift+Tab cycle the areas. Enter customizes the selection, the menu key (or Shift+F10) opens its context menu and Delete takes it away. Shift+arrows move it one slot, cell or edge over, Ctrl+arrows make it one step bigger or smaller. `m` opens the mode pie, where an arrow picks the mode that lies that way, and `?` lists every key the mode answers.
//!
//! **One press, one undo entry.** A move or a resize previews from the first press of its key and is committed when the key is released, or when any other key is pressed first: a held Shift+Right that the keyboard repeats ten times moves the chip ten slots and is taken back by one undo. While it is held it sits on the dismiss stack, so Esc puts it back as it was. The selection sits there too, under it: the first Esc clears what is selected and only the next reaches the mode's own way out.
//!
//! **One table.** Every key an edit mode answers is a row of [`table`], generic rows and the ones the per-layer tools add through [`add_key_op`] and [`add_mode_key_op`]; what a key does is looked up there, and the strip's key list is drawn from it. A tool's row wins over a generic row on the same key.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::thread::LocalKey;
use std::time::Duration;

use telar::{
    DismissRegistration, Key, ModifiersState, NamedKey, Rect, RwSignal, detached, effect, signal,
};

use config::{Config, Edge};
use layout::{LayerKind, Layout, LayoutOp};
use surfaces::reconcile;
use surfaces::rects::{self, Node, Part};
use surfaces::transient;
use ui::keynav::{KeyNav, Move};

use crate::mode::{self, Mode};
use crate::session::{self, Edit, EditError, Selection};
use crate::{context, host, pie, popover, steps};

/// How often a held key is looked at to see whether it has been let go, which is what commits the edit it drives.
const RELEASE_POLL: Duration = Duration::from_millis(30);

/// Which way an arrow points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const ALL: [Direction; 4] = [
        Direction::Left,
        Direction::Right,
        Direction::Up,
        Direction::Down,
    ];

    pub fn is_horizontal(self) -> bool {
        matches!(self, Direction::Left | Direction::Right)
    }

    /// Whether it points the way reading goes: right, or down.
    pub fn is_forward(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }

    /// The edge of the screen it points at.
    pub fn edge(self) -> Edge {
        match self {
            Direction::Left => Edge::Left,
            Direction::Right => Edge::Right,
            Direction::Up => Edge::Top,
            Direction::Down => Edge::Bottom,
        }
    }

    fn of(movement: Move) -> Option<Self> {
        match movement {
            Move::Previous => Some(Direction::Left),
            Move::Next => Some(Direction::Right),
            Move::PreviousRow => Some(Direction::Up),
            Move::NextRow => Some(Direction::Down),
            _ => None,
        }
    }

    /// The arrow key that points this way.
    pub fn arrow(self) -> NamedKey {
        match self {
            Direction::Left => NamedKey::ArrowLeft,
            Direction::Right => NamedKey::ArrowRight,
            Direction::Up => NamedKey::ArrowUp,
            Direction::Down => NamedKey::ArrowDown,
        }
    }

    fn vim(self) -> char {
        match self {
            Direction::Left => 'h',
            Direction::Right => 'l',
            Direction::Up => 'k',
            Direction::Down => 'j',
        }
    }
}

/// A key and the modifiers held with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub modifiers: ModifiersState,
}

impl Chord {
    pub fn new(key: Key) -> Self {
        Self {
            key,
            modifiers: ModifiersState::default(),
        }
    }

    pub fn named(key: NamedKey) -> Self {
        Self::new(Key::Named(key))
    }

    pub fn char(ch: char) -> Self {
        Self::new(Key::Char(ch))
    }

    pub fn shift(mut self) -> Self {
        self.modifiers.is_shift = true;
        self
    }

    pub fn ctrl(mut self) -> Self {
        self.modifiers.is_ctrl = true;
        self
    }

    pub fn alt(mut self) -> Self {
        self.modifiers.is_alt = true;
        self
    }

    /// Whether pressing `key` with `modifiers` held is this chord. A letter is the same key whichever case the layout reports it in, Shift telling the two apart; a symbol already says whether Shift made it (`?` is Shift+`/`), so Shift is not asked for again.
    pub fn matches(&self, key: &Key, modifiers: ModifiersState) -> bool {
        normalized(&self.key, self.modifiers) == normalized(key, modifiers)
    }

    /// The chord as a key list shows it, in the active locale: `Ctrl+Shift+Z`, `Shift+←`.
    pub fn spelled(&self) -> String {
        let mut said = modifiers_spelled(self.modifiers);
        said.push_str(&key_spelled(&self.key));
        said
    }
}

fn normalized(key: &Key, modifiers: ModifiersState) -> (Key, ModifiersState) {
    match key {
        Key::Char(ch) if ch.is_alphabetic() => {
            let shifted = ModifiersState {
                is_shift: modifiers.is_shift || ch.is_uppercase(),
                ..modifiers
            };
            let lower = ch.to_lowercase().next().unwrap_or(*ch);
            (Key::Char(lower), shifted)
        }
        Key::Char(ch) => (
            Key::Char(*ch),
            ModifiersState {
                is_shift: false,
                ..modifiers
            },
        ),
        Key::Named(_) => (key.clone(), modifiers),
    }
}

fn modifiers_spelled(modifiers: ModifiersState) -> String {
    let mut said = String::new();
    for (held, name) in [
        (modifiers.is_ctrl, telar::t!("editor.keys.key.ctrl")),
        (modifiers.is_alt, telar::t!("editor.keys.key.alt")),
        (modifiers.is_meta, telar::t!("editor.keys.key.super")),
        (modifiers.is_shift, telar::t!("editor.keys.key.shift")),
    ] {
        if held {
            said.push_str(&name);
            said.push('+');
        }
    }
    said
}

fn key_spelled(key: &Key) -> String {
    let named = match key {
        Key::Char(ch) => return ch.to_uppercase().collect(),
        Key::Named(named) => named,
    };
    match named {
        NamedKey::ArrowLeft => "←".to_string(),
        NamedKey::ArrowRight => "→".to_string(),
        NamedKey::ArrowUp => "↑".to_string(),
        NamedKey::ArrowDown => "↓".to_string(),
        NamedKey::Enter => telar::t!("editor.keys.key.enter"),
        NamedKey::Escape => telar::t!("editor.keys.key.escape"),
        NamedKey::Tab => telar::t!("editor.keys.key.tab"),
        NamedKey::Delete => telar::t!("editor.keys.key.delete"),
        NamedKey::Backspace => telar::t!("editor.keys.key.backspace"),
        NamedKey::Home => telar::t!("editor.keys.key.home"),
        NamedKey::End => telar::t!("editor.keys.key.end"),
        NamedKey::ContextMenu => telar::t!("editor.keys.key.menu"),
        other => format!("{other:?}"),
    }
}

/// Chords as a key list shows them: those sharing their modifiers once, their keys after them — `Shift+←/→/↑/↓`.
pub fn spell(chords: &[Chord]) -> String {
    let mut runs: Vec<(ModifiersState, Vec<String>)> = Vec::new();
    for chord in chords {
        let (key, modifiers) = normalized(&chord.key, chord.modifiers);
        let key = key_spelled(&key);
        match runs.last_mut() {
            Some((held, keys)) if *held == modifiers => keys.push(key),
            _ => runs.push((modifiers, vec![key])),
        }
    }
    runs.into_iter()
        .map(|(modifiers, keys)| modifiers_spelled(modifiers) + &keys.join("/"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a tool's key does.
#[derive(Clone, Copy)]
pub enum Run {
    /// One step of a layout edit: the operations that take the layout, as the draft has it now, one step further. Every press is one entry in the history, previewed until the key is let go and reverted by Esc before that; each repeat of a held key plans one more step from where the last one left the draft.
    Step(fn(&Selection, &Layout) -> Result<Vec<LayoutOp>, EditError>),
    /// A [`Run::Step`] that goes the way its arrow points — a join with the region that way — answering the arrows it is registered on and, under `[keynav] vim`, the vim keys that point the same way.
    Toward(fn(&Selection, &Layout, Direction) -> Result<Vec<LayoutOp>, EditError>),
    /// Anything that is not a step of a layout edit — a palette, a popover, a mode's own switch — run once a press; the repeats of a held key do nothing.
    Act(fn(&Selection) -> Result<(), EditError>),
}

/// A key a per-layer tool adds to its mode (T-7.x), through [`add_key_op`] or [`add_mode_key_op`].
#[derive(Clone)]
pub struct KeyOp {
    /// The operation it performs, as TA-5 names it (`region-split`, `bar-create`, …): what the coverage table checks and the key list is keyed by.
    pub name: &'static str,
    /// Every chord that runs it.
    pub keys: Vec<Chord>,
    /// What the key list and the history call it.
    pub label: fn() -> String,
    pub run: Run,
}

/// Which selections a row answers for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Every mode, whatever is selected.
    Every,
    /// One mode, whatever is selected.
    Mode(LayerKind),
    /// Whatever is selected in, or is, an area of this kind, as the layout file spells it (`bar`, `wallpaper_region`, …).
    Kind(&'static str),
}

/// One keyboard operation of an edit mode.
#[derive(Clone)]
pub struct Row {
    pub name: &'static str,
    /// The TA-5 operations it performs in the mode it was listed for, its own name among them.
    pub covers: Vec<&'static str>,
    pub keys: Vec<Chord>,
    pub label: fn() -> String,
    pub scope: Scope,
    does: Does,
}

#[derive(Clone, Copy)]
enum Does {
    Select,
    Ends,
    Inside,
    Around,
    Cycle,
    Customize,
    Menu,
    Remove,
    Move,
    Resize,
    History,
    /// Esc, which the dismiss stack answers before the tree hears it.
    Dismiss,
    Switch,
    Help,
    Tool(Run),
}

/// How a key press arrived: the key going down, or the keyboard repeating it while it is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Press {
    First,
    Repeat,
}

/// The move or resize a held key is previewing, until the key is let go.
struct Held {
    edit: Edit,
    chord: Chord,
    serial: u64,
}

type Fold = RefCell<Option<DismissRegistration>>;

/// A generic row before it is told which mode it is listed for: its name, chords, label and what it does.
type Generic = (&'static str, Vec<Chord>, fn() -> String, Does);

thread_local! {
    static KIND_OPS: RefCell<Vec<(&'static str, KeyOp)>> = const { RefCell::new(Vec::new()) };
    static MODE_OPS: RefCell<Vec<(LayerKind, KeyOp)>> = const { RefCell::new(Vec::new()) };
    static HELD: RefCell<Option<Held>> = const { RefCell::new(None) };
    static REVERTED: RefCell<Option<Chord>> = const { RefCell::new(None) };
    static SERIAL: Cell<u64> = const { Cell::new(0) };
    static HELP: RwSignal<bool> = detached(|| signal(false));
    static SELECTION_FOLD: Fold = const { RefCell::new(None) };
    static HELD_FOLD: Fold = const { RefCell::new(None) };
    static HELP_FOLD: Fold = const { RefCell::new(None) };
}

/// Adds `op` to every mode, for whatever is selected in or as an area of the kind `kind` (`bar`, `grid`, … as the layout file spells it). Registered inside [`crate::install`]; a key it shares with a generic row is its own for that kind.
pub fn add_key_op(kind: &'static str, op: KeyOp) {
    KIND_OPS.with(|ops| ops.borrow_mut().push((kind, op)));
}

/// Adds `op` to the mode of `layer`, whatever is selected: an operation of the mode itself, such as creating a bar.
pub fn add_mode_key_op(layer: LayerKind, op: KeyOp) {
    MODE_OPS.with(|ops| ops.borrow_mut().push((layer, op)));
}

/// Keeps what the keyboard holds in step with the mode and the selection: the selection on the dismiss stack while there is one, a held edit committed and the pie and key list folded when the mode changes. Installed once, on the driver thread.
pub(crate) fn install() {
    detached(|| {
        effect(|| {
            let chosen = session::selection().with(|selection| selection.node().is_some());
            fold_while(&SELECTION_FOLD, chosen, session::clear_selection);
        });
        effect(|| {
            mode::active().with(|_| ());
            settle();
            pie::shown().set(false);
            HELP.with(|help| help.set(false));
        });
        effect(|| {
            let shown = HELP.with(|help| help.get());
            fold_while(&HELP_FOLD, shown, fold_help);
        });
    });
}

/// Holds a registration on the dismiss stack in `slot` while `wanted`, so Esc runs `dismiss`; a registration already held stays where it is.
fn fold_while(slot: &'static LocalKey<Fold>, wanted: bool, dismiss: fn()) {
    let held = slot.with(|held| held.borrow().is_some());
    match (wanted, held) {
        (true, false) => {
            let fold = DismissRegistration::new(Rc::new(dismiss));
            slot.with(|held| *held.borrow_mut() = Some(fold));
        }
        (false, true) => drop(slot.with(|held| held.borrow_mut().take())),
        _ => {}
    }
}

/// Puts what the keyboard holds on the dismiss stack back on top of it. The host's window built it again — a config reload does — and that put the host's own entry back on top, where the first Esc would leave the mode instead of clearing the selection.
pub(crate) fn reclaim() {
    for (slot, dismiss) in [
        (&SELECTION_FOLD, session::clear_selection as fn()),
        (&HELD_FOLD, revert_held as fn()),
        (&HELP_FOLD, fold_help as fn()),
    ] {
        if let Some(stale) = slot.with(|held| held.borrow_mut().take()) {
            drop(stale);
            let fold = DismissRegistration::new(Rc::new(dismiss));
            slot.with(|held| *held.borrow_mut() = Some(fold));
        }
    }
}

/// Whether the key list is showing, as a signal the strip draws it from.
pub fn help() -> RwSignal<bool> {
    HELP.with(|help| *help)
}

fn fold_help() {
    HELP.with(|help| help.set(false));
}

/// What the host hears of a key: the one entry point of the keyboard path.
pub(crate) fn on_key(key: &Key) -> bool {
    let press = match telar::key_pressed(key) {
        true => Press::First,
        false => Press::Repeat,
    };
    press_as(key, telar::modifiers(), press)
}

/// Answers `key`, pressed with `modifiers`, in the mode that is up, saying whether it took it. Everything a popover, a menu or any other transient taking the keyboard is open for is theirs.
pub(crate) fn press_as(key: &Key, modifiers: ModifiersState, press: Press) -> bool {
    let Some(mode) = mode::current() else {
        return false;
    };
    if transient::takes_keyboard_besides(&host::transient_id(&mode.output)) {
        settle();
        return false;
    }
    if pie::shown().peek() {
        settle();
        return on_pie(&mode, key, modifiers);
    }
    let chord = Chord {
        key: key.clone(),
        modifiers,
    };
    if still_reverted(key, modifiers, press) {
        return true;
    }
    let continuing = press == Press::Repeat
        && HELD.with(|held| {
            held.borrow()
                .as_ref()
                .is_some_and(|held| held.chord.matches(key, modifiers))
        });
    if !continuing {
        settle();
    }
    let Some(row) = matched(&mode, key, modifiers) else {
        return false;
    };
    if mode.refused.is_some() && !matches!(row.does, Does::Switch | Does::Help) {
        return false;
    }
    if press == Press::Repeat && !repeats(row.does) {
        return true;
    }
    mode::clear_refusal();
    let done = run(&mode, &row, &chord);
    match done {
        Ok(taken) => taken,
        Err(why) => {
            mode::refuse(why);
            true
        }
    }
}

/// Whether this is the keyboard repeating a key whose step Esc put back while it was held: the rest of that press does nothing, rather than starting the step over. Any key going down ends it.
fn still_reverted(key: &Key, modifiers: ModifiersState, press: Press) -> bool {
    REVERTED.with(|reverted| match press {
        Press::First => {
            reverted.borrow_mut().take();
            false
        }
        Press::Repeat => reverted
            .borrow()
            .as_ref()
            .is_some_and(|chord| chord.matches(key, modifiers)),
    })
}

/// Whether a held key keeps doing what its first press did: moving the selection and stepping an edit do, a popover or a removal happens once.
fn repeats(does: Does) -> bool {
    matches!(
        does,
        Does::Select
            | Does::Cycle
            | Does::Move
            | Does::Resize
            | Does::History
            | Does::Tool(Run::Step(_) | Run::Toward(_))
    )
}

/// The row that answers `key` in `mode` for what is selected: a tool's row for the selection's kind first, then one for the mode, then a generic one.
fn matched(mode: &Mode, key: &Key, modifiers: ModifiersState) -> Option<Row> {
    let kind = selected_kind();
    let applies = |row: &Row| match row.scope {
        Scope::Every => true,
        Scope::Mode(layer) => layer == mode.layer,
        Scope::Kind(wanted) => kind == Some(wanted),
    };
    let rank = |row: &Row| match row.scope {
        Scope::Kind(_) => 0,
        Scope::Mode(_) => 1,
        Scope::Every => 2,
    };
    table(mode.layer)
        .into_iter()
        .filter(|row| applies(row) && row.keys.iter().any(|chord| chord.matches(key, modifiers)))
        .min_by_key(rank)
}

/// The kind of the area the selection is, or is in.
fn selected_kind() -> Option<&'static str> {
    let selected = session::selected();
    let node = selected.node()?;
    reconcile::desktop(node.output.as_deref())?
        .resolved
        .area(node.layer, &node.area)
        .map(|area| area.kind.name())
}

fn run(mode: &Mode, row: &Row, chord: &Chord) -> Result<bool, EditError> {
    let navigation = navigation();
    let direction = || direction_of(&navigation, &chord.key).ok_or_else(EditError::nothing);
    match row.does {
        Does::Select => select_toward(mode, direction()?),
        Does::Ends => select_end(mode, navigation.interpret(&chord.key) == Some(Move::Last)),
        Does::Inside => select_inside(mode),
        Does::Around => select_around(),
        Does::Cycle => cycle(mode, !chord.modifiers.is_shift),
        Does::Customize => {
            let selected = session::selected();
            if selected == Selection::None {
                return Ok(false);
            }
            popover::open_for(&selected)?;
        }
        Does::Menu => return Ok(context::open_selected()),
        Does::Remove => remove(&session::selected())?,
        Does::Move => {
            let direction = direction()?;
            let label = telar::t!(
                "editor.keys.moved",
                name = steps::name_of(&session::selected())
            );
            step(label, chord, |selection, draft| {
                steps::moved(selection, draft, direction)
            })?;
        }
        Does::Resize => {
            let direction = direction()?;
            let label = telar::t!(
                "editor.keys.resized",
                name = steps::name_of(&session::selected())
            );
            step(label, chord, |selection, draft| {
                steps::resized(selection, draft, direction)
            })?;
        }
        Does::History => {
            let way =
                session::history_key(&chord.key, chord.modifiers).ok_or_else(EditError::nothing)?;
            let walked = match way {
                session::History::Undo => session::undo(),
                session::History::Redo => session::redo(),
            };
            walked.map_err(EditError::Refused)?;
        }
        Does::Dismiss => return Ok(false),
        Does::Switch => pie::shown().update(|open| *open = !*open),
        Does::Help => HELP.with(|help| help.update(|shown| *shown = !*shown)),
        Does::Tool(Run::Act(act)) => act(&session::selected())?,
        Does::Tool(Run::Step(plan)) => {
            let label = telar::t!(
                "editor.keys.did",
                what = (row.label)(),
                name = steps::name_of(&session::selected())
            );
            step(label, chord, plan)?;
        }
        Does::Tool(Run::Toward(plan)) => {
            let direction = direction()?;
            let label = telar::t!(
                "editor.keys.did",
                what = (row.label)(),
                name = steps::name_of(&session::selected())
            );
            step(label, chord, |selection, draft| {
                plan(selection, draft, direction)
            })?;
        }
    }
    Ok(true)
}

/// What a key means on the open pie: an arrow picks the mode that lies that way, the switcher key folds it again.
fn on_pie(mode: &Mode, key: &Key, modifiers: ModifiersState) -> bool {
    if switcher_key().matches(key, modifiers) {
        pie::shown().set(false);
        return true;
    }
    let plain = modifiers == ModifiersState::default();
    let Some(direction) = direction_of(&navigation(), key).filter(|_| plain) else {
        return false;
    };
    if let Some(layer) = pie::toward(mode.layer, direction) {
        pie::shown().set(false);
        mode::switch(layer);
    }
    true
}

/// One step of the edit the chord drives: added to the edit its held key is previewing, or the first step of a new one.
fn step(
    label: String,
    chord: &Chord,
    plan: impl FnOnce(&Selection, &Layout) -> Result<Vec<LayoutOp>, EditError>,
) -> Result<(), EditError> {
    let ops = plan(&session::selected(), &session::draft().peek())?;
    if let Some(edit) = HELD.with(|held| held.borrow().as_ref().map(|held| held.edit.clone())) {
        let mut all = edit.ops();
        all.extend(ops);
        return edit.preview(all);
    }
    let edit = session::begin(label)?;
    if let Err(why) = edit.preview(ops) {
        let _ = edit.revert();
        return Err(why);
    }
    let serial = SERIAL.with(|next| {
        next.set(next.get() + 1);
        next.get()
    });
    HELD.with(|held| {
        *held.borrow_mut() = Some(Held {
            edit,
            chord: chord.clone(),
            serial,
        })
    });
    fold_while(&HELD_FOLD, true, revert_held);
    watch_release(serial);
    Ok(())
}

/// Commits the held edit once its key is let go. Looked at on a timer because a key's release is heard by the window, never by the tree.
fn watch_release(serial: u64) {
    platform_wayland::timeout(RELEASE_POLL, move || {
        let current =
            HELD.with(|held| held.borrow().as_ref().map(|held| held.serial)) == Some(serial);
        if current && !settle_released() {
            watch_release(serial);
        }
    });
}

/// Commits the held edit if its key is no longer down, answering whether it did.
pub(crate) fn settle_released() -> bool {
    let released = HELD.with(|held| {
        held.borrow()
            .as_ref()
            .is_some_and(|held| !telar::key_held(&held.chord.key))
    });
    if released {
        settle();
    }
    released
}

/// Commits the edit a held key was previewing, as one entry in the history.
pub(crate) fn settle() {
    let Some(held) = take_held() else {
        return;
    };
    if let Err(why) = held.edit.commit() {
        mode::refuse(why);
    }
}

/// Esc on a held edit: the layout goes back to what it was before the key went down.
fn revert_held() {
    if let Some(held) = take_held() {
        let _ = held.edit.revert();
        REVERTED.with(|reverted| *reverted.borrow_mut() = Some(held.chord));
    }
}

fn take_held() -> Option<Held> {
    let held = HELD.with(|held| held.borrow_mut().take());
    fold_while(&HELD_FOLD, false, revert_held);
    held
}

/// Takes the selection away as one entry in the history: an instance the way its menu's Remove does, a group or an area out of the rule that writes it. The lock screen's prompt is refused by the layout itself (TA-8).
fn remove(selection: &Selection) -> Result<(), EditError> {
    let name = steps::name_of(selection);
    match selection {
        Selection::None => Err(EditError::nothing()),
        Selection::Instance(node) => context::remove(node, &name),
        Selection::Group(_) | Selection::Area(_) => {
            let ops = steps::removal(selection, &session::draft().peek())?;
            context::commit(telar::t!("editor.menu.removed", name = name), ops)
        }
    }
}

/// How the edited screen reads lists: arrows always, the vim keys when its `[keynav]` says so, over a grid so both pairs of arrows count.
fn navigation() -> KeyNav {
    let keynav = edited_config()
        .or_else(config::config)
        .map(|config| config.keynav)
        .unwrap_or_default();
    ui::keynav::from_config(&keynav).grid()
}

fn edited_config() -> Option<Arc<Config>> {
    let mode = mode::current()?;
    reconcile::desktop(Some(&mode.output)).map(|desktop| desktop.config)
}

/// Which way `key` points, whatever the modifiers held with it: Shift+`L` points right as `l` does.
fn direction_of(navigation: &KeyNav, key: &Key) -> Option<Direction> {
    let key = match key {
        Key::Char(ch) => Key::Char(ch.to_ascii_lowercase()),
        named => named.clone(),
    };
    navigation.interpret(&key).and_then(Direction::of)
}

/// The four arrows, each held as `held` says: `arrows_with(|chord| chord.alt())` for Alt+arrows.
pub fn arrows_with(held: fn(Chord) -> Chord) -> Vec<Chord> {
    Direction::ALL
        .into_iter()
        .map(|direction| held(Chord::named(direction.arrow())))
        .collect()
}

/// The arrows with `modifiers`, and the vim keys too when `vim`.
pub(crate) fn arrows(vim: bool, modifiers: ModifiersState) -> Vec<Chord> {
    let mut chords: Vec<Chord> = Direction::ALL
        .into_iter()
        .map(|direction| Chord {
            key: Key::Named(direction.arrow()),
            modifiers,
        })
        .collect();
    if vim {
        chords.extend(Direction::ALL.into_iter().map(|direction| Chord {
            key: Key::Char(direction.vim()),
            modifiers,
        }));
    }
    chords
}

/// The vim key that points the way `chord`'s arrow does, held with the same modifiers.
fn vim_of(chord: &Chord) -> Option<Chord> {
    let direction = Direction::ALL
        .into_iter()
        .find(|direction| chord.key == Key::Named(direction.arrow()))?;
    Some(Chord {
        key: Key::Char(direction.vim()),
        modifiers: chord.modifiers,
    })
}

fn switcher_key() -> Chord {
    Chord::char('m')
}

/// Every keyboard operation of the mode of `layer`: the generic rows, then what the tools added for the mode, then what they added for area kinds, which answer in every mode an area of theirs is in.
pub fn table(layer: LayerKind) -> Vec<Row> {
    let vim = navigation().vim;
    let shift = ModifiersState {
        is_shift: true,
        ..ModifiersState::default()
    };
    let ctrl = ModifiersState {
        is_ctrl: true,
        ..ModifiersState::default()
    };
    let mut ends = vec![Chord::named(NamedKey::Home), Chord::named(NamedKey::End)];
    if vim {
        ends.extend([Chord::char('g'), Chord::char('G')]);
    }
    let generic: Vec<Generic> = vec![
        (
            "select",
            arrows(vim, ModifiersState::default()),
            || telar::t!("editor.keys.op.select"),
            Does::Select,
        ),
        (
            "select-ends",
            ends,
            || telar::t!("editor.keys.op.select-ends"),
            Does::Ends,
        ),
        (
            "select-inside",
            vec![Chord::named(NamedKey::ArrowDown).alt()],
            || telar::t!("editor.keys.op.select-inside"),
            Does::Inside,
        ),
        (
            "select-around",
            vec![Chord::named(NamedKey::ArrowUp).alt()],
            || telar::t!("editor.keys.op.select-around"),
            Does::Around,
        ),
        (
            "cycle-areas",
            vec![
                Chord::named(NamedKey::Tab),
                Chord::named(NamedKey::Tab).shift(),
            ],
            || telar::t!("editor.keys.op.cycle-areas"),
            Does::Cycle,
        ),
        (
            "customize",
            vec![Chord::named(NamedKey::Enter)],
            || telar::t!("editor.keys.op.customize"),
            Does::Customize,
        ),
        (
            "context-menu",
            vec![
                Chord::named(NamedKey::ContextMenu),
                Chord::named(NamedKey::F10).shift(),
            ],
            || telar::t!("editor.keys.op.context-menu"),
            Does::Menu,
        ),
        (
            "remove",
            vec![
                Chord::named(NamedKey::Delete),
                Chord::named(NamedKey::Backspace),
            ],
            || telar::t!("editor.keys.op.remove"),
            Does::Remove,
        ),
        (
            "move",
            arrows(vim, shift),
            || telar::t!("editor.keys.op.move"),
            Does::Move,
        ),
        (
            "resize",
            arrows(vim, ctrl),
            || telar::t!("editor.keys.op.resize"),
            Does::Resize,
        ),
        (
            "undo",
            vec![Chord::char('z').ctrl()],
            || telar::t!("editor.keys.op.undo"),
            Does::History,
        ),
        (
            "redo",
            vec![Chord::char('z').ctrl().shift(), Chord::char('y').ctrl()],
            || telar::t!("editor.keys.op.redo"),
            Does::History,
        ),
        (
            "escape",
            vec![Chord::named(NamedKey::Escape)],
            || telar::t!("editor.keys.op.escape"),
            Does::Dismiss,
        ),
        (
            "switch-mode",
            vec![switcher_key()],
            || telar::t!("editor.keys.op.switch-mode"),
            Does::Switch,
        ),
        (
            "keys",
            vec![Chord::char('?'), Chord::named(NamedKey::F1)],
            || telar::t!("editor.keys.op.keys"),
            Does::Help,
        ),
    ];
    let mut rows: Vec<Row> = generic
        .into_iter()
        .map(|(name, keys, label, does)| {
            let mut covers = vec![name];
            covers.extend(covered(name, layer));
            Row {
                name,
                covers,
                keys,
                label,
                scope: Scope::Every,
                does,
            }
        })
        .collect();
    let tool = |op: &KeyOp, scope: Scope| Row {
        name: op.name,
        covers: [op.name]
            .into_iter()
            .chain(covered(op.name, layer))
            .collect(),
        keys: match (op.run, vim) {
            (Run::Toward(_), true) => op
                .keys
                .iter()
                .cloned()
                .chain(op.keys.iter().filter_map(vim_of))
                .collect(),
            _ => op.keys.clone(),
        },
        label: op.label,
        scope,
        does: Does::Tool(op.run),
    };
    MODE_OPS.with(|ops| {
        rows.extend(
            ops.borrow()
                .iter()
                .filter(|(of, _)| *of == layer)
                .map(|(_, op)| tool(op, Scope::Mode(layer))),
        )
    });
    KIND_OPS.with(|ops| {
        rows.extend(
            ops.borrow()
                .iter()
                .map(|(kind, op)| tool(op, Scope::Kind(kind))),
        )
    });
    rows
}

/// A TA-5 operation Enter reaches through an area's popover: on which layer, the kind of area whose popover performs it, and the value its row edits there ([`crate::popover::AreaDraft::value`]) — what the coverage test opens that popover to find.
pub struct Customized {
    pub layer: LayerKind,
    pub operation: &'static str,
    pub kind: &'static str,
    pub value: &'static str,
}

const fn customized(
    layer: LayerKind,
    operation: &'static str,
    kind: &'static str,
    value: &'static str,
) -> Customized {
    Customized {
        layer,
        operation,
        kind,
        value,
    }
}

/// Every operation a popover performs that the key table counts as reached by Enter.
pub const CUSTOMIZED: &[Customized] = &[
    customized(
        LayerKind::Background,
        "region-source",
        "wallpaper_region",
        "source",
    ),
    customized(
        LayerKind::Background,
        "region-fit",
        "wallpaper_region",
        "fit",
    ),
    customized(
        LayerKind::Background,
        "region-transition",
        "wallpaper_region",
        "transition",
    ),
    customized(
        LayerKind::Background,
        "texture-tile",
        "texture",
        "texture.tile",
    ),
    customized(
        LayerKind::Background,
        "texture-blend",
        "texture",
        "texture.blend",
    ),
    customized(
        LayerKind::Background,
        "texture-opacity",
        "texture",
        "texture.opacity",
    ),
    customized(LayerKind::Desktop, "widget-visibility", "grid", "visible"),
    customized(LayerKind::Top, "bar-offset", "bar", "offset"),
    customized(LayerKind::Top, "bar-shape", "bar", "mode"),
    customized(LayerKind::Top, "bar-reserve", "bar", "reserve"),
    customized(LayerKind::Top, "bar-autohide", "bar", "autohide"),
    customized(
        LayerKind::Top,
        "bar-above-fullscreen",
        "bar",
        "above_fullscreen",
    ),
    customized(
        LayerKind::Overlay,
        "stack-output-policy",
        "stack",
        "output_policy",
    ),
    customized(LayerKind::Overlay, "stack-routes", "stack", "routes"),
    customized(LayerKind::Lock, "prompt-style", "prompt", "style.fill"),
];

/// The TA-4 names a row stands for besides its own, and the TA-5 operations of `layer` it performs there: the popover's rows are reached with Enter ([`CUSTOMIZED`]), the menu's moves with the menu key, the generic moves and resizes work on every kind of area, and a tool's row is the operation of a layer that borrows the tool — a region split on the lock layer.
fn covered(name: &str, layer: LayerKind) -> Vec<&'static str> {
    if name == "customize" {
        return CUSTOMIZED
            .iter()
            .filter(|reached| reached.layer == layer)
            .map(|reached| reached.operation)
            .collect();
    }
    let operations: &[&'static str] = match (name, layer) {
        ("escape", _) => &["deselect", "revert", "exit"],
        ("context-menu", LayerKind::Desktop) => &["widget-from-bar"],
        ("move", LayerKind::Background) => &["region-move"],
        ("move", LayerKind::Desktop) => &["widget-cell"],
        ("move", LayerKind::Top) => &["bar-edge", "bar-offset", "chip-zone", "chip-bar"],
        ("move", LayerKind::Overlay) => &["stack-anchor"],
        ("resize", LayerKind::Background) => &["region-resize"],
        ("resize", LayerKind::Desktop) => &["widget-size"],
        ("resize", LayerKind::Top) => &["bar-thickness", "bar-length"],
        ("resize", LayerKind::Overlay) => &["stack-width"],
        ("move", LayerKind::Lock) => &["prompt-move"],
        ("region-split", LayerKind::Lock) => &["lock-regions"],
        ("widget-add", LayerKind::Lock) => &["lock-palette"],
        ("grid-create", LayerKind::Lock) => &["lock-grid"],
        _ => &[],
    };
    operations.to_vec()
}

/// One line of the key list: the chords, and what they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyLine {
    pub keys: String,
    pub what: String,
}

/// The key list the strip shows for the mode of `layer`: each row's chords and what they do, for the rows that can answer on this screen — a tool's row only where an area of its kind is on the edited layer.
pub fn help_rows(layer: LayerKind) -> Vec<KeyLine> {
    let kinds: Vec<&'static str> = mode::current()
        .and_then(|mode| reconcile::desktop(Some(&mode.output)))
        .and_then(|desktop| {
            let layer = desktop.resolved.layer(layer)?;
            Some(layer.areas.iter().map(|area| area.kind.name()).collect())
        })
        .unwrap_or_default();
    table(layer)
        .into_iter()
        .filter(|row| match row.scope {
            Scope::Kind(kind) => kinds.contains(&kind),
            Scope::Every | Scope::Mode(_) => true,
        })
        .map(|row| KeyLine {
            keys: spell(&row.keys),
            what: (row.label)(),
        })
        .collect()
}

/// What is placed on the edited layer, each node once.
fn placed(mode: &Mode) -> Vec<(Node, Rect)> {
    let mut placed: Vec<(Node, Rect)> = Vec::new();
    for (node, rect) in rects::on(Some(&mode.output), mode.layer) {
        if !placed.iter().any(|(held, _)| *held == node) {
            placed.push((node, rect));
        }
    }
    placed
}

fn depth(part: &Part) -> u8 {
    match part {
        Part::Area => 0,
        Part::Group(_) => 1,
        Part::Instance(..) => 2,
    }
}

/// Top to bottom, then left to right.
fn in_reading_order(placed: &mut [(Node, Rect)]) {
    placed.sort_by(|(_, a), (_, b)| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
}

/// What is placed at the depth of `node` — areas, groups or instances — in reading order.
fn level_of(mode: &Mode, part: &Part) -> Vec<(Node, Rect)> {
    let mut level: Vec<(Node, Rect)> = placed(mode)
        .into_iter()
        .filter(|(node, _)| depth(&node.part) == depth(part))
        .collect();
    in_reading_order(&mut level);
    level
}

fn select_node(node: Option<Node>) {
    if let Some(node) = node {
        session::select(Selection::of(node));
    }
}

/// Selects the nearest thing of the selection's depth `direction` of it, or the first area when nothing is selected.
fn select_toward(mode: &Mode, direction: Direction) {
    let selected = session::selected();
    let Some(current) = selected.node() else {
        select_node(
            level_of(mode, &Part::Area)
                .first()
                .map(|(node, _)| node.clone()),
        );
        return;
    };
    let level = level_of(mode, &current.part);
    let Some(from) = level
        .iter()
        .find(|(node, _)| node == current)
        .map(|(_, rect)| *rect)
    else {
        return;
    };
    let others = level.iter().filter(|(node, _)| node != current).cloned();
    select_node(nearest(from, direction, others));
}

/// Selects the first or the last thing of the selection's depth, in reading order.
fn select_end(mode: &Mode, last: bool) {
    let selected = session::selected();
    let part = selected.node().map_or(Part::Area, |node| node.part.clone());
    let level = level_of(mode, &part);
    let end = match last {
        true => level.last(),
        false => level.first(),
    };
    select_node(end.map(|(node, _)| node.clone()));
}

/// Selects the first thing inside the selection: an area's first group, a group's first instance.
fn select_inside(mode: &Mode) {
    let selected = session::selected();
    let Some(current) = selected.node() else {
        return;
    };
    let inside = |node: &Node| match (&current.part, &node.part) {
        (Part::Area, Part::Group(_)) => node.area == current.area,
        (Part::Group(group), Part::Instance(held, _)) => node.area == current.area && held == group,
        _ => false,
    };
    let mut within: Vec<(Node, Rect)> = placed(mode)
        .into_iter()
        .filter(|(node, _)| inside(node))
        .collect();
    in_reading_order(&mut within);
    select_node(within.first().map(|(node, _)| node.clone()));
}

/// Selects what holds the selection: an instance's group, a group's area.
fn select_around() {
    let selected = session::selected();
    let Some(current) = selected.node() else {
        return;
    };
    let area = Node::area(current.output.as_deref(), current.layer, &current.area);
    let around = match &current.part {
        Part::Instance(group, _) => area.group(group),
        Part::Group(_) => area,
        Part::Area => return,
    };
    session::select(Selection::of(around));
}

/// Selects the next area in reading order, or the previous one, coming round at either end.
fn cycle(mode: &Mode, forward: bool) {
    let areas = level_of(mode, &Part::Area);
    if areas.is_empty() {
        return;
    }
    let selected = session::selected();
    let at = selected
        .node()
        .and_then(|node| areas.iter().position(|(held, _)| held.area == node.area));
    let next = match (at, forward) {
        (Some(at), true) => ui::keynav::apply(at, areas.len(), Move::Next),
        (Some(at), false) => ui::keynav::apply(at, areas.len(), Move::Previous),
        (None, true) => 0,
        (None, false) => areas.len() - 1,
    };
    select_node(areas.get(next).map(|(node, _)| node.clone()));
}

/// The one of `candidates` nearest `direction` of `from`: ahead of it that way, in line with it if anything is — sharing some of its span across that way — and the closest of those.
pub(crate) fn nearest<T>(
    from: Rect,
    direction: Direction,
    candidates: impl IntoIterator<Item = (T, Rect)>,
) -> Option<T> {
    let centre = |rect: &Rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
    let (fx, fy) = centre(&from);
    let apart = |a: (f32, f32), b: (f32, f32)| {
        let overlap = a.1.min(b.1) - a.0.max(b.0);
        (-overlap).max(0.0)
    };
    candidates
        .into_iter()
        .filter_map(|(item, rect)| {
            let (cx, cy) = centre(&rect);
            let (ahead, across) = match direction {
                Direction::Right => (
                    cx - fx,
                    apart(
                        (from.y, from.y + from.height),
                        (rect.y, rect.y + rect.height),
                    ),
                ),
                Direction::Left => (
                    fx - cx,
                    apart(
                        (from.y, from.y + from.height),
                        (rect.y, rect.y + rect.height),
                    ),
                ),
                Direction::Down => (
                    cy - fy,
                    apart((from.x, from.x + from.width), (rect.x, rect.x + rect.width)),
                ),
                Direction::Up => (
                    fy - cy,
                    apart((from.x, from.x + from.width), (rect.x, rect.x + rect.width)),
                ),
            };
            (ahead > 0.5).then_some((item, (across > 0.0, ahead + 2.0 * across)))
        })
        .min_by(|(_, (a_off, a)), (_, (b_off, b))| a_off.cmp(b_off).then(a.total_cmp(b)))
        .map(|(item, _)| item)
}
