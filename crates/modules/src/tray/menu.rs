//! The popup menu behind a tray icon.
//!
//! A transient hanging off the icon that opened it, in the window of the bar that icon is on, placed the way a drawer and a hover card are.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{
    AlignItems, Color, Container, LayoutError, LayoutItem, LayoutStyle, RectStyle, SizeDimension,
    StyledContainer, Text, box_item,
};

use config::theme::{FontRole, NordTheme};
use services::dbusmenu::{self, MenuItem, Toggle};
use services::tray::TrayItem;
use surfaces::transient::{self, Anchor, Motion, Place, Spec};
use ui::chrome::{Chrome, content_radius, panel_fill};
use ui::icon::{app_icon_view, icon_view};
use ui::scale::{corner, space};

/// The shell's id for the menu transient. One at a time: a second tray menu on screen would be two context menus at once, which no desktop does.
const ID: &str = "tray-menu";

/// Fixed rather than content-derived, so every application's tray menu reads as the same shape instead of a different width per app.
const MENU_WIDTH: f32 = 260.0;

const ROW_HEIGHT: f32 = 30.0;
const SEPARATOR_HEIGHT: f32 = 9.0;

thread_local! {
    /// Which item's menu is showing, so a second click on the same chip closes it while a click on another chip switches. Driver-thread only, like the rest of this module's state.
    static OPEN_FOR: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn close() {
    OPEN_FOR.with(|o| *o.borrow_mut() = None);
    transient::close(ID);
}

/// Opens `item`'s menu under its chip, or closes it if that same menu is already up.
///
/// The layout is fetched on a worker thread and the transient opened from the handler, which [`platform_wayland::watch`] runs on the driver thread — the only place a transient may be opened. Doing the round trip inline would stall the frame on however long another application takes to answer.
pub fn toggle(item: &TrayItem, anchor: Anchor) {
    // Both halves matter: `OPEN_FOR` alone would still name this item after the menu was dismissed by a click outside it, and the next click on the same chip would read as "close" and do nothing.
    let already_open = transient::is_open(ID)
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
            let (bus, path) = (event_bus.clone(), event_path.clone());
            let edge = anchor.edge;
            transient::open(
                Spec::new(
                    ID,
                    Place::Beside(anchor.clone()),
                    Rc::new(move |chrome: &Chrome| {
                        menu_view(
                            &root,
                            &bus,
                            &path,
                            chrome.config.resolve_theme(),
                            content_radius(),
                        )
                    }),
                )
                .dismiss_on_outside()
                .motion(Motion::Slide(edge))
                .on_close(|| OPEN_FOR.with(|o| *o.borrow_mut() = None)),
            );
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

/// Renders a level of the menu. A submenu is expanded inline, one indent deeper, rather than flying out into a second surface: it keeps every row reachable in one place, and a tray menu is rarely more than two deep.
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

/// The tick a toggled row carries. A radio and a checkbox are different promises — one of a set versus on/off — so they get different glyphs rather than one shared dot.
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
    let text = Text::new(
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

    // The row's own half of the panel's inset, so a label lands exactly where a drawer's content does. A submenu is expanded in place, so each level adds its indent on top of that.
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

    // A row that opens a submenu is already showing it (submenus expand inline), and a disabled one is a label — neither is pressable, so neither gets hover feedback that promises otherwise.
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
