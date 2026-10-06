//! The layout model: where everything the shell draws lives.
//!
//! One layout describes every output as five layers — four wlr layer-shell layers and the session lock — each holding areas, each holding groups of placed module instances. A layout file is written at four levels (a built-in default, an `extends` parent, an output rule and a workspace rule) and [`resolve`] flattens them, by id rather than by array position, into the arrangement one output shows right now. A group can instead draw a komponent, a saved group in `components/<name>.toml` with parameters a use sets ([`components`]).
//!
//! Nothing here draws, opens a surface or reads a service. Resolution is a pure function of the layout, the [`Library`] it is resolved against, the output and the active workspace, so the same input always gives the same arrangement and a test can ask for one without a compositor.

telar::rsx_modules!();

pub use crate::built_in::{bar as default_bar, layout as built_in};
pub use crate::components::{
    Candidate, DetachError, UseError, check_use, komponents_of, used_with,
};
pub use crate::container::Placement;
pub use crate::library::{Library, is_komponent_name, komponent_path};
pub use crate::model::*;
pub use crate::ops::{LayoutOp, Listed, OpError, PromptEdit, Site, Spot};
pub use crate::placed::Placed;
pub use crate::resolve::{
    ActiveWorkspace, EXTENDS_DEPTH, Holder, KomponentUse, Level, Origin, Paint, Resolved,
    ResolvedArea, ResolvedAreaKind, ResolvedExpr, ResolvedGroup, ResolvedInstance, ResolvedLayer,
    ResolvedParameter, held_sources, layout_path, lays_over, resolve, sources,
};
pub use crate::routing::{RoutedCard, route_card};
pub use crate::store::{
    BUILT_IN, History, LayoutStore, SETTLE, StoreError, Transaction, Written, components_beside,
    read_komponent, running, set_running,
};
pub use crate::take_back::{Held, TakeBack, Taken, taking_back};
pub use crate::trust::{
    Carried, Item, ItemKind, Rule, Trust, Verdict, carried, grants_trust, held, komponent_items,
    layout_items, library_items, set_of,
};
pub use crate::validate::{
    Catalogue, INDEX, ITEM, Locals, Mistake, binding_errors, binding_errors_with,
    check_unknown_keys, child_locals, locate_expressions, locate_komponent_expressions,
    locate_unsets, parameter_errors, repeat_errors, repeat_item, repeat_locals, validate,
    validate_komponent, validate_komponents, validate_komponents_lock, validate_lock,
    validate_resolved, validate_unsets, visible_errors,
};
