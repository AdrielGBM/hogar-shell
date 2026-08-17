//! In-shell toasts: the small, self-dismissing messages the shell says about itself.
//!
//! What is here is the card and the switches behind it. Where it goes is not: a toast, a notification popup and
//! an OSD are one column now ([`crate::stack`]), and this module says only what a toast *is* — its glyph, its
//! title, the line under it, and that a press takes it away.

mod events;

pub use events::{config_reloaded, watch_events};

use telar::{
    AlignItems, Container, LayoutError, LayoutItem, LayoutStyle, RectStyle, SizeDimension,
    StyledContainer, Text, box_item,
};

use config::theme::{FontRole, NordTheme};
use services::toaster::{self, Toast};
use ui::panel::panel_fill;
use ui::scale::space;

const ICON: f32 = 22.0;

/// Two sample toasts as the column draws them, for [`crate::preview`]: a live queue is empty on every run that
/// is not a running shell, and an empty stack is a blank page.
pub(crate) fn stack_preview() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let theme = telar::use_theme::<NordTheme>();
    let cards = [
        Toast::sample(
            toaster::Event::Charging,
            "battery-charging",
            "Charging",
            "84%",
        ),
        Toast::sample(
            toaster::Event::KbLayout,
            "keyboard",
            "Keyboard layout",
            "Spanish",
        ),
    ]
    .iter()
    .map(|toast| card(toast, theme, 14.0))
    .collect::<Result<Vec<_>, _>>()?;
    Ok(Box::new(Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(ui::panel::card_gap())
            .width(SizeDimension::Percent(1.0)),
        cards,
    )?))
}

/// One toast: its glyph, its title, and the line under it. A press dismisses it — the only interaction a toast
/// has, because everything a toast says is already true whether or not it is read.
pub(crate) fn card(
    toast: &Toast,
    theme: NordTheme,
    radius: f32,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let icon = ui::icon::icon_view(
        {
            let name = toast.icon.clone();
            move || name.clone()
        },
        move || theme.accent,
        ICON,
    )?;

    let title = Text::auto(
        {
            let title = toast.title.clone();
            move || title.clone()
        },
        LayoutStyle::new(),
        move || {
            theme
                .text_style(FontRole::Body, theme.text)
                .with_weight(700)
                .with_max_lines(1)
                .with_ellipsis(true)
        },
    )?;
    let mut column: Vec<Box<dyn LayoutItem>> = vec![box_item(title)];
    if !toast.body.trim().is_empty() {
        let body = Text::auto(
            {
                let body = toast.body.clone();
                move || body.clone()
            },
            LayoutStyle::new(),
            move || {
                theme
                    .text_style(FontRole::Caption, theme.muted)
                    .with_max_lines(2)
                    .with_ellipsis(true)
            },
        )?;
        column.push(box_item(body));
    }
    let text = Container::new(
        LayoutStyle::new()
            .flex_column()
            .gap(space::XS)
            .flex_grow(1.0)
            .width(SizeDimension::Percent(1.0)),
        column,
    )?;

    let id = toast.id;
    let card = StyledContainer::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::LG)
            .padding_all(space::XL)
            .width(SizeDimension::Percent(1.0)),
        move |_| RectStyle::filled(panel_fill(), radius),
        vec![icon, Box::new(text)],
    )?
    .hover_style(move |_| RectStyle::filled(theme.overlay, radius));
    // Dragged aside, never pressed away. A toast reports something that already happened, so a click on it is a
    // click the user meant for whatever it landed on top of — which pressing to dismiss would swallow.
    let Some(threshold) = crate::stack::swipe::column_threshold() else {
        return Ok(Box::new(card));
    };
    Ok(Box::new(crate::stack::swipe::swipe_aside(
        card,
        threshold,
        move || toaster::dismiss(id),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_and_the_preview_both_build() {
        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        let toast = Toast::sample(toaster::Event::Dnd, "bell-off", "Do Not Disturb", "On");
        assert!(card(&toast, NordTheme::new(), 12.0).is_ok());

        telar::reset_layout_runtime();
        telar::set_theme(NordTheme::new());
        assert!(stack_preview().is_ok());
    }
}
