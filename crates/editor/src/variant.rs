//! Editing one workspace's variant of the edited screen (TA-2, TA-5): with it switched on, every edit of the mode is written into the workspace rule of the workspace that is up, and resolves on that workspace alone.
//!
//! It is a setting of the mode, not of one tool, so a region split, a popover row and a keyboard move all land in the same place while it is on, and the strip says which workspace that is. It follows the workspace that is up — what is being edited is always what is on screen — and goes off with the mode. Only some layers have it ([`allowed`]).

use telar::{RwSignal, detached, effect, signal};

use layout::{LayerKind, WorkspaceMatch};
use surfaces::reconcile;
use surfaces::rects::Node;

use crate::keys::{self, Chord, KeyOp, Run};
use crate::mode::{self, Mode};
use crate::session::EditError;

thread_local! {
    static ONLY: RwSignal<bool> = detached(|| signal(false));
}

/// Adds `w`, which switches the variant on and off, to the mode of every layer that has one, and switches it off whenever the mode changes. Installed once, on the driver thread.
pub(crate) fn install() {
    for layer in LayerKind::SESSION
        .into_iter()
        .filter(|layer| allowed(*layer))
    {
        keys::add_mode_key_op(
            layer,
            KeyOp {
                name: "workspace-variant",
                keys: vec![Chord::char('w')],
                label: || telar::t!("editor.keys.op.workspace-variant"),
                run: Run::Act(|_| toggle()),
            },
        );
    }
    detached(|| {
        effect(|| {
            mode::active().with(|_| ());
            ONLY.with(|only| {
                if only.peek() {
                    only.set(false);
                }
            })
        });
    });
}

/// Whether edits are written for the workspace that is up alone, as a signal a toggle row binds to.
pub fn only() -> RwSignal<bool> {
    ONLY.with(|only| *only)
}

/// The workspace edits are written for now, `None` for every workspace. Reactive: it follows the switch, the mode and the workspace that is up.
pub fn workspace() -> Option<WorkspaceMatch> {
    ONLY.with(|only| only.get())
        .then(|| up_on(mode::active().get(), reconcile::desktop))
        .flatten()
}

/// [`workspace`] as it is now: where an edit made now lands. Not reactive, so an edit planned inside the effect that previews it does not run again because its preview changed the screen.
pub fn editing() -> Option<WorkspaceMatch> {
    ONLY.with(|only| only.peek())
        .then(|| up_on(mode::current(), reconcile::desktop_now))
        .flatten()
}

/// The workspace that is up on the screen being edited, where the compositor has said, and `None` outside a mode or in the mode of a layer without variants ([`allowed`]). Reactive.
pub fn active() -> Option<WorkspaceMatch> {
    up_on(mode::active().get(), reconcile::desktop)
}

/// Whether `layer` has workspace variants. Top has none because its bars are what reserves space, and a workspace rule may not add, remove or resize a reserving area (TA-2); the lock has none because no workspace is shown over it.
pub fn allowed(layer: LayerKind) -> bool {
    refusal(layer).is_none()
}

/// Whether an edit of what `node` names is written where the variant says: anything on the screen of the mode that is up. What a workspace rule may not change there — a reserving bar's geometry, from another mode's menu or popover — is refused by the layout's own operations (`OpError::Reservation`) rather than written for every workspace behind the strip's back.
pub(crate) fn applies_to(node: &Node) -> bool {
    mode::current().is_some_and(|mode| node.output.as_deref() == Some(mode.output.as_str()))
}

/// Where an edit of what `node` names made now lands: [`editing`] where the variant [`applies_to`] it, every workspace anywhere else.
pub(crate) fn editing_for(node: &Node) -> Option<WorkspaceMatch> {
    applies_to(node).then(editing).flatten()
}

fn refusal(layer: LayerKind) -> Option<String> {
    match layer {
        LayerKind::Background | LayerKind::Desktop | LayerKind::Overlay => None,
        LayerKind::Top => Some(telar::t!("editor.variant.top")),
        LayerKind::Lock => Some(telar::t!("editor.variant.lock")),
    }
}

fn up_on(
    mode: Option<Mode>,
    desktop: fn(Option<&str>) -> Option<reconcile::Desktop>,
) -> Option<WorkspaceMatch> {
    let mode = mode?;
    if !allowed(mode.layer) {
        return None;
    }
    let workspace = desktop(Some(&mode.output))?.resolved.workspace?;
    Some(WorkspaceMatch(workspace.name))
}

/// Switches editing one workspace alone on or off, refusing to switch it on outside a mode, in the mode of a layer without variants ([`allowed`]), or where the compositor has not said which workspace is up.
pub fn set(on: bool) -> Result<(), EditError> {
    if on {
        let mode = mode::required()?;
        if let Some(why) = refusal(mode.layer) {
            return Err(EditError::Refused(why));
        }
        if up_on(Some(mode), reconcile::desktop_now).is_none() {
            return Err(EditError::Refused(telar::t!("editor.variant.unknown")));
        }
    }
    ONLY.with(|only| {
        if only.peek() != on {
            only.set(on);
        }
    });
    Ok(())
}

/// [`set`] to the other way.
pub fn toggle() -> Result<(), EditError> {
    set(!ONLY.with(|only| only.peek()))
}
