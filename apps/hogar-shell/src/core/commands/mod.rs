//! The command table, one file per area of the shell.
//!
//! Split by what a command acts on rather than by what it needs: `hogar-shell volume up` and `hogar-shell mic mute` are one thing to a user reading `--list`, and were one thing to whoever is adding the next one.

pub mod args;
pub mod audio;
pub mod display;
pub mod layout;
pub mod shell;
pub mod surfaces;
pub mod system;

pub(crate) struct Command {
    pub(crate) name: &'static str,
    pub(crate) args: &'static str,
    pub(crate) help: &'static str,
    pub(crate) run: fn(&[&str]) -> Result<String, String>,
}

pub(crate) struct Target {
    pub(crate) name: &'static str,
    pub(crate) commands: &'static [Command],
}

/// Every command the shell answers. One table, so `--list`, `hogar-shell(1)` and what actually dispatches cannot drift from one another.
pub(crate) static TARGETS: &[Target] = &[
    shell::SHELL,
    system::LOCK,
    system::IDLE,
    surfaces::PANEL,
    surfaces::LAUNCHER,
    audio::AUDIO,
    surfaces::DASHBOARD,
    surfaces::APPS,
    surfaces::NOTIFS,
    display::SCREENSHOT,
    display::RECORD,
    surfaces::TOAST,
    audio::VOLUME,
    audio::MIC,
    system::WEATHER,
    system::GAMEMODE,
    system::WIFI,
    system::VPN,
    system::BLUETOOTH,
    audio::MEDIA,
    shell::SESSION,
    system::KEYBOARD,
    display::BRIGHTNESS,
    display::NIGHTLIGHT,
    display::WALLPAPER,
    shell::SCHEME,
    shell::CONFIG,
    layout::LAYOUT,
    shell::DEPS,
    shell::MAN,
];

/// Whether `line` names a command the shell answers, **without running it**.
///
/// The distinction is the whole reason [`resolve`] is split out of [`dispatch`]: anything that wants to check a request line — the global-shortcut table, a future config validator — must be able to do so without performing it. Half this table changes the machine.
pub fn resolves(line: &str) -> bool {
    resolve(line).is_ok()
}

/// Looks a request line up in the command table without running anything, yielding the command and its arguments, or the `err …` reply the caller should send back.
///
/// Split out from [`dispatch`] so that "is this command wired up" can be answered *without executing it*. The listing test used to answer that by dispatching every advertised command with no arguments — which for any command that needs none is not a lookup, it is the command. `wifi disconnect` and `vpn toggle` both take no arguments, so running the test suite dropped the machine off the network; `volume up` and `brightness down` had been quietly moving the user's settings for far longer.
fn resolve(line: &str) -> Result<(&'static Command, Vec<&str>), String> {
    let mut words = line.split_whitespace();
    let Some(target_name) = words.next() else {
        return Err("empty request".to_string());
    };
    let command_name = words.next().unwrap_or("");
    let args: Vec<&str> = words.collect();

    let Some(target) = TARGETS.iter().find(|t| t.name == target_name) else {
        return Err(format!("unknown target '{target_name}'"));
    };
    let Some(command) = target.commands.iter().find(|c| c.name == command_name) else {
        let known: Vec<&str> = target.commands.iter().map(|c| c.name).collect();
        return Err(format!(
            "unknown command '{command_name}' for '{target_name}' (try: {})",
            known.join(", ")
        ));
    };
    Ok((command, args))
}

/// Runs one request line and renders the reply. `ok`/`err` prefixes let a caller branch on the outcome without parsing the message; the payload follows on the same line when there is one.
pub fn dispatch(line: &str) -> String {
    let (command, args) = match resolve(line) {
        Ok(found) => found,
        Err(message) => return format!("err {message}"),
    };
    match (command.run)(&args) {
        Ok(payload) if payload.is_empty() => "ok".to_string(),
        Ok(payload) => format!("ok {payload}"),
        Err(message) => format!("err {message}"),
    }
}

/// Runs one request line in *this* process rather than sending it to the shell, for the commands that are a function of the binary and the machine rather than of a running shell.
///
/// `deps` is the case that matters: what a dependency report is for is the machine where something is missing, and "nothing starts" is precisely when there is no shell to ask.
pub fn dispatch_locally(line: &str) -> Result<String, String> {
    let (command, args) = resolve(line)?;
    (command.run)(&args)
}

/// The column the help text starts at in `--list`.
const HELP_COLUMN: usize = 28;

/// Every target and command, one per line, for `hogar-shell --list`.
pub fn describe() -> String {
    let mut out = String::new();
    for target in TARGETS {
        out.push_str(&format!("target {}\n", target.name));
        for command in target.commands {
            let signature = if command.args.is_empty() {
                command.name.to_string()
            } else {
                format!("{} {}", command.name, command.args)
            };
            // A signature wider than the column gets its help on the next line rather than running into it.
            if signature.len() >= HELP_COLUMN {
                out.push_str(&format!(
                    "  {signature}\n  {:HELP_COLUMN$}{}\n",
                    "", command.help
                ));
            } else {
                out.push_str(&format!("  {signature:HELP_COLUMN$}{}\n", command.help));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::args::*;
    use super::*;

    /// The failure this catches: a stage shipped in the defaults naming a command that was renamed, which would be a timeout that silently does nothing on every fresh install.
    ///
    /// Here rather than beside the stages: the table it checks them against is this file's, and a service has no way to reach it.
    #[test]
    fn the_default_idle_stages_are_commands_the_shell_actually_answers() {
        for stage in config::IdleConfig::default().stages {
            assert!(
                resolves(&stage.action),
                "'{}' is not a command this shell answers",
                stage.action
            );
            if !stage.return_action.is_empty() {
                assert!(
                    resolves(&stage.return_action),
                    "'{}' is not a command this shell answers",
                    stage.return_action
                );
            }
        }
    }

    /// The table is only useful if each line resolves — a shortcut bound to a typo is a key that does nothing with no way to tell. Resolved, never dispatched: half of these change the machine.
    #[test]
    fn every_registered_shortcut_runs_a_command_the_shell_answers() {
        for shortcut in services::shortcuts::SHORTCUT_TABLE {
            assert!(
                resolves(shortcut.command),
                "'{}' runs '{}', which is not a command the shell answers",
                shortcut.id,
                shortcut.command
            );
        }
    }

    #[test]
    fn unknown_targets_and_commands_report_what_is_available() {
        assert_eq!(dispatch("nope ping"), "err unknown target 'nope'");
        let reply = dispatch("panel wiggle");
        assert!(reply.starts_with("err unknown command 'wiggle'"), "{reply}");
        assert!(
            reply.contains("toggle"),
            "it lists the real commands: {reply}"
        );
        assert_eq!(dispatch("   "), "err empty request");
    }

    #[test]
    fn a_command_with_no_arguments_reports_which_one_is_missing() {
        assert_eq!(dispatch("panel toggle"), "err missing argument <module>");
        assert_eq!(
            dispatch("volume set abc"),
            "err <percent> must be a whole number, got 'abc'"
        );
    }

    #[test]
    fn replies_carry_an_ok_or_err_prefix() {
        assert_eq!(dispatch("shell ping"), "ok pong");
        assert_eq!(
            dispatch("shell version"),
            format!("ok {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn describe_covers_every_target_and_is_what_dispatch_accepts() {
        let listing = describe();
        for target in TARGETS {
            assert!(
                listing.contains(&format!("target {}", target.name)),
                "'{}' missing from the listing",
                target.name
            );
            for command in target.commands {
                // Resolved, never run. Half of this table changes the machine — the network it is on, the volume, the backlight, whether the process is still alive — and a test that proved the wiring by *executing* every entry was doing all of that to whoever ran `cargo test`.
                let line = format!("{} {}", target.name, command.name);
                let (found, _) = resolve(&line)
                    .unwrap_or_else(|e| panic!("{line} is advertised but does not resolve: {e}"));
                assert_eq!(
                    found.name, command.name,
                    "'{} {}' resolved to a different command",
                    target.name, command.name
                );
            }
        }
        // The lookup still reports what it cannot find, which is the other half of the contract.
        assert!(resolve("nosuchtarget ping").is_err());
        assert!(resolve("shell nosuchcommand").is_err());
        assert!(resolve("").is_err());
    }

    #[test]
    fn a_command_that_changes_the_machine_is_never_run_by_the_suite() {
        // A standing guard on the test above: these take no arguments, so dispatching one "just to check it resolves" performs it. Listed by name so that adding another argumentless mutation is a decision someone makes here rather than something a green test run hides.
        const ARGUMENTLESS_MUTATIONS: &[(&str, &str)] = &[
            ("shell", "quit"),
            ("shell", "reload"),
            ("notifs", "clear"),
            ("wifi", "scan"),
            ("wifi", "disconnect"),
            ("vpn", "toggle"),
            ("volume", "up"),
            ("volume", "down"),
            ("volume", "mute"),
            ("mic", "mute"),
            ("brightness", "up"),
            ("brightness", "down"),
            ("media", "play-pause"),
            ("wallpaper", "reload"),
            ("wallpaper", "random"),
            ("wallpaper", "clear"),
            ("scheme", "refresh"),
            ("scheme", "export"),
            ("screenshot", "screen"),
            ("screenshot", "region"),
            ("screenshot", "cancel"),
            ("record", "start"),
            ("record", "stop"),
            ("record", "toggle"),
            ("record", "pause"),
            ("toast", "clear"),
            ("notifs", "center"),
            ("keyboard", "next"),
        ];
        for (target, command) in ARGUMENTLESS_MUTATIONS {
            assert!(
                resolve(&format!("{target} {command}")).is_ok(),
                "'{target} {command}' is listed here but no longer exists"
            );
        }
    }

    #[test]
    fn a_name_that_is_not_a_monitor_is_refused_rather_than_saved() {
        // Not hypothetical: a dev harness appending `--features rsx/dev` to the program's arguments had `wallpaper random` read `--features` as a screen, write it into the persisted assignment, and answer `ok`. Nothing would ever have painted it.
        let screens = vec!["DP-1".to_string(), "HDMI-A-1".to_string()];
        assert_eq!(known_screen("DP-1", &screens), Ok("DP-1".to_string()));

        let refused = known_screen("--features", &screens).expect_err("a flag is not a screen");
        assert!(refused.contains("not a monitor"), "{refused}");
        assert!(
            refused.contains("DP-1"),
            "and it says what there is: {refused}"
        );
        // A typo in a real name is the common case and must not read as "no monitors".
        assert!(known_screen("DP1", &screens).is_err());
        assert!(
            known_screen("DP-1", &[])
                .unwrap_err()
                .contains("none are connected"),
            "no screens at all is its own message, not an empty list"
        );
    }

    #[test]
    fn a_wallpaper_command_with_no_screen_named_means_every_screen() {
        // `clear` defaulting to the focused screen removed one entry, answered `cleared`, and left the rest — including one saved under a name no monitor has, which validation then made unreachable. A command whose help says "every screen" has to mean it.
        assert_eq!(target_output(None), Ok(None));
        // The reading side is the opposite and deliberately so: "which image is showing" needs a screen. Only the `None` branch is asserted here — resolving a name asks the compositor.
        assert!(reading_output(None).is_ok());
    }
}
