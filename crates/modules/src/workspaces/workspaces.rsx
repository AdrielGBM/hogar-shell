[logic]
use crate::workspaces::{PillGridProps, pill_grid};
use crate::workspaces::{Pill, PillStyle, pills};
use ::config::theme::NordTheme;
use ::services::hyprland::{self, Snapshot};

// Routed per click rather than bound here, so the handler stays capture-free and the choice between activating over `ext-workspace-v1` and dispatching over Hyprland's socket lives with the service that owns both.
fn focus(id: i32) {
    hyprland::focus_workspace_id(id);
}

let host = ui::host::Host::current()?;
let config = host.options::<::config::WorkspacesConfig>();
let output = host.output.clone();

let occupied_background = config.occupied_background;
let indicator = config.indicator;
let trail = config.trail();

// Seeded from the last snapshot rather than left empty until the first event lands: the subscription below delivers it, but only on the next turn of the loop, so the bar's first frame would draw an empty row.
let list = signal(
    hyprland::current_workspaces()
        .map(|snap| pills(&snap, &config, output.as_deref()))
        .unwrap_or_default(),
);
let items = list.read_only();
// Subscribe to the single shared workspaces source; the consumer writes the signal on this surface's thread.
platform_wayland::watch(hyprland::subscribe, move |snap: Snapshot| {
    list.set(pills(&snap, &config, output.as_deref()));
});

let style = PillStyle {
    theme: use_theme::<NordTheme>(),
    radius: host.corner_radius(),
    side: host.thickness(),
    vertical: host.is_vertical(),
    occupied_background,
    indicator,
    spring: host.config().animation.chase(),
    trail,
};
// A stretched horizontal chip can't derive its width from its height, so both sides are sized to make a square, and the pills are drawn again at the thickness the chip is given whenever that changes.
let side = memo({
    let host = host.clone();
    move || host.thickness()
});
[view]
match $side as side key side.to_bits()
    side
        pill_grid items:items style:(PillStyle { side, ..style }) on_press:focus

[preview "Workspaces" fixture:crate::preview::workspaces]
workspaces
