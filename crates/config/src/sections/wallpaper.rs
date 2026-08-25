//! `[background]` and `[wallpaper]`: the picture behind the desktop and the library it is picked from.
//!
//! What is *drawn over* it is `[widgets]`, on a surface of its own — see [`crate::sections::widgets`].
//!
//! One type per `[toml]` table, each with the defaults the shell falls back to. The doc comment on a field is what `hogar-shell config schema` prints for it, so it is written for a user reading the reference.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How one wallpaper gives way to the next.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperTransition {
    /// The new image simply replaces the old one.
    None,
    /// Cross-fade.
    #[default]
    Fade,
    /// The new image sweeps across from one side.
    Wipe,
}

impl WallpaperTransition {
    pub const ALL: [WallpaperTransition; 3] = [
        WallpaperTransition::None,
        WallpaperTransition::Fade,
        WallpaperTransition::Wipe,
    ];

    pub fn id(self) -> &'static str {
        match self {
            WallpaperTransition::None => "none",
            WallpaperTransition::Fade => "fade",
            WallpaperTransition::Wipe => "wipe",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "none" => Some(WallpaperTransition::None),
            "fade" => Some(WallpaperTransition::Fade),
            "wipe" => Some(WallpaperTransition::Wipe),
            _ => None,
        }
    }
}

/// Full-screen wallpaper behind everything, one surface per monitor. Off by default so the compositor's own background shows through; setting an `image` — or `enabled = true` for a plain themed background — turns it on. `[background.monitors]` maps output names to per-monitor images, each falling back to the global `image`, and `hogar-shell wallpaper set` overrides both at runtime. Paths may use `~`.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct BackgroundConfig {
    pub enabled: bool,
    pub image: Option<PathBuf>,
    pub monitors: HashMap<String, PathBuf>,
    /// How a change from one wallpaper to the next is drawn: `fade` (the default), `wipe` or `none`.
    pub transition: WallpaperTransition,
    /// How long that transition runs, before `[animation] duration_scale`. Ignored while `[animation] enabled` is off, which makes every change instant.
    pub transition_ms: u64,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            image: None,
            monitors: HashMap::new(),
            transition: WallpaperTransition::default(),
            transition_ms: 600,
        }
    }
}

impl BackgroundConfig {
    /// Whether hogar-shell paints a background surface at all; opt-in so it never clobbers the compositor's wallpaper unless asked (an image or a per-monitor entry implies it).
    pub fn is_enabled(&self) -> bool {
        self.enabled || self.image.is_some() || !self.monitors.is_empty()
    }

    /// The image `[background]` alone would paint on `output`: its per-monitor entry, else the global `image`. The runtime override lives in the wallpaper service, so read `wallpaper::current_image` rather than this at a call site that draws.
    pub fn image_for(&self, output: Option<&str>) -> Option<&PathBuf> {
        output
            .and_then(|name| self.monitors.get(name))
            .or(self.image.as_ref())
    }
}

/// The wallpaper library: which folder is browsed and how (`[wallpaper]`). The folder itself is `[paths] wallpapers`, so the two settings that name a directory stay in one place.
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(default)]
pub struct WallpaperConfig {
    /// Whether the library is scanned at all. Off means `[background] image` is the only wallpaper the shell knows, which is what it did before the library existed.
    pub enabled: bool,
    /// Descend into sub-folders. On, because a wallpaper collection is almost always filed by theme or by artist.
    pub recursive: bool,
    /// How many images the library holds at most, so pointing it at a picture archive cannot cost a scan of the whole disk.
    pub max_entries: u32,
    /// The edge length of a cached thumbnail, in px.
    pub thumbnail_size: u32,
    /// The file extensions counted as wallpapers, lowercase and without the dot.
    pub extensions: Vec<String>,
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            recursive: true,
            max_entries: 2000,
            thumbnail_size: 320,
            extensions: ["png", "jpg", "jpeg", "webp"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }
}

impl WallpaperConfig {
    /// Whether `path` names a file the library should list.
    pub fn accepts(&self, path: &Path) -> bool {
        let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        let extension = extension.to_ascii_lowercase();
        self.extensions.iter().any(|allowed| {
            allowed
                .trim()
                .trim_start_matches('.')
                .eq_ignore_ascii_case(&extension)
        })
    }
}
