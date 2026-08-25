//! The settings application's previews, registered in Rust because the page they open is assembled by [`crate::panel`] from a nav and a search rather than owned by one `.rsx` component. The forms on it are `.rsx`; the window around them is not.

use telar::{LayoutError, LayoutItem, PreviewEntry, PreviewSurface};

/// The float this application opens in (`ui::placement::window`), so both previews are laid out at the size the user actually reads them at.
const FLOAT: PreviewSurface = PreviewSurface {
    width: 920.0,
    height: 680.0,
    animate: false,
};

pub fn entries() -> Vec<PreviewEntry> {
    vec![
        PreviewEntry {
            component_name: "settings",
            preview_name: "Settings panel",
            build: crate::panel::settings_panel,
            surface: Some(FLOAT),
        },
        PreviewEntry {
            component_name: "settings",
            preview_name: "A page of switches",
            build: switches_page,
            surface: Some(FLOAT),
        },
        PreviewEntry {
            component_name: "settings",
            preview_name: "The application list",
            build: applications_page,
            surface: Some(FLOAT),
        },
    ]
}

/// The panel opened on the applications page — the shell's one virtualised list, and the only place a preview can show that a window onto thousands of rows still draws the dozen that are on screen.
fn applications_page() -> Result<Box<dyn LayoutItem>, LayoutError> {
    page("applications")
}

/// The panel opened on a page of toggles. The application's first page is text fields and colour swatches, so previewing only that leaves every switch in the settings unseen — and they are the rows the catalogue's `toggle` draws.
fn switches_page() -> Result<Box<dyn LayoutItem>, LayoutError> {
    page("notifications")
}

/// The panel opened on the page `label` names.
fn page(label: &str) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let at = crate::pages::PAGES
        .iter()
        .position(|page| page.label == label)
        .unwrap_or(0);
    util::state::kept("settings.page", || telar::signal(0usize)).set(at);
    crate::panel::settings_panel()
}
