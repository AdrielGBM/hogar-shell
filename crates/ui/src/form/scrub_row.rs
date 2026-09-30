use std::rc::Rc;

use telar::{AlignItems, Children, Container, LayoutStyle, Reactive, RwSignal, box_item, signal};

use crate::descriptor::Built;
use crate::form::labelled::labelled;
use crate::form::recorder::record_field;

/// A labelled number bound to `value`, changed by dragging it (Shift ×10, Alt ×0.1), by the arrow keys once it has focus, or by typing it after a double-click or Enter — the one convention every number in the shell is scrubbed by.
#[derive(telar::Props)]
pub struct ScrubRowProps {
    #[props(into, default)]
    pub label: Reactive<String>,
    #[props(default = signal(0.0))]
    pub value: RwSignal<f32>,
    #[props(default = f32::NEG_INFINITY)]
    pub min: f32,
    #[props(default = f32::INFINITY)]
    pub max: f32,
    #[props(default = 1.0)]
    pub step: f32,
    /// Whole numbers only, shown and typed without a fraction.
    #[props(default)]
    pub integer: bool,
    /// A − and + beside the field too, for a value with few enough steps to count through.
    #[props(default)]
    pub stepper: bool,
}

pub fn scrub_row(props: ScrubRowProps, _children: Children) -> Built {
    let ScrubRowProps {
        label,
        value,
        min,
        max,
        step,
        integer,
        stepper,
    } = props;
    record_field(&value);
    let mut field = telar::ScrubFieldProps::props()
        .value(value)
        .min(min)
        .max(max)
        .step(step);
    if integer {
        field = field
            .format(Rc::new(|value: f32| format!("{}", value.round() as i64)))
            .parse(Rc::new(|typed: &str| {
                typed
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .map(f32::round)
            }));
    }
    let mut controls = vec![telar::scrub_field(field.build(), Children::default())?];
    if stepper {
        controls.push(telar::stepper(
            telar::StepperProps::props()
                .value(value)
                .min(min.max(f32::MIN))
                .max(max.min(f32::MAX))
                .step(step)
                .build(),
            Children::default(),
        )?);
    }
    let row = Container::new(
        LayoutStyle::new()
            .flex_row()
            .align_items(AlignItems::CENTER)
            .gap(crate::scale::space::sm()),
        controls,
    )?;
    labelled(label, box_item(row))
}
