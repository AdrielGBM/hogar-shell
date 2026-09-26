//! The `[toml]` sections `Config` is made of, grouped the way the settings application groups their forms.
//!
//! Split by area rather than one file per section: `[bars]`, `[panels]` and `[popouts]` are read together and changed together, and forty files of thirty lines would hide that.

pub mod appearance;
pub mod audio;
pub mod bars;
pub mod lock;
pub mod notifications;
pub mod system;
pub mod wallpaper;

pub use appearance::*;
pub use audio::*;
pub use bars::*;
pub use lock::*;
pub use notifications::*;
pub use system::*;
pub use wallpaper::*;

pub(crate) use bars::{SETTINGS_CHROME, application_panel};
pub use system::glob_matches;
