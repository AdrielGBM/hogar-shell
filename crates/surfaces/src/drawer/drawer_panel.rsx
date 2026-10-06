[logic]
use crate::drawer::{current_drawer_host, panel_pad};
use ui::descriptor::{PanelProps, panel};
use ui::chrome::{content_radius, panel_fill};

let host = current_drawer_host()?;
let dw = host.extent().width;
let dmh = host.extent().height;
let rad = content_radius();

[view]
box width:dw pad:panel_pad() fill:panel_fill() radius:rad input_opaque
    scroll width:100% height:dmh keep:"drawer.body"
        panel host:host

[preview "Drawer" fixture:crate::preview::drawer surface:520x420]
drawer_panel
