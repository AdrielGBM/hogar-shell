//! The layout model: where everything the shell draws lives.
//!
//! One layout describes every output as five layers — four wlr layer-shell layers and the session lock — each holding areas, each holding groups of placed module instances. A layout file is written at four levels (a built-in default, an `extends` parent, an output rule and a workspace rule) and [`resolve`] flattens them, by id rather than by array position, into the arrangement one output shows right now.
//!
//! Nothing here draws, opens a surface or reads a service. Resolution is a pure function of the layout, the output and the active workspace, so the same input always gives the same arrangement and a test can ask for one without a compositor.

telar::rsx_modules!();

pub use crate::model::*;
pub use crate::ops::{LayoutOp, OpError, Site, Spot};
pub use crate::resolve::{
    ActiveWorkspace, Paint, Resolved, ResolvedArea, ResolvedAreaKind, ResolvedGroup,
    ResolvedInstance, ResolvedLayer, resolve,
};
pub use crate::store::{BUILT_IN, LayoutStore, SETTLE, StoreError, Transaction};
pub use crate::validate::{Catalogue, check_unknown_keys, validate, validate_resolved};
