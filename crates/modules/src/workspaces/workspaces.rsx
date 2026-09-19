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
let config = host.options::<::config::WorkspacesConfig>().clone();
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
    // Pills round like the sibling chips instead of a fixed radius, so they follow the theme/`[shape]` radius.
    radius: host.corner_radius(),
    // A stretched horizontal chip can't derive its width from its height, so size both sides to make a square.
    side: host.thickness(),
    vertical: host.is_vertical(),
    occupied_background,
    indicator,
    spring: host.config().animation.spring(),
    trail,
};
[view]
pill_grid items:items style:style on_press:focus

[preview "Workspaces" fixture:crate::preview::workspaces]
workspaces
