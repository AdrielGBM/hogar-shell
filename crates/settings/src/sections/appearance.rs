//! How the shell looks: the palette, the shapes it draws and how it moves.
//!
//! What is left here is the forms this area cannot say in `.rsx`: the ones whose rows are a list the machine decides the length of. The static-shape forms are `.rsx` components beside this file.

use std::rc::Rc;
use std::sync::{Arc, OnceLock};
use ui::scale::space;

use telar::{
    AlignItems, Container, JustifyContent, LayoutError, LayoutItem, LayoutStyle, RectStyle,
    RwSignal, ShapeStyle, StyledContainer, Text, box_item, signal,
};

use crate::form::*;
use config::theme::{BUILT_IN_THEMES, FontRole, NordTheme, THEME_TOKENS};
use config::{Config, ScaleConfig, ThemeConfig};

/// A palette a control draws from and re-reads: the pending `[theme]` selection resolved through [`Config::theme_with`], so a swatch shows the theme being chosen rather than the one being worn.
type Palette = Rc<dyn Fn() -> NordTheme>;

/// What the theme picker cycles: every built-in palette, `custom` (which starts from nord for `[theme.colors]` to override) and `dynamic` (the wallpaper's own). Derived from [`BUILT_IN_THEMES`] so a new palette shows up here on its own.
fn theme_options() -> &'static [&'static str] {
    static OPTIONS: OnceLock<Vec<&'static str>> = OnceLock::new();
    OPTIONS.get_or_init(|| {
        let mut options = BUILT_IN_THEMES.to_vec();
        options.push("custom");
        options.push(config::scheme::DYNAMIC);
        options
    })
}

/// The palette tokens the preview strip shows, in the order they read as a design rather than as a list: the three surfaces the shell is built out of, the two inks over them, then the hues.
const PREVIEW_TOKENS: &[&str] = &[
    "base", "surface", "overlay", "text", "subtle", "accent", "red", "orange", "yellow", "green",
    "cyan", "blue", "teal", "purple",
];

/// What `[theme] accent` accepts, in the order [`NordTheme::accent_by_name`] resolves them. `""` is the palette's own accent, which is the value a config that never set one carries.
const ACCENT_NAMES: &[&str] = &[
    "", "blue", "cyan", "teal", "red", "orange", "yellow", "green", "purple",
];

const SWATCH: f32 = 22.0;
const SWATCH_RADIUS: f32 = 6.0;
const TILE_WIDTH: f32 = 76.0;
const TILE_HEIGHT: f32 = 40.0;

/// Resolves the page's unsaved `[theme]` selection into a palette, on every read.
///
/// Not a [`Live`](util::reactive::Live): that is a `Memo`, which needs its value to be `PartialEq` to know whether it moved, and a palette is twenty-two colours and a font table. A closure re-resolving is a match and a struct copy — cheaper than the comparison would be.
fn pending_palette(
    config: &Config,
    name: telar::ReadSignal<String>,
    mode: telar::ReadSignal<String>,
    accent: telar::ReadSignal<String>,
) -> Palette {
    let base = Arc::new(config.clone());
    let saved = config.theme.clone();
    Rc::new(move || {
        // Read out first: each is a separate signal, and `theme_with` is not something to run inside one's borrow.
        let (name, mode, accent) = (name.get(), mode.get(), accent.get());
        base.theme_with(&ThemeConfig {
            name,
            mode,
            accent,
            ..saved.clone()
        })
    })
}

/// The palette as fourteen swatches. The one control on this page that is not a field: a scheme is a thing you look at, and `accent = "cyan"` in a text box is a name for a colour rather than the colour.
fn palette_preview(palette: Palette, theme: NordTheme) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut swatches: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(PREVIEW_TOKENS.len());
    for token in PREVIEW_TOKENS {
        let palette = palette.clone();
        swatches.push(Box::new(StyledContainer::new(
            LayoutStyle::new().width(SWATCH).height(SWATCH),
            move |_r| {
                RectStyle::filled(palette().token(token), SWATCH_RADIUS)
                    .with_stroke(telar::Stroke::new(theme.overlay, 1.0))
            },
            vec![],
        )?));
    }
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .flex_grow(1.0)
            .min_width(0.0),
        swatches,
    )?;
    labelled(|| telar::t!("settings.field.palette"), Box::new(row), theme)
}

/// One tile per selectable theme, each painted in its own colours: the surface it would give the shell, the ink it would write with, and its accent. The tile a cycle button replaces — ten presses to see ten palettes is the control this page had, and the reason K2 existed.
fn theme_swatches(
    name: RwSignal<String>,
    mode: telar::ReadSignal<String>,
    config: Config,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let config = Arc::new(config);
    let mut tiles: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(theme_options().len());
    for option in theme_options() {
        tiles.push(theme_tile(
            option,
            name.clone(),
            mode.clone(),
            &config,
            theme,
        )?);
    }
    let grid = Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .flex_grow(1.0)
            .min_width(0.0),
        tiles,
    )?;
    labelled(|| telar::t!("settings.field.name"), Box::new(grid), theme)
}

fn theme_tile(
    option: &'static str,
    name: RwSignal<String>,
    mode: telar::ReadSignal<String>,
    config: &Arc<Config>,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let saved = config.theme.clone();
    let config = Arc::clone(config);
    // Resolved with the page's *pending* mode, so switching to light repaints every tile rather than showing ten dark palettes above a mode the user has already changed.
    let swatch_of = move || {
        let mode = mode.get();
        config.theme_with(&ThemeConfig {
            name: option.to_string(),
            mode,
            ..saved.clone()
        })
    };

    let ink = swatch_of.clone();
    let label = Text::auto(
        move || option.to_string(),
        LayoutStyle::new(),
        move || theme.text_style(FontRole::Caption, ink().text),
    )?;
    let dot_of = swatch_of.clone();
    let dot = StyledContainer::new(
        LayoutStyle::new().width(10.0).height(10.0).flex_shrink(0.0),
        move |_r| RectStyle::filled(dot_of().accent, 5.0),
        vec![],
    )?;
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(space::sm()),
        vec![Box::new(dot), box_item(label)],
    )?;

    let selected = name.read_only();
    let fill = swatch_of;
    let tile = StyledContainer::new(
        LayoutStyle::new()
            .width(TILE_WIDTH)
            .height(TILE_HEIGHT)
            .padding_horizontal(space::md())
            .align_items(AlignItems::CENTER)
            .justify_content(JustifyContent::CENTER),
        move |_r| {
            // Read both out before painting: `selected` and the palette closure each touch the runtime.
            let chosen = selected.get() == option;
            let palette = fill();
            let border = if chosen { theme.accent } else { theme.overlay };
            RectStyle::filled(palette.surface, SWATCH_RADIUS)
                .with_stroke(telar::Stroke::new(border, if chosen { 2.0 } else { 1.0 }))
        },
        vec![Box::new(row)],
    )?
    .on_press(move || name.set(option.to_string()));
    Ok(Box::new(tile))
}

/// The accents `[theme] accent` accepts, each drawn in the pending palette's own version of that hue — so "cyan" under rosé-pine is rosé-pine's cyan, which is the whole point of naming a hue rather than a hex.
fn accent_swatches(
    accent: RwSignal<String>,
    palette: Palette,
    theme: NordTheme,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    let mut swatches: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(ACCENT_NAMES.len());
    for option in ACCENT_NAMES {
        let selected = accent.read_only();
        let palette = palette.clone();
        let set = accent.clone();
        swatches.push(Box::new(
            StyledContainer::new(
                LayoutStyle::new().width(SWATCH).height(SWATCH),
                move |_r| {
                    let chosen = selected.get() == *option;
                    let colour = palette().accent_by_name(option);
                    let border = if chosen { theme.text } else { theme.overlay };
                    RectStyle::filled(colour, SWATCH_RADIUS)
                        .with_stroke(telar::Stroke::new(border, if chosen { 2.0 } else { 1.0 }))
                },
                vec![],
            )?
            .on_press(move || set.set(option.to_string())),
        ));
    }
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .flex_wrap()
            .gap(space::md())
            .flex_grow(1.0)
            .min_width(0.0),
        swatches,
    )?;
    labelled(|| telar::t!("settings.field.accent"), Box::new(row), theme)
}

pub(crate) fn theme_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let t = &config.theme;
    let name = signal(t.name.clone());
    let mode = signal(t.mode.clone());
    let variant = signal(t.variant.clone());
    let fallback = signal(t.fallback.clone());
    let accent = signal(t.accent.clone());
    let font_family = signal(t.font_family.clone().unwrap_or_default());
    let radius = signal(opt_num(t.radius));
    let spacing = signal(opt_num(t.spacing));
    let font_size = signal(opt_num(t.font_size));
    let opacity = signal(t.opacity.to_string());
    let icon_size = signal(opt_num(t.icon_size));
    let icon_stroke = signal(opt_num(t.icon_stroke));
    let scale_rounding = signal(t.scale.rounding.to_string());
    let scale_spacing = signal(t.scale.spacing.to_string());
    let scale_font = signal(t.scale.font.to_string());
    let scale_icon = signal(t.scale.icon.to_string());

    // What the pickers below and the preview above them all read: the palette the *pending* selection resolves to, not the one the shell is currently wearing. A swatch row showing the saved theme while the user is choosing another one is a preview of the wrong thing.
    let pending = pending_palette(
        &config,
        name.read_only(),
        mode.read_only(),
        accent.read_only(),
    );

    let rows = vec![
        palette_preview(pending.clone(), theme)?,
        theme_swatches(name.clone(), mode.read_only(), config.clone(), theme)?,
        accent_swatches(accent.clone(), pending, theme)?,
        enum_field(
            || telar::t!("settings.field.color_mode"),
            mode.clone(),
            MODES,
            theme,
        )?,
        enum_field(
            || telar::t!("settings.field.variant"),
            variant.clone(),
            VARIANTS,
            theme,
        )?,
        enum_field(
            || telar::t!("settings.field.fallback"),
            fallback.clone(),
            BUILT_IN_THEMES,
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.font_family"),
            font_family.clone(),
            "(default)",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.radius"),
            radius.clone(),
            "(theme)",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.spacing"),
            spacing.clone(),
            "(theme)",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.font_size"),
            font_size.clone(),
            "(theme)",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.opacity"),
            opacity.clone(),
            "1",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.icon_size"),
            icon_size.clone(),
            "(theme)",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.icon_stroke"),
            icon_stroke.clone(),
            "(glyph)",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.scale_rounding"),
            scale_rounding.clone(),
            "1",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.scale_spacing"),
            scale_spacing.clone(),
            "1",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.scale_font"),
            scale_font.clone(),
            "1",
            theme,
        )?,
        text_field(
            || telar::t!("settings.field.scale_icon"),
            scale_icon.clone(),
            "1",
            theme,
        )?,
    ];

    let base = t.clone();
    let path = path.to_path_buf();
    let save = save_button(
        || telar::t!("settings.save.theme"),
        move || {
            let value = ThemeConfig {
                opacity: parse_f32(&opacity.peek(), base.opacity),
                name: name.peek(),
                mode: mode.peek(),
                variant: variant.peek(),
                fallback: fallback.peek(),
                accent: accent.peek(),
                font_family: opt_string(&font_family.peek()),
                radius: opt_u32(&radius.peek()),
                spacing: opt_u32(&spacing.peek()),
                font_size: opt_f32(&font_size.peek()),
                icon_size: opt_f32(&icon_size.peek()),
                icon_stroke: opt_f32(&icon_stroke.peek()),
                scale: ScaleConfig {
                    rounding: parse_f32(&scale_rounding.peek(), base.scale.rounding),
                    spacing: parse_f32(&scale_spacing.peek(), base.scale.spacing),
                    font: parse_f32(&scale_font.peek(), base.scale.font),
                    icon: parse_f32(&scale_icon.peek(), base.scale.icon),
                },
                // Carried through unchanged, like `colors`: per-role overrides and the export switches are nested tables the flat panel has no rows for, and rewriting the section must not drop them.
                fonts: base.fonts,
                export: base.export.clone(),
                colors: base.colors.clone(),
            };
            persist(&path, "theme", &value);
        },
    )?;
    section(|| telar::t!("settings.section.theme"), rows, save, theme)
}

/// K13, first half: the maps whose keys are enumerable.
///
/// `background.monitors` came off this list with J9 by the route that generalises worst and works best — its keys are not free text, they are the monitors that exist, so the panel names them instead of asking the user to type one. Three of the four remaining maps take the same route, each with its own answer to "what are the keys":
///
/// - `[theme.colors]` — the palette's own token names, which are fixed and shipped ([`THEME_TOKENS`]).
/// - `[modules.<id>]` — every module registered in the shell, so a chip can be restyled before it is on a bar.
/// - `[media.aliases]` — the players that have been seen on the bus, plus whatever the config already names. The one genuinely open set here, handled exactly as `monitor_keys` handles a monitor left at the office: listing only what is running now would delete an alias for a player that happens to be closed.
///
/// A row per key with the *resolved* value as its placeholder, so an empty field reads as "whatever the theme says" rather than as a value that got lost.
pub(crate) fn theme_colors_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let resolved = config.resolve_theme();
    let fields: Vec<(&'static str, RwSignal<String>)> = THEME_TOKENS
        .iter()
        .map(|token| {
            (
                *token,
                signal(config.theme.colors.get(*token).cloned().unwrap_or_default()),
            )
        })
        .collect();

    let mut rows: Vec<Box<dyn LayoutItem>> = Vec::with_capacity(fields.len());
    for (token, value) in fields.iter().map(|(t, v)| (*t, v.clone())) {
        rows.push(text_field(
            move || token.to_string(),
            value,
            &config::theme::hex(resolved.token(token)),
            theme,
        )?);
    }

    let path = path.to_path_buf();
    let save = save_button(
        || telar::t!("settings.save.theme_colors"),
        move || {
            let colors: std::collections::HashMap<String, String> = fields
                .iter()
                .filter_map(|(token, value)| {
                    opt_string(&value.peek()).map(|hex| (token.to_string(), hex))
                })
                .collect();
            // Only this form's key: `theme_section` above owns every other one in `[theme]`.
            persist_with(&path, "theme", |current| ThemeConfig {
                colors,
                ..current.theme.clone()
            });
        },
    )?;
    section(
        || telar::t!("settings.section.theme_colors"),
        rows,
        save,
        theme,
    )
}
