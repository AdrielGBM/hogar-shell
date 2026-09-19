//! What the shell draws where a config names something this build does not have, or where a module failed to build; a placeholder keeps the entry's place in the error colour rather than vanishing it silently into a log nobody reads.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{
    AlignItems, Color, Container, LayoutError, LayoutItem, LayoutStyle, RectStyle, SizeDimension,
    Slots, StyledContainer, Text, box_item,
};

use config::Variant;
use config::theme::{FontRole, NordTheme};

use crate::host::{Host, Representation};
use crate::icon::icon_view;
use crate::module::{module_foreground, open_panel};
use crate::module_shell::{ModuleShellProps, module_shell};
use crate::scale::space;

/// The glyph an unknown entry is drawn with, wherever it is drawn.
pub const GLYPH: &str = "circle-alert";

/// The window a press opens: the one the config is edited in.
const FIXED_IN: &str = "settings";

/// What an unknown entry rests on: the theme's error token, so it reads as wrong on every palette rather than as one more chip.
pub fn fill(theme: NordTheme) -> Color {
    theme.error
}

/// Whichever of the theme's two foregrounds reads over [`fill`].
pub fn ink(theme: NordTheme) -> Color {
    module_foreground(Variant::Filled, fill(theme), theme)
}

/// What pressing a placeholder does, wherever it is drawn. Opens rather than toggles: a second press on the thing that is wrong means "show me where to fix it" again, not "put the window away".
pub fn open_settings() {
    open_panel(FIXED_IN);
}

/// What stands in for `id` as `host.representation`: a chip on a bar, and elsewhere a box saying which id, and `reason` when there is one.
pub fn placeholder(
    id: &str,
    reason: Option<&str>,
    host: &Host,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    match host.representation {
        Representation::Chip => placeholder_chip(id, host, theme),
        _ => placeholder_box(id, reason, host, theme),
    }
}

/// Built on [`module_shell`] like every other chip, so it takes the size, padding, corner radius and hover of the chips beside it in every shape mode — a placeholder that sized itself would be the one chip on the bar that got the bar wrong. Along a horizontal bar it shows the glyph and the id; down a vertical one there is no length to write an id along, so it is the glyph alone in a square chip, the way a text chip shows only its glyph down one.
fn placeholder_chip(
    id: &str,
    host: &Host,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let ink = ink(theme);
    let glyph = icon_view(|| GLYPH.to_string(), move || ink, host.icon_size())?;
    let vertical = host.is_vertical();
    let content: Box<dyn LayoutItem> = if vertical {
        glyph
    } else {
        let id = id.to_string();
        let label = Text::new(
            move || id.clone(),
            LayoutStyle::new(),
            move || theme.text_style(FontRole::Body, ink),
        )?;
        Box::new(Container::new(
            LayoutStyle::new()
                .flex_row()
                .align_items(AlignItems::CENTER)
                .gap(space::sm()),
            vec![glyph, box_item(label)],
        )?)
    };
    let mut inner = Slots::new();
    inner.push(None, content);
    module_shell(
        ModuleShellProps::props()
            .variant(Variant::Filled)
            .accent(fill(theme))
            .radius(host.corner_radius())
            .square(vertical)
            .inset(host.inset())
            .vertical(vertical)
            .on_press(Some(Rc::new(open_settings) as Rc<dyn Fn()>))
            .build(),
        telar::Children::new({
            let inner = RefCell::new(Some(inner));
            move || {
                inner
                    .borrow_mut()
                    .take()
                    .ok_or_else(|| LayoutError::Engine("children built twice".into()))
            }
        }),
    )
}

/// Takes no press, unlike the chip: it may stand where a `ReadOnly` representation was promised — on the lock screen, over the desktop — and a placeholder must not be the one thing there that takes input.
fn placeholder_box(
    id: &str,
    reason: Option<&str>,
    host: &Host,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let ink = ink(theme);
    let glyph = icon_view(|| GLYPH.to_string(), move || ink, 18.0)?;
    let text = |line: String, role: FontRole| {
        Text::new(
            move || line.clone(),
            LayoutStyle::new(),
            move || theme.text_style(role, ink),
        )
        .map(box_item)
    };
    let heading = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::sm()),
        vec![glyph, text(id.to_string(), FontRole::Body)?],
    )?;
    let mut lines: Vec<Box<dyn LayoutItem>> = vec![Box::new(heading)];
    if let Some(reason) = reason {
        lines.push(text(reason.to_string(), FontRole::Caption)?);
    }
    let style = LayoutStyle::new()
        .flex_column()
        .gap(space::sm())
        .padding_all(space::lg());
    let style = match host.widget_size() {
        Some(size) => {
            let extent = size.extent();
            style.width(extent.width).height(extent.height)
        }
        None if host.extent.width.is_finite() => style.width(host.extent.width),
        None => style.width(SizeDimension::Percent(1.0)),
    };
    let radius = host.shape.radius;
    let fill = fill(theme);
    Ok(Box::new(StyledContainer::new(
        style,
        move |_| RectStyle::filled(fill, radius),
        lines,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use telar::{
        AvailableSpace, Event, PointerButton, PointerSource, compute_layout, reset_layout_runtime,
        set_theme, track_layout,
    };

    fn host() -> Host {
        Host::chip(
            crate::host::InstanceId::new("clokc"),
            std::sync::Arc::new(config::Config::starter()),
            config::Edge::Top,
            fill(NordTheme::new()),
            ink(NordTheme::new()),
            None,
        )
    }

    thread_local! {
        static OPENED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    /// The placeholder stands where a chip the user asked for should have been, so a press on it has to land on it: a press that fell through would click the window under the bar, and one taken without a response looks like a shell that has stopped listening.
    #[test]
    fn a_press_on_a_placeholder_opens_the_settings_window() {
        reset_layout_runtime();
        set_theme(NordTheme::new());
        crate::module::set_panel_opener(|panel| {
            OPENED.with(|opened| opened.borrow_mut().push(panel.to_string()))
        });
        let mut chip =
            placeholder_chip("clokc", &host(), NordTheme::new()).expect("the chip builds");
        let rect = track_layout(chip.layout_node()).expect("the chip registers its rect");
        compute_layout(
            chip.layout_node(),
            AvailableSpace::Definite(200.0),
            AvailableSpace::Definite(34.0),
        )
        .expect("the chip lays out");

        let centre = rect.get();
        let (x, y) = (
            f64::from(centre.width / 2.0),
            f64::from(centre.height / 2.0),
        );
        chip.on_event(&Event::PointerPressed {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });
        chip.on_event(&Event::PointerReleased {
            x,
            y,
            button: PointerButton::Primary,
            source: PointerSource::Mouse,
        });

        assert_eq!(
            OPENED.with(|opened| opened.borrow().clone()),
            [FIXED_IN],
            "pressing the placeholder must open the window the config is edited in, once"
        );
    }
}
