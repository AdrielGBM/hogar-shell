[logic]
use crate::field_row::{field_row, FieldRowProps};
use ::config::theme::FontRole;

/// A label and a value the user cannot change — what a page of readings is made of.
pub struct Props {
    #[props(into)]
    pub label: Reactive<String> = Reactive::of(String::new),
    #[props(into)]
    pub value: Reactive<String> = Reactive::of(String::new),
}

let label = props.label;
let value = props.value;

[view]
field_row label:(Reactive::of(move || label.get()))
    text "{value.get()}" grow:1 color:$theme.text font_size:$theme.font(FontRole::Body)
