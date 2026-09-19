//! Hover popouts: the readout a chip shows while the pointer rests on it.
//!
//! Distinct from the drawer a click opens, and the shell's primary status interaction: a bar chip has room for one glyph, and everything that glyph stands for — the level behind it, the sensor it came from, the whole window title it truncated — lives here.
//!
//! Three things make it a popout rather than a flicker. **Delays**: the pointer has to rest on a chip before anything opens, and the popout survives long enough after leaving for the pointer to reach it. **One surface**: moving from chip to chip replaces the card rather than stacking a second one. **A carved input region**: the surface is sized to the tallest card a popout may be, and everything the card doesn't cover falls through to the window underneath, so a popout that is up while you click elsewhere costs nothing.
//!
//! Placement is [`shared::anchor`](ui::anchor), the same helper the tray's context menus use.

use std::cell::RefCell;
use std::sync::Arc;

use platform_wayland::timeout;
use telar::{
    AlignItems, Color, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, Rect,
    RectStyle, SizeDimension, StyledContainer,
};

use config::Edge;
use config::SurfaceEnv;
use ui::descriptor;
use ui::host::{Host, InstanceId, Representation, Size};
use ui::panel::PanelSurface;
use ui::placement::{OffChip, Placement};

/// One popout at a time: a second card on screen would be two readouts competing for the same glance.
const SURFACE_ID: &str = "popout";

thread_local! {
    static STATE: RefCell<Popout> = const { RefCell::new(Popout::new()) };
}

struct Popout {
    /// The module whose popout is up, or on its way there.
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
    crate::shell::window_is_open(SURFACE_ID)
        && STATE.with(|s| s.borrow().target.as_deref() == Some(module_id))
}

pub fn close() {
    STATE.with(|s| s.borrow_mut().target = None);
    crate::shell::close(SURFACE_ID);
}

/// The bar reports every chip's pointer enter and leave here. Both directions are scheduled rather than acted on: an instant open would fire on a bar the pointer is only crossing, and an instant close would fire while the pointer crosses the gap towards the card it just opened.
///
/// `chip` is the rect as it stands now, not the signal behind it: the timer fires on the shared loop rather than inside the bar surface, and a chip cannot move under a resting pointer without the bar being rebuilt, which tears this down anyway.
pub fn hover(module_id: &str, chip: Rect, entered: bool) {
    let Some(env) = config::surface_env() else {
        return;
    };
    if !env.config.popouts.enabled {
        return;
    }
    let generation = bump();
    if entered {
        // Re-entering the chip under an open card has already voided the pending close, so nothing is left to schedule.
        if showing(module_id) {
            return;
        }
        let module = module_id.to_string();
        timeout(env.config.popouts.open_after(), move || {
            if current(generation) {
                open(&module, chip, &env);
            }
        });
    } else {
        timeout(env.config.popouts.close_after(), move || {
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

/// Opens `module_id`'s card under its chip, replacing whatever was up. Runs on the driver thread — it is reached from a hover handler through a timer on the shared loop, which is where a surface may be opened.
fn open(module_id: &str, chip: Rect, env: &SurfaceEnv) {
    if !descriptor::has_popout(module_id) {
        return;
    }
    // The module's own panel is already showing what the card would preview, and two of it — one hanging off the chip, one over it — is harder to read than either alone. Checked here rather than at the hover, because the delay is long enough for the panel to open inside it: pressing a chip the pointer is already resting on schedules the card first and opens the panel second.
    if crate::panel::is_panel_open(module_id) {
        return;
    }
    // The delay is long enough for a config reload to land inside it, and a card built against the outgoing config would carry a stale theme; compared against *this screen's* config, because the global one is a different `Arc` whenever the compositor names its outputs, which is always.
    if !Arc::ptr_eq(&config::config_for(env.output.as_deref()), &env.config) {
        return;
    }
    close();
    STATE.with(|s| s.borrow_mut().target = Some(module_id.to_string()));
    let module = module_id.to_string();
    let placement = placement(env, chip);
    crate::shell::toggle_window(SURFACE_ID, move || {
        PanelSurface::new(placement, move |env| popout_content(&module, env)).open()
    });
}

/// The surface is the tallest a popout may be, not the size of this card: a layer surface pinned to two edges has to name a size, and a card's height is content-derived. `interactive_input_region` is what makes that affordable — the compositor is handed only the card's own rect, so the surplus stays click-through.
fn placement(env: &SurfaceEnv, chip: Rect) -> Placement {
    let popouts = env.config.popouts;
    let (width, height) = (popouts.card_width(), popouts.card_height());
    let span = if env.edge.is_vertical() {
        height
    } else {
        width
    };
    Placement::off_chip(OffChip::Card, env, Some(chip), Some(span))
        .size(width.ceil() as u32, height.ceil() as u32)
}

/// Which corner of its surface the card sits in — the one the surface is anchored to, so the card lands exactly where the margin put that corner instead of floating in the middle of an oversized surface.
fn corner_alignment(edge: Edge) -> (JustifyContent, AlignItems) {
    match edge {
        Edge::Top | Edge::Left => (JustifyContent::START, AlignItems::START),
        Edge::Bottom => (JustifyContent::START, AlignItems::END),
        Edge::Right => (JustifyContent::END, AlignItems::START),
    }
}

fn corner_style(edge: Edge) -> LayoutStyle {
    let (justify, align) = corner_alignment(edge);
    LayoutStyle::new()
        .flex_row()
        .width(SizeDimension::Percent(1.0))
        .height(SizeDimension::Percent(1.0))
        .justify_content(justify)
        .align_items(align)
}

/// The volume chip's hover card, for [`crate::preview`] — the card that exercises every part one has: glyph, title, reading, meter and the device line under it.
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
    popout_content("volume", &ui::preview::bar_surface())
}

/// Builds a popout's tree for `module_id`; public so a surface that only *presents* one — and a preview — can build it without a compositor.
pub fn popout_content(
    module_id: &str,
    env: &SurfaceEnv,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let popouts = env.config.popouts;
    let host = Host::on_surface(
        InstanceId::of_module(module_id),
        Representation::Popout,
        env,
        Size {
            width: popouts.card_width(),
            height: popouts.card_height(),
        },
    );
    let card = descriptor::place(module_id, &host, LayoutStyle::new().flex_column())?;
    let tracked = StyledContainer::new(
        LayoutStyle::new(),
        |_| RectStyle::filled(Color::TRANSPARENT, 0.0),
        vec![card],
    )?
    .on_hover(keep_open);
    Ok(Box::new(Container::new(
        corner_style(env.edge),
        vec![Box::new(tracked)],
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::Config;
    use platform_wayland::KeyboardInteractivity;

    fn env(edge: Edge) -> SurfaceEnv {
        SurfaceEnv::for_edge(Arc::new(Config::starter()), edge, None)
    }

    fn chip() -> Rect {
        Rect {
            x: 400.0,
            y: 0.0,
            width: 30.0,
            height: 30.0,
        }
    }

    #[test]
    fn the_surface_pins_itself_to_the_bar_edge_and_the_one_it_runs_along() {
        // A margin only positions a layer surface on an edge it is anchored to, so pinning the bar edge alone would centre the popout on screen instead of lining it up with its chip.
        for edge in Edge::ALL {
            let anchor = placement(&env(edge), chip()).layer_config().anchor;
            assert_eq!(
                anchor.iter().count(),
                2,
                "{edge:?} must pin both the off-bar and the along-bar axis"
            );
        }
    }

    #[test]
    fn the_surface_never_reserves_space_or_takes_the_keyboard() {
        let config = placement(&env(Edge::Top), chip()).layer_config();
        assert_eq!(
            config.exclusive_zone, 0,
            "a popout must not carve space out of the desktop"
        );
        assert!(matches!(
            config.keyboard_interactivity,
            KeyboardInteractivity::None
        ));
        assert!(
            config.interactive_input_region,
            "the surplus around the card has to stay click-through"
        );
        assert!(
            !config.input_transparent,
            "the card itself must take the pointer"
        );
    }

    #[test]
    fn a_disabled_section_never_schedules_anything() {
        let config: Config = toml::from_str("[popouts]\nenabled = false\n").expect("config parses");
        assert!(!config.popouts.enabled);
        // The guard lives in `hover`, which needs a surface; the flag it returns on is what a unit test can reach.
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

    /// An id the hover wiring let through with no card builds a placeholder rather than taking the surface down.
    #[test]
    fn a_module_with_no_card_builds_a_placeholder_popout() {
        telar::reset_layout_runtime();
        telar::set_theme(Config::starter().resolve_theme());
        assert!(popout_content("nothing-registered", &env(Edge::Top)).is_ok());
    }

    #[test]
    fn the_card_pins_itself_into_the_corner_its_surface_is_anchored_to() {
        assert_eq!(corner_alignment(Edge::Bottom).1, AlignItems::END);
        assert_eq!(corner_alignment(Edge::Right).0, JustifyContent::END);
        assert_eq!(corner_alignment(Edge::Top).1, AlignItems::START);
        assert_eq!(corner_alignment(Edge::Left).0, JustifyContent::START);
    }
}
