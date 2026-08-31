//! A 0..1 bar as an element, so a `[view]` can place one by name.
//!
//! [`crate::widget::meter`] is the Rust helper that draws it and stays one — a dashboard builds its bars in
//! Rust and a card places one in markup, and both want the same drawing.

use telar::{Children, Color, LayoutError, LayoutItem};

use util::reactive::Live;

use crate::widget;

#[derive(telar::Props)]
pub struct MeterProps {
    pub fraction: Live<f32>,
    pub tint: Live<Color>,
    /// What shows through where the fill has not reached.
    pub track: Color,
    pub height: f32,
}

pub fn meter(props: MeterProps, _children: Children) -> Result<Box<dyn LayoutItem>, LayoutError> {
    widget::meter(props.fraction, props.tint, props.track, props.height)
}
