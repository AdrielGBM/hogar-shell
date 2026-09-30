use telar::{Children, LayoutItem, Reactive, Slots};

use crate::descriptor::Built;
use crate::form::field_row::{FieldRowProps, field_row};

/// `control` in a form row under `label`, for a control built before its row.
pub fn labelled(label: Reactive<String>, control: Box<dyn LayoutItem>) -> Built {
    let mut slots = Slots::new();
    slots.push(None, control);
    field_row(
        FieldRowProps::props().label(label).build(),
        Children::from(slots),
    )
}
