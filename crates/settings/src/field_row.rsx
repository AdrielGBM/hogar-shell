[logic]
use ::config::theme::FontRole;

/// One form row: its label in the left column, whatever control the caller puts in the slot on the right.
///
/// The label column is a fixed width so every row on a page lines up down the same edge — a form whose controls start at a different x on each row reads as a list of unrelated things.
pub struct Props {
    pub label: Box<dyn Fn() -> String> = Box::new(String::new),
}

let label = props.label;

[view]
row align:center gap(::ui::scale::space::md()) width:100%
    text "{label()}" width:120 color:subtle size:theme.font(FontRole::Body)
    children
