//! How the shell looks: the palette, the shapes it draws and how it moves.
//!
//! What is left here is the forms this area cannot say in `.rsx`: the ones whose rows are a list the machine decides the length of. The static-shape forms are `.rsx` components beside this file.

use std::rc::Rc;
use std::sync::Arc;
use ui::scale::space;

use telar::{
    Children, Container, LayoutError, LayoutItem, LayoutStyle, Reactive, RectStyle, RwSignal,
    StyledContainer, signal,
};

use crate::form::*;
use config::theme::{ACCENTS, BUILT_IN_THEMES, NordTheme, THEME_TOKENS};
use config::{Config, ScaleConfig, ThemeConfig};
use ui::form::enum_row::{EnumRowProps, enum_row};
use ui::form::labelled::labelled;
use ui::form::swatch_row::{SwatchRowProps, swatch_row};
use ui::form::text_row::{TextRowProps, text_row};
use ui::form::theme_tiles::theme_tiles;

/// A palette a control draws from and re-reads: the pending `[theme]` selection resolved through [`Config::theme_with`], so a swatch shows the theme being chosen rather than the one being worn.
type Palette = Rc<dyn Fn() -> NordTheme>;

/// The palette tokens the preview strip shows, in the order they read as a design rather than as a list: the three surfaces the shell is built out of, the two inks over them, then the hues.
const PREVIEW_TOKENS: &[&str] = &[
    "base", "surface", "overlay", "text", "subtle", "accent", "red", "orange", "yellow", "green",
    "cyan", "blue", "teal", "purple",
];

const SWATCH: f32 = 22.0;
const SWATCH_RADIUS: f32 = 6.0;

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
                    .with_border(telar::Border::uniform(theme.overlay, 1.0))
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
    labelled(
        Reactive::of(|| telar::t!("settings.field.palette")),
        Box::new(row),
    )
}

fn theme_swatches(
    name: RwSignal<String>,
    mode: telar::ReadSignal<String>,
    config: Config,
) -> Result<Box<dyn LayoutItem>, LayoutError> {
    labelled(
        Reactive::of(|| telar::t!("settings.field.name")),
        theme_tiles(name, mode, Arc::new(config))?,
    )
}

fn accent_row(accent: RwSignal<String>) -> Result<Box<dyn LayoutItem>, LayoutError> {
    swatch_row(
        SwatchRowProps::props()
            .label(Reactive::of(|| telar::t!("settings.field.accent")))
            .value(accent)
            .tokens(Rc::<[&'static str]>::from(ACCENTS))
            .accepts(Rc::new(NordTheme::has_accent))
            .build(),
        Children::default(),
    )
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
        theme_swatches(name, mode.read_only(), config.clone())?,
        accent_row(accent)?,
        enum_row(
            EnumRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.color_mode")))
                .value(mode)
                .options(MODES)
                .build(),
            Children::default(),
        )?,
        enum_row(
            EnumRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.variant")))
                .value(variant)
                .options(VARIANTS)
                .build(),
            Children::default(),
        )?,
        enum_row(
            EnumRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.fallback")))
                .value(fallback)
                .options(BUILT_IN_THEMES)
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.font_family")))
                .value(font_family)
                .placeholder("(default)")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.radius")))
                .value(radius)
                .placeholder("(theme)")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.spacing")))
                .value(spacing)
                .placeholder("(theme)")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.font_size")))
                .value(font_size)
                .placeholder("(theme)")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.opacity")))
                .value(opacity)
                .placeholder("1")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.icon_size")))
                .value(icon_size)
                .placeholder("(theme)")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.icon_stroke")))
                .value(icon_stroke)
                .placeholder("(glyph)")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.scale_rounding")))
                .value(scale_rounding)
                .placeholder("1")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.scale_spacing")))
                .value(scale_spacing)
                .placeholder("1")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.scale_font")))
                .value(scale_font)
                .placeholder("1")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.scale_icon")))
                .value(scale_icon)
                .placeholder("1")
                .build(),
            Children::default(),
        )?,
    ];

    let base = t.clone();
    let path = path.to_path_buf();
    let save = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| telar::t!("settings.save.theme")))
            .on_press(std::rc::Rc::new(move || {
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
            }))
            .build(),
        telar::Children::default(),
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
    for (token, value) in fields.iter().map(|(t, v)| (*t, *v)) {
        rows.push(text_row(
            TextRowProps::props()
                .label(Reactive::of(move || token.to_string()))
                .value(value)
                .placeholder(config::theme::hex(resolved.token(token)))
                .build(),
            Children::default(),
        )?);
    }

    let path = path.to_path_buf();
    let save = save_button(
        SaveButtonProps::props()
            .label(telar::Reactive::of(|| {
                telar::t!("settings.save.theme_colors")
            }))
            .on_press(std::rc::Rc::new(move || {
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
            }))
            .build(),
        telar::Children::default(),
    )?;
    section(
        || telar::t!("settings.section.theme_colors"),
        rows,
        save,
        theme,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hex_accent_is_the_pending_palettes_accent_and_lights_no_swatch() {
        let config = Config::default();
        let (name, mode) = (signal(config.theme.name.clone()), signal(String::new()));
        let accent = signal("#ff8800".to_string());
        let palette = pending_palette(
            &config,
            name.read_only(),
            mode.read_only(),
            accent.read_only(),
        );
        assert_eq!(
            palette().accent,
            telar::Color::from_hex("#ff8800").expect("a colour")
        );
        assert!(!ACCENTS.contains(&accent.peek().as_str()));
    }

    #[test]
    fn the_accent_row_takes_what_the_theme_takes() {
        assert!(NordTheme::has_accent("#ff8800"));
        for refused in ["surface", "bad", "add", "#12345", "#ff880080"] {
            assert!(!NordTheme::has_accent(refused), "{refused:?} accepted");
        }
    }
}
