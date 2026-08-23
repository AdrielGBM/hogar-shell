//! The shell's binding of Telar's list-navigation primitive.
//!
//! [`KeyNav`] and the rest are Telar's: nothing about "which row of a list is selected" is this shell's, and a
//! user who learns that `j` moves down in the launcher is owed the same answer in every telar app. What stays
//! here is the one thing that is ours — reading the shell's `[keynav]` config.

pub use telar::{
    KeyNav, KeyNavMove as Move, key_nav_apply as apply, key_nav_apply_grid as apply_grid,
};

use config::KeyNavConfig;

/// A vertical list reading the shell's configured bindings.
pub fn from_config(config: &KeyNavConfig) -> KeyNav {
    KeyNav {
        vim: config.vim,
        horizontal: false,
        grid: false,
    }
}
