//! What the shell draws where a config names something this build does not have.
//!
//! Drawing nothing was the old answer, and the worst one available: a misspelt module id vanished from its bar and a misspelt toggle from its grid, and the only record that either had ever been asked for was a warning in a log nobody reads. A placeholder keeps the place the user gave the entry, in the error colour, carrying the id as it was written — so the mistake is on screen, where the user is looking, beside whatever they meant it to sit next to.
//!
//! **What a press does, and what it deliberately does not.** It opens the settings window, where the config is edited, through the same route a chip opens its own panel by. It offers no "remove" or "reset": both would act on the config model that a later sprint replaces with a layout model, and would be written against a shape already slated for deletion. But a press has to do *something*: one that fell through would click whatever is under the bar, and one swallowed without a response reads as a shell that has stopped working.

use std::cell::RefCell;
use std::rc::Rc;

use telar::{
    AlignItems, Color, Container, LayoutError, LayoutItem, LayoutStyle, Slots, Text, box_item,
};

use config::theme::{FontRole, NordTheme};
use config::{Edge, Variant};

use crate::icon::icon_view;
use crate::module::{icon_px, module_foreground, open_panel};
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

/// The chip a bar draws for `id` when no module answers to it.
///
/// Built on [`module_shell`] like every other chip, so it takes the size, padding, corner radius and hover of the chips beside it in every shape mode — a placeholder that sized itself would be the one chip on the bar that got the bar wrong. Along a horizontal bar it shows the glyph and the id; down a vertical one there is no length to write an id along, so it is the glyph alone in a square chip, the way a text chip shows only its glyph down one.
pub fn placeholder_chip(
    id: &str,
    edge: Edge,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let ink = ink(theme);
    let glyph = icon_view(|| GLYPH.to_string(), move || ink, icon_px())?;
    let content: Box<dyn LayoutItem> = if edge.is_vertical() {
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
            .radius(radius)
            .square(edge.is_vertical())
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

#[cfg(test)]
mod tests {
    use super::*;
    use telar::{
        AvailableSpace, Event, PointerButton, PointerSource, compute_layout, reset_layout_runtime,
        set_theme, track_layout,
    };

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
            placeholder_chip("clokc", Edge::Top, NordTheme::new(), 8.0).expect("the chip builds");
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
