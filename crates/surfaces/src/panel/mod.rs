mod owned;

use std::rc::Rc;

use config::OpenMode;
use layout::{InstanceId, Resolved};
use telar::Rect;
use ui::chrome::Chrome;
use ui::descriptor;
use ui::host::{Audience, Host, Instance, Representation};
use ui::module::{Placement, Pressed, from_chip, pressed_chip};

use crate::rects::Node;
use crate::transient::{self, Motion, Place, Slot, Spec, chips};
use crate::{drawer, float, popout};

pub use owned::{Owner, owner_area};

/// Where a module's panel opens and whom it speaks for.
struct Opening {
    output: Option<String>,
    place: Option<Place>,
    instance: Instance,
    from: Option<Node>,
}

/// A drawer hangs off the pressed chip, else its module's chip on the focused screen, else the middle of the screen, so a press, a drag, IPC and a keybind all open the same transient. The instance that chip is — else the module's first on that screen, else its first anywhere — is the one whose options size and fill the panel.
pub fn toggle_panel(module_id: &str) {
    if !descriptor::has_panel(module_id) {
        tracing::warn!("'{module_id}' has no panel to toggle");
        return;
    }
    if is_panel_open(module_id) {
        close_panel(module_id);
        return;
    }
    let focused = transient::focused_output();
    let opening = match chips::find(module_id, pressed_chip(), focused.as_deref()) {
        Some(found) => Opening {
            output: found.place.output(),
            place: Some(found.place),
            instance: found.instance,
            from: Some(found.at),
        },
        None => Opening {
            instance: crate::reconcile::instance_of(module_id, focused.as_deref()),
            output: focused,
            place: None,
            from: None,
        },
    };
    open_module_panel(module_id, opening);
}

fn open_module_panel(module_id: &str, opening: Opening) {
    // A panel and the hover card of the same chip say the same thing twice, overlapping, and the card is the one the user did not ask for: it opened by resting the pointer somewhere.
    popout::close();
    let Opening {
        output,
        place,
        instance,
        from,
    } = opening;
    let config = config::config_for(output.as_deref());
    let module = module_id.to_string();
    let spec = match instance.presentation(&config).open_mode(module_id) {
        OpenMode::Drawer => {
            let place = place.unwrap_or(Place::Centred);
            let motion = place.motion();
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
            .from(from)
            .keyboard(descriptor::wants_keyboard(module_id))
            .on_close(move || descriptor::closed(&module)),
    );
}

/// What a chip placed outside a bar, built under `host` at `at`, does when nothing bound and no owned panel answers its press: what its module's chip does on a bar — its own press, else its module's panel — run with this chip pressed, so what it opens hangs off this chip; and the settings window where its module is missing. Nothing on the lock layer.
pub fn chip_press(host: &Host, at: &Node) -> Option<Rc<dyn Fn()>> {
    if host.representation != Representation::Chip || host.audience != Audience::Owner {
        return None;
    }
    let Some(module) = descriptor::find(host.module()) else {
        return Some(Rc::new(ui::placeholder::open_settings));
    };
    let own: Rc<dyn Fn()> = match module.representations.chip.and_then(|chip| chip.press) {
        Some(press) => Rc::new(press),
        None if descriptor::has_panel(module.id) => {
            let id = module.id;
            Rc::new(move || toggle_panel(id))
        }
        None => return None,
    };
    let at = at.clone();
    Some(Rc::new(move || match pressed_at(&at) {
        Some(pressed) => from_chip(pressed, || own()),
        None => own(),
    }))
}

/// The chip at `at` as a press of it names it, where the chip itself is drawn now.
fn pressed_at(at: &Node) -> Option<Pressed> {
    let (_, _, rect) = crate::rects::chips()
        .into_iter()
        .rfind(|(node, _, _)| node == at)?;
    Some(pressed(at.clone(), rect))
}

fn pressed(at: Node, rect: Rect) -> Pressed {
    Pressed {
        rect,
        output: at.output.clone(),
        placement: Some(Placement::new(at)),
    }
}

/// What `panel toggle <id>` opens: for an instance, what a press on it opens past a bound action — the panel the layout gives it, else its module's panel hung off it — and for a module id its panel, as [`toggle_panel`] does. An instance on the focused screen is found first.
pub fn toggle_named(id: &str) -> Result<(), String> {
    let placed = placed_instance(id);
    if let Some((owner, _)) = &placed
        && owned::toggle(owner)
    {
        return Ok(());
    }
    if descriptor::has_panel(id) {
        toggle_panel(id);
        return Ok(());
    }
    let Some((owner, module)) = placed.filter(|(_, module)| descriptor::has_panel(module)) else {
        return Err(format!("'{id}' has no panel"));
    };
    let chip = crate::rects::instance(owner.output.as_deref(), &owner.instance)
        .map(|(node, slot)| pressed_at(&node).unwrap_or_else(|| pressed(node, slot)));
    match chip {
        Some(chip) => from_chip(chip, || toggle_panel(&module)),
        None => toggle_panel(&module),
    }
    Ok(())
}

/// The instance `id` and its module, on the focused screen, else on the first that places it.
fn placed_instance(id: &str) -> Option<(Owner, String)> {
    let instance = InstanceId::new(id);
    let focused = transient::focused_output();
    let desktops = crate::reconcile::desktops_now();
    let focused_first = desktops
        .iter()
        .filter(|desktop| desktop.output == focused)
        .chain(desktops.iter().filter(|desktop| desktop.output != focused));
    for desktop in focused_first {
        if let Some(placed) = desktop
            .resolved
            .instances()
            .find(|placed| placed.id == instance)
        {
            let owner = Owner {
                output: desktop.output.clone(),
                instance,
            };
            return Some((owner, placed.module.clone()));
        }
    }
    None
}

/// Opens a module's panel where it is not up: what IPC's `panel open` and the settings window's openers ask for.
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

/// Whether the instance at `node` owns a panel in the layout the windows show.
pub fn owns_panel(node: &Node) -> bool {
    owned::owns_panel(node)
}

/// Toggles the panel the instance at `node` owns, answering whether it owns one.
pub fn toggle_owned(node: &Node) -> bool {
    Owner::of(node).is_some_and(|owner| owned::toggle(&owner))
}

/// Opens the panel the instance at `node` owns, answering whether it owns one.
pub(crate) fn open_owned_at(node: &Node) -> bool {
    Owner::of(node).is_some_and(|owner| owned::open(&owner))
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
