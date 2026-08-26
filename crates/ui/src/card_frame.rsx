[logic]
/// The card's own box: the panel background at the bar's radius, and the pointer tracking that keeps the popout up while it is hovered. `on_hover` is also what registers the box as an interactive target, which is how the surface knows which part of itself to take input over.
///
/// `fill` is a prop rather than a theme read, because it carries `[theme] opacity` — and a popout surface is not a bar, so the config it should resolve against is the one its opener had in hand.
pub struct Props {
pub fill: Color = Color::TRANSPARENT,
pub width: f32 = 320.0,
pub radius: f32 = 0.0,
pub on_hover: Box<dyn Fn(bool)> = Box::new(|_| {}),
}

let fill = props.fill;
let width = props.width;
let radius = props.radius;
let on_hover = props.on_hover;

[view]
col width:width pad:crate::scale::space::xl() shrink:0 fill:fill radius:radius on_hover(|hovered| on_hover(hovered))
    children

[preview "Popout frame"]
card_frame width:240 radius:12 fill:(use_theme::<::config::theme::NordTheme>().overlay)
    text "A card sits in here" color:theme.text
