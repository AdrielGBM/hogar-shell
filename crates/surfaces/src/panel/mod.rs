mod owned;

use std::rc::Rc;

use config::OpenMode;
use layout::Resolved;
use ui::chrome::Chrome;
use ui::descriptor;
use ui::module::pressed_chip;

use crate::rects::{Node, Part};
use crate::transient::{self, Motion, Place, Slot, Spec, chips};
use crate::{drawer, float, popout};

pub use owned::Owner;

/// A drawer hangs off the pressed chip, else its module's chip on the focused screen, else the middle of the screen, so a press, a drag, IPC and a keybind all open the same transient. The instance that chip is — else the module's first on that screen, else its first anywhere — is the one whose options size and fill the panel (TA-2).
pub fn toggle_panel(module_id: &str) {
    if !descriptor::has_panel(module_id) {
        tracing::warn!("'{module_id}' has no panel to toggle");
        return;
    }
    if is_panel_open(module_id) {
        close_panel(module_id);
        return;
    }
    // A panel and the hover card of the same chip say the same thing twice, overlapping, and the card is the one the user did not ask for: it opened by resting the pointer somewhere.
    popout::close();
    let focused = transient::focused_output();
    let found = chips::find(module_id, pressed_chip(), focused.as_deref());
    let output = found
        .as_ref()
        .map_or(focused, |found| found.anchor.output.clone());
    let (anchor, instance) = match found {
        Some(found) => (Some(found.anchor), found.instance),
        None => (
            None,
            crate::reconcile::instance_of(module_id, output.as_deref()),
        ),
    };
    let config = config::config_for(output.as_deref());
    let module = module_id.to_string();
    let spec = match instance.presentation(&config).open_mode(module_id) {
        OpenMode::Drawer => {
            let (place, motion) = match anchor {
                Some(anchor) => {
                    let edge = anchor.edge;
                    (Place::Beside(anchor), Motion::Slide(edge))
                }
                None => (Place::Centred, Motion::Fade),
            };
            Spec::new(
                module_id,
                place,
                Rc::new(move |chrome: &Chrome| drawer::content(&instance, chrome)),
            )
            .slot(Slot::Drawer)
            .dismiss_on_outside()
            .motion(motion)
        }
        OpenMode::Float => Spec::new(
            module_id,
            Place::Centred,
            Rc::new(move |chrome: &Chrome| float::content(&instance, chrome)),
        )
        .slot(Slot::Standing)
        .motion(Motion::Fade),
    };
    transient::toggle(
        spec.output(output)
            .keyboard(descriptor::wants_keyboard(module_id))
            .on_close(move || descriptor::closed(&module)),
    );
}

/// A chip pulled away from its bar opens what its press would: the panel the layout gives its instance, where it has one.
pub fn open_panel(module_id: &str) {
    if let Some(owner) = pressed_owner() {
        owned::open(&owner);
        return;
    }
    if !is_panel_open(module_id) {
        toggle_panel(module_id);
    }
}

pub fn close_panel(module_id: &str) {
    transient::close(module_id);
}

pub fn is_panel_open(module_id: &str) -> bool {
    transient::is_open(module_id)
}

/// Whether the instance at `node` owns a panel in the layout the windows show.
pub fn owns_panel(node: &Node) -> bool {
    owned::owns_panel(node)
}

/// Toggles the panel the instance at `node` owns, answering whether it owns one.
pub fn toggle_owned(node: &Node) -> bool {
    Owner::of(node).is_some_and(|owner| owned::toggle(&owner))
}

#[cfg(test)]
pub(crate) fn open_owned(owner: &Owner) -> bool {
    owned::open(owner)
}

#[cfg(test)]
pub(crate) fn close_owned(owner: &Owner) {
    owned::close(owner);
}

#[cfg(test)]
pub(crate) fn is_owned_open(owner: &Owner) -> bool {
    owned::is_open(owner)
}

pub(crate) fn prune_owned(desktops: &[(Option<&str>, &Resolved)]) {
    owned::prune(desktops);
}

/// The instance whose chip a press or a drag came from, where it owns a panel.
fn pressed_owner() -> Option<Owner> {
    let pressed = pressed_chip()?;
    let node = pressed.placement.as_ref()?.get::<Node>()?;
    match (&node.part, owns_panel(node)) {
        (Part::Instance(..), true) => Owner::of(node),
        _ => None,
    }
}
