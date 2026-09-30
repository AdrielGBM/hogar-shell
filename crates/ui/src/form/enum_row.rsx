[logic]
use crate::form::field_row::{field_row, FieldRowProps};
use crate::form::recorder::{option_index, pick_option};

/// A labelled dropdown over a fixed set of options, bound to the one `value` holds, spelled as the file writes it.
pub struct Props {
    #[props(into)]
    pub label: Reactive<String> = Reactive::of(String::new),
    pub value: RwSignal<String> = signal(String::new()),
    #[props(into)]
    pub options: std::rc::Rc<[&'static str]> = std::rc::Rc::from(Vec::new()),
}

let options = props.options;
let value = props.value;
let picked = option_index(value.clone(), options.clone());
let label = props.label;

[view]
field_row label:(Reactive::of(move || label.get()))
    select selected:$picked color:$theme.accent stretch:true on_select:(move |at| pick_option(&value, &options, at))
        for opt in options.iter()
            item label:opt.to_string()
