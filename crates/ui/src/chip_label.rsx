[logic]
use ::config::theme::{FontRole, NordTheme};

// Inline defaults rather than `#[derive(Default)]`: a boxed closure has no `Default`, and this sugar synthesizes the impl the catalogue writes by hand.
pub struct Props {
    pub text: Box<dyn Fn() -> String> = Box::new(String::new),
    pub muted: bool = false,
}

fn tint(muted: bool) -> telar::Color {
    let theme = use_theme::<NordTheme>();
    if muted { theme.muted } else { theme.text }
}

let muted = props.muted;
let label = props.text;

[view]
text "{label()}" font_size:theme.font(FontRole::Body) color:tint(muted)

[preview "Chip label"]
chip_label text:"12:04"
