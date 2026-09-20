[logic]
use crate::drawer::{current_drawer_host, panel_pad};
use ui::descriptor::{PanelProps, panel};
use ui::panel::{content_radius, panel_fill};

let host = current_drawer_host()?;
let dw = host.extent.width;
let dmh = host.extent.height;
let rad = content_radius();
// The box below fills with `panel_fill()` rather than the `surface` token: it is that token at the configured `[theme] opacity`, so a compositor `layer_rule = blur, ^hogar-shell` has something to show through.

[view]
box width:dw pad:panel_pad() fill:panel_fill() radius:rad input_opaque
    scroll width:100% height:dmh keep:"drawer.body"
        panel host:host

[preview "Drawer" fixture:crate::preview::drawer surface:520x420]
drawer_panel
