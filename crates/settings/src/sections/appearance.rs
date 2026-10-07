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
use config::presets;
use config::theme::{ACCENTS, BUILT_IN_THEMES, FontRole, NordTheme, THEME_TOKENS};
use config::{Config, IconMask, IconsConfig, ScaleConfig, ThemeConfig};
use ui::form::enum_row::{EnumRowProps, enum_row};
use ui::form::labelled::labelled;
use ui::form::listed_row::{keeping_current, listed_row};
use ui::form::named_list::{NamedActions, named_list};
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
    let weights = FontRole::ALL.map(|role| signal(opt_num(t.fonts.spec(role).weight)));

    // What the pickers below and the preview above them all read: the palette the *pending* selection resolves to, not the one the shell is currently wearing. A swatch row showing the saved theme while the user is choosing another one is a preview of the wrong thing.
    let pending = pending_palette(
        &config,
        name.read_only(),
        mode.read_only(),
        accent.read_only(),
    );

    let mut rows = vec![
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
    for (weight, role) in weights.into_iter().zip(FontRole::ALL) {
        rows.push(listed_row(
            weight_label(role),
            weight,
            weight_options(&weight.peek()),
        )?);
    }

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
                    fonts: with_weights(base.fonts, weights.map(|weight| weight.peek())),
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

fn weight_label(role: FontRole) -> Reactive<String> {
    match role {
        FontRole::Display => Reactive::of(|| telar::t!("settings.field.weight_display")),
        FontRole::Title => Reactive::of(|| telar::t!("settings.field.weight_title")),
        FontRole::Body => Reactive::of(|| telar::t!("settings.field.weight_body")),
        FontRole::Caption => Reactive::of(|| telar::t!("settings.field.weight_caption")),
    }
}

/// The role's own weight, every hundred a font can be drawn at, and `current` where it is none of them.
fn weight_options(current: &str) -> Rc<[(String, String)]> {
    let mut options = vec![(String::new(), telar::t!("settings.field.weight_default"))];
    options.extend(
        config::theme::font_weights().map(|weight| (weight.to_string(), weight.to_string())),
    );
    keeping_current(options, current)
}

fn with_weights(mut fonts: config::FontsConfig, weights: [String; 4]) -> config::FontsConfig {
    for (role, weight) in FontRole::ALL.into_iter().zip(weights) {
        fonts.spec_mut(role).weight =
            opt_u32(&weight).and_then(|weight| u16::try_from(weight).ok());
    }
    fonts
}

/// Where icons come from and how an application's own is drawn. In Rust rather than `.rsx` because the icon themes it offers are the ones installed on this machine.
pub(crate) fn icons_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (config, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let icons = &config.icons;
    let provider = signal(icons.provider.clone());
    let default_set = signal(icons.default_set.clone());
    let app_icon_theme = signal(icons.app_icon_theme.clone());
    let mask = signal(icons.mask.id().to_string());

    let mut themes = vec![(String::new(), telar::t!("settings.field.icon_theme_auto"))];
    themes.extend(
        ui::icon::installed_icon_themes()
            .into_iter()
            .map(|theme| (theme.id, theme.name)),
    );
    let masks: Vec<(String, String)> = IconMask::ALL
        .into_iter()
        .map(|mask| {
            let shown = match mask {
                IconMask::None => telar::t!("settings.field.mask_none"),
                IconMask::Circle => telar::t!("settings.field.mask_circle"),
                IconMask::Squircle => telar::t!("settings.field.mask_squircle"),
            };
            (mask.id().to_string(), shown)
        })
        .collect();

    let rows = vec![
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.provider")))
                .value(provider)
                .placeholder("https://api.iconify.design")
                .build(),
            Children::default(),
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.default_set")))
                .value(default_set)
                .placeholder("lucide")
                .build(),
            Children::default(),
        )?,
        listed_row(
            Reactive::of(|| telar::t!("settings.field.app_icon_theme")),
            app_icon_theme,
            keeping_current(themes, &app_icon_theme.peek()),
        )?,
        listed_row(
            Reactive::of(|| telar::t!("settings.field.icon_mask")),
            mask,
            Rc::from(masks),
        )?,
    ];

    let path = path.to_path_buf();
    let save = save_button(
        SaveButtonProps::props()
            .label(Reactive::of(|| telar::t!("settings.save.icons")))
            .on_press(Rc::new(move || {
                persist(
                    &path,
                    "icons",
                    &IconsConfig {
                        provider: provider.peek(),
                        default_set: default_set.peek(),
                        app_icon_theme: app_icon_theme.peek(),
                        mask: IconMask::from_id(&mask.peek()).unwrap_or_default(),
                    },
                );
            }))
            .build(),
        Children::default(),
    )?;
    section(|| telar::t!("settings.section.icons"), rows, save, theme)
}

/// The theme presets kept beside the config: each one put into `[theme]` or deleted, and the theme the file says now kept under a new name.
///
/// Putting one into `[theme]` writes the file without vouching for the window, so the reload it causes rebuilds every form here from the theme it put there.
pub(crate) fn theme_presets_section() -> Result<Box<dyn LayoutItem>, LayoutError> {
    let (_, path) = crate::form::source();
    let theme = telar::use_theme::<NordTheme>();
    let names = signal(presets::list(&path));
    let naming = signal(String::new());
    let status = signal(String::new());
    let refresh = {
        let path = path.clone();
        move || names.set(presets::list(&path))
    };
    let said = move |result: Result<String, presets::PresetError>| match result {
        Ok(done) => status.set(done),
        Err(why) => status.set(telar::t!(
            "settings.presets.failed",
            why = why.message().render()
        )),
    };

    let actions = NamedActions {
        pick: Rc::new(|| telar::t!("settings.presets.apply")),
        on_pick: Rc::new({
            let path = path.clone();
            move |name: &str| {
                said(
                    presets::apply(&path, name)
                        .map(|_| telar::t!("settings.presets.applied", name = name)),
                )
            }
        }),
        remove: Rc::new(|| telar::t!("settings.presets.delete")),
        on_remove: Rc::new({
            let (path, refresh) = (path.clone(), refresh.clone());
            move |name: &str| {
                said(
                    presets::delete(&path, name)
                        .map(|()| telar::t!("settings.presets.deleted", name = name)),
                );
                refresh();
            }
        }),
    };
    let rows = vec![
        named_list(
            names.read_only(),
            || telar::t!("settings.presets.empty"),
            actions,
        )?,
        text_row(
            TextRowProps::props()
                .label(Reactive::of(|| telar::t!("settings.field.preset_name")))
                .value(naming)
                .placeholder("dusk")
                .build(),
            Children::default(),
        )?,
        Box::new(telar::Text::declaring(
            move || status.get(),
            LayoutStyle::new(),
            move |inherited| theme.text_over(inherited, FontRole::Caption, theme.subtle),
        )?) as Box<dyn LayoutItem>,
    ];
    let save = telar::button(
        telar::ButtonProps::props()
            .label(Reactive::of(|| telar::t!("settings.presets.save")))
            .on_press(Rc::new(move || {
                let name = naming.peek().trim().to_string();
                if !presets::is_name(&name) {
                    status.set(telar::t!("settings.presets.bad_name"));
                    return;
                }
                let current = Config::load_or_default(&path).theme;
                said(
                    presets::save(&path, &name, &current)
                        .map(|()| telar::t!("settings.presets.saved", name = name)),
                );
                refresh();
            }))
            .build(),
        Children::default(),
    )?;
    section(
        || telar::t!("settings.section.theme_presets"),
        rows,
        save,
        theme,
    )
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
    fn each_weight_row_writes_its_own_role_and_an_empty_one_unsets_it() {
        let mut fonts = config::FontsConfig::default();
        fonts.caption.weight = Some(300);
        fonts.body.size = Some(15.0);
        let rows = FontRole::ALL.map(|role| match role {
            FontRole::Title => "650".to_string(),
            FontRole::Body => "500".to_string(),
            _ => String::new(),
        });
        let saved = with_weights(fonts, rows);
        assert_eq!(saved.title.weight, Some(650));
        assert_eq!(saved.body.weight, Some(500));
        assert_eq!(saved.body.size, Some(15.0), "the rest of a role stays");
        assert_eq!(saved.caption.weight, None);
        assert_eq!(saved.display.weight, None);
    }

    #[test]
    fn the_accent_row_takes_what_the_theme_takes() {
        assert!(NordTheme::has_accent("#ff8800"));
        for refused in ["surface", "bad", "add", "#12345", "#ff880080"] {
            assert!(!NordTheme::has_accent(refused), "{refused:?} accepted");
        }
    }
}
