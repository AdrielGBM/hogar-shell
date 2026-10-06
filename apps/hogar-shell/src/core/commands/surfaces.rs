//! The surfaces a command can open: panels, the launcher, the dashboard, toasts and notifications.

use super::args::*;
use super::{Command, Target};
use surfaces::transient;

pub(crate) const PANEL: Target = Target {
    name: "panel",
    commands: &[
        Command {
            name: "toggle",
            args: "<module|instance>",
            help: "open a module's panel, or close it if it is up; an instance opens the panel the layout gives it, else its module's panel beside it",
            run: |args| {
                let id = arg(args, 0, "module|instance")?;
                surfaces::panel::toggle_named(id)?;
                Ok(id.to_string())
            },
        },
        Command {
            name: "open",
            args: "<module>",
            help: "open a module's panel (idempotent)",
            run: |args| {
                let module = with_panel(arg(args, 0, "module")?)?;
                surfaces::panel::open_panel(module);
                Ok(module.to_string())
            },
        },
        Command {
            name: "close",
            args: "<module>",
            help: "close a module's panel",
            run: |args| {
                let module = arg(args, 0, "module")?;
                surfaces::panel::close_panel(module);
                Ok(module.to_string())
            },
        },
        Command {
            name: "list",
            args: "",
            help: "which transients are open right now — a panel, but also the drawer, popout, tray menu, float, launcher, notification centre or region picker",
            run: |_| Ok(transient::open_ids().join("\t")),
        },
    ],
};

/// `module`, when it has a panel to open; the reply says so when it has none, rather than an `ok` for a request nothing acted on.
fn with_panel(module: &str) -> Result<&str, String> {
    if ui::descriptor::has_panel(module) {
        Ok(module)
    } else {
        Err(format!("'{module}' has no panel"))
    }
}

pub(crate) const LAUNCHER: Target = Target {
    name: "launcher",
    commands: &[
        Command {
            name: "toggle",
            args: "",
            help: "open the application launcher, or close it if it is up",
            run: |_| {
                modules::launcher::toggle();
                Ok("toggled".to_string())
            },
        },
        Command {
            name: "close",
            args: "",
            help: "close the launcher",
            run: |_| {
                surfaces::transient::close(modules::launcher::ID);
                Ok("closed".to_string())
            },
        },
    ],
};

pub(crate) const DASHBOARD: Target = Target {
    name: "dashboard",
    commands: &[
        Command {
            name: "toggle",
            args: "",
            help: "open the dashboard, or close it if it is up",
            run: |_| {
                surfaces::panel::toggle_panel(modules::dashboard::ID);
                Ok("toggled".to_string())
            },
        },
        Command {
            name: "open",
            args: "[tab]",
            help: "open the dashboard, optionally on a named page",
            run: |args| {
                if let Some(name) = args.first() {
                    set_dashboard_tab(name)?;
                }
                surfaces::panel::open_panel(modules::dashboard::ID);
                Ok(dashboard_tab())
            },
        },
        Command {
            name: "close",
            args: "",
            help: "close the dashboard",
            run: |_| {
                surfaces::panel::close_panel(modules::dashboard::ID);
                Ok("closed".to_string())
            },
        },
        Command {
            name: "tab",
            args: "[dash|media|performance|weather]",
            help: "read or switch the page the dashboard shows",
            run: |args| {
                if let Some(name) = args.first() {
                    set_dashboard_tab(name)?;
                }
                Ok(dashboard_tab())
            },
        },
    ],
};

pub(crate) const APPS: Target = Target {
    name: "apps",
    commands: &[
        Command {
            name: "count",
            args: "",
            help: "how many desktop entries are known",
            run: |_| Ok(services::apps::all().len().to_string()),
        },
        Command {
            name: "reload",
            args: "",
            help: "re-scan the application directories",
            run: |_| Ok(services::apps::reload().to_string()),
        },
        Command {
            name: "search",
            args: "<query>",
            help: "the launcher's ranking for a query, best first",
            run: |args| {
                use modules::launcher;
                let query = args.rest(0);
                let config = config::config()
                    .map(|c| c.launcher.clone())
                    .unwrap_or_default();
                let names: Vec<String> = launcher::results(services::apps::all(), query, &config)
                    .into_iter()
                    .map(|a| a.name)
                    .collect();
                Ok(names.join("\t"))
            },
        },
    ],
};

pub(crate) const NOTIFS: Target = Target {
    name: "notifs",
    commands: &[
        Command {
            name: "clear",
            args: "[app]",
            help: "drop one application's notifications, or the whole history",
            run: |args| {
                use services::notifications as notifs;
                let app = args.rest(0);
                match app.trim() {
                    "" => notifs::clear_all(),
                    app => notifs::clear_app(app),
                }
                Ok("cleared".to_string())
            },
        },
        Command {
            name: "mute",
            args: "<app> [on|off|toggle]",
            help: "read or set whether an application's notifications may pop",
            run: |args| {
                use services::notifications as notifs;
                let (app, rest) = args.split_first().ok_or("missing argument <app>")?;
                let current = notifs::is_app_muted(app);
                let next = match rest.first().copied() {
                    None => return Ok(on_off(current).to_string()),
                    Some("on") => true,
                    Some("off") => false,
                    Some("toggle") => !current,
                    Some(other) => {
                        return Err(format!("expected on|off|toggle, got '{other}'"));
                    }
                };
                notifs::set_app_muted(app, next);
                Ok(on_off(next).to_string())
            },
        },
        Command {
            name: "muted",
            args: "",
            help: "the applications whose notifications are muted",
            run: |_| {
                let muted = services::notifications::snapshot_now()
                    .map(|s| s.muted_apps.clone())
                    .unwrap_or_default();
                Ok(muted.join("\t"))
            },
        },
        Command {
            name: "dnd",
            args: "<on|off|toggle>",
            help: "read or set do-not-disturb",
            run: |args| {
                let current = services::notifications::snapshot_now()
                    .map(|s| s.dnd)
                    .unwrap_or(false);
                let next = match args.first().copied() {
                    None => return Ok(on_off(current).to_string()),
                    Some("on") => true,
                    Some("off") => false,
                    Some("toggle") => !current,
                    Some(other) => {
                        return Err(format!("expected on|off|toggle, got '{other}'"));
                    }
                };
                services::notifications::set_dnd(next);
                Ok(on_off(next).to_string())
            },
        },
        Command {
            name: "center",
            args: "[open|close|toggle]",
            help: "the notification centre: history and quick toggles on a full-height surface",
            run: |args| {
                use modules::sidebar;
                match args.first().copied().unwrap_or("toggle") {
                    "open" => sidebar::open(),
                    "close" => sidebar::close(),
                    "toggle" => sidebar::toggle(),
                    other => {
                        return Err(format!("expected open|close|toggle, got '{other}'"));
                    }
                }
                Ok(on_off(sidebar::is_open()).to_string())
            },
        },
    ],
};

pub(crate) const TOAST: Target = Target {
    name: "toast",
    commands: &[
        Command {
            name: "show",
            args: "<text…>",
            help: "show an in-shell toast, for a script that wants to say something",
            run: |args| {
                use services::toaster::{self, Event};
                let text = args.rest(0).to_string();
                if text.trim().is_empty() {
                    return Err("missing argument <text>".to_string());
                }
                // Under the config-reload event, which is the one that means "the shell itself is talking"; a script's toast should be switchable off by the same key.
                toaster::post(Event::ConfigLoaded, "info", text.clone(), String::new());
                Ok(text)
            },
        },
        Command {
            name: "clear",
            args: "",
            help: "dismiss every toast on screen",
            run: |_| {
                services::toaster::clear();
                Ok("cleared".to_string())
            },
        },
    ],
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use layout::{
        AreaId, InstanceId, LayerKind, Library, Resolved, ResolvedArea, ResolvedAreaKind, Style,
        Within,
    };

    use crate::core::commands::dispatch;

    /// The shipped layout on DP-1 with a panel owned by its bar's clock.
    fn clock_owning_a_panel() -> Resolved {
        let shipped = layout::resolve(&layout::built_in(), &Library::default(), "DP-1", None).0;
        let panel = ResolvedArea {
            id: AreaId::new("clock-panel"),
            kind: ResolvedAreaKind::Panel {
                owner: InstanceId::new("clock"),
                along: false,
                cols: 2,
                rows: 2,
                cell: 40.0,
                gap: 8.0,
            },
            reserve: false,
            above_fullscreen: false,
            within: Within::Output,
            style: Style::default(),
            visible: None,
            groups: Vec::new(),
            actions: BTreeMap::new(),
        };
        let layers = LayerKind::ALL.into_iter().filter_map(|kind| {
            let mut layer = shipped.layer(kind)?.clone();
            if kind == LayerKind::Top {
                layer.areas.push(panel.clone());
            }
            Some((kind, layer))
        });
        Resolved::of("DP-1", layers)
    }

    /// `panel toggle <instance>` opens and closes the panel the layout gives that instance, under the id `panel list` names it by; a name nothing answers to is refused.
    #[test]
    fn panel_toggle_an_instance_toggles_the_panel_it_owns() {
        ui::descriptor::install(crate::core::modules::MODULES);
        surfaces::transient::close_all();
        surfaces::reconcile::publish(&[surfaces::reconcile::Desktop {
            output: Some("DP-1".to_string()),
            config: Arc::new(config::Config::default()),
            resolved: clock_owning_a_panel(),
            reserved: Default::default(),
            size: (1920.0, 1080.0),
        }]);

        assert_eq!(dispatch("panel toggle clock"), "ok clock");
        assert!(
            surfaces::transient::is_open("clock@DP-1"),
            "{:?}",
            surfaces::transient::open_ids()
        );
        assert!(
            !surfaces::transient::is_open("clock"),
            "not the module's own"
        );
        assert_eq!(dispatch("panel toggle clock"), "ok clock");
        assert!(!surfaces::transient::is_open("clock@DP-1"));

        assert_eq!(
            dispatch("panel toggle no-such-thing"),
            "err 'no-such-thing' has no panel"
        );
        surfaces::transient::close_all();
    }
}
