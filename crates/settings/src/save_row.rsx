[logic]
use crate::form::{SaveButtonProps, save_button};
use std::rc::Rc;


/// A form's action button. The one escape in this vocabulary, and it earns it: `save_button` is where the fields recorded above it are drained and wired to the write — plumbing with no shape in the view.
pub struct Props {
    #[props(into)]
    pub label: Reactive<String> = Reactive::of(String::new),
    pub on_press: Rc<dyn Fn()> = Rc::new(|| {}),
}

let label = props.label;
let on_press = props.on_press;

[view]
save_button label:label on_press:on_press
