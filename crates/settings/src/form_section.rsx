[logic]
use ::config::theme::FontRole;

/// A form: its heading, then whatever fields the caller lists, then its Save button.
pub struct Props {
    #[props(into)]
    pub title: Reactive<String> = Reactive::of(String::new),
}

let title = props.title;

[view]
col gap:(::ui::scale::space::md()) width:100%
    text "{title.get()}" color:$theme.text font_size:$theme.font(FontRole::Body) font_weight:700
    children
