use telar::LayoutError;

use ui::chrome::Chrome;
use ui::descriptor::Built;
use ui::host::{Host, InstanceId, Representation, Size};
use ui::scale::space;

pub(crate) fn panel_pad() -> f32 {
    space::xl()
}

/// Set on the drawer's own scope so `drawer_panel.rsx`, which takes no props, can read it.
#[derive(Clone)]
struct DrawerHost(Host);

pub fn set_drawer_host(module: &str, chrome: &Chrome) {
    let drawer = chrome.config.panels.drawer;
    util::state::set_context(DrawerHost(Host::in_chrome(
        InstanceId::of_module(module),
        Representation::Panel,
        chrome,
        Size {
            width: drawer.width,
            height: drawer.max_height,
        },
    )));
}

pub fn current_drawer_host() -> Result<Host, LayoutError> {
    util::state::context::<DrawerHost>()
        .map(|DrawerHost(host)| host)
        .ok_or_else(|| LayoutError::Engine("a drawer was built outside its transient".to_string()))
}

pub(crate) fn content(module: &str, chrome: &Chrome) -> Built {
    set_drawer_host(module, chrome);
    drawer_panel::drawer_panel(
        drawer_panel::DrawerPanelProps::props().build(),
        telar::Children::default(),
    )
}
