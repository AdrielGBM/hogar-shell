//! The per-component half of the world a `[preview]` builds against; the process-wide half (theme, locale, config, icon store) is seeded once by `telar::dev_entry`'s `setup` closure instead.

use std::sync::Arc;

use telar::{PreviewEntry, PreviewSurface};

use config::{Config, Edge, Variant};

use crate::chrome::Chrome;
use crate::host::{Host, InstanceId, Representation, Size};
use crate::module::module_foreground;

const BAR: f32 = 34.0;

/// The previews this crate registers by hand, because what they draw is built by a Rust function and a `[preview]` block needs a `.rsx` component to hang off. The app collects these next to every generated `telar_all_preview_entries()`, so `cargo telar preview`/`test` sees no difference between the two.
///
/// Each one replaces a `TELAR_VISUAL_*` test that rendered the same tree only when asked by an environment variable; as an entry it is rendered on every run instead.
pub fn entries() -> Vec<PreviewEntry> {
    vec![
        PreviewEntry {
            component_name: "icon_picker",
            preview_name: "Glyph grid",
            build: crate::icon::grid_preview,
            surface: Some(PreviewSurface::new(304.0, 280.0)),
        },
        PreviewEntry {
            component_name: "spectrum",
            preview_name: "Sweep",
            build: crate::widget::spectrum_preview,
            surface: Some(PreviewSurface::new(520.0, 480.0)),
        },
        PreviewEntry {
            component_name: "card",
            preview_name: "Compact",
            build: crate::card::compact_preview,
            surface: Some(PreviewSurface::new(264.0, 200.0)),
        },
        PreviewEntry {
            component_name: "card",
            preview_name: "Page",
            build: crate::card::page_preview,
            surface: Some(PreviewSurface::new(420.0, 240.0)),
        },
    ]
}

/// Puts the component on the bar the running config draws, as a chip of that bar, so its icon size, padding and corner radius come out as they do on screen. Returns the host it provided, for a caller that builds a Rust chip by hand.
pub fn bar_chip() -> Host {
    bar_chip_with(|_| {})
}

pub fn chrome() -> Chrome {
    chrome_with(|_| {})
}

/// [`bar_chip`] with the bar's config edited first, for a preview whose module reads a setting that decides whether it draws anything at all. The edited config is the host's, and is published as the running one too, for whatever the chip reads outside its host.
pub fn bar_chip_with(edit: impl FnOnce(&mut Config)) -> Host {
    let chrome = chrome_with(edit);
    let theme = chrome.config.resolve_theme();
    let host = Host::placed(
        InstanceId::new("preview"),
        Arc::clone(&chrome.config),
        Representation::Chip,
        Size {
            width: f32::INFINITY,
            height: BAR,
        },
        Some(Edge::Top),
        chrome.shape,
        theme.accent,
        module_foreground(Variant::Default, theme.accent, theme),
        None,
    );
    host.provide();
    host
}

fn chrome_with(edit: impl FnOnce(&mut Config)) -> Chrome {
    let mut config = config::config()
        .map(|live| (*live).clone())
        .unwrap_or_else(Config::starter);
    edit(&mut config);
    let config = Arc::new(config);
    config::set_config(Arc::clone(&config));
    let chrome = Chrome::global(config, None);
    chrome.provide();
    chrome
}

/// A host for `module` shown as `representation`, `extent` across, resolved against the running config — or the starter config where nothing runs. Nothing is put in scope: the caller builds under the host it is handed.
pub fn surface_host(module: &str, representation: Representation, extent: Size) -> Host {
    let config = config::config().unwrap_or_else(|| Arc::new(Config::starter()));
    host_on(config, module, representation, extent)
}

/// [`surface_host`] against `config` rather than the running one.
pub fn host_on(
    config: Arc<Config>,
    module: &str,
    representation: Representation,
    extent: Size,
) -> Host {
    Host::in_chrome(
        InstanceId::of_module(module),
        representation,
        &Chrome::global(config, None),
        extent,
    )
}

/// [`host_on`] for a representation an area hands an edge to — a dock's row of bars, which stands on the edge its area hugs rather than on one of its own.
pub fn host_facing(
    config: Arc<Config>,
    module: &str,
    representation: Representation,
    extent: Size,
    edge: config::Edge,
) -> Host {
    let mut host = host_on(config, module, representation, extent);
    host.axis = Some(edge);
    host
}
