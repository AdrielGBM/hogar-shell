use std::any::Any;

use telar::{Color, TextStyle, Theme, ThemeTokens};

use crate::{FontSpec, FontsConfig};

/// Semantic text sizes, each a step off the theme's base [`font_size`](NordTheme::font_size).
#[derive(Clone, Copy)]
pub enum FontRole {
    /// Small secondary text: badges, pills, chip labels, notification bodies.
    Caption,
    /// Default body text.
    Body,
    /// Section/panel headers.
    Title,
    /// Large display text, e.g. the clock face.
    Display,
}

#[derive(Clone, Copy)]
pub struct NordTheme {
    /// Base corner radius the theme rounds panels and bars to (the design default; `[shape]`/per-bar can override).
    pub radius: f32,
    /// Base gap between adjacent modules inside a bar/section (the design default; `[shape]`/per-bar can override).
    pub spacing: f32,
    /// Base (body) font size in px. Every other text size steps off this via [`font`](Self::font), so scaling it scales all text.
    pub font_size: f32,
    /// Default size for a standalone icon (px), e.g. the OSD glyph. Bar chips derive their icon size from the bar thickness instead, so they scale with the bar.
    pub icon_size: f32,
    /// Stroke width (SVG userspace units) forced on stroke-based icon glyphs, e.g. `1.5` to thin Lucide's default `2`. `None` keeps each glyph's own stroke. `[theme] icon_stroke` overrides it.
    pub icon_stroke: Option<f32>,
    /// Per-role size/weight/italic overrides from `[theme.fonts]`, applied by [`text_style`](Self::text_style).
    pub fonts: FontsConfig,
    pub base: Color,
    pub surface: Color,
    pub overlay: Color,
    pub muted: Color,
    pub subtle: Color,
    pub text: Color,
    pub accent: Color,
    pub blue: Color,
    pub cyan: Color,
    pub teal: Color,
    pub red: Color,
    pub orange: Color,
    pub yellow: Color,
    pub green: Color,
    pub purple: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub highlight_low: Color,
    pub highlight_med: Color,
    pub highlight_high: Color,
}

/// A theme's descriptive metadata (§9), so a theme can present itself (in a picker, logs, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeMeta {
    pub name: &'static str,
    pub author: &'static str,
    pub description: &'static str,
    pub version: &'static str,
}

/// Every palette token, in the order an export lists them — the surfaces, the inks, the hues, the semantic
/// four, then the highlights.
///
/// One list rather than four: the scheme exporter, the IPC palette dump, `[theme.colors]`'s editor and
/// [`NordTheme::token`] all need the same names, and each copy that existed was a place a token added to the
/// theme could go missing from without anything failing.
pub const THEME_TOKENS: &[&str] = &[
    "base",
    "surface",
    "overlay",
    "muted",
    "subtle",
    "text",
    "accent",
    "blue",
    "cyan",
    "teal",
    "red",
    "orange",
    "yellow",
    "green",
    "purple",
    "success",
    "warning",
    "error",
    "info",
    "highlight_low",
    "highlight_med",
    "highlight_high",
];

/// A colour as `#rrggbb`, which is the only spelling `[theme.colors]` and every export file use.
pub fn hex(color: Color) -> String {
    let [r, g, b, _] = color.to_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Every built-in palette's config name, in the order a picker should offer them.
pub const BUILT_IN_THEMES: &[&str] = &[
    "nord",
    "rose-pine",
    "rose-pine-moon",
    "rose-pine-dawn",
    "catppuccin-mocha",
    "catppuccin-macchiato",
    "catppuccin-frappe",
    "catppuccin-latte",
    "gruvbox",
    "gruvbox-light",
    "tokyo-night",
    "everforest",
];

/// A theme name reduced to what identifies it, so `rose-pine`, `rose_pine` and `rosepine` are one theme and a
/// user's separator preference never becomes a "unknown theme" warning.
fn normalize(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl NordTheme {
    /// The built-in palette for `name` (see [`BUILT_IN_THEMES`]); `custom` starts from nord for config to
    /// override, `dynamic` likewise until [`Config::resolve_theme`](crate::Config::resolve_theme) substitutes
    /// the wallpaper's own palette, and an unknown name falls back to nord with a warning.
    pub fn named(name: &str) -> Self {
        match normalize(name).as_str() {
            "nord" | "custom" | "dynamic" => Self::nord(),
            "rosepine" => Self::rose_pine(),
            "rosepinemoon" => Self::rose_pine_moon(),
            "rosepinedawn" => Self::rose_pine_dawn(),
            "catppuccin" | "catppuccinmocha" => Self::catppuccin_mocha(),
            "catppuccinmacchiato" => Self::catppuccin_macchiato(),
            "catppuccinfrappe" => Self::catppuccin_frappe(),
            "catppuccinlatte" => Self::catppuccin_latte(),
            "gruvbox" | "gruvboxdark" => Self::gruvbox(),
            "gruvboxlight" => Self::gruvbox_light(),
            "tokyonight" => Self::tokyo_night(),
            "everforest" | "everforestdark" => Self::everforest(),
            other => {
                tracing::warn!("unknown theme '{other}', falling back to nord");
                Self::nord()
            }
        }
    }

    /// The sibling of `name` in `mode`, or `name` itself when the family has no palette at that end.
    ///
    /// A light/dark switch is a *family* choice, not a recolouring: Gruvbox Light is a designed palette, and
    /// inverting Gruvbox Dark's ramp would produce something that is neither. A family with only one side —
    /// Nord, Tokyo Night, Everforest — therefore keeps what it has rather than being forced through an
    /// inversion its author never drew.
    pub fn in_mode(name: &str, mode: crate::scheme::Mode) -> &'static str {
        use crate::scheme::Mode;
        let normalized = normalize(name);
        let family: &[(&str, &str)] = &[
            ("rosepine", "rosepinedawn"),
            ("rosepinemoon", "rosepinedawn"),
            ("catppuccinmocha", "catppuccinlatte"),
            ("catppuccin", "catppuccinlatte"),
            ("catppuccinmacchiato", "catppuccinlatte"),
            ("catppuccinfrappe", "catppuccinlatte"),
            ("gruvbox", "gruvboxlight"),
            ("gruvboxdark", "gruvboxlight"),
        ];
        let sibling = match mode {
            Mode::Light => family
                .iter()
                .find(|(dark, _)| *dark == normalized)
                .map(|(_, light)| *light),
            Mode::Dark => family
                .iter()
                .find(|(_, light)| *light == normalized)
                .map(|(dark, _)| *dark),
        };
        Self::canonical(sibling.unwrap_or(&normalized))
    }

    /// A name as [`BUILT_IN_THEMES`] spells it, so [`in_mode`](Self::in_mode) hands back something a user can
    /// read and `hyprshell scheme set` accepts — not the separator-stripped form the lookup matches on. An
    /// unknown name resolves to nord, which is where [`named`](Self::named) would send it anyway.
    fn canonical(name: &str) -> &'static str {
        let normalized = normalize(name);
        BUILT_IN_THEMES
            .iter()
            .copied()
            .find(|known| normalize(known) == normalized)
            .unwrap_or("nord")
    }

    /// Metadata for a built-in theme name (falls back to nord's).
    pub fn meta(name: &str) -> ThemeMeta {
        match normalize(name).as_str() {
            "rosepine" => ThemeMeta {
                name: "Rosé Pine",
                author: "Rosé Pine",
                description: "Soho vibes — a warm, low-contrast dark theme.",
                version: "1.0.0",
            },
            "rosepinemoon" => ThemeMeta {
                name: "Rosé Pine Moon",
                author: "Rosé Pine",
                description: "A softer, slightly brighter take on the dark theme.",
                version: "1.0.0",
            },
            "rosepinedawn" => ThemeMeta {
                name: "Rosé Pine Dawn",
                author: "Rosé Pine",
                description: "The light variant — warm parchment tones.",
                version: "1.0.0",
            },
            "catppuccin" | "catppuccinmocha" => ThemeMeta {
                name: "Catppuccin Mocha",
                author: "Catppuccin",
                description: "The darkest flavour — soothing pastels on deep violet.",
                version: "1.0.0",
            },
            "catppuccinmacchiato" => ThemeMeta {
                name: "Catppuccin Macchiato",
                author: "Catppuccin",
                description: "A medium-dark flavour, warmer than Mocha.",
                version: "1.0.0",
            },
            "catppuccinfrappe" => ThemeMeta {
                name: "Catppuccin Frappé",
                author: "Catppuccin",
                description: "The lightest dark flavour — low contrast, easy on the eyes.",
                version: "1.0.0",
            },
            "catppuccinlatte" => ThemeMeta {
                name: "Catppuccin Latte",
                author: "Catppuccin",
                description: "The light flavour — pastels on cool paper.",
                version: "1.0.0",
            },
            "gruvbox" | "gruvboxdark" => ThemeMeta {
                name: "Gruvbox Dark",
                author: "Pavel Pertsev",
                description: "Retro groove — warm, high-contrast earth tones.",
                version: "1.0.0",
            },
            "gruvboxlight" => ThemeMeta {
                name: "Gruvbox Light",
                author: "Pavel Pertsev",
                description: "The light variant — the same earth tones on cream.",
                version: "1.0.0",
            },
            "tokyonight" => ThemeMeta {
                name: "Tokyo Night",
                author: "Enkia",
                description: "A dark blue palette from a night in downtown Tokyo.",
                version: "1.0.0",
            },
            "everforest" | "everforestdark" => ThemeMeta {
                name: "Everforest",
                author: "sainnhe",
                description: "A green-based forest palette, soft and low contrast.",
                version: "1.0.0",
            },
            _ => ThemeMeta {
                name: "Nord",
                author: "Arctic Ice Studio",
                description: "An arctic, north-bluish palette.",
                version: "1.0.0",
            },
        }
    }

    /// The Nord palette (the default built-in). [`new`](Self::new) is kept as its alias.
    pub fn nord() -> Self {
        Self::new()
    }

    /// The Rosé Pine palette — its own colours and a slightly rounder, airier metric set, to show a theme carries all its design tokens.
    pub fn rose_pine() -> Self {
        Self {
            radius: 10.0,
            spacing: 8.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(25, 23, 36),
            surface: Color::from_rgb_u8(31, 29, 46),
            overlay: Color::from_rgb_u8(38, 35, 58),
            muted: Color::from_rgb_u8(110, 106, 134),
            subtle: Color::from_rgb_u8(144, 140, 170),
            text: Color::from_rgb_u8(224, 222, 244),
            accent: Color::from_rgb_u8(156, 207, 216),
            blue: Color::from_rgb_u8(49, 116, 143),
            cyan: Color::from_rgb_u8(156, 207, 216),
            teal: Color::from_rgb_u8(49, 116, 143),
            red: Color::from_rgb_u8(235, 111, 146),
            orange: Color::from_rgb_u8(246, 193, 119),
            yellow: Color::from_rgb_u8(246, 193, 119),
            green: Color::from_rgb_u8(156, 207, 216),
            purple: Color::from_rgb_u8(196, 167, 231),
            success: Color::from_rgb_u8(49, 116, 143),
            warning: Color::from_rgb_u8(246, 193, 119),
            error: Color::from_rgb_u8(235, 111, 146),
            info: Color::from_rgb_u8(49, 116, 143),
            highlight_low: Color::from_rgb_u8(33, 32, 46),
            highlight_med: Color::from_rgb_u8(64, 61, 82),
            highlight_high: Color::from_rgb_u8(82, 79, 103),
        }
    }

    /// Rosé Pine Moon — the softer, slightly brighter sibling of the dark palette; shares Rosé Pine's metrics.
    pub fn rose_pine_moon() -> Self {
        Self {
            radius: 10.0,
            spacing: 8.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(35, 33, 54),
            surface: Color::from_rgb_u8(42, 39, 63),
            overlay: Color::from_rgb_u8(57, 53, 82),
            muted: Color::from_rgb_u8(110, 106, 134),
            subtle: Color::from_rgb_u8(144, 140, 170),
            text: Color::from_rgb_u8(224, 222, 244),
            accent: Color::from_rgb_u8(156, 207, 216),
            blue: Color::from_rgb_u8(62, 143, 176),
            cyan: Color::from_rgb_u8(156, 207, 216),
            teal: Color::from_rgb_u8(62, 143, 176),
            red: Color::from_rgb_u8(235, 111, 146),
            orange: Color::from_rgb_u8(246, 193, 119),
            yellow: Color::from_rgb_u8(246, 193, 119),
            green: Color::from_rgb_u8(156, 207, 216),
            purple: Color::from_rgb_u8(196, 167, 231),
            success: Color::from_rgb_u8(62, 143, 176),
            warning: Color::from_rgb_u8(246, 193, 119),
            error: Color::from_rgb_u8(235, 111, 146),
            info: Color::from_rgb_u8(62, 143, 176),
            highlight_low: Color::from_rgb_u8(42, 40, 62),
            highlight_med: Color::from_rgb_u8(68, 65, 90),
            highlight_high: Color::from_rgb_u8(86, 82, 110),
        }
    }

    /// Rosé Pine Dawn — the light variant, warm parchment tones; shares Rosé Pine's metrics.
    pub fn rose_pine_dawn() -> Self {
        Self {
            radius: 10.0,
            spacing: 8.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(250, 244, 237),
            surface: Color::from_rgb_u8(255, 250, 243),
            overlay: Color::from_rgb_u8(242, 233, 225),
            muted: Color::from_rgb_u8(152, 147, 165),
            subtle: Color::from_rgb_u8(121, 117, 147),
            text: Color::from_rgb_u8(87, 82, 121),
            accent: Color::from_rgb_u8(86, 148, 159),
            blue: Color::from_rgb_u8(40, 105, 131),
            cyan: Color::from_rgb_u8(86, 148, 159),
            teal: Color::from_rgb_u8(40, 105, 131),
            red: Color::from_rgb_u8(180, 99, 122),
            orange: Color::from_rgb_u8(234, 157, 52),
            yellow: Color::from_rgb_u8(234, 157, 52),
            green: Color::from_rgb_u8(86, 148, 159),
            purple: Color::from_rgb_u8(144, 122, 169),
            success: Color::from_rgb_u8(40, 105, 131),
            warning: Color::from_rgb_u8(234, 157, 52),
            error: Color::from_rgb_u8(180, 99, 122),
            info: Color::from_rgb_u8(40, 105, 131),
            highlight_low: Color::from_rgb_u8(244, 237, 232),
            highlight_med: Color::from_rgb_u8(223, 218, 217),
            highlight_high: Color::from_rgb_u8(206, 202, 205),
        }
    }

    /// Catppuccin Mocha — the darkest flavour. `cyan` carries the flavour's *sky*, since the shipped default
    /// `[theme] accent = "cyan"` resolves through it; `accent` itself is *mauve*, the flavour's signature.
    pub fn catppuccin_mocha() -> Self {
        Self {
            radius: 12.0,
            spacing: 8.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(30, 30, 46),
            surface: Color::from_rgb_u8(49, 50, 68),
            overlay: Color::from_rgb_u8(69, 71, 90),
            muted: Color::from_rgb_u8(108, 112, 134),
            subtle: Color::from_rgb_u8(166, 173, 200),
            text: Color::from_rgb_u8(205, 214, 244),
            accent: Color::from_rgb_u8(203, 166, 247),
            blue: Color::from_rgb_u8(137, 180, 250),
            cyan: Color::from_rgb_u8(137, 220, 235),
            teal: Color::from_rgb_u8(148, 226, 213),
            red: Color::from_rgb_u8(243, 139, 168),
            orange: Color::from_rgb_u8(250, 179, 135),
            yellow: Color::from_rgb_u8(249, 226, 175),
            green: Color::from_rgb_u8(166, 227, 161),
            purple: Color::from_rgb_u8(203, 166, 247),
            success: Color::from_rgb_u8(166, 227, 161),
            warning: Color::from_rgb_u8(249, 226, 175),
            error: Color::from_rgb_u8(243, 139, 168),
            info: Color::from_rgb_u8(137, 180, 250),
            highlight_low: Color::from_rgb_u8(49, 50, 68),
            highlight_med: Color::from_rgb_u8(69, 71, 90),
            highlight_high: Color::from_rgb_u8(88, 91, 112),
        }
    }

    /// Catppuccin Macchiato — a step lighter than Mocha, and warmer.
    pub fn catppuccin_macchiato() -> Self {
        Self {
            base: Color::from_rgb_u8(36, 39, 58),
            surface: Color::from_rgb_u8(54, 58, 79),
            overlay: Color::from_rgb_u8(73, 77, 100),
            muted: Color::from_rgb_u8(110, 115, 141),
            subtle: Color::from_rgb_u8(165, 173, 203),
            text: Color::from_rgb_u8(202, 211, 245),
            accent: Color::from_rgb_u8(198, 160, 246),
            blue: Color::from_rgb_u8(138, 173, 244),
            cyan: Color::from_rgb_u8(145, 215, 227),
            teal: Color::from_rgb_u8(139, 213, 202),
            red: Color::from_rgb_u8(237, 135, 150),
            orange: Color::from_rgb_u8(245, 169, 127),
            yellow: Color::from_rgb_u8(238, 212, 159),
            green: Color::from_rgb_u8(166, 218, 149),
            purple: Color::from_rgb_u8(198, 160, 246),
            success: Color::from_rgb_u8(166, 218, 149),
            warning: Color::from_rgb_u8(238, 212, 159),
            error: Color::from_rgb_u8(237, 135, 150),
            info: Color::from_rgb_u8(138, 173, 244),
            highlight_low: Color::from_rgb_u8(54, 58, 79),
            highlight_med: Color::from_rgb_u8(73, 77, 100),
            highlight_high: Color::from_rgb_u8(91, 96, 120),
            ..Self::catppuccin_mocha()
        }
    }

    /// Catppuccin Frappé — the lightest of the dark flavours.
    pub fn catppuccin_frappe() -> Self {
        Self {
            base: Color::from_rgb_u8(48, 52, 70),
            surface: Color::from_rgb_u8(65, 69, 89),
            overlay: Color::from_rgb_u8(81, 87, 109),
            muted: Color::from_rgb_u8(115, 121, 148),
            subtle: Color::from_rgb_u8(165, 173, 206),
            text: Color::from_rgb_u8(198, 208, 245),
            accent: Color::from_rgb_u8(202, 158, 230),
            blue: Color::from_rgb_u8(140, 170, 238),
            cyan: Color::from_rgb_u8(153, 209, 219),
            teal: Color::from_rgb_u8(129, 200, 190),
            red: Color::from_rgb_u8(231, 130, 132),
            orange: Color::from_rgb_u8(239, 159, 118),
            yellow: Color::from_rgb_u8(229, 200, 144),
            green: Color::from_rgb_u8(166, 209, 137),
            purple: Color::from_rgb_u8(202, 158, 230),
            success: Color::from_rgb_u8(166, 209, 137),
            warning: Color::from_rgb_u8(229, 200, 144),
            error: Color::from_rgb_u8(231, 130, 132),
            info: Color::from_rgb_u8(140, 170, 238),
            highlight_low: Color::from_rgb_u8(65, 69, 89),
            highlight_med: Color::from_rgb_u8(81, 87, 109),
            highlight_high: Color::from_rgb_u8(98, 104, 128),
            ..Self::catppuccin_mocha()
        }
    }

    /// Catppuccin Latte — the light flavour. Its surfaces run *darker* than the base, which is how a light
    /// theme raises a panel off the page.
    pub fn catppuccin_latte() -> Self {
        Self {
            base: Color::from_rgb_u8(239, 241, 245),
            surface: Color::from_rgb_u8(230, 233, 239),
            overlay: Color::from_rgb_u8(204, 208, 218),
            muted: Color::from_rgb_u8(156, 160, 176),
            subtle: Color::from_rgb_u8(108, 111, 133),
            text: Color::from_rgb_u8(76, 79, 105),
            accent: Color::from_rgb_u8(136, 57, 239),
            blue: Color::from_rgb_u8(30, 102, 245),
            cyan: Color::from_rgb_u8(4, 165, 229),
            teal: Color::from_rgb_u8(23, 146, 153),
            red: Color::from_rgb_u8(210, 15, 57),
            orange: Color::from_rgb_u8(254, 100, 11),
            yellow: Color::from_rgb_u8(223, 142, 29),
            green: Color::from_rgb_u8(64, 160, 43),
            purple: Color::from_rgb_u8(136, 57, 239),
            success: Color::from_rgb_u8(64, 160, 43),
            warning: Color::from_rgb_u8(223, 142, 29),
            error: Color::from_rgb_u8(210, 15, 57),
            info: Color::from_rgb_u8(30, 102, 245),
            highlight_low: Color::from_rgb_u8(230, 233, 239),
            highlight_med: Color::from_rgb_u8(204, 208, 218),
            highlight_high: Color::from_rgb_u8(188, 192, 204),
            ..Self::catppuccin_mocha()
        }
    }

    /// Gruvbox Dark (medium contrast) — a boxier metric set to match the palette's retro character.
    pub fn gruvbox() -> Self {
        Self {
            radius: 4.0,
            spacing: 6.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(40, 40, 40),
            surface: Color::from_rgb_u8(60, 56, 54),
            overlay: Color::from_rgb_u8(80, 73, 69),
            muted: Color::from_rgb_u8(146, 131, 116),
            subtle: Color::from_rgb_u8(168, 153, 132),
            text: Color::from_rgb_u8(235, 219, 178),
            accent: Color::from_rgb_u8(254, 128, 25),
            blue: Color::from_rgb_u8(131, 165, 152),
            cyan: Color::from_rgb_u8(142, 192, 124),
            teal: Color::from_rgb_u8(104, 157, 106),
            red: Color::from_rgb_u8(251, 73, 52),
            orange: Color::from_rgb_u8(254, 128, 25),
            yellow: Color::from_rgb_u8(250, 189, 47),
            green: Color::from_rgb_u8(184, 187, 38),
            purple: Color::from_rgb_u8(211, 134, 155),
            success: Color::from_rgb_u8(184, 187, 38),
            warning: Color::from_rgb_u8(250, 189, 47),
            error: Color::from_rgb_u8(251, 73, 52),
            info: Color::from_rgb_u8(131, 165, 152),
            highlight_low: Color::from_rgb_u8(60, 56, 54),
            highlight_med: Color::from_rgb_u8(80, 73, 69),
            highlight_high: Color::from_rgb_u8(102, 92, 84),
        }
    }

    /// Gruvbox Light — the same earth tones inverted onto cream; its accents darken so they read on paper.
    pub fn gruvbox_light() -> Self {
        Self {
            base: Color::from_rgb_u8(251, 241, 199),
            surface: Color::from_rgb_u8(235, 219, 178),
            overlay: Color::from_rgb_u8(213, 196, 161),
            muted: Color::from_rgb_u8(146, 131, 116),
            subtle: Color::from_rgb_u8(124, 111, 100),
            text: Color::from_rgb_u8(60, 56, 54),
            accent: Color::from_rgb_u8(175, 58, 3),
            blue: Color::from_rgb_u8(7, 102, 120),
            cyan: Color::from_rgb_u8(66, 123, 88),
            teal: Color::from_rgb_u8(66, 123, 88),
            red: Color::from_rgb_u8(157, 0, 6),
            orange: Color::from_rgb_u8(175, 58, 3),
            yellow: Color::from_rgb_u8(181, 118, 20),
            green: Color::from_rgb_u8(121, 116, 14),
            purple: Color::from_rgb_u8(143, 63, 113),
            success: Color::from_rgb_u8(121, 116, 14),
            warning: Color::from_rgb_u8(181, 118, 20),
            error: Color::from_rgb_u8(157, 0, 6),
            info: Color::from_rgb_u8(7, 102, 120),
            highlight_low: Color::from_rgb_u8(235, 219, 178),
            highlight_med: Color::from_rgb_u8(213, 196, 161),
            highlight_high: Color::from_rgb_u8(189, 174, 147),
            ..Self::gruvbox()
        }
    }

    /// Tokyo Night — the "night" variant, the darkest of the family.
    pub fn tokyo_night() -> Self {
        Self {
            radius: 8.0,
            spacing: 6.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(26, 27, 38),
            surface: Color::from_rgb_u8(36, 40, 59),
            overlay: Color::from_rgb_u8(41, 46, 66),
            muted: Color::from_rgb_u8(86, 95, 137),
            subtle: Color::from_rgb_u8(169, 177, 214),
            text: Color::from_rgb_u8(192, 202, 245),
            accent: Color::from_rgb_u8(122, 162, 247),
            blue: Color::from_rgb_u8(122, 162, 247),
            cyan: Color::from_rgb_u8(125, 207, 255),
            teal: Color::from_rgb_u8(26, 188, 156),
            red: Color::from_rgb_u8(247, 118, 142),
            orange: Color::from_rgb_u8(255, 158, 100),
            yellow: Color::from_rgb_u8(224, 175, 104),
            green: Color::from_rgb_u8(158, 206, 106),
            purple: Color::from_rgb_u8(187, 154, 247),
            success: Color::from_rgb_u8(158, 206, 106),
            warning: Color::from_rgb_u8(224, 175, 104),
            error: Color::from_rgb_u8(247, 118, 142),
            info: Color::from_rgb_u8(122, 162, 247),
            highlight_low: Color::from_rgb_u8(31, 35, 53),
            highlight_med: Color::from_rgb_u8(41, 46, 66),
            highlight_high: Color::from_rgb_u8(59, 66, 97),
        }
    }

    /// Everforest (dark, medium contrast) — green-based and deliberately low contrast.
    pub fn everforest() -> Self {
        Self {
            radius: 8.0,
            spacing: 8.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(45, 53, 59),
            surface: Color::from_rgb_u8(52, 63, 68),
            overlay: Color::from_rgb_u8(61, 72, 77),
            muted: Color::from_rgb_u8(133, 146, 137),
            subtle: Color::from_rgb_u8(157, 169, 160),
            text: Color::from_rgb_u8(211, 198, 170),
            accent: Color::from_rgb_u8(167, 192, 128),
            blue: Color::from_rgb_u8(127, 187, 179),
            cyan: Color::from_rgb_u8(131, 192, 146),
            teal: Color::from_rgb_u8(131, 192, 146),
            red: Color::from_rgb_u8(230, 126, 128),
            orange: Color::from_rgb_u8(230, 152, 117),
            yellow: Color::from_rgb_u8(219, 188, 127),
            green: Color::from_rgb_u8(167, 192, 128),
            purple: Color::from_rgb_u8(214, 153, 182),
            success: Color::from_rgb_u8(167, 192, 128),
            warning: Color::from_rgb_u8(219, 188, 127),
            error: Color::from_rgb_u8(230, 126, 128),
            info: Color::from_rgb_u8(127, 187, 179),
            highlight_low: Color::from_rgb_u8(52, 63, 68),
            highlight_med: Color::from_rgb_u8(61, 72, 77),
            highlight_high: Color::from_rgb_u8(71, 82, 88),
        }
    }

    pub fn new() -> Self {
        Self {
            // Match today's defaults so no config changes look; a theme is free to round more / space wider.
            radius: 0.0,
            spacing: 6.0,
            font_size: 14.0,
            icon_size: 24.0,
            icon_stroke: None,
            fonts: FontsConfig::default(),
            base: Color::from_rgb_u8(46, 52, 64),
            surface: Color::from_rgb_u8(59, 66, 82),
            overlay: Color::from_rgb_u8(67, 76, 94),
            muted: Color::from_rgb_u8(76, 86, 106),
            subtle: Color::from_rgb_u8(216, 222, 233),
            text: Color::from_rgb_u8(236, 239, 244),
            accent: Color::from_rgb_u8(136, 192, 208),
            blue: Color::from_rgb_u8(94, 129, 172),
            cyan: Color::from_rgb_u8(136, 192, 208),
            teal: Color::from_rgb_u8(143, 188, 187),
            red: Color::from_rgb_u8(191, 97, 106),
            orange: Color::from_rgb_u8(208, 135, 112),
            yellow: Color::from_rgb_u8(235, 203, 139),
            green: Color::from_rgb_u8(163, 190, 140),
            purple: Color::from_rgb_u8(180, 142, 173),
            success: Color::from_rgb_u8(163, 190, 140),
            warning: Color::from_rgb_u8(235, 203, 139),
            error: Color::from_rgb_u8(191, 97, 106),
            info: Color::from_rgb_u8(94, 129, 172),
            highlight_low: Color::from_rgb_u8(67, 76, 94),
            highlight_med: Color::from_rgb_u8(76, 86, 106),
            highlight_high: Color::from_rgb_u8(94, 105, 128),
        }
    }

    /// Applies the configured accent to the theme's `accent` field, so everything reading `use_theme().accent` follows `[theme] accent` uniformly, not just the modules the bar resolves per-id.
    pub fn with_accent(mut self, name: &str) -> Self {
        self.accent = self.accent_by_name(name);
        self
    }

    /// A text size by semantic role, stepping off [`font_size`](Self::font_size) so a theme scales its whole type ramp from one number.
    pub fn font(&self, role: FontRole) -> f32 {
        let derived = match role {
            FontRole::Caption => self.font_size - 2.0,
            FontRole::Body => self.font_size,
            FontRole::Title => self.font_size + 1.0,
            FontRole::Display => (self.font_size * 2.4).round(),
        };
        self.font_spec(role).size_for(derived)
    }

    fn font_spec(&self, role: FontRole) -> FontSpec {
        match role {
            FontRole::Caption => self.fonts.caption,
            FontRole::Body => self.fonts.body,
            FontRole::Title => self.fonts.title,
            FontRole::Display => self.fonts.display,
        }
    }

    /// A [`TextStyle`] carrying everything `[theme.fonts.<role>]` has to say — size, weight and slant.
    ///
    /// The one way to start a text style, so a per-role override reaches every label instead of only the ones
    /// that remembered to ask. A call site that chains `.with_weight(…)` afterwards still wins, which is what
    /// keeps a deliberately bold heading bold when the body weight is lowered: that is emphasis relative to the
    /// role, not the role itself.
    pub fn text_style(&self, role: FontRole, paint: impl Into<telar::Paint>) -> TextStyle {
        self.text_style_at(role, paint, self.font(role))
    }

    /// [`text_style`](Self::text_style) at a size the caller decides: the role still supplies weight and
    /// slant, but not the size — a clock face scales with the surface it is drawn on, not with the body font.
    pub fn text_style_at(
        &self,
        role: FontRole,
        paint: impl Into<telar::Paint>,
        size: f32,
    ) -> TextStyle {
        let spec = self.font_spec(role);
        let mut style = TextStyle::new(size, paint);
        if let Some(weight) = spec.weight {
            style = style.with_weight(weight.clamp(100, 900));
        }
        if let Some(italic) = spec.italic {
            style = style.with_italic(italic);
        }
        style
    }

    /// Overrides one palette token by name (as used in `[theme.colors]`), for config-defined custom colors; an unknown name is ignored with a warning.
    pub fn with_color(mut self, name: &str, color: Color) -> Self {
        match name {
            "base" => self.base = color,
            "surface" => self.surface = color,
            "overlay" => self.overlay = color,
            "muted" => self.muted = color,
            "subtle" => self.subtle = color,
            "text" => self.text = color,
            "accent" => self.accent = color,
            "blue" => self.blue = color,
            "cyan" => self.cyan = color,
            "teal" => self.teal = color,
            "red" => self.red = color,
            "orange" => self.orange = color,
            "yellow" => self.yellow = color,
            "green" => self.green = color,
            "purple" => self.purple = color,
            "success" => self.success = color,
            "warning" => self.warning = color,
            "error" => self.error = color,
            "info" => self.info = color,
            "highlight_low" => self.highlight_low = color,
            "highlight_med" => self.highlight_med = color,
            "highlight_high" => self.highlight_high = color,
            other => tracing::warn!("unknown theme color token '{other}'"),
        }
        self
    }

    /// One token by name — the read half of [`with_color`](Self::with_color), and what lets a palette be drawn
    /// as swatches, exported, or edited from [`THEME_TOKENS`] rather than as twenty-two hardcoded fields.
    pub fn token(&self, name: &str) -> Color {
        match name {
            "base" => self.base,
            "surface" => self.surface,
            "overlay" => self.overlay,
            "muted" => self.muted,
            "subtle" => self.subtle,
            "text" => self.text,
            "accent" => self.accent,
            "blue" => self.blue,
            "cyan" => self.cyan,
            "teal" => self.teal,
            "red" => self.red,
            "orange" => self.orange,
            "yellow" => self.yellow,
            "green" => self.green,
            "purple" => self.purple,
            "success" => self.success,
            "warning" => self.warning,
            "error" => self.error,
            "info" => self.info,
            "highlight_low" => self.highlight_low,
            "highlight_med" => self.highlight_med,
            "highlight_high" => self.highlight_high,
            _ => self.accent,
        }
    }

    pub fn accent_by_name(&self, name: &str) -> Color {
        match name {
            "blue" => self.blue,
            "cyan" => self.cyan,
            "teal" => self.teal,
            "red" => self.red,
            "orange" => self.orange,
            "yellow" => self.yellow,
            "green" => self.green,
            "purple" => self.purple,
            _ => self.accent,
        }
    }
}

impl Default for NordTheme {
    fn default() -> Self {
        Self::new()
    }
}

impl Theme for NordTheme {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl ThemeTokens for NordTheme {
    /// The metrics as well as the colours. Without these four a catalogue component sizes itself from the
    /// trait's own defaults — 4px radius, 8px spacing, 14px text — which is a different design from the one the
    /// user configured, on the same screen as the bars that follow it.
    fn radius(&self) -> f32 {
        self.radius
    }
    fn spacing(&self) -> f32 {
        self.spacing
    }
    fn font_size(&self) -> f32 {
        self.font_size
    }
    fn icon_size(&self) -> f32 {
        self.icon_size
    }

    fn primary(&self) -> Color {
        self.accent
    }
    /// Whichever of the theme's two foregrounds reads on the accent, rather than always the dark one: a light
    /// accent takes `base`, a dark one takes `text`. A component that fills with `primary` and writes with
    /// `on_primary` is otherwise unreadable on half the palettes this shell ships.
    fn on_primary(&self) -> Color {
        self.accent.most_readable(&[self.text, self.base])
    }
    fn muted(&self) -> Color {
        self.muted
    }
    fn scrollbar(&self) -> Color {
        self.muted
    }
    fn ink(&self) -> Color {
        self.text
    }
    /// What a floating catalogue panel — a dropdown, a dialog, a drawer sheet — sits on. Unanswered it is the
    /// trait's opaque white, which on this shell is a white slab in the middle of a dark panel.
    fn surface(&self) -> Color {
        self.surface
    }
    /// The quieter tone under a chip or a tag: the raised overlay, not the panel it is raised *on*.
    fn surface_alt(&self) -> Color {
        self.overlay
    }
    fn border(&self) -> Color {
        self.muted
    }
    fn success(&self) -> Color {
        self.success
    }
    fn warning(&self) -> Color {
        self.warning
    }
    fn error(&self) -> Color {
        self.error
    }
    fn info(&self) -> Color {
        self.info
    }
    fn highlight_low(&self) -> Color {
        self.highlight_low
    }
    fn highlight_med(&self) -> Color {
        self.highlight_med
    }
    fn highlight_high(&self) -> Color {
        self.highlight_high
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A catalogue component asks the theme its questions through `ThemeTokens`, not through `NordTheme`, so a
    /// metric this shell configures has to be answerable there — otherwise a `text_field` dropped into the
    /// settings float rounds and spaces itself to telar's defaults while every bar on screen follows the theme.
    #[test]
    fn the_token_contract_answers_with_this_theme_s_own_metrics() {
        let theme = NordTheme::rose_pine();
        assert_eq!(ThemeTokens::radius(&theme), theme.radius);
        assert_eq!(ThemeTokens::spacing(&theme), theme.spacing);
        assert_eq!(ThemeTokens::font_size(&theme), theme.font_size);
        assert_eq!(ThemeTokens::icon_size(&theme), theme.icon_size);
        assert_ne!(
            ThemeTokens::radius(&theme),
            4.0,
            "rose-pine rounds to 10px, so a default answer here would be silently wrong"
        );
    }

    /// Filling with `primary` and writing with `on_primary` has to stay readable on every palette, so the ink
    /// is the more contrasting of the theme's two foregrounds rather than always its `base`.
    #[test]
    fn on_primary_is_whichever_foreground_reads_on_the_accent() {
        for name in ["nord", "rose-pine-dawn", "catppuccin-latte", "gruvbox"] {
            let theme = NordTheme::named(name);
            let ink = ThemeTokens::on_primary(&theme);
            assert!(
                ink == theme.text || ink == theme.base,
                "{name}: the ink is one of the theme's own foregrounds"
            );
            assert_eq!(
                ink,
                theme.accent.most_readable(&[theme.text, theme.base]),
                "{name}: and it is the one that reads on the accent"
            );
        }
    }

    /// The two halves of the token table have to name the same tokens. A swatch drawn from a name `token`
    /// does not know reads the accent, which is a preview that quietly shows the wrong colour rather than a
    /// blank — so the guard is that every name `with_color` writes, `token` reads back.
    #[test]
    fn every_token_reads_back_what_with_color_wrote() {
        let marker = Color::from_hex("#123456").expect("a hex colour");
        for name in THEME_TOKENS {
            let written = NordTheme::new().with_color(name, marker);
            assert_eq!(written.token(name), marker, "token '{name}'");
        }
        assert_eq!(
            NordTheme::new().token("no-such-token"),
            NordTheme::new().accent,
            "an unknown name falls back rather than panicking a settings page"
        );
        assert_eq!(hex(marker), "#123456", "the spelling every export uses");
    }

    #[test]
    fn named_selects_a_palette_and_falls_back_to_nord() {
        assert_eq!(NordTheme::named("nord").base, NordTheme::nord().base);
        assert_eq!(
            NordTheme::named("rose-pine").base,
            NordTheme::rose_pine().base
        );
        // Rosé Pine is a different palette with its own metrics.
        assert_ne!(NordTheme::rose_pine().base, NordTheme::nord().base);
        assert_eq!(NordTheme::rose_pine().radius, 10.0);
        // Moon and Dawn are their own palettes, distinct from the main Rosé Pine, sharing its metrics.
        assert_eq!(
            NordTheme::named("rose-pine-moon").base,
            NordTheme::rose_pine_moon().base
        );
        assert_eq!(
            NordTheme::named("rose-pine-dawn").base,
            NordTheme::rose_pine_dawn().base
        );
        assert_ne!(
            NordTheme::rose_pine_moon().base,
            NordTheme::rose_pine().base
        );
        assert_ne!(
            NordTheme::rose_pine_dawn().base,
            NordTheme::rose_pine().base
        );
        assert_eq!(NordTheme::rose_pine_moon().radius, 10.0);
        assert_eq!(NordTheme::rose_pine_dawn().radius, 10.0);
        // Dawn is the light variant: a pale base under dark text (the reverse of the dark palettes).
        assert_ne!(
            NordTheme::rose_pine_dawn().base,
            NordTheme::rose_pine_dawn().text
        );
        // "custom" and unknown names both fall back to nord (custom for config to override).
        assert_eq!(NordTheme::named("custom").base, NordTheme::nord().base);
        assert_eq!(
            NordTheme::named("does-not-exist").base,
            NordTheme::nord().base
        );
        assert_eq!(NordTheme::meta("rose-pine").name, "Rosé Pine");
        assert_eq!(NordTheme::meta("rose-pine-moon").name, "Rosé Pine Moon");
        assert_eq!(NordTheme::meta("rose-pine-dawn").name, "Rosé Pine Dawn");
        assert_eq!(NordTheme::meta("whatever").name, "Nord");
    }

    #[test]
    fn separators_and_case_do_not_change_which_theme_a_name_selects() {
        for spelling in [
            "rose-pine",
            "rose_pine",
            "rosepine",
            "Rose Pine",
            "ROSE-PINE",
        ] {
            assert_eq!(
                NordTheme::named(spelling).base,
                NordTheme::rose_pine().base,
                "'{spelling}' selects Rosé Pine"
            );
        }
        assert_eq!(
            NordTheme::named("catppuccin_mocha").base,
            NordTheme::catppuccin_mocha().base
        );
        assert_eq!(
            NordTheme::named("catppuccin").base,
            NordTheme::catppuccin_mocha().base,
            "the bare family name lands on its flagship flavour"
        );
        assert_eq!(
            NordTheme::named("gruvbox-dark").base,
            NordTheme::gruvbox().base
        );
        assert_eq!(NordTheme::meta("TokyoNight").name, "Tokyo Night");
    }

    #[test]
    fn every_built_in_theme_resolves_and_is_readable() {
        let mut palettes = Vec::new();
        for name in BUILT_IN_THEMES {
            let theme = NordTheme::named(name);
            assert_ne!(
                theme.base, theme.text,
                "'{name}' must not paint text in its own background"
            );
            assert_ne!(theme.base, theme.surface, "'{name}' needs a raised surface");
            assert_ne!(
                theme.muted, theme.text,
                "'{name}' needs a de-emphasised token distinct from body text"
            );
            for (label, token) in [
                ("highlight_low", theme.highlight_low),
                ("highlight_med", theme.highlight_med),
                ("highlight_high", theme.highlight_high),
            ] {
                assert_ne!(token, theme.base, "'{name}' {label} must lift off the base");
            }
            assert!(
                theme.radius >= 0.0 && theme.spacing > 0.0,
                "'{name}' has sane metrics"
            );
            // Nord is the metadata fallback, so only the others prove they registered their own.
            if *name != "nord" {
                assert_ne!(
                    NordTheme::meta(name).name,
                    "Nord",
                    "'{name}' must carry its own metadata"
                );
            }
            palettes.push((name, theme.base));
        }
        // A copy-paste that left two flavours identical would still pass every check above.
        for (i, (name, base)) in palettes.iter().enumerate() {
            for (other, other_base) in &palettes[i + 1..] {
                assert_ne!(
                    base, other_base,
                    "'{name}' and '{other}' are the same palette"
                );
            }
        }
    }

    #[test]
    fn the_shipped_default_accent_resolves_through_every_theme() {
        // `[theme] accent` defaults to "cyan", so a theme whose `cyan` token is its background would ship a
        // shell with an invisible accent.
        for name in BUILT_IN_THEMES {
            let theme = NordTheme::named(name).with_accent("cyan");
            assert_ne!(
                theme.accent, theme.base,
                "'{name}' accent vanishes into the bar"
            );
        }
    }

    #[test]
    fn light_themes_are_light_and_dark_ones_are_dark() {
        let luminance = |c: Color| {
            let [r, g, b, _] = c.to_rgba8();
            0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
        };
        for light in ["rose-pine-dawn", "catppuccin-latte", "gruvbox-light"] {
            let theme = NordTheme::named(light);
            assert!(
                luminance(theme.base) > luminance(theme.text),
                "'{light}' is the light variant: dark text on a pale base"
            );
        }
        for dark in [
            "nord",
            "catppuccin-mocha",
            "gruvbox",
            "tokyo-night",
            "everforest",
        ] {
            let theme = NordTheme::named(dark);
            assert!(
                luminance(theme.base) < luminance(theme.text),
                "'{dark}' is a dark palette"
            );
        }
    }

    #[test]
    fn font_scale_steps_off_the_base_size() {
        let t = NordTheme::new();
        assert_eq!(t.font(FontRole::Body), t.font_size);
        assert_eq!(t.font(FontRole::Caption), t.font_size - 2.0);
        assert_eq!(t.font(FontRole::Title), t.font_size + 1.0);
        assert_eq!(t.font(FontRole::Display), (t.font_size * 2.4).round());
        // Scaling the base scales the whole ramp.
        let big = NordTheme {
            font_size: 20.0,
            ..NordTheme::new()
        };
        assert_eq!(big.font(FontRole::Body), 20.0);
        assert_eq!(big.font(FontRole::Display), 48.0);
    }

    #[test]
    fn semantic_tokens_map_to_nord_palette() {
        let t = NordTheme::new();
        assert_eq!(t.success, t.green);
        assert_eq!(t.warning, t.yellow);
        assert_eq!(t.error, t.red);
        assert_eq!(t.info, t.blue);
        // The catalogue reads them through the ThemeTokens contract.
        assert_eq!(ThemeTokens::success(&t), t.green);
        assert_eq!(ThemeTokens::error(&t), t.red);
        assert_eq!(ThemeTokens::info(&t), t.blue);
        assert_ne!(t.highlight_low, t.highlight_med);
        assert_ne!(t.highlight_med, t.highlight_high);
    }
}
