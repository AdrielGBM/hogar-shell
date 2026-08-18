[logic]
use crate::form::{LANGUAGES, record_field};
use ::config::theme::FontRole;

/// The UI language, as a cycle rather than a picker: there are two of them, and each press both stores the
/// code and broadcasts it, so every surface on screen switches with the form.
pub struct Props {
    pub label: Box<dyn Fn() -> String> = Box::new(String::new),
    pub value: RwSignal<String> = signal(String::new()),
}

let value = props.value;
record_field(&value);
let label = props.label;

fn language_name(code: &str) -> String {
    match code {
        "en" => "English".to_string(),
        "es" => "Español".to_string(),
        other => other.to_uppercase(),
    }
}

let cycle = {
    let value = value.clone();
    Box::new(move || {
        let current = value.peek();
        let index = LANGUAGES.iter().position(|o| *o == current).unwrap_or(0);
        let next = LANGUAGES[(index + 1) % LANGUAGES.len()].to_string();
        value.set(next.clone());
        services::locale::set(next);
    })
};
let rad = ::ui::scale::corner::md();

[view]
field_row label(move || label())
    box grow:1 pad_x(::ui::scale::space::md()) pad_y(::ui::scale::space::sm()) fill:base radius:rad hover_style(fill:overlay) on_press(|| cycle())
        text "{language_name(&$value)}" color:text size:theme.font(FontRole::Body)
