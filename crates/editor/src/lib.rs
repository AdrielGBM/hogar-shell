telar::rsx_modules!();

thread_local! {
    static INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Starts the editor on the driver thread: the mode's watcher, the session that keeps the selection, the draft and the screen in step, the context menus every item and area opens, the keyboard path through every mode, and every mode's tools. Called once, after the transient registry is installed; a second call changes nothing.
pub fn install() {
    if INSTALLED.with(|installed| installed.replace(true)) {
        return;
    }
    mode::install();
    session::install();
    variant::install();
    popover::install();
    context::install();
    shell_menu::install();
    keys::install();
    for layer in layout::LayerKind::SESSION {
        host::add_tool(layer, select::tool);
    }
    modes::install();
    tools::install();
    select::install();
    theme::install();
    templates::install();
}
