//! The surfaces a command can open: panels, the launcher, the dashboard, toasts and notifications.

use super::args::*;
use super::{Command, Target};
use surfaces::shell;

pub(crate) const PANEL: Target = Target {
    name: "panel",
    commands: &[
        Command {
            name: "toggle",
            args: "<module>",
            help: "open a module's panel, or close it if it is up",
            run: |args| {
                let module = arg(args, 0, "module")?;
                surfaces::panel::toggle_panel(module);
                Ok(module.to_string())
            },
        },
        Command {
            name: "open",
            args: "<module>",
            help: "open a module's panel (idempotent)",
            run: |args| {
                let module = arg(args, 0, "module")?;
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
            help: "which panels are open right now",
            run: |_| Ok(shell::open_ids().join("\t")),
        },
    ],
};

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
                surfaces::shell::close(modules::launcher::ID);
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
                Ok(modules::dashboard::tab().id().to_string())
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
                Ok(modules::dashboard::tab().id().to_string())
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
                let query = args.join(" ");
                let config = config::config()
                    .map(|c| c.launcher.clone())
                    .unwrap_or_default();
                let names: Vec<String> = launcher::results(services::apps::all(), &query, &config)
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
                let app = args.join(" ");
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
                let text = args.join(" ");
                if text.trim().is_empty() {
                    return Err("missing argument <text>".to_string());
                }
                // Under the config-reload event, which is the one that means "the shell itself is talking";
                // a script's toast should be switchable off by the same key.
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
