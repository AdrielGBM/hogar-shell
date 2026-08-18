//! State that outlives the tree that reads it.
//!
//! A surface is rebuilt when the config changes: the widget tree is dropped and built again on the same
//! surface, so everything that lived *in* the tree goes with it — the search a user had typed, which page of
//! the settings they were on, how far a transition had got. Both answers are Telar's, since nothing about
//! either is this shell's: [`kept`] hands back a value created once, and [`set_context`]/[`context`] carry
//! what a surface's content wants everything under it to be able to read.
//!
//! Keys are namespaced by whoever owns them (`"launcher.query"`, `"settings.page"`) because a *bar* surface is
//! shared by every module on it — two modules reaching for `"query"` would be reaching for the same value.

pub use telar::{context, kept, set_context};
