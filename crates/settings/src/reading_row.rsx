[logic]
use ::config::theme::FontRole;

/// A label and a value the user cannot change — what a page of readings is made of.
pub struct Props {
    pub label: Box<dyn Fn() -> String> = Box::new(String::new),
    pub value: Box<dyn Fn() -> String> = Box::new(String::new),
}

let label = props.label;
let value = props.value;

[view]
field_row label(move || label())
    text "{value()}" grow:1 color:theme.text font_size:theme.font(FontRole::Body)
