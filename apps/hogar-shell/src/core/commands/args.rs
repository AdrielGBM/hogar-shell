//! Turning a command line into what it acts on: the arguments, the monitor it names, and the readings a
//! reply prints.
use services::pipewire::NodeKind;
use surfaces::shell;

pub(crate) fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}

pub(crate) fn arg<'a>(args: &'a [&'a str], index: usize, name: &str) -> Result<&'a str, String> {
    args.get(index)
        .copied()
        .ok_or_else(|| format!("missing argument <{name}>"))
}

pub(crate) fn number(args: &[&str], index: usize, name: &str) -> Result<i32, String> {
    let raw = arg(args, index, name)?;
    raw.parse()
        .map_err(|_| format!("<{name}> must be a whole number, got '{raw}'"))
}

/// One row per node, tab-separated so a script can cut columns: id, level, mute, whether it is the default,
/// and the label last because it is the only field that can contain spaces.
pub(crate) fn list_nodes(kind: NodeKind) -> String {
    use services::pipewire;
    let Some(graph) = pipewire::current() else {
        return String::new();
    };
    let default = match kind {
        NodeKind::Source => graph.default_source().map(|node| node.id),
        _ => graph.default_sink().map(|node| node.id),
    };
    graph
        .of_kind(kind)
        .map(|node| {
            format!(
                "{}\t{}\t{}\t{}\t{}",
                node.id,
                node.level,
                on_off(node.muted),
                if Some(node.id) == default {
                    "default"
                } else {
                    "-"
                },
                node.label()
            )
        })
        .collect::<Vec<String>>()
        .join("\n")
}

pub(crate) fn node_id(args: &[&str]) -> Result<u32, String> {
    let raw = arg(args, 0, "id")?;
    raw.parse()
        .map_err(|_| format!("<id> must be a PipeWire node id, got '{raw}'"))
}

/// Which screen a wallpaper command means: the one named, else the focused one.
///
/// Resolved to a name rather than left as `None`, because `None` means "every screen" to the service and
/// "wherever the user is looking" to a keybind — and a `wallpaper random` bound to a key should change the
/// screen in front of them, not all of them.
///
/// A name that is not a monitor is refused. Accepting one writes an entry into the persisted assignment that
/// no surface will ever read, and answers `ok` while changing nothing — which is how a stray `--features` from
/// a dev harness ended up saved as a screen. It costs one lookup to make a typo say so.
/// Which screen a wallpaper command changes: the one named, or every screen when none is.
///
/// "Every screen" and not "the focused one", because that is what each command's own help says it does, and
/// because the focused-screen reading made `wallpaper clear` unable to do the one thing it exists for: it
/// removed the focused monitor's entry, answered `cleared`, and left every other entry — including one written
/// under a name no monitor has — sitting in the state file with no way left to reach it.
pub(crate) fn target_output(named: Option<&str>) -> Result<Option<String>, String> {
    named.map(validated).transpose()
}

/// Which screen a wallpaper command *reads*: the one named, else the focused one. A reading has to be about
/// some screen, so here the focused one is the only sensible default.
pub(crate) fn reading_output(named: Option<&str>) -> Result<Option<String>, String> {
    match named {
        Some(name) => validated(name).map(Some),
        None => Ok(shell::focused_output()),
    }
}

/// Which displays a brightness *mutation* means: the one named, `all` of them, or — with nothing named — the
/// primary one.
///
/// Deliberately not the wallpaper rule, where an unnamed mutation means every screen. A wallpaper is one desktop
/// look; brightness is per-panel hardware, and `brightness up` is overwhelmingly a laptop's function key, which
/// means *this* panel. `all` is there for the desk, and both are in the command's help so neither is a surprise.
pub(crate) fn dimmable_targets(named: Option<&str>) -> Result<Vec<String>, String> {
    use services::brightness;
    let snapshot = brightness::snapshot();
    match named {
        Some(name) if name.eq_ignore_ascii_case("all") => {
            let outputs: Vec<String> = snapshot
                .displays
                .iter()
                .map(|display| display.output.clone())
                .collect();
            if outputs.is_empty() {
                return Err("no controllable display".to_string());
            }
            Ok(outputs)
        }
        Some(name) => Ok(vec![dimmable_output(name)?]),
        None => snapshot
            .primary()
            .map(|display| vec![display.output.clone()])
            .ok_or_else(|| "no controllable display".to_string()),
    }
}

/// `name` if a display on it can be dimmed, else an error naming the ones that can.
///
/// Checked against the brightness snapshot rather than against the compositor's outputs: those are two different
/// sets. A monitor with no DDC support is an output that cannot be dimmed, and a DDC monitor whose connector could
/// not be resolved answers to `i2c-6` — a name no compositor has ever heard of.
pub(crate) fn dimmable_output(name: &str) -> Result<String, String> {
    use services::brightness;
    let snapshot = brightness::snapshot();
    if let Some(display) = snapshot.get(name) {
        return Ok(display.output.clone());
    }
    if snapshot.is_empty() {
        return Err(format!(
            "'{name}' has no controllable brightness (nothing on this machine does)"
        ));
    }
    let known: Vec<&str> = snapshot
        .displays
        .iter()
        .map(|display| display.output.as_str())
        .collect();
    Err(format!(
        "'{name}' has no controllable brightness (this machine has: {})",
        known.join(", ")
    ))
}

pub(crate) fn validated(name: &str) -> Result<String, String> {
    let screens: Vec<String> = platform_wayland::outputs()
        .into_iter()
        .filter_map(|output| output.name)
        .collect();
    known_screen(name, &screens)
}

/// `name` if it is one of `screens`, else an error naming the real ones.
pub(crate) fn known_screen(name: &str, screens: &[String]) -> Result<String, String> {
    if screens.iter().any(|screen| screen == name) {
        return Ok(name.to_string());
    }
    if screens.is_empty() {
        return Err(format!("'{name}' is not a monitor (none are connected)"));
    }
    Err(format!(
        "'{name}' is not a monitor (this session has: {})",
        screens.join(", ")
    ))
}

/// Re-derives the dynamic palette after a wallpaper change, once the transition to the new image has finished.
/// A no-op unless `[theme] name = "dynamic"`, so every wallpaper command can call it blind.
pub(crate) fn refresh_scheme() {
    config::scheme::refresh_current();
}

/// The palette as one `name<TAB>#rrggbb` row per token, which is what a script recolouring something else needs.
pub(crate) fn palette_rows(theme: &config::theme::NordTheme) -> String {
    config::theme::THEME_TOKENS
        .iter()
        .map(|name| format!("{name}\t{}", config::theme::hex(theme.token(name))))
        .collect::<Vec<String>>()
        .join("\n")
}

/// Switches the dashboard's page by its config id, refusing an unknown one by name — a keybind bound to a page
/// that was renamed should say so rather than silently leaving the dashboard where it was.
pub(crate) fn set_dashboard_tab(name: &str) -> Result<(), String> {
    use config::DashboardTab;
    let tab = DashboardTab::from_id(name).ok_or_else(|| {
        let known: Vec<&str> = DashboardTab::ALL.iter().map(|t| t.id()).collect();
        format!("unknown tab '{name}', expected one of {}", known.join("|"))
    })?;
    modules::dashboard::set_tab(tab);
    Ok(())
}
