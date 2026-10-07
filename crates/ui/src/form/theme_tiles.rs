use std::sync::{Arc, OnceLock};

use telar::{
    AlignItems, Container, JustifyContent, LayoutItem, LayoutStyle, ReadSignal, RectStyle,
    RwSignal, StyledContainer, Text, box_item,
};

use config::theme::{BUILT_IN_THEMES, FontRole, NordTheme};
use config::{Config, ThemeConfig};

use crate::descriptor::Built;
use crate::scale::space;

const WIDTH: f32 = 76.0;
const HEIGHT: f32 = 40.0;
const RADIUS: f32 = 6.0;

/// Every value `[theme] name` selects a palette by: the built-ins, `custom` (nord for `[theme.colors]` to override) and `dynamic` (the wallpaper's own).
pub fn theme_options() -> &'static [&'static str] {
    static OPTIONS: OnceLock<Vec<&'static str>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = BUILT_IN_THEMES.to_vec();
        options.push("custom");
        options.push(config::scheme::DYNAMIC);
        options
    })
}

/// One tile per selectable palette, each painted in its own surface, ink and accent as `config` would resolve it in the pending `mode`; pressing one writes its name into `name`.
pub fn theme_tiles(name: RwSignal<String>, mode: ReadSignal<String>, config: Arc<Config>) -> Built {
    let theme = telar::use_theme::<NordTheme>();
    let tiles = theme_options()
        .iter()
        .map(|option| tile(option, name, mode, &config, theme))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(box_item(Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .flex_grow(1.0)
            .min_width(0.0),
        tiles,
    )?))
}

fn tile(
    option: &'static str,
    name: RwSignal<String>,
    mode: ReadSignal<String>,
    config: &Arc<Config>,
    theme: NordTheme,
) -> Built {
    let saved = config.theme.clone();
    let config = Arc::clone(config);
    let palette = move || {
        config.theme_with(&ThemeConfig {
            name: option.to_string(),
            mode: mode.get(),
            ..saved.clone()
        })
    };

    let ink = palette.clone();
    let label = Text::declaring(
        move || option.to_string(),
        LayoutStyle::new(),
        move |inherited| theme.text_over(inherited, FontRole::Caption, ink().text),
    )?;
    let dot_of = palette.clone();
    let dot = StyledContainer::new(
        LayoutStyle::new().width(10.0).height(10.0).flex_shrink(0.0),
        move |_| RectStyle::filled(dot_of().accent, 5.0),
        Vec::new(),
    )?;
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::sm()),
        vec![Box::new(dot) as Box<dyn LayoutItem>, box_item(label)],
    )?;

    let selected = name.read_only();
    let tile = StyledContainer::new(
        LayoutStyle::new()
            .width(WIDTH)
            .height(HEIGHT)
            .padding_horizontal(space::md())
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        move |_| {
            let chosen = selected.get() == option;
            let surface = palette().surface;
            let (border, width) = match chosen {
                true => (theme.accent, 2.0),
                false => (theme.overlay, 1.0),
            };
            RectStyle::filled(surface, RADIUS).with_border(telar::Border::uniform(border, width))
        },
        vec![Box::new(row)],
    )?
    .on_press(move || name.set(option.to_string()));
    Ok(Box::new(tile))
}
