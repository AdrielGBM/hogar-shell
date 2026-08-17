[logic]
use crate::form::{option_index, pick_option};

/// A labelled picker over a fixed set of options, bound to the `String` a section writes to `config.toml`.
pub struct Props {
    pub label: Box<dyn Fn() -> String> = Box::new(String::new),
    pub value: RwSignal<String> = signal(String::new()),
    pub options: &'static [&'static str] = &[],
}

let options = props.options;
let value = props.value;
let picked = option_index(value.clone(), options);
let label = props.label;

[view]
field_row label(move || label())
    select selected:$picked color:theme.accent stretch:true on_select(|at| pick_option(&value, options, at))
        for opt in options
            item label:opt.to_string()
