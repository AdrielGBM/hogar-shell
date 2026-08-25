[logic]
use crate::form::save_button;

/// A form's action button. The one escape in this vocabulary, and it earns it: `save_button` is where the fields recorded above it are drained and wired to the write — plumbing with no shape in the view.
pub struct Props {
    pub label: Box<dyn Fn() -> String> = Box::new(String::new),
    pub on_press: Box<dyn Fn()> = Box::new(|| {}),
}

let label = props.label;
let on_press = props.on_press;

[view]
build "save_button(move || label(), move || on_press())?"
