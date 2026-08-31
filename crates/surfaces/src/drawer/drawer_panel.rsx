[logic]
use crate::drawer::{
    content_radius, current_drawer_config, current_drawer_module, panel_fill, panel_pad,
};
use ui::panels::{PanelProps, panel};

let drawer = current_drawer_config();
let dw = drawer.width;
let dmh = drawer.max_height;
let rad = content_radius();
let module = current_drawer_module();
// The box below fills with `panel_fill()` rather than the `surface` token: it is that token at the configured `[theme] opacity`, so a compositor `layer_rule = blur, ^hogar-shell` has something to show through.

[view]
box width:dw pad:panel_pad() fill:panel_fill() radius:rad
    scroll width:100% height:dmh keep:"drawer.body"
        panel module:module

[preview "Drawer" fixture:crate::preview::drawer surface:520x420]
drawer_panel
