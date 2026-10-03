//! The layout model: where everything the shell draws lives.
//!
//! One layout describes every output as five layers — four wlr layer-shell layers and the session lock — each holding areas, each holding groups of placed module instances. A layout file is written at four levels (a built-in default, an `extends` parent, an output rule and a workspace rule) and [`resolve`] flattens them, by id rather than by array position, into the arrangement one output shows right now.
//!
//! Nothing here draws, opens a surface or reads a service. Resolution is a pure function of the layout, the output and the active workspace, so the same input always gives the same arrangement and a test can ask for one without a compositor.

telar::rsx_modules!();

pub use crate::built_in::{bar as default_bar, layout as built_in};
pub use crate::model::*;
pub use crate::ops::{LayoutOp, Listed, OpError, PromptEdit, Site, Spot};
pub use crate::placed::Placed;
pub use crate::resolve::{
    ActiveWorkspace, Origin, Paint, Resolved, ResolvedArea, ResolvedAreaKind, ResolvedExpr,
    ResolvedGroup, ResolvedInstance, ResolvedLayer, lays_over, resolve, sources,
};
pub use crate::routing::{RoutedCard, route_card};
pub use crate::store::{
    BUILT_IN, LayoutStore, SETTLE, StoreError, Transaction, running, set_running,
};
pub use crate::take_back::{Held, TakeBack, Taken, taking_back};
pub use crate::validate::{
    Catalogue, INDEX, ITEM, Locals, Mistake, binding_errors, binding_errors_with,
    check_unknown_keys, child_locals, locate_expressions, locate_unsets, repeat_errors,
    repeat_item, repeat_locals, validate, validate_lock, validate_resolved, validate_unsets,
    visible_errors,
};
