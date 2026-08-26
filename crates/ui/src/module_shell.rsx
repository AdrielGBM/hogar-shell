[logic]
use crate::module::{DragOpen, chip_pad, from_chip, open_panel};
use ::config::Variant;
use ::config::theme::NordTheme;
use std::cell::RefCell;
use std::rc::Rc;

/// The base container every simple module sits in: a rounded, pressable box with hover/press feedback. `Filled` overrides the resting background with a solid accent.
///
/// Every handler is optional and forwarded as one: a chip with nothing to do must stay transparent to the pointer, and a no-op stand-in would report the event handled instead.
pub struct Props {
    pub variant: Variant = Variant::Default,
    /// The resting background: transparent when blending into the bar, the surface token as a free-standing chip.
    pub rest: Color = Color::TRANSPARENT,
    pub accent: Color = Color::TRANSPARENT,
    pub radius: f32 = 0.0,
    /// A square icon chip that scales with the bar, rather than a content-width text pill.
    pub square: bool = false,
    /// Gives up width when the bar is short of it, instead of holding the chip's content width. Only for a chip whose label elides — otherwise it hides its own tail with nothing to say so.
    pub elastic: bool = false,
    pub on_press: Option<Box<dyn Fn()>> = None,
    pub on_scroll: Option<Box<dyn Fn(f32, f32)>> = None,
    pub drag_open: Option<DragOpen> = None,
}

let theme = use_theme::<NordTheme>();
let radius = props.radius;
let accent = props.accent;
let (base, hover, active) = match props.variant {
    Variant::Default => (props.rest, theme.overlay, theme.overlay.darken(0.14)),
    Variant::Filled => (accent, accent.darken(0.08), accent.darken(0.16)),
};

// A square chip stretches to the bar's thickness, and symmetric padding around a bar-proportional icon (see `icon_px`) makes the other side match.
let inset_x = if props.square { chip_pad() } else { 8.0 };
let inset_y = if props.square { chip_pad() } else { 2.0 };

// An elastic chip needs the floor out from under it as well as the willingness to shrink: a flex item's own minimum is its content, and a label that has not been told it may elide reports the whole title as content. Only along a horizontal bar, where there is a length to give up — down a vertical one a chip's width is the bar's, and these modules show their glyph alone anyway.
let elastic = props.elastic && !crate::module::bar_is_vertical();
let shrink = if elastic { 1.0 } else { 0.0 };
let floor = if elastic {
    SizeDimension::Px(0.0)
} else {
    SizeDimension::Auto
};

// Where the chip ended up on its bar. Whatever its press opens hangs off this rather than off an end of the bar, so a drawer lands under the chip exactly as the hover popout does.
let chip = signal(Rect::default());

let pressed = chip.clone();
let press = props
    .on_press
    .map(|press| move || from_chip(pressed.get(), || press()));
let scroll = props.on_scroll;

let dragged = chip.clone();
let drag = props.drag_open;
let origin: Rc<RefCell<Option<(f32, f32)>>> = Rc::new(RefCell::new(None));
let released = Rc::clone(&origin);
let arm = drag.is_some().then(|| {
    move |x: f32, y: f32| {
        origin.borrow_mut().get_or_insert((x, y));
    }
});
let settle = drag.map(|drag| {
    move |x: f32, y: f32| {
        let from = released.borrow_mut().take().unwrap_or((x, y));
        if drag.travel(from, (x, y)) >= drag.threshold {
            from_chip(dragged.get(), || open_panel(&drag.module));
        }
    }
});

[view]
// Both halves of the drag sit on the pressable box itself, not on a wrapper: a child hit-tests first, so a drag armed outside it would never see the press.
row track_rect:$chip align:center justify:center pad_x:inset_x pad_y:inset_y shrink:shrink min_width:floor fill:theme.base radius:radius hover_style(fill:hover) active_style(fill:active) on_press(press) on_scroll(scroll) on_drag(arm) on_drag_end(settle)
    children

[preview "Module chip" fixture:crate::preview::bar_chip]
// Wrapped in a row so the chip keeps its own width: on the preview page's column it would stretch the full width instead, which is the one shape a bar never gives it.
row
    module_shell radius:8 square:true rest:(use_theme::<::config::theme::NordTheme>().overlay)
        icon_glyph name(|| "cpu".to_string()) size:18
