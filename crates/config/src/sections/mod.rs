//! The `[toml]` sections `Config` is made of, grouped the way the settings application groups their forms.
//!
//! Split by area rather than one file per section: `[panels]` and `[popouts]` are read together and changed together, and forty files of thirty lines would hide that.

pub mod appearance;
pub mod audio;
pub mod lock;
pub mod notifications;
pub mod panels;
pub mod system;
pub mod wallpaper;

pub use appearance::*;
pub use audio::*;
pub use lock::*;
pub use notifications::*;
pub use panels::*;
pub use system::*;
pub use wallpaper::*;

pub(crate) use panels::{SETTINGS_CHROME, application_panel};
pub use system::glob_matches;
