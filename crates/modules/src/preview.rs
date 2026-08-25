//! What this crate's `[preview]`s stand in for: the machine a headless render cannot reach.
//!
//! Each seeds its service rather than starting it — see [`util::broadcast::Service::seed`] — so a preview draws the same thing on a laptop, on a build box and with the shell already running. The bar every chip sits on comes from [`ui::preview`].

use std::sync::Arc;

use telar::{PreviewEntry, PreviewSurface};

use services::hyprland::{Snapshot, Workspace};
use services::tray::{Pixmap, Status, TrayItem};
use services::volume::Volume;

const MONITOR: &str = "DP-1";

/// The previews this crate registers by hand, for the surfaces whose content is still built by a Rust function and so has no `.rsx` component for a `[preview]` block to hang off. Each replaces a `TELAR_VISUAL_*` test that rendered the same tree only when an environment variable asked it to.
pub fn entries() -> Vec<PreviewEntry> {
    vec![
        PreviewEntry {
            component_name: "notifications",
            preview_name: "Popup stack",
            build: crate::notifications::popups_preview,
            surface: Some(PreviewSurface::new(
                ::config::StackConfig::default().width,
                360.0,
            )),
        },
        PreviewEntry {
            component_name: "notifications",
            preview_name: "History panel",
            build: crate::notifications::panel_preview,
            surface: Some(PreviewSurface::new(340.0, 360.0)),
        },
        PreviewEntry {
            component_name: "toast",
            preview_name: "Toast stack",
            build: crate::toast::stack_preview,
            surface: Some(PreviewSurface::new(300.0, 200.0)),
        },
        PreviewEntry {
            component_name: "utilities",
            preview_name: "Utilities panel",
            build: crate::utilities::panel_preview,
            surface: Some(PreviewSurface::new(420.0, 520.0)),
        },
        PreviewEntry {
            component_name: "launcher",
            preview_name: "Wallpaper grid",
            build: crate::launcher::wallpaper_grid_preview,
            surface: Some(PreviewSurface::new(640.0, 320.0)),
        },
        PreviewEntry {
            component_name: "lock",
            preview_name: "Lock screen",
            build: crate::lock::screen_preview,
            surface: Some(PreviewSurface::new(960.0, 600.0)),
        },
    ]
}

/// A volume the OSD can draw a bar for: `volume::current()` is `None` until PipeWire has published something, which on a headless render is never, and a meter at 0% reads as broken rather than as quiet.
pub fn osd() {
    services::volume::seed(Volume {
        level: 64,
        muted: false,
    });
}

/// Two tray applications, so the module previews as the row it is instead of as the blank page an empty tray honestly draws. Both hand over their own pixels — the one resolution path that needs neither an installed icon theme nor a network, and the one most items on the bus take anyway.
pub fn tray() {
    ui::preview::bar_chip_with(|config| config.tray.enabled = true);
    services::tray::seed(vec![
        item("Discord", "Discord", (88, 101, 242)),
        item("Steam", "Steam — 3 friends online", (102, 192, 244)),
    ]);
}

/// Five workspaces on one monitor, three of them holding windows and the second focused — enough for the pill row to show every state it draws: active, occupied and empty.
pub fn workspaces() {
    ui::preview::bar_chip();
    services::hyprland::seed_workspaces(Snapshot {
        workspaces: (1..=5)
            .map(|id| Workspace {
                id,
                name: id.to_string(),
                windows: match id {
                    1 => 2,
                    2 => 1,
                    4 => 3,
                    _ => 0,
                },
                monitor: MONITOR.to_string(),
                clients: Vec::new(),
                handle: None,
            })
            .collect(),
        active: 2,
        focused_monitor: MONITOR.to_string(),
    });
}

fn item(id: &str, tooltip: &str, rgb: (u8, u8, u8)) -> TrayItem {
    TrayItem {
        key: format!(":1.{id}/StatusNotifierItem"),
        id: id.to_string(),
        title: id.to_string(),
        tooltip: tooltip.to_string(),
        status: Status::Active,
        pixmap: Some(app_pixmap(rgb)),
        ..TrayItem::default()
    }
}

/// An item's `IconPixmap`: a rounded square in the application's colour, at the 24px most of them publish.
fn app_pixmap(rgb: (u8, u8, u8)) -> Arc<Pixmap> {
    const SIDE: u32 = 24;
    let radius = SIDE as f32 * 0.3;
    let mut rgba = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let corner_x = px.clamp(radius, SIDE as f32 - radius);
            let corner_y = py.clamp(radius, SIDE as f32 - radius);
            let inside = (px - corner_x).hypot(py - corner_y) <= radius;
            rgba.extend_from_slice(&[rgb.0, rgb.1, rgb.2, if inside { 255 } else { 0 }]);
        }
    }
    Arc::new(Pixmap {
        width: SIDE,
        height: SIDE,
        rgba,
    })
}
