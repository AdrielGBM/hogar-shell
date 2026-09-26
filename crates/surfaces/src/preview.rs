//! What this crate's previews need before they build, and the ones it registers in Rust.
//!
//! Each of these is a *surface*, so each declares the size the compositor would give it and is rendered under the same root the runner mounts — see [`telar::PreviewSurface`]. What is still the surface host's alone is the clear colour behind the tree and the window's own transparency.

use std::collections::BTreeMap;
use std::sync::Arc;

use telar::{LayoutError, LayoutItem, PreviewEntry, PreviewSurface, Rect};

use config::Config;
use layout::{
    LayerKind, LayoutStore, NOMINAL_OUTPUT, Resolved, ResolvedArea, ResolvedAreaKind, resolve,
};

use crate::area::Surround;
use crate::layer_window::Reserved;

/// The screen a preview of an area stands on. Areas place themselves on an output now, so a preview of one has to be an output rather than the strip the area happens to occupy — a bar previewed on a surface its own size would be a bar with nowhere to sit at its gap. Square, so a vertical bar is given the same length to lay its chips out in as a horizontal one.
const SCREEN: (f32, f32) = (940.0, 940.0);

/// The previews this crate registers by hand, for the surfaces whose content is still built by a Rust function. The drawer is not among them: its panel is `drawer_panel.rsx`, so its preview is a `[preview]` block there.
pub fn entries() -> Vec<PreviewEntry> {
    let config = config::config().unwrap_or_else(|| Arc::new(Config::starter()));
    let popouts = config.popouts;
    vec![
        PreviewEntry {
            component_name: "bar",
            preview_name: "Configured bar",
            build: || {
                area_preview(LayerKind::Top, |area| {
                    matches!(area.kind, ResolvedAreaKind::Bar { .. })
                })
            },
            surface: Some(PreviewSurface::new(SCREEN.0, SCREEN.1)),
        },
        PreviewEntry {
            component_name: "float",
            preview_name: "Window frame",
            build: crate::float::frame_preview,
            // The one preview that animates: a float fades in from transparent, and a transition that never settles is a window the user cannot see while every other check passes.
            surface: Some(PreviewSurface::new(360.0, 240.0).animated()),
        },
        PreviewEntry {
            component_name: "wallpaper",
            preview_name: "Desktop",
            build: || {
                area_preview(LayerKind::Background, |area| {
                    matches!(area.kind, ResolvedAreaKind::WallpaperRegion { .. })
                })
            },
            surface: Some(PreviewSurface::new(SCREEN.0, SCREEN.1)),
        },
        PreviewEntry {
            component_name: "popout",
            preview_name: "Hover card",
            build: crate::popout::preview,
            surface: Some(PreviewSurface::new(
                popouts.card_width(),
                popouts.card_height(),
            )),
        },
    ]
}

/// The first area of `layer` that `wanted` accepts, built where it would sit on a screen of [`SCREEN`].
///
/// The running layout where there is one and the shipped one otherwise, which is the same rule the previews already follow for the config: a preview is a picture of what this shell draws, and on a machine that has never been configured that is the default.
fn area_preview(
    layer: LayerKind,
    wanted: impl Fn(&ResolvedArea) -> bool,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = config::config().unwrap_or_else(|| Arc::new(Config::starter()));
    let (resolved, area) = previewed(layer, &wanted)
        .ok_or_else(|| LayoutError::Engine(format!("no layout has a {layer} area to preview")))?;

    let surround = Surround {
        theme: config.resolve_theme(),
        config: &config,
        output: None,
        bounds: Rect::new(0.0, 0.0, SCREEN.0, SCREEN.1),
        reserved: Reserved::of(&resolved, &config),
    };
    crate::area::build(&area, surround)
        .unwrap_or_else(|| Err(LayoutError::Engine("nothing draws that area yet".into())))
}

/// The arrangement a preview of `layer` is taken from, and the area in it: the running layout first, and the shipped one where that layout has no such area.
///
/// A preview is a picture of what this shell draws, so the user's own layout wins. Falling back to the shipped one is what keeps a picture of a wallpaper available on a machine that has not set one — the alternative is a preview that shows an error where a feature the shell has is simply switched off.
fn previewed(
    layer: LayerKind,
    wanted: &impl Fn(&ResolvedArea) -> bool,
) -> Option<(Resolved, ResolvedArea)> {
    let shipped = LayoutStore::safe(std::env::temp_dir());
    let running = layout::running();
    let candidates = running
        .as_deref()
        .into_iter()
        .chain(std::iter::once(shipped.active()));
    for candidate in candidates {
        let (resolved, _) = resolve(candidate, &BTreeMap::new(), NOMINAL_OUTPUT, None);
        let found = resolved
            .layer(layer)
            .and_then(|layer| layer.areas.iter().find(|area| wanted(area)).cloned());
        if let Some(area) = found {
            return Some((resolved, area));
        }
    }
    None
}

/// Where on [`SCREEN`] the previewed bar lands.
///
/// An area places itself on an output, so "on the bar" is the strip it took and no longer the whole preview surface. A sweep asking what a bar painted, or whether it claimed what it painted, has to ask about that strip — measured from the same layout the preview built from, so the two cannot disagree.
pub fn bar_strip() -> Option<Rect> {
    let config = config::config().unwrap_or_else(|| Arc::new(Config::starter()));
    let (resolved, area) = previewed(LayerKind::Top, &|area| {
        matches!(area.kind, ResolvedAreaKind::Bar { .. })
    })?;
    Some(crate::bar::strip_of_area(
        &area,
        Surround {
            theme: config.resolve_theme(),
            config: &config,
            output: None,
            bounds: Rect::new(0.0, 0.0, SCREEN.0, SCREEN.1),
            reserved: Reserved::of(&resolved, &config),
        },
    ))
}

/// Which module the drawer is showing, which is decided by the chip that opened it and so has to be put in scope before the panel builds. `clock` because it needs nothing from the machine to draw.
pub fn drawer() {
    let env = ui::preview::bar_surface();
    crate::drawer::set_drawer_host("clock", &env);
}
