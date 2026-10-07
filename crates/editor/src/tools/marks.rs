//! What the tools draw beside their handles: a tag saying a value, centred where it is put.

use telar::{Border, LayoutStyle, Rect, RectStyle, StyledContainer, Text, box_item, use_theme};

use config::theme::FontRole;
use config::theme::NordTheme;
use surfaces::rects::{self, Node};
use ui::descriptor::Built;

/// A small plate saying `value` as a whole number, centred on wherever `at` puts it in the box `node` is drawn at; it takes no pointer.
pub(super) fn value_tag(
    node: Node,
    value: impl Fn() -> f32 + 'static,
    at: impl Fn(Rect) -> (f32, f32) + 'static,
) -> Built {
    let theme = use_theme::<NordTheme>();
    let text = box_item(Text::declaring(
        move || format!("{}", value().round()),
        LayoutStyle::new(),
        move |inherited| theme.text_over(inherited, FontRole::Caption, theme.text),
    )?);
    Ok(Box::new(
        StyledContainer::new(
            LayoutStyle::new()
                .absolute()
                .inset_start(0.0)
                .inset_top(0.0)
                .padding_horizontal(ui::scale::space::sm())
                .padding_vertical(ui::scale::space::xs()),
            move |_| {
                RectStyle::filled(theme.surface, ui::scale::corner::xs())
                    .with_border(Border::uniform(theme.accent, 1.0))
            },
            vec![text],
        )?
        .with_transform(move |laid| {
            let (x, y) = at(rects::rect(&node).unwrap_or_default());
            Some([
                1.0,
                0.0,
                0.0,
                1.0,
                x - laid.width / 2.0 - laid.x,
                y - laid.height / 2.0 - laid.y,
            ])
        })
        .input_transparent(),
    ))
}
