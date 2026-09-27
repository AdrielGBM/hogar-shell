use std::rc::Rc;

use config::OpenMode;
use ui::chrome::Chrome;
use ui::descriptor;
use ui::module::pressed_chip;

use crate::transient::{self, Motion, Place, Slot, Spec, chips};
use crate::{drawer, float, popout};

/// A drawer hangs off the pressed chip, else its module's chip on the focused screen, else the middle of the screen, so a press, a drag, IPC and a keybind all open the same transient.
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
    let anchor = chips::find(module_id, pressed_chip(), focused.as_deref());
    let output = anchor
        .as_ref()
        .map_or(focused, |anchor| anchor.output.clone());
    let config = config::config_for(output.as_deref());
    let module = module_id.to_string();
    let spec = match config.open_mode_for(module_id) {
        OpenMode::Drawer => {
            let (place, motion) = match anchor {
                Some(anchor) => {
                    let edge = anchor.edge;
                    (Place::Beside(anchor), Motion::Slide(edge))
                }
                None => (Place::Centred, Motion::Fade),
            };
            let shown = module.clone();
            Spec::new(
                module_id,
                place,
                Rc::new(move |chrome: &Chrome| drawer::content(&shown, chrome)),
            )
            .slot(Slot::Drawer)
            .dismiss_on_outside()
            .motion(motion)
        }
        OpenMode::Float => {
            let shown = module.clone();
            Spec::new(
                module_id,
                Place::Centred,
                Rc::new(move |chrome: &Chrome| float::content(&shown, chrome)),
            )
            .slot(Slot::Standing)
            .motion(Motion::Fade)
        }
    };
    transient::toggle(
        spec.output(output)
            .keyboard(descriptor::wants_keyboard(module_id))
            .on_close(move || descriptor::closed(&module)),
    );
}

pub fn open_panel(module_id: &str) {
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
