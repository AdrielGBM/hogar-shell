//! Which modules exist, and what each one does when it is clicked, scrolled or hovered.
//!
//! The composition root for the bar: the only place that knows both the module vocabulary — [`ModuleRegistry`]
//! and friends, which live in `ui` and know nothing about any particular module — and the modules themselves.
//! Keeping the two apart is what lets a module crate be built without the shell that arranges it, and what
//! stops a chip helper from reaching sideways into the chip next to it.

use ui::module::{ModuleDef, ModuleRegistry, icon_px, module_fg};
use ui::popouts::PopoutRegistry;

pub fn default_registry(popouts: &PopoutRegistry) -> ModuleRegistry {
    let mut registry = ModuleRegistry::new();
    registry.register("clock", ModuleDef::new(|_ctx| modules::clock()).opens());
    registry.register(
        "dashboard",
        ModuleDef::new(|_ctx| modules::dashboard::dashboard_chip())
            .icon()
            .opens(),
    );
    registry.register(
        "workspaces",
        ModuleDef::new(|_ctx| modules::workspaces())
            .self_managed()
            .on_scroll(modules::workspaces::scroll),
    );
    registry.register(
        "activewindow",
        ModuleDef::new(|_ctx| modules::activewindow())
            .on_click(modules::activewindow::focus_active)
            .elastic(),
    );
    registry.register(
        "logo",
        ModuleDef::new(|_ctx| modules::logo::logo_chip())
            .icon()
            .on_click(|| surfaces::panel::toggle_panel("session")),
    );
    // A gap has no chip: self-managed so the bar places it bare, without padding, hover or a press state.
    registry.register(
        "spacer",
        ModuleDef::new(|_ctx| modules::spacer::spacer()).self_managed(),
    );
    registry.register(
        "launcher",
        ModuleDef::new(|_ctx| {
            let fg = module_fg();
            ui::icon::icon_view(|| "search".to_string(), move || fg.get(), icon_px())
        })
        .icon()
        .on_click(modules::launcher::toggle),
    );
    registry.register(
        "session",
        ModuleDef::new(|_ctx| modules::session::power_chip())
            .icon()
            .opens(),
    );
    registry.register(
        "mic",
        ModuleDef::new(|_ctx| modules::mic())
            .icon()
            .on_click(modules::osd::mic_action)
            .on_scroll(modules::osd::mic_scroll),
    );
    registry.register(
        "kblayout",
        ModuleDef::new(|_ctx| modules::kblayout())
            .on_click(services::hyprland::cycle_main_keyboard_layout),
    );
    // Self-managed: it draws its own indicator row, and with `hide_inactive` that row can be empty — a chip
    // shell would leave a padded gap in the bar where nothing is shown.
    registry.register(
        "lockstatus",
        ModuleDef::new(|_ctx| modules::lockstatus()).self_managed(),
    );
    // Self-managed: it draws one pressable box per application, each with its own click, middle-click,
    // right-click and scroll — a single chip shell around the row could carry none of that.
    registry.register(
        "tray",
        ModuleDef::new(|_ctx| modules::tray()).self_managed(),
    );
    // The chip shell but no click: which of several readings would a press act on? Each keeps its standalone module.
    registry.register(
        "statusicons",
        ModuleDef::new(|_ctx| modules::statusicons::cluster()),
    );
    registry.register(
        "media",
        ModuleDef::new(|_ctx| modules::media())
            .on_click(modules::media::toggle)
            .on_scroll(modules::media::scroll)
            .elastic(),
    );
    registry.register("cpu", ModuleDef::new(|_ctx| modules::cpu()));
    registry.register("gpu", ModuleDef::new(|_ctx| modules::gpu()));
    registry.register("memory", ModuleDef::new(|_ctx| modules::memory()));
    registry.register("temperature", ModuleDef::new(|_ctx| modules::temperature()));
    registry.register("netspeed", ModuleDef::new(|_ctx| modules::netspeed()));
    registry.register(
        "battery",
        ModuleDef::new(|_ctx| modules::battery()).icon().opens(),
    );
    registry.register(
        "network",
        ModuleDef::new(|_ctx| modules::network()).icon().opens(),
    );
    registry.register(
        "bluetooth",
        ModuleDef::new(|_ctx| modules::bluetooth::chip())
            .icon()
            .opens(),
    );
    registry.register(
        "volume",
        ModuleDef::new(|_ctx| modules::volume())
            .icon()
            .on_click(modules::osd::volume_action)
            .on_scroll(modules::osd::volume_scroll),
    );
    // The pointer path to a non-default device. The volume chip stays what it is — a level, a mute and a wheel
    // — because a chip that opened a panel could no longer toggle mute with the same press.
    registry.register(
        "mixer",
        ModuleDef::new(|_ctx| {
            let fg = module_fg();
            ui::icon::icon_view(
                || "sliders-horizontal".to_string(),
                move || fg.get(),
                icon_px(),
            )
        })
        .icon()
        .opens(),
    );
    registry.register(
        "brightness",
        ModuleDef::new(|_ctx| modules::brightness())
            .icon()
            .on_click(modules::osd::brightness_action)
            .on_scroll(modules::osd::brightness_scroll),
    );
    registry.register(
        "notifications",
        ModuleDef::new(|_ctx| modules::notifications::bell_module()).opens(),
    );
    registry.register(
        "notes",
        ModuleDef::new(|_ctx| modules::notes::notes_chip())
            .icon()
            .opens(),
    );
    registry.register(
        "settings",
        ModuleDef::new(|_ctx| settings::panel::settings_chip())
            .icon()
            .opens(),
    );
    registry.register(
        "utilities",
        ModuleDef::new(|_ctx| modules::utilities::utilities_chip())
            .icon()
            .opens(),
    );
    registry.register(
        "windowinfo",
        ModuleDef::new(|_ctx| modules::windowinfo::window_chip())
            .icon()
            .opens(),
    );
    // Wired from the one list that knows which modules have card content, so no chip is given a hover target it would open empty.
    registry.wire_popouts(|id| popouts.has(id));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui::module::ModuleClick;

    fn default_registry() -> ModuleRegistry {
        super::default_registry(&crate::core::popouts::default_popouts())
    }

    /// The two halves of the composition root have to agree, and neither direction is a compile error: a module
    /// wired with `.opens()` and left out of the panel registry falls back to the clock on screen, and a panel
    /// registered for a module that does not exist can never be reached. Checked against each other rather than
    /// against a hand-kept list, so adding a module in one place and forgetting the other fails here.
    ///
    /// The reverse of the first check is deliberately weaker than "its chip opens it": `logo` is an alias that
    /// shows the session panel, and its chip runs an action rather than opening its own.
    #[test]
    fn a_module_opens_a_panel_exactly_when_one_is_registered_for_it() {
        let modules = default_registry();
        let panels = crate::core::panels::default_panels();
        for id in panels.ids() {
            assert!(
                modules.def(&id).is_some(),
                "'{id}' has a panel but is not a registered module, so nothing can reach it"
            );
        }
        for (id, def) in modules.iter() {
            if matches!(def.click, Some(ModuleClick::Panel)) {
                assert!(
                    panels.def(id).is_some(),
                    "'{id}' opens a panel, so one must be registered for it"
                );
            }
        }
    }

    #[test]
    fn the_new_bar_modules_are_registered_with_the_right_roles() {
        let r = default_registry();
        assert!(
            r.def("spacer").unwrap().self_managed,
            "a gap gets no chip shell, padding or hover state"
        );
        assert!(
            r.def("activewindow").unwrap().click.is_some(),
            "clicking the title focuses the window it names"
        );
        assert!(
            matches!(r.def("mic").unwrap().click, Some(ModuleClick::Action(_)))
                && r.def("mic").unwrap().scroll.is_some(),
            "the mic chip mutes on click and adjusts on scroll, like the volume chip"
        );
        for id in ["cpu", "memory", "temperature", "netspeed"] {
            assert!(
                r.def(id).unwrap().click.is_none(),
                "'{id}' is a readout, not a control"
            );
        }
        assert!(
            r.def("logo").unwrap().icon,
            "the logo is a square icon chip"
        );
    }

    /// A reading shown in the `statusicons` cluster and a reading shown as its own chip are the same thing under
    /// the same name, so a user moving one between the two does not have to rename it. Checked here because the
    /// cluster and the module list are two different modules' business, and this is where both are in scope.
    #[test]
    fn a_cluster_icon_and_its_own_chip_share_one_name() {
        use modules::statusicons::StatusIcon;
        let modules = default_registry();
        for name in ["volume", "mic", "network", "bluetooth", "battery"] {
            assert!(StatusIcon::from_id(name).is_some(), "'{name}' is an icon");
            assert!(modules.def(name).is_some(), "'{name}' is also a module id");
        }
        assert!(
            modules.def("wifi").is_none(),
            "`wifi` is a cluster-only reading: the `network` chip already covers being online over any link"
        );
    }

    #[test]
    fn registry_flags_reflect_module_roles() {
        let r = default_registry();
        assert!(
            matches!(r.def("clock").unwrap().click, Some(ModuleClick::Panel)),
            "clock opens a panel"
        );
        assert!(
            matches!(r.def("volume").unwrap().click, Some(ModuleClick::Action(_))),
            "volume runs a custom action (mute + OSD)"
        );
        assert!(
            r.def("workspaces").unwrap().self_managed,
            "workspaces manages its own layout"
        );
        let tray = r.def("tray").unwrap();
        assert!(
            tray.self_managed,
            "each tray icon carries its own click, middle-click, right-click and scroll"
        );
        assert!(
            tray.click.is_none() && tray.scroll.is_none(),
            "a single chip-level handler would act on the row, not on the application clicked"
        );
        assert!(
            matches!(r.def("battery").unwrap().click, Some(ModuleClick::Panel)),
            "battery opens its detail panel"
        );
        assert!(
            r.def("network").unwrap().icon
                && matches!(r.def("network").unwrap().click, Some(ModuleClick::Panel)),
            "network is an icon chip that opens its network list"
        );
    }
}
