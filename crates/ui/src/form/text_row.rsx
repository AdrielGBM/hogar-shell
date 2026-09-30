[logic]
use crate::form::field_row::{field_row, FieldRowProps};
use crate::form::recorder::record_field;
use ::config::theme::FontRole;

/// A labelled text field bound to `value`.
pub struct Props {
    #[props(into)]
    pub label: Reactive<String> = Reactive::of(String::new),
    pub value: RwSignal<String> = signal(String::new()),
    #[props(into)]
    pub placeholder: Reactive<String> = Reactive::of(String::new),
}

let value = props.value;
record_field(&value);
let placeholder = props.placeholder;
let label = props.label;
let rad = crate::scale::corner::md();

[view]
field_row label:(Reactive::of(move || label.get()))
    box grow:1 pad_x:(crate::scale::space::md()) pad_y:(crate::scale::space::sm()) fill:$theme.base radius:rad
        input value:$value placeholder:placeholder.get() color:$theme.text font_size:$theme.font(FontRole::Body)
