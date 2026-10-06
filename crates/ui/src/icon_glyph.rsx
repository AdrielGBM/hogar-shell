[logic]
use crate::icon::icon_state;
use ::config::theme::NordTheme;

pub struct Props {
    #[props(into)]
    pub name: Reactive<String> = Reactive::of(String::new),
    #[props(into)]
    pub tint: Reactive<Color> = Reactive::of(|| Color::WHITE),
    #[props(into)]
    pub size: Reactive<f32> = Reactive::Const(16.0),
}

/// Inset so a missing glyph reads as a gap in the row rather than a filled chip, and keeps the module's footprint identical to a loaded one so nothing shifts when it settles.
fn inset(size: f32) -> f32 {
    (size * 0.25).max(1.0)
}

fn side(size: f32) -> f32 {
    size - inset(size) * 2.0
}

let stroke = use_theme::<NordTheme>().icon_stroke;
let name = props.name;
let tint_fn = props.tint;
let size_fn = props.size;

// Every prop goes through a memo so the arms below can read them as `$signal`s: each arm is its own closure, and a signal read is what the view's clone prelude knows how to hand to every one of them. The size is in the arm's key, so a glyph whose host is laid out at another size is drawn again at it.
let state = memo(move || (icon_state(&name.get()), size_fn.get()));
let tint = memo(move || tint_fn.get());

[view]
match $state as s key (s.0.as_ready().map(|svg| svg.id()), s.1.to_bits())
    (AssetState::Ready(svg), size)
        svg src:svg color:$tint stroke:stroke width:size height:size
    (AssetState::Failed, size)
        box width:(side(size)) height:(side(size)) margin_start:(inset(size)) margin_end:(inset(size)) fill:$tint radius:(side(size))
    (_, size)
        spinner color:$tint size:size
