//! The popup menu behind a tray icon.
//!
//! A real layer-shell surface rather than an in-surface overlay: a bar is only its own thickness tall, so a
//! menu drawn inside it would be clipped to a sliver. The anchoring is
//! [`shared::anchor`](ui::anchor), shared with the hover popouts.

use std::cell::RefCell;

use telar::{
    AlignItems, Color, Container, LayoutError, LayoutItem, LayoutStyle, Rect, RectStyle,
    SizeDimension, StyledContainer, Text, box_item,
};

use config::SurfaceEnv;
use config::theme::{FontRole, NordTheme};
use services::dbusmenu::{self, MenuItem, Toggle};
use services::tray::TrayItem;
use ui::icon::{app_icon_view, icon_view};
use ui::panel::{PanelSurface, content_radius, panel_fill};
use ui::placement::{OffChip, Placement};
use ui::scale::{corner, space};

/// The shell's id for the menu surface. One at a time: a second tray menu on screen would be two context
/// menus at once, which no desktop does.
const SURFACE_ID: &str = "tray-menu";

/// Fixed rather than content-derived, so the anchoring maths knows the width before the menu is laid out and
/// can keep it on screen. A tray menu is a list of short labels; letting it size to its longest one would make
/// every application's menu a different width.
const MENU_WIDTH: f32 = 260.0;

const ROW_HEIGHT: f32 = 30.0;
const SEPARATOR_HEIGHT: f32 = 9.0;

thread_local! {
    /// Which item's menu is showing, so a second click on the same chip closes it while a click on another
    /// chip switches. Driver-thread only, like the rest of the surface bookkeeping.
    static OPEN_FOR: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn close() {
    OPEN_FOR.with(|o| *o.borrow_mut() = None);
    surfaces::shell::close(SURFACE_ID);
}

/// Opens `item`'s menu under its chip, or closes it if that same menu is already up.
///
/// The layout is fetched on a worker thread and the surface opened from the handler, which
/// [`platform_wayland::watch`] runs on the driver thread — the only place a surface may be opened. Doing the
/// round trip inline would stall the frame on however long another application takes to answer.
pub fn toggle(item: &TrayItem, chip: Rect, env: SurfaceEnv) {
    // Both halves matter: `OPEN_FOR` alone would still name this item after the menu was dismissed by a click
    // outside it, and the next click on the same chip would read as "close" and do nothing.
    let already_open = surfaces::shell::window_is_open(SURFACE_ID)
        && OPEN_FOR.with(|o| o.borrow().as_deref() == Some(item.key.as_str()));
    close();
    if already_open || item.menu.trim().is_empty() {
        return;
    }
    OPEN_FOR.with(|o| *o.borrow_mut() = Some(item.key.clone()));

    let key = item.key.clone();
    let label = item.label().to_string();
    let bus = item.bus.clone();
    let path = item.menu.clone();
    let event_bus = bus.clone();
    let event_path = path.clone();

    platform_wayland::watch(
        dbusmenu::fetch_into(bus, path),
        move |menu: Option<MenuItem>| {
            // The chip may have been clicked again, or another one opened, while the application was answering.
            if OPEN_FOR.with(|o| o.borrow().as_deref() != Some(key.as_str())) {
                return;
            }
            let Some(root) = menu.filter(|m| !m.children.is_empty()) else {
                tracing::info!("tray item '{label}' offers no menu to show");
                OPEN_FOR.with(|o| *o.borrow_mut() = None);
                return;
            };
            // Along a horizontal bar the menu's extent is its fixed width; along a vertical one it would be its height, which is content-derived and unknown before layout.
            let span = (!env.edge.is_vertical()).then_some(MENU_WIDTH);
            // The placement is built from the bar's own env, so the menu already resolves against the bar its
            // chip sits on — its radius and fill match the drawer that chip would have opened.
            let placement = Placement::off_chip(OffChip::Panel, &env, Some(chip), span);
            let (bus, path) = (event_bus.clone(), event_path.clone());
            surfaces::shell::toggle_window(SURFACE_ID, move || {
                PanelSurface::new(placement, move |env| {
                    menu_view(
                        &root,
                        &bus,
                        &path,
                        env.config.resolve_theme(),
                        content_radius(),
                    )
                    .expect("tray menu build failed")
                })
                .open()
            });
        },
    );
}

fn menu_view(
    root: &MenuItem,
    bus: &str,
    path: &str,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let rows = rows_for(&root.children, bus, path, theme, 0)?;
    let panel = StyledContainer::new(
        LayoutStyle::new()
            .flex_column()
            .width(MENU_WIDTH)
            .padding_all(space::md())
            .gap(space::xs()),
        move |_| RectStyle::filled(panel_fill(), radius),
        rows,
    )?;
    Ok(Box::new(panel))
}

/// Renders a level of the menu. A submenu is expanded inline, one indent deeper, rather than flying out into a
/// second surface: it keeps every row reachable in one place, and a tray menu is rarely more than two deep.
fn rows_for(
    items: &[MenuItem],
    bus: &str,
    path: &str,
    theme: NordTheme,
    depth: usize,
) -> Result<Vec<Box<dyn LayoutItem>>, LayoutError> {
    let mut out: Vec<Box<dyn LayoutItem>> = Vec::new();
    for item in items {
        if item.separator {
            out.push(separator_row(theme)?);
            continue;
        }
        out.push(row(item, bus, path, theme, depth)?);
        if item.has_submenu() {
            out.extend(rows_for(&item.children, bus, path, theme, depth + 1)?);
        }
    }
    Ok(out)
}

fn separator_row(theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let line = StyledContainer::new(
        LayoutStyle::new()
            .width(SizeDimension::Percent(1.0))
            .height(1.0),
        move |_| RectStyle::filled(theme.overlay, 0.0),
        vec![],
    )?;
    let holder = Container::new(
        LayoutStyle::new()
            .flex_column()
            .justify_content(telar::JustifyContent::CENTER)
            .width(SizeDimension::Percent(1.0))
            .height(SEPARATOR_HEIGHT),
        vec![box_item(line)],
    )?;
    Ok(Box::new(holder))
}

/// The tick a toggled row carries. A radio and a checkbox are different promises — one of a set versus on/off —
/// so they get different glyphs rather than one shared dot.
fn toggle_glyph(toggle: Toggle) -> Option<&'static str> {
    match toggle {
        Toggle::None => None,
        Toggle::Checkmark(true) => Some("mdi:checkbox-marked-outline"),
        Toggle::Checkmark(false) => Some("mdi:checkbox-blank-outline"),
        Toggle::Radio(true) => Some("mdi:radiobox-marked"),
        Toggle::Radio(false) => Some("mdi:radiobox-blank"),
    }
}

fn row(
    item: &MenuItem,
    bus: &str,
    path: &str,
    theme: NordTheme,
    depth: usize,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let enabled = item.enabled;
    let fg = if enabled { theme.text } else { theme.muted };
    let icon_size = 16.0;
    let mut content: Vec<Box<dyn LayoutItem>> = Vec::new();

    if let Some(glyph) = toggle_glyph(item.toggle) {
        content.push(icon_view(move || glyph.to_string(), move || fg, icon_size)?);
    } else if !item.icon_name.is_empty()
        && let Some(icon) = app_icon_view(&item.icon_name, icon_size)?
    {
        content.push(icon);
    }

    let label = item.label.clone();
    let text = Text::auto(
        move || label.clone(),
        LayoutStyle::new().flex_grow(1.0),
        move || theme.text_style(FontRole::Body, fg),
    )?;
    content.push(box_item(text));

    if item.has_submenu() {
        content.push(icon_view(
            || "mdi:chevron-down".to_string(),
            move || theme.muted,
            icon_size,
        )?);
    }

    // The row's own half of the panel's inset, so a label lands exactly where a drawer's content does. A
    // submenu is expanded in place, so each level adds its indent on top of that.
    let indent = space::md() + depth as f32 * 14.0;
    let style = LayoutStyle::new()
        .flex_row()
        .align_items(AlignItems::CENTER)
        .gap(space::md())
        .width(SizeDimension::Percent(1.0))
        .height(ROW_HEIGHT)
        .padding_left(indent)
        .padding_right(space::md());

    // Resolved here and captured, not read inside the closures: a style closure runs on every paint.
    let pill = corner::xs();
    let mut container = StyledContainer::new(
        style,
        move |_| RectStyle::filled(Color::TRANSPARENT, pill),
        content,
    )?;

    // A row that opens a submenu is already showing it (submenus expand inline), and a disabled one is a
    // label — neither is pressable, so neither gets hover feedback that promises otherwise.
    if item.is_actionable() {
        let (bus, path, id) = (bus.to_string(), path.to_string(), item.id);
        container = container
            .hover_style(move |_| RectStyle::filled(theme.overlay, pill))
            .on_press(move || {
                dbusmenu::activate(&bus, &path, id);
                close();
            });
    }
    Ok(Box::new(container))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_separator_never_renders_as_a_pressable_row() {
        let separator = MenuItem {
            separator: true,
            enabled: true,
            ..MenuItem::default()
        };
        assert!(!separator.is_actionable());
    }
}
