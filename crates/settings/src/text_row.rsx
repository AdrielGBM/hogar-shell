[logic]
use crate::form::record_field;
use ::config::theme::FontRole;

/// A labelled text field, bound to the signal a section seeds from `config.toml` and writes back on save.
///
/// `record_field` runs here, before the row is built, which is the half of the form contract this component
/// carries: a form's fields must be registered before its Save button drains them.
pub struct Props {
    pub label: Box<dyn Fn() -> String> = Box::new(String::new),
    pub value: RwSignal<String> = signal(String::new()),
    pub placeholder: Box<dyn Fn() -> String> = Box::new(String::new),
}

let value = props.value;
record_field(&value);
let placeholder = props.placeholder;
let label = props.label;
let rad = ::ui::scale::corner::md();

[view]
field_row label(move || label())
    box grow:1 pad_x(::ui::scale::space::md()) pad_y(::ui::scale::space::sm()) fill:base radius:rad
        input value:$value placeholder:placeholder() color:text size:theme.font(FontRole::Body)
