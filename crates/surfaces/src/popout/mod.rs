//! Hover popouts: the pointer has to rest on a chip before a card opens, and the card outlives the pointer leaving long enough for it to reach the card. The card is a transient in the chip's own window, so hovering maps nothing.

use std::cell::RefCell;
use std::rc::Rc;

use platform_wayland::timeout;
use telar::{Color, LayoutError, LayoutItem, LayoutStyle, RectStyle, StyledContainer};

use ui::chrome::Chrome;
use ui::descriptor;
use ui::host::{Host, Instance, Representation, Size};

use crate::transient::{self, Anchor, Place, Spec};

/// One popout at a time: a second card on screen would be two readouts competing for the same glance.
const ID: &str = "popout";

thread_local! {
    static STATE: RefCell<Popout> = const { RefCell::new(Popout::new()) };
}

struct Popout {
    target: Option<String>,
    /// Bumped on every hover transition, on the chip and on the card alike. A delay that fires after the pointer has moved on reads a generation that no longer matches and does nothing — cheaper and more reliable than cancelling a timer, and it is the whole of the arbitration: entering the card voids the close the chip scheduled on the way out, without either side tracking where the pointer is.
    generation: u64,
}

impl Popout {
    const fn new() -> Self {
        Self {
            target: None,
            generation: 0,
        }
    }
}

fn bump() -> u64 {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        state.generation = state.generation.wrapping_add(1);
        state.generation
    })
}

fn current(generation: u64) -> bool {
    STATE.with(|s| s.borrow().generation == generation)
}

fn showing(module_id: &str) -> bool {
    transient::is_open(ID) && STATE.with(|s| s.borrow().target.as_deref() == Some(module_id))
}

pub fn close() {
    STATE.with(|s| s.borrow_mut().target = None);
    transient::close(ID);
}

/// Both directions are scheduled rather than acted on: an instant open would fire on a bar the pointer is only crossing, and an instant close while it crosses towards the card. The card is the hovered chip's instance's, sized by its options.
pub fn hover(instance: &Instance, anchor: Anchor, entered: bool) {
    let config = config::config_for(anchor.output.as_deref());
    if !config.popouts.enabled {
        return;
    }
    let generation = bump();
    if entered {
        // Re-entering the chip under an open card has already voided the pending close, so nothing is left to schedule.
        if showing(&instance.module) {
            return;
        }
        let instance = instance.clone();
        timeout(config.popouts.open_after(), move || {
            if current(generation) {
                open(&instance, anchor);
            }
        });
    } else {
        timeout(config.popouts.close_after(), move || {
            if current(generation) {
                close();
            }
        });
    }
}

/// The card's own pointer tracking, so moving into the popout keeps it up and leaving it starts the same grace the chip does. Without this the popout would close under a pointer resting on it.
fn keep_open(entered: bool) {
    let generation = bump();
    // Entering needs no work of its own: the bump has already voided the close the chip scheduled on the way here.
    if entered {
        return;
    }
    let delay = config::config()
        .map(|c| c.popouts.close_after())
        .unwrap_or_default();
    timeout(delay, move || {
        if current(generation) {
            close();
        }
    });
}

fn open(instance: &Instance, anchor: Anchor) {
    let module_id = &*instance.module;
    if !descriptor::has_popout(module_id) {
        return;
    }
    // The module's own panel is already showing what the card would preview, and two of it — one hanging off the chip, one over it — is harder to read than either alone. Checked here rather than at the hover, because the delay is long enough for the panel to open inside it.
    if crate::panel::is_panel_open(module_id) {
        return;
    }
    STATE.with(|s| s.borrow_mut().target = Some(module_id.to_string()));
    let instance = instance.clone();
    transient::open(Spec::new(
        ID,
        Place::Beside(anchor),
        Rc::new(move |chrome: &Chrome| popout_content(&instance, chrome)),
    ));
}

pub(crate) fn preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    // Seeded here rather than inherited: previews share a process, so a card that relied on another one having published a reading first would draw a different number depending on the order they ran in.
    services::volume::seed(services::volume::Volume {
        level: 64,
        muted: false,
    });
    let speakers = "alsa_output.pci-0000_0a_00.6.analog-stereo";
    services::pipewire::seed(services::pipewire::Graph {
        nodes: vec![services::pipewire::Node {
            id: 48,
            name: speakers.to_string(),
            description: "Family 17h/19h HD Audio Controller Analog Stereo".to_string(),
            app: String::new(),
            media: String::new(),
            icon: String::new(),
            kind: services::pipewire::NodeKind::Sink,
            level: 64,
            muted: false,
        }],
        default_sink: speakers.to_string(),
        default_source: String::new(),
    });
    popout_content(&Instance::of_module("volume"), &ui::preview::chrome())
}

pub fn popout_content(
    instance: &Instance,
    chrome: &Chrome,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (width, height) = instance.presentation(&chrome.config).popout_size();
    let host = Host::in_chrome(
        instance.clone(),
        Representation::Popout,
        chrome,
        Size { width, height },
    );
    let card = descriptor::place(&instance.module, &host, LayoutStyle::new().flex_column())?;
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new(),
            |_| RectStyle::filled(Color::TRANSPARENT, 0.0),
            vec![card],
        )?
        .on_hover(keep_open),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Config;

    #[test]
    fn a_disabled_section_never_schedules_anything() {
        let config: Config = toml::from_str("[popouts]\nenabled = false\n").expect("config parses");
        assert!(!config.popouts.enabled);
        assert!(Config::starter().popouts.enabled, "on by default");
    }

    #[test]
    fn the_delays_are_clamped_so_a_typo_cannot_make_a_popout_instant() {
        let config: Config =
            toml::from_str("[popouts]\nopen_delay = 0\nclose_delay = 0\n").expect("config parses");
        assert!(
            config.popouts.open_after().as_millis() >= 60,
            "an instant popout would fire on a bar the pointer is only crossing"
        );
        assert!(
            config.popouts.close_after().as_millis() >= 60,
            "an instant close would fire while the pointer crosses towards the card"
        );
    }

    #[test]
    fn a_module_with_no_card_builds_a_placeholder_popout() {
        telar::reset_layout_runtime();
        telar::set_theme(Config::starter().resolve_theme());
        assert!(
            popout_content(
                &Instance::of_module("nothing-registered"),
                &ui::preview::chrome()
            )
            .is_ok()
        );
    }
}
