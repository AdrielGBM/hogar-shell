//! Which layer of which screen is being edited: at most one of them, process-wide, because layers are never edited together (DEC-2).
//!
//! Every way in — `hogar-shell layout edit`, a keybind bound to it, the strip's own switcher — goes through [`enter`], and every way out — Esc, Done, `layout edit off`, the screen being unplugged — through [`leave`]. What the host holds on the compositor for the session is released on the way out, so leaving puts every window's layer, keyboard and mapped state back exactly as they were.
//!
//! **Lock mode is a preview** (TA-8). It is refused while the session is locked, it never takes or touches a session lock, and on a machine that cannot lock it still opens, saying why in the strip instead of offering tools.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use telar::{ReadSignal, RwSignal, detached, effect, signal};

use layout::{LayerKind, LayoutStore, StoreError};
use surfaces::{reconcile, transient};

use crate::host::{self, Session};

/// One edit mode: the layer being edited and the screen it is edited on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    pub layer: LayerKind,
    pub output: String,
    /// Why this mode offers no tools: lock mode on a machine that cannot lock (TA-8). The strip says it instead.
    pub refused: Option<String>,
}

/// What the compositor and the session say about entering a mode, read at the moment of entering.
pub(crate) struct Compositor {
    /// Whether a layer window can be moved to another layer (`zwlr_layer_shell_v1` version 2); without it the host draws the edited layer in the overlay window instead (TA-4, R-6).
    pub(crate) restack: bool,
    pub(crate) locked: bool,
    /// Whether this machine can lock at all, and why not when it cannot.
    pub(crate) lockable: Result<(), String>,
}

impl Compositor {
    fn now() -> Self {
        Self {
            restack: platform_wayland::layer_restack_supported(),
            locked: services::lock::locked(),
            lockable: services::lock::can_lock(),
        }
    }
}

thread_local! {
    static ACTIVE: RwSignal<Option<Mode>> = detached(|| signal(None));
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    static REFUSAL: RwSignal<Option<String>> = detached(|| signal(None));
    static CONFIRMATION: RwSignal<Option<String>> = detached(|| signal(None));
    static SAID: Cell<u64> = const { Cell::new(0) };
}

/// How long the strip keeps saying something before it lets it go.
pub const SAID_FOR: Duration = Duration::from_secs(6);

/// Why the last thing asked of the mode was not done — a key, a press, a drop — as the strip says it: for [`SAID_FOR`], until something else is said, or until the mode changes or something else is asked.
pub fn refusal() -> ReadSignal<Option<String>> {
    REFUSAL.with(|refusal| refusal.read_only())
}

/// What the last thing asked of the mode did where that is not on screen to see — "Undone: …", a fork of the built-in layout — as the strip says it: for [`SAID_FOR`], or until something else is said or the mode changes.
pub fn confirmation() -> ReadSignal<Option<String>> {
    CONFIRMATION.with(|confirmation| confirmation.read_only())
}

/// Says in the strip why what was just asked was not done, so a refusal is never only a line in the log.
pub fn refuse(why: impl std::fmt::Display) {
    let why = why.to_string();
    tracing::info!("{why}");
    say(Some(why), None);
}

/// Says in the strip what was just done, in place of whatever it said before.
pub fn confirm(what: impl std::fmt::Display) {
    let what = what.to_string();
    tracing::info!("{what}");
    say(None, Some(what));
}

fn say(refusal: Option<String>, confirmation: Option<String>) {
    let serial = SAID.with(util::serial::next_serial);
    REFUSAL.with(|slot| put(*slot, refusal));
    CONFIRMATION.with(|slot| put(*slot, confirmation));
    platform_wayland::timeout(SAID_FOR, move || unsay(serial));
}

/// Takes what the strip says away once it has been said for [`SAID_FOR`], unless something else has been said since `serial` said it.
pub(crate) fn unsay(serial: u64) {
    if SAID.with(Cell::get) == serial {
        forget_said();
    }
}

/// The serial of the last thing said, which [`unsay`] is handed when its time is up.
#[cfg(test)]
pub(crate) fn said_serial() -> u64 {
    SAID.with(Cell::get)
}

fn forget_said() {
    REFUSAL.with(|slot| put(*slot, None));
    CONFIRMATION.with(|slot| put(*slot, None));
}

fn put(slot: RwSignal<Option<String>>, value: Option<String>) {
    if slot.peek() != value {
        slot.set(value);
    }
}

/// What came of something asked of the mode — a button, a menu row, a drop — which has nowhere else to say it once it has run: a refusal goes to the strip.
pub(crate) fn said(done: Result<(), crate::session::EditError>) {
    if let Err(why) = done {
        refuse(why);
    }
}

/// Takes the last refusal out of the strip, as something new is asked.
pub(crate) fn clear_refusal() {
    REFUSAL.with(|slot| put(*slot, None));
}

/// The mode being edited, as a signal: read inside an effect or a build, it runs again when a mode is entered, switched or left. This is how a tool learns which mode it is in.
pub fn active() -> ReadSignal<Option<Mode>> {
    ACTIVE.with(|active| active.read_only())
}

/// The mode being edited right now, without subscribing.
pub fn current() -> Option<Mode> {
    ACTIVE.with(|active| active.peek())
}

/// [`current`], for what only a mode can do — open the palette, make a bar on the edited screen — refused outside one.
pub(crate) fn required() -> Result<Mode, crate::session::EditError> {
    current().ok_or_else(|| crate::session::EditError::Refused(telar::t!("editor.refused.no_mode")))
}

/// Whether `node`'s layer is the one being edited on its screen. A context menu offers an item's own actions in and out of edit mode (TA-4) and adds the entries that act on the mode — a new stack, a new grid — only where this holds.
pub fn editing(node: &surfaces::rects::Node) -> bool {
    current().is_some_and(|mode| {
        mode.layer == node.layer && node.output.as_deref() == Some(mode.output.as_str())
    })
}

/// Ends the mode on its own when the screen it edits goes away. The windows on that screen are already gone by then and the host's transient went with them without closing, so this is the one place the session's holds on the compositor are given back. Installed once, on the driver thread.
pub fn install() {
    if WATCHING.with(|watching| watching.replace(true)) {
        return;
    }
    detached(|| {
        effect(|| {
            let Some(mode) = active().get() else {
                return;
            };
            if reconcile::desktop(Some(&mode.output)).is_none() {
                tracing::info!(
                    layer = %mode.layer,
                    output = mode.output,
                    "the screen being edited went away; leaving its edit mode"
                );
                leave();
            }
        });
    });
}

/// Edits `layer` on `output`, or on the focused screen when none is named, leaving whatever mode was up first. Entering the mode already up changes nothing.
pub fn enter(layer: LayerKind, output: Option<&str>) -> Result<Mode, String> {
    enter_as(layer, output, &Compositor::now())
}

pub(crate) fn enter_as(
    layer: LayerKind,
    output: Option<&str>,
    compositor: &Compositor,
) -> Result<Mode, String> {
    if layer == LayerKind::Lock && compositor.locked {
        return Err(telar::t!("editor.refused.locked"));
    }
    if surfaces::layouts::read(LayoutStore::is_safe).unwrap_or(false) {
        return Err(StoreError::Safe.to_string());
    }
    let output = edited_output(output)?;
    let refused = match layer {
        LayerKind::Lock => compositor
            .lockable
            .clone()
            .err()
            .map(|reason| telar::t!("editor.refused.cannot_lock", reason = reason)),
        _ => None,
    };
    let mode = Mode {
        layer,
        output,
        refused,
    };
    if current().as_ref() == Some(&mode) {
        return Ok(mode);
    }
    leave();
    let session = host::open(&mode, compositor.restack);
    SESSION.with(|held| *held.borrow_mut() = Some(session));
    forget_said();
    ACTIVE.with(|active| active.set(Some(mode.clone())));
    Ok(mode)
}

/// Edits `layer` on the screen already being edited, which is what the strip's switcher asks for.
pub fn switch(layer: LayerKind) {
    let Some(mode) = current() else {
        return;
    };
    if let Err(why) = enter(layer, Some(&mode.output)) {
        tracing::warn!(layer = %layer, "could not switch edit mode: {why}");
    }
}

/// Leaves the mode that is up, answering with what it was; `None` when nothing was being edited.
pub fn leave() -> Option<Mode> {
    let session = SESSION.with(|held| held.borrow_mut().take());
    let left = ACTIVE.with(|active| {
        let left = active.peek();
        if left.is_some() {
            active.set(None);
        }
        left
    });
    drop(session);
    forget_said();
    left
}

/// Leaves the mode only if it is still the session `serial` opened: the host's transient closing (Esc, or anything else that closes it) ends the mode it belongs to, and never one entered after it.
pub(crate) fn leave_session(serial: u64) {
    let current = SESSION.with(|held| held.borrow().as_ref().map(Session::serial));
    if current == Some(serial) {
        leave();
    }
}

/// The screen a mode is entered on: the one named, which must be one the shell draws on, else the focused one, else the first.
fn edited_output(named: Option<&str>) -> Result<String, String> {
    let screens: Vec<String> = reconcile::desktops()
        .iter()
        .filter_map(|desktop| desktop.output.clone())
        .collect();
    match named {
        Some(name) if screens.iter().any(|screen| screen == name) => Ok(name.to_string()),
        Some(name) => Err(telar::t!(
            "editor.refused.no_such_output",
            output = name,
            outputs = screens.join(", ")
        )),
        None => transient::focused_output()
            .filter(|focused| screens.contains(focused))
            .or_else(|| screens.first().cloned())
            .ok_or_else(|| telar::t!("editor.refused.no_output")),
    }
}

/// What a layer's mode is called in the strip.
pub fn name_of(layer: LayerKind) -> String {
    match layer {
        LayerKind::Background => telar::t!("editor.layer.background"),
        LayerKind::Desktop => telar::t!("editor.layer.desktop"),
        LayerKind::Top => telar::t!("editor.layer.top"),
        LayerKind::Overlay => telar::t!("editor.layer.overlay"),
        LayerKind::Lock => telar::t!("editor.layer.lock"),
    }
}

/// The glyph a layer's mode is drawn with, beside its name.
pub fn icon_of(layer: LayerKind) -> &'static str {
    match layer {
        LayerKind::Background => "image",
        LayerKind::Desktop => "layout-grid",
        LayerKind::Top => "panel-top",
        LayerKind::Overlay => "layers",
        LayerKind::Lock => "lock",
    }
}

/// What stands in for a mark on a line that has none, so the lines of a list line up.
const NO_MARK: &str = "   ";

/// `text` with `mark` in front of it where `on`, and blank where not.
pub(crate) fn marked(on: bool, mark: &str, text: &str) -> String {
    let lead = if on { mark } else { NO_MARK };
    format!("{lead}{text}")
}
