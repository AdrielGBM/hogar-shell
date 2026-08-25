[logic]
use crate::icon::icon_state;
use ::config::theme::NordTheme;

pub struct Props {
    pub name: Box<dyn Fn() -> String> = Box::new(String::new),
    pub tint: Box<dyn Fn() -> Color> = Box::new(|| Color::WHITE),
    pub size: f32 = 16.0,
}

let size = props.size;
let stroke = use_theme::<NordTheme>().icon_stroke;
let name = props.name;
let tint_fn = props.tint;

// Both props go through a memo so the arms below can read them as `$signal`s: each arm is its own closure, and a signal read is what the view's clone prelude knows how to hand to every one of them.
let state = memo(move || icon_state(&name()));
let tint = memo(move || tint_fn());

// Inset so a missing glyph reads as a gap in the row rather than a filled chip, and keeps the module's footprint identical to a loaded one so nothing shifts when it settles.
let inset = (size * 0.25).max(1.0);
let side = size - inset * 2.0;

[view]
match $state as s key s.as_ready().map(|svg| svg.id())
    AssetState::Ready(svg)
        svg src:svg tint:$tint stroke:stroke width:size height:size
    AssetState::Failed
        box width:side height:side margin_start:inset margin_end:inset fill:$tint radius:side
    _
        spinner color:$tint size:size
