[logic]
use crate::drawer::{
    content_radius, current_drawer_config, current_drawer_module, module_panel, panel_pad,
    panel_fill,
};

let drawer = current_drawer_config();
let dw = drawer.width;
let dmh = drawer.max_height;
let rad = content_radius();
let module = current_drawer_module();
// The module's panel content, dispatched in Rust and embedded below with `widget`.
let content = module_panel(&module)?;
// The box below fills with `panel_fill()` rather than the `surface` token: it is that token at the configured `[theme] opacity`, so a compositor `layer_rule = blur, ^hyprshell` has something to show through.

[view]
box width:dw pad:panel_pad() fill:panel_fill() radius:rad
    scroll width:100% height:dmh keep:"drawer.body"
        widget "content"

[preview "Drawer" fixture:crate::preview::drawer surface:520x420]
drawer_panel
