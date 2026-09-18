//! What the user's config asks for that the shell cannot do, and where the file says it.
//!
//! Each name is asked of the owner that knows it, because no one of them knows every id: `config` knows the dashboard's pages, the palettes, accents and colour tokens, and which corners a bar is there to draw; `modules` knows the utility toggles and the status icons; and which ids a bar can hold is the module registry, which exists nowhere below this crate. This file asks each of them, adds where each answer sits in the text, and keeps the user told.
//!
//! **Building a report runs nothing and changes nothing**, the discipline [`resolves`](crate::core::commands::resolves) keeps for the command table. A module id is looked up with [`ModuleRegistry::def`] and never built, because building a chip is what subscribes its service; and a file is read, never loaded through [`Config::load`], which writes the starter config when there is none — so asking whether a config is fine on a machine that has none would have answered by creating one. The one read that does go through the loader, merging a monitor override over the global config, is only made when both files exist, which is the case where the loader has nothing to write.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::ops::Range;
use std::path::{Path, PathBuf};

use config::{Config, Corner, Edge, GLOBAL_ONLY_SECTIONS, LoadError};
use services::notifications::{Urgency, notify_status, withdraw_status};
use toml::de::{DeTable, DeValue};
use ui::module::ModuleRegistry;
use util::report::{Finding, Report, Span};

/// How many new findings a notification lists before it counts the rest. A card is read at a glance; the full list is what `config check` is for.
const LISTED: usize = 3;

/// Every id on a bar or in a corner that no module answers to — each of which the bar draws as a placeholder.
fn unknown_modules(config: &Config, file: &Path, registry: &ModuleRegistry) -> Report {
    let mut report = Report::default();
    let mut unknown = |key: String, id: &str| {
        if registry.def(id).is_none() {
            report.error(Finding::new(
                file,
                key,
                telar::t!("config.unknown_module", id = id),
            ));
        }
    };
    for edge in Edge::ALL {
        let bar = config.bars.get(edge);
        for (zone, entries) in [
            ("start", &bar.start),
            ("center", &bar.center),
            ("end", &bar.end),
        ] {
            for (index, entry) in entries.iter().enumerate() {
                unknown(format!("bars.{}.{zone}[{index}]", edge.as_str()), &entry.id);
            }
        }
    }
    for corner in Corner::ALL {
        if let Some(id) = config.corners.get(corner) {
            unknown(format!("corners.{}", corner.key()), id);
        }
    }
    report
}

/// What is wrong with a config that has already been parsed, attributed to `file`: every id it names that its owner does not have. Spans are left to whoever has the text.
fn problems(config: &Config, file: &Path, registry: &ModuleRegistry) -> Report {
    let mut report = unknown_modules(config, file, registry);
    report.merge(config.dashboard.check(file));
    report.merge(modules::utilities::check(&config.utilities, file));
    report.merge(modules::statusicons::check(&config.status_icons, file));
    report.merge(config.theme.check(file));
    report
}

/// Every corner that names a module no bar is there to draw: neither edge beside it has a bar, so the module is on no screen at all. A warning rather than an error, because the id may well be fine — it is the placement that goes nowhere.
///
/// `drawn` is the config the screens are actually built from, which for a monitor override is not the file on its own: a corner the override names can be drawn by a bar the global config set, and one the global config names can lose its bar to the override. `global` is that global config, for an override: a corner that already goes nowhere there, with the same module in it, is the global file's to report, and saying it again against every override would name a file that did not cause it.
fn corners_shown_nowhere(drawn: &Config, global: Option<&Config>, file: &Path) -> Report {
    let mut report = Report::default();
    for corner in Corner::ALL {
        let inherited = global.is_some_and(|global| {
            global.corners.get(corner) == drawn.corners.get(corner)
                && global.corner_owner(corner).is_none()
        });
        if let Some(id) = drawn.corners.get(corner)
            && drawn.corner_owner(corner).is_none()
            && !inherited
        {
            report.warn(Finding::new(
                file,
                format!("corners.{}", corner.key()),
                telar::t!("config.corner_nowhere", id = id),
            ));
        }
    }
    report
}

/// The sections a monitor override sets that only `config.toml` may. The loader drops them without a word; the report is where the user hears that it did.
fn global_only(table: &DeTable, file: &Path) -> Report {
    let mut report = Report::default();
    for section in GLOBAL_ONLY_SECTIONS {
        if table.contains_key(*section) {
            report.warn(Finding::new(
                file,
                *section,
                telar::t!("config.global_only", section = section),
            ));
        }
    }
    report
}

/// Everything wrong with one file's text: that it does not parse, or what it names that the shell does not have. `global` is the `config.toml` that `file` overrides, for a `monitors/<output>/config.toml` — which may not set every section, and whose screen is drawn from the two merged.
///
/// Parsed straight into [`Config`] rather than through a `toml::Value` first, as the loader does: the result is the same config, but a type error keeps the place it was made at.
fn check_text(file: &Path, text: &str, registry: &ModuleRegistry, global: Option<&Path>) -> Report {
    let config: Config = match toml::from_str(text) {
        Ok(config) => config,
        Err(error) => {
            let mut report = Report::default();
            report.error(unparsable(file, &error, Some(text)));
            return report;
        }
    };
    let mut report = problems(&config, file, registry);
    match global {
        None => report.merge(corners_shown_nowhere(&config, None, file)),
        Some(global) => {
            if let Some((global, merged)) = merged(global, file) {
                report.merge(corners_shown_nowhere(&merged, Some(&global), file));
            }
        }
    }
    if let Ok(document) = DeTable::parse(text) {
        let document = DeValue::Table(document.into_inner());
        if global.is_some()
            && let DeValue::Table(table) = &document
        {
            report.merge(global_only(table, file));
        }
        for finding in report.findings_mut() {
            if finding.span.is_none() && !finding.key.is_empty() {
                finding.span =
                    span_of(&document, &finding.key).map(|bytes| Span::locate(text, bytes));
            }
        }
    }
    report
}

/// The global config `file` overrides, and the config its screen is drawn from — the global one with the override merged over it, the way the shell builds that screen. `None` when the two do not merge: a global config that is missing or does not parse, which is reported on its own.
///
/// Both files are made sure of first, because the loader falls back to loading the global config when the override cannot be read — and loading one that is missing writes the starter config in its place.
fn merged(global: &Path, file: &Path) -> Option<(Config, Config)> {
    if !global.is_file() || !file.is_file() {
        return None;
    }
    let output = file.parent()?.file_name()?.to_str()?;
    let unmerged = toml::from_str(&std::fs::read_to_string(global).ok()?).ok()?;
    Some((unmerged, Config::for_output(global, Some(output)).ok()?))
}

/// Where the value at a key path like `bars.top.center[1]` sits in a parsed document, or `None` when the path leads nowhere — a finding about a default the file never wrote has nothing in the text to point at.
fn span_of(document: &DeValue, key: &str) -> Option<Range<usize>> {
    let mut at = document;
    let mut span = None;
    for segment in key.split('.') {
        let mut parts = segment.split('[');
        let name = parts.next()?;
        let found = at.get(name)?;
        (at, span) = (found.get_ref(), Some(found.span()));
        for index in parts {
            let found = at.get(index.strip_suffix(']')?.parse::<usize>().ok()?)?;
            (at, span) = (found.get_ref(), Some(found.span()));
        }
    }
    span
}

/// A file whose text is not a config: why, and where it stops making sense when there is `text` to place it in. Keyless, because what is wrong is the file as a whole — which is also how the notice tells a file that was not applied from one that was.
fn unparsable(file: &Path, error: &toml::de::Error, text: Option<&str>) -> Finding {
    let mut finding = Finding::new(file, "", error.message().trim());
    finding.span = text.and_then(|text| error.span().map(|bytes| Span::locate(text, bytes)));
    finding
}

/// A file that is there and could not be read. Keyless for the same reason as [`unparsable`].
fn unreadable(file: &Path, error: &std::io::Error) -> Finding {
    Finding::new(file, "", error.to_string())
}

/// The monitor overrides beside `path` that there is a file to read for.
fn overrides(path: &Path) -> Vec<PathBuf> {
    Config::monitor_overrides(path)
        .into_iter()
        .filter(|file| file.is_file())
        .collect()
}

/// Every override beside `path`, each checked on its own terms.
fn check_overrides(path: &Path, registry: &ModuleRegistry) -> Report {
    let mut report = Report::default();
    for file in overrides(path) {
        match std::fs::read_to_string(&file) {
            Ok(text) => report.merge(check_text(&file, &text, registry, Some(path))),
            Err(error) => report.error(unreadable(&file, &error)),
        }
    }
    report
}

/// The files at `path` as they are on disk, for `hogar-shell config check`: `config.toml` and every monitor override beside it, each read and parsed here. A missing `config.toml` is not a problem — the shell writes its starter config there on first run — and is never created by asking.
pub(crate) fn on_disk(path: &Path, registry: &ModuleRegistry) -> Report {
    let mut report = match std::fs::read_to_string(path) {
        Ok(text) => check_text(path, &text, registry, None),
        Err(error) if error.kind() == ErrorKind::NotFound => Report::default(),
        Err(error) => {
            let mut report = Report::default();
            report.error(unreadable(path, &error));
            report
        }
    };
    report.merge(check_overrides(path, registry));
    report
}

/// What the notice shows while the shell runs `config`: `config.toml` as `config` was loaded from it at `path` — or, when `failed` says it did not load, that failure — and the monitor overrides beside it as they are on disk, against the registry the bars build from.
///
/// **The files, and not what is running, decide what the notice says.** The two differ exactly when `config.toml` did not load: a reload that fails keeps the last config that loaded, and startup, which has none, runs the starter config. `config`'s own problems then describe a text the user has already edited away — fixed, perhaps, in the very save that broke the file — so they are set aside, and the file's one problem stands in for them: it did not load, and why. The overrides are checked on their own terms either way. That is also exactly what `hogar-shell config check` says about the same files, which is where the card sends the user.
///
/// When the file loads again, the reload that loads it reports what it holds. When putting the file back to what is on screen was the whole fix there is no reload to do it, and the reload path asks for this report itself, `failed` gone.
///
/// The loaded config rather than the file when it did load, because it is what the load itself read: reading the file again here would be a second look at a different moment, with room for an edit the shell has not applied yet.
pub fn running(config: &Config, path: &Path, failed: Option<&LoadError>) -> Report {
    ui::module::with_registry(|registry| {
        let mut report = Report::default();
        match failed {
            Some(LoadError::Parse(error)) => report.error(unparsable(path, error, None)),
            Some(LoadError::Io(error)) => report.error(unreadable(path, error)),
            None => {
                report.merge(problems(config, path, registry));
                report.merge(corners_shown_nowhere(config, None, path));
            }
        }
        report.merge(check_overrides(path, registry));
        report
    })
}

/// `hogar-shell config check`: the report on what is at `path`, failing when there is an error so a script can branch on it, and saying plainly when there is nothing wrong.
pub(crate) fn command(path: &Path) -> Result<String, String> {
    let registry =
        crate::core::registry::default_registry(&crate::core::popouts::default_popouts());
    let report = on_disk(path, &registry);
    if report.is_clean() {
        return Ok(nothing_wrong(path));
    }
    let text = format!("{}\n{}", report.summary(), report.render());
    if report.errors.is_empty() {
        Ok(text)
    } else {
        Err(text.trim_end().to_string())
    }
}

/// What `config check` says when it has nothing to report, naming what it read — so "nothing is wrong" cannot be mistaken for "nothing was looked at".
fn nothing_wrong(path: &Path) -> String {
    let beside = match overrides(path).len() {
        0 => String::new(),
        1 => " or the monitor override beside it".to_string(),
        n => format!(" or the {n} monitor overrides beside it"),
    };
    if path.exists() {
        format!("nothing is wrong with {}{beside}\n", path.display())
    } else {
        format!(
            "there is no {} yet — the shell writes its starter config there on first run — and nothing is wrong \
             with that{beside}\n",
            path.display()
        )
    }
}

/// What makes two findings the same problem: the file and what is wrong, not where in the file it is. Moving a module in front of a misspelt one shifts its index, and a notice redrawn every time a list was reordered would be a notice about the reordering.
fn identity(finding: &Finding) -> (PathBuf, String) {
    (finding.file.clone(), finding.message.clone())
}

/// What [`announce`] needs from a notification daemon: the freedesktop pair of raising a notice — or replacing the one named — and taking it down.
trait Notices {
    fn post(
        &mut self,
        replaces: Option<u32>,
        summary: &str,
        body: &str,
        urgency: Urgency,
    ) -> Option<u32>;
    fn withdraw(&mut self, id: u32);
}

/// The shell's own notification daemon.
struct Daemon;

impl Notices for Daemon {
    fn post(
        &mut self,
        replaces: Option<u32>,
        summary: &str,
        body: &str,
        urgency: Urgency,
    ) -> Option<u32> {
        notify_status(
            replaces,
            "hogar-shell",
            summary,
            body,
            "dialog-warning",
            urgency,
        )
    }

    fn withdraw(&mut self, id: u32) {
        withdraw_status(id);
    }
}

/// The config-problems notice as it stands: which of the daemon's notices it is, and the problems it shows.
#[derive(Default)]
struct Notice {
    id: Option<u32>,
    showing: HashSet<(PathBuf, String)>,
}

thread_local! {
    static NOTICE: RefCell<Notice> = RefCell::new(Notice::default());
}

/// Starts this thread's notice over. It is per thread, and a test that inherited another's would be asked about a card it never raised.
#[cfg(test)]
pub(crate) fn fresh_notice() {
    NOTICE.with(|notice| *notice.borrow_mut() = Notice::default());
}

/// The messages the notice is showing, sorted: for a test driving the reload path, which reaches the notice through [`announce`] and a daemon that is not running, so this is the only record of what the card says.
#[cfg(test)]
pub(crate) fn showing() -> Vec<String> {
    let mut messages: Vec<String> = NOTICE.with(|notice| {
        notice
            .borrow()
            .showing
            .iter()
            .map(|(_, message)| message.clone())
            .collect()
    });
    messages.sort();
    messages
}

/// Keeps one notice up that shows what is wrong with the config now: raised with the first problem, redrawn in place whenever the set of problems changes, and withdrawn once there are none. Nothing at all while the set is the one it already shows.
///
/// **One card, redrawn, rather than one per change.** Every save from the settings window reloads the config, and a field being typed into saves at every pause — so a card per change was a stack of cards for `p`, `pe` and `per`, each about a moment already gone. Replaced in place, the partial ids pass through a single card, and it goes away when the id is whole.
///
/// **What counts as a change is the set of problems** as [`identity`] sees them: what is wrong and in which file, not where in it. Losing a problem is a change as much as gaining one, since the card shows the current set and has to stop showing the one that went. A reordered list is not a change, which is also why the card names no key paths — it would go stale on the very reorders it deliberately ignores; `hogar-shell config check` has the locations. A language switch is one, since every message changes with it, and the card is redrawn in the new language.
///
/// Each problem is logged once, when it first appears — what replaces the warning the bar, the dashboard and the grid each used to log on every build, and the loader at every merge of a monitor override.
///
/// **A file that did not load is on this card too, and changes how it reads.** A file that does not parse or cannot be read was not applied at all — the edit the user just made had no effect — so while one is on the card it is titled as a config not applied, and is critical, waiting to be read. Otherwise nothing stopped the config from applying: the card is normal, retires to the history on its own and waits there to be redrawn or withdrawn. Either way it is the same card, never a second one beside it — so an editor saving broken text at every pause redraws one card rather than stacking one per save, and the card goes when the file loads again with nothing else wrong in it.
pub fn announce(report: &Report) {
    announce_to(report, &mut Daemon);
}

fn announce_to(report: &Report, notices: &mut impl Notices) {
    let current: HashSet<(PathBuf, String)> = report.findings().map(identity).collect();
    NOTICE.with(|notice| {
        let mut notice = notice.borrow_mut();
        if current == notice.showing {
            return;
        }
        let mut logged = HashSet::new();
        for finding in report.findings() {
            let problem = identity(finding);
            if !notice.showing.contains(&problem) && logged.insert(problem) {
                tracing::warn!("{}: {finding}", finding.location());
            }
        }
        if current.is_empty() {
            if let Some(id) = notice.id.take() {
                notices.withdraw(id);
            }
        } else if report.errors.iter().any(|finding| finding.key.is_empty()) {
            notice.id = notices.post(
                notice.id,
                &telar::t!("config.error_title"),
                &card(report),
                Urgency::Critical,
            );
        } else {
            notice.id = notices.post(
                notice.id,
                &telar::t!("config.problems_title"),
                &card(report),
                Urgency::Normal,
            );
        }
        notice.showing = current;
    });
}

/// The notice's text: each problem once, by what is wrong, then how many more there are and where to find them all.
fn card(report: &Report) -> String {
    let mut seen = HashSet::new();
    let problems: Vec<&str> = report
        .findings()
        .map(|finding| finding.message.as_str())
        .filter(|message| seen.insert(*message))
        .collect();
    let mut lines: Vec<String> = problems
        .iter()
        .take(LISTED)
        .map(|message| message.to_string())
        .collect();
    if problems.len() > LISTED {
        lines.push(telar::t!(
            "config.problems_more",
            count = problems.len() - LISTED
        ));
    }
    lines.push(telar::t!("config.problems_hint"));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ModuleRegistry {
        crate::core::registry::default_registry(&crate::core::popouts::default_popouts())
    }

    /// A config written by hand with one of each mistake the report knows about, beside ids that are fine, so a report that named the wrong entry — or every entry in a list with one bad one — fails here.
    const FIXTURE: &str = r#"[bars.top]
start = ["workspaces"]
center = ["clock", "clokc"]
end = ["notes"]

[dashboard]
tabs = ["dash", "wether"]

[utilities]
toggles = ["wifi", "teleporter"]
"#;

    /// The report, snapshotted: one entry each for the module, the tab and the toggle, at the line and column the id is written, in the order `config check` prints them — and nothing for the ids that are fine.
    #[test]
    fn a_config_with_an_unknown_module_tab_and_toggle_reports_exactly_those_three() {
        telar::set_locale("en");
        let report = check_text(Path::new("config.toml"), FIXTURE, &registry(), None);

        assert_eq!(
            report.render(),
            "config.toml:3:20: error: bars.top.center[1]: there is no module called 'clokc'\n\
             config.toml:7:17: error: dashboard.tabs[1]: the dashboard has no page called 'wether'\n\
             config.toml:10:20: error: utilities.toggles[1]: there is no toggle called 'teleporter'\n",
            "each of the three is named once, where it is written, by the owner that knows it is wrong"
        );
    }

    /// The other ids that used to vanish without a word, one of each, beside names that are fine: a status icon the cluster has none for, a theme, accent and `[theme.colors]` token the palette has none for, and a corner module on a corner no bar runs along — which is on no screen at all.
    const MORE_DROPS: &str = r##"[bars.left]
start = ["workspaces"]

[corners]
top_left = "clock"
bottom_right = "notes"

[status_icons]
icons = ["volume", "wfi"]

[theme]
name = "gruvbx"
accent = "cyna"

[theme.colors]
base = "#2e3440"
bse = "#2e3440"
"##;

    /// Each is reported once, at the line it is written on, by the owner that knows the name: the cluster for its icons, the palette for its names and tokens, the config's own corner routing for where a corner lands. The theme and the accent are warnings, since the shell puts a palette and an accent of its own in their place; the corner is one too — `clock` is a real module, it is the placement that reaches no screen — and the other corner, on a left bar, is not reported at all.
    #[test]
    fn every_other_silent_drop_is_reported_where_it_is_written() {
        telar::set_locale("en");
        let report = check_text(Path::new("config.toml"), MORE_DROPS, &registry(), None);

        assert_eq!(
            report.render(),
            "config.toml:9:20: error: status_icons.icons[1]: there is no status icon called 'wfi'\n\
             config.toml:17:7: error: theme.colors.bse: there is no colour token called 'bse', so this colour is not applied\n\
             config.toml:12:8: warning: theme.name: there is no theme called 'gruvbx', so the shell uses nord\n\
             config.toml:13:10: warning: theme.accent: there is no accent called 'cyna', so the palette's own accent is used\n\
             config.toml:6:16: warning: corners.bottom_right: no bar runs along either edge of this corner, so 'notes' is shown nowhere\n",
            "one entry per name nothing answers to, and one for the corner that reaches no screen"
        );
    }

    /// Where a corner module lands on a monitor with an override is decided by the two files merged, so it is checked against the merge: a corner the override names can be drawn by a bar the global config set, and a bar the override takes away can leave the global config's corner with nowhere to go — which is the override's doing, and reported against it. A corner that already goes nowhere in the global config is that file's problem, and reported there alone.
    #[test]
    fn a_corner_in_a_monitor_override_is_checked_against_the_merged_config() {
        telar::set_locale("en");
        let dir = std::env::temp_dir().join(format!("hogar-shell-corners-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("monitors/DP-1")).expect("a scratch directory");
        std::fs::create_dir_all(dir.join("monitors/HDMI-A-1")).expect("a scratch directory");
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[bars.top]\ncenter = [\"clock\"]\n\n[corners]\ntop_right = \"notes\"\nbottom_left = \"clock\"\n",
        )
        .expect("a global config");
        std::fs::write(
            dir.join("monitors/DP-1/config.toml"),
            "[corners]\ntop_left = \"clock\"\n",
        )
        .expect("an override naming a corner the global top bar draws");
        std::fs::write(
            dir.join("monitors/HDMI-A-1/config.toml"),
            "[bars.top]\ncenter = []\n",
        )
        .expect("an override taking the top bar away");

        let report = on_disk(&path, &registry());
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            report
                .warnings
                .iter()
                .map(|finding| {
                    (
                        finding
                            .file
                            .strip_prefix(&dir)
                            .unwrap()
                            .display()
                            .to_string(),
                        finding.key.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            [
                ("config.toml".to_string(), "corners.bottom_left"),
                (
                    "monitors/HDMI-A-1/config.toml".to_string(),
                    "corners.top_right"
                ),
            ],
            "DP-1's corner is drawn by the global top bar; HDMI-A-1 took that bar away from the global corner, and is \
             reported for it; and the corner that goes nowhere in the global config is reported there once, not again \
             against every override that inherits it"
        );
    }

    /// The configs a user is handed — the starter bar a first run writes and the annotated file `config schema` prints — must check clean, or the first thing `config check` says to a new user is that the shell's own defaults are wrong.
    #[test]
    fn the_configs_the_shell_hands_out_report_nothing() {
        let starter = toml::to_string_pretty(&Config::starter()).expect("the starter serialises");
        let schema = config::schema::render(None).expect("the schema renders");
        for (name, text) in [("starter", starter), ("schema", schema)] {
            let report = check_text(Path::new("config.toml"), &text, &registry(), None);
            assert!(
                report.is_clean(),
                "the {name} config reports:\n{}",
                report.render()
            );
        }
    }

    thread_local! {
        static BUILT: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    fn built(
        _ctx: &ui::module::ModuleCtx,
    ) -> Result<Box<dyn telar::LayoutItem>, telar::LayoutError> {
        BUILT.with(|built| built.borrow_mut().push("clock"));
        Err(telar::LayoutError::Engine(
            "a report must never build a module".into(),
        ))
    }

    /// Asking is not doing. A module built to see whether it exists is a module subscribed to its service, and a config loaded to see whether it parses is a starter config written to a machine that had none — so the report looks ids up, reads files, and does neither.
    #[test]
    fn building_a_report_runs_nothing_and_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("hogar-shell-check-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("monitors/DP-1")).expect("a scratch directory");
        let path = dir.join("config.toml");
        let _ = std::fs::remove_file(&path);
        std::fs::write(
            dir.join("monitors/DP-1/config.toml"),
            "[bars.left]\nstart = [\"clock\", \"clokc\"]\n",
        )
        .expect("an override to read");
        let mut registry = ModuleRegistry::new();
        registry.register("clock", ui::module::ModuleDef::new(built));

        let report = on_disk(&path, &registry);
        ui::module::install(registry);
        let running = running(&Config::starter(), &path, None);

        assert!(
            BUILT.with(|built| built.borrow().is_empty()),
            "checking an id built its module, which subscribes whatever service it reads"
        );
        assert!(
            !path.exists(),
            "checking a config that is not there created one — the loader writes the starter config, and a check must not"
        );
        assert_eq!(
            report
                .errors
                .iter()
                .map(|finding| finding.key.as_str())
                .collect::<Vec<_>>(),
            ["bars.left.start[1]"],
            "the override was read all the same, and only its unknown id reported"
        );
        assert_eq!(
            running
                .errors
                .iter()
                .filter(|finding| finding.key == "bars.left.start[1]")
                .count(),
            1,
            "the running shell's report reads the overrides too, since they are what its other screens draw"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `config check` answers a script as well as a person: it fails on an error and only on one, and when there is nothing to report it says so and names what it read — the way `deps` says "nothing is missing" rather than printing nothing.
    #[test]
    fn config_check_fails_on_an_error_and_says_plainly_when_nothing_is_wrong() {
        telar::set_locale("en");
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-check-command-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("config.toml");
        let _ = std::fs::remove_file(&path);

        let absent = command(&path).expect("a config that is not there yet is not an error");
        assert!(
            absent.starts_with("there is no ") && !path.exists(),
            "a missing config is said to be missing, and is not created by asking: {absent}"
        );

        std::fs::write(&path, "[bars.top]\ncenter = [\"clock\"]\n").expect("a config to check");
        assert_eq!(
            command(&path),
            Ok(format!("nothing is wrong with {}\n", path.display())),
            "a clean config gets a sentence, not silence"
        );

        std::fs::write(&path, "[dashboard]\ntabs = []\n").expect("a config to check");
        let warned = command(&path).expect("a warning alone does not fail the check");
        assert!(
            warned.starts_with("1 warning\n"),
            "the fallback to every page is reported: {warned}"
        );

        std::fs::write(&path, "[dashboard]\ntabs = [\"wether\", \"dash\"]\n")
            .expect("a config to check");
        let failed = command(&path).expect_err("an unknown page is an error");
        assert!(
            failed.starts_with("1 error\n") && failed.contains("dashboard.tabs[0]"),
            "the failure says how much is wrong and where: {failed}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only what the file actually wrote has a place in it: a monitor override that names `[general]` gets a warning pointing at it, and the section it may set gets nothing.
    #[test]
    fn a_monitor_override_setting_a_global_section_is_warned_about() {
        telar::set_locale("en");
        let report = check_text(
            Path::new("monitors/DP-1/config.toml"),
            "[general]\nlanguage = \"es\"\n\n[bars.top]\ncenter = [\"clock\"]\n",
            &registry(),
            Some(Path::new("config.toml")),
        );

        assert!(report.errors.is_empty(), "{}", report.render());
        assert_eq!(
            report.render(),
            "monitors/DP-1/config.toml:1:1: warning: general: [general] can only be set in config.toml, so a monitor override's copy of it is ignored\n",
            "the override's [general] is dropped by the loader, which only ever said so in its log"
        );
    }

    #[test]
    fn a_file_that_does_not_parse_is_one_error_at_the_place_it_breaks() {
        let report = check_text(
            Path::new("config.toml"),
            "[bars.top]\nsize = \"tall\"\n",
            &registry(),
            None,
        );

        assert_eq!(report.errors.len(), 1, "{}", report.render());
        let finding = &report.errors[0];
        assert_eq!(
            finding.span.as_ref().map(|span| span.line),
            Some(2),
            "a type error keeps its line, which is what parsing straight into the config buys"
        );
    }

    /// A daemon that keeps its cards the way the shell's does: a post naming a card redraws it where it stands, a post naming none adds one, and a withdrawal takes one down. `services::notifications` pins the real daemon to the same three rules; this one lets the notice be driven without claiming the session's notification bus name.
    #[derive(Default)]
    struct Cards {
        shown: Vec<(u32, String)>,
        next: u32,
        posts: usize,
        /// The title and urgency of the latest post.
        heading: Option<(String, Urgency)>,
    }

    impl Notices for Cards {
        fn post(
            &mut self,
            replaces: Option<u32>,
            summary: &str,
            body: &str,
            urgency: Urgency,
        ) -> Option<u32> {
            self.posts += 1;
            self.heading = Some((summary.to_string(), urgency));
            let id = replaces.unwrap_or_else(|| {
                self.next += 1;
                self.next
            });
            match self.shown.iter_mut().find(|(shown, _)| *shown == id) {
                Some(card) => card.1 = body.to_string(),
                None => self.shown.push((id, body.to_string())),
            }
            Some(id)
        }

        fn withdraw(&mut self, id: u32) {
            self.shown.retain(|(shown, _)| *shown != id);
        }
    }

    /// What the dashboard reports while a page name is being typed into the settings window, one save per pause.
    fn typing(page: &str) -> Report {
        config::DashboardConfig {
            tabs: vec!["dash".to_string(), page.to_string()],
            ..config::DashboardConfig::default()
        }
        .check(Path::new("config.toml"))
    }

    /// The settings window saves at every pause in typing, and each save reloads the config — so `performance`, typed slowly, is three unknown pages on the way to a known one. One card follows them, showing only the latest, and goes away when the name is whole.
    #[test]
    fn typing_through_three_partial_ids_leaves_one_card_showing_the_last() {
        telar::set_locale("en");
        fresh_notice();
        let mut daemon = Cards::default();

        for partial in ["p", "pe", "per"] {
            announce_to(&typing(partial), &mut daemon);
        }
        assert_eq!(
            daemon.shown.len(),
            1,
            "three partial ids are one card, not a stack of three"
        );
        let body = &daemon.shown[0].1;
        assert!(
            body.contains("'per'") && !body.contains("'pe'") && !body.contains("'p'"),
            "and the card shows the last of them only: {body}"
        );

        announce_to(&typing("performance"), &mut daemon);
        assert!(
            daemon.shown.is_empty(),
            "a config with nothing wrong in it takes the card down"
        );
    }

    /// The card shows the current set, so it is redrawn whenever the set changes — including when a problem goes away, or it would go on showing one that was fixed — and never when it has not, since every save reloads.
    #[test]
    fn the_card_is_redrawn_when_the_set_of_problems_changes_and_only_then() {
        telar::set_locale("en");
        fresh_notice();
        let mut daemon = Cards::default();
        let both = config::DashboardConfig {
            tabs: vec!["wether".to_string(), "dash".to_string(), "meda".to_string()],
            ..config::DashboardConfig::default()
        };
        let path = Path::new("config.toml");

        announce_to(&both.check(path), &mut daemon);
        announce_to(&both.check(path), &mut daemon);
        assert_eq!(
            daemon.posts, 1,
            "the same problems after another save leave the card alone"
        );

        let reordered = config::DashboardConfig {
            tabs: vec!["dash".to_string(), "meda".to_string(), "wether".to_string()],
            ..config::DashboardConfig::default()
        };
        announce_to(&reordered.check(path), &mut daemon);
        assert_eq!(
            daemon.posts, 1,
            "nor does moving them around the list, which changes where they are and not what is wrong"
        );

        announce_to(&typing("meda"), &mut daemon);
        assert_eq!(daemon.posts, 2, "fixing one of two is a change");
        assert!(
            !daemon.shown[0].1.contains("'wether'") && daemon.shown[0].1.contains("'meda'"),
            "and the card stops showing the one that was fixed: {}",
            daemon.shown[0].1
        );

        telar::set_locale("es");
        announce_to(&typing("meda"), &mut daemon);
        telar::set_locale("en");
        assert_eq!(
            daemon.posts, 3,
            "a language switch rewrites every message, and the card follows it into the new language"
        );
        assert_eq!(daemon.shown.len(), 1, "still one card throughout");
    }

    /// **A file that does not load is one of the problems on the one card, and loading again takes it off, leaving whatever else is still wrong.** It used to be a card of its own: a new one for every broken save, critical, and never withdrawn once the file was fixed. Now it is on the card the other problems are on — titled as a config not applied, and critical, while it lasts — and the fix redraws that card rather than adding one.
    #[test]
    fn a_parse_failure_shows_on_the_one_card_and_a_fix_takes_it_off_leaving_the_rest() {
        telar::set_locale("en");
        fresh_notice();
        ui::module::install(registry());
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-check-unloaded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("monitors/DP-1")).expect("a scratch directory");
        let path = dir.join("config.toml");
        let over = dir.join("monitors/DP-1/config.toml");
        std::fs::write(&path, "[bars.top]\ncenter = [\"clock\"]\n").expect("a config");
        std::fs::write(&over, "[bars.left]\nstart = [\"clokc\"]\n").expect("an override");
        let last_good = Config::load(&path).expect("the last config that loaded");
        std::fs::write(&path, "[bars.top\ncenter = ").expect("a broken save");
        let failed = Config::load(&path).expect_err("a file that does not parse");
        let LoadError::Parse(error) = &failed else {
            panic!("a syntax error is a parse failure: {failed}");
        };
        let why = error.message().trim().to_string();
        let mut daemon = Cards::default();

        announce_to(&running(&last_good, &path, Some(&failed)), &mut daemon);
        assert_eq!(
            daemon.shown.len(),
            1,
            "the failure and the override's problem are one card"
        );
        let body = &daemon.shown[0].1;
        assert!(
            body.contains(&why) && body.contains("'clokc'"),
            "showing both: {body}"
        );
        assert_eq!(
            daemon.heading,
            Some((telar::t!("config.error_title"), Urgency::Critical)),
            "titled as the config not applied, and waiting to be read"
        );

        std::fs::write(&path, "[bars.top]\ncenter = [\"clock\"]\n").expect("the fix");
        let fixed = Config::load(&path).expect("the fixed file loads");
        announce_to(&running(&fixed, &path, None), &mut daemon);
        assert_eq!(daemon.shown.len(), 1, "the fix redraws the same card");
        let body = &daemon.shown[0].1;
        assert!(
            !body.contains(&why) && body.contains("'clokc'"),
            "without the failure, and with the override's problem, which the fix did not touch: {body}"
        );
        assert_eq!(
            daemon.heading,
            Some((telar::t!("config.problems_title"), Urgency::Normal)),
            "and back to a card about problems in a config that applied"
        );

        std::fs::remove_file(&over).expect("the override's fix");
        announce_to(&running(&fixed, &path, None), &mut daemon);
        assert!(
            daemon.shown.is_empty(),
            "with nothing left wrong, the card is taken down"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_notice_speaks_the_users_language() {
        for (locale, title) in [
            ("en", "Problems in the configuration"),
            ("es", "Problemas en la configuración"),
        ] {
            telar::set_locale(locale);
            assert_eq!(telar::t!("config.problems_title"), title);
            assert!(!telar::t!("config.problems_hint").is_empty());
            assert!(telar::t!("config.problems_more", count = 2).contains('2'));
            assert!(telar::t!("config.unknown_module", id = "x").contains("'x'"));
            assert!(telar::t!("config.global_only", section = "general").contains("[general]"));
            assert!(telar::t!("config.corner_nowhere", id = "x").contains("'x'"));
        }
        telar::set_locale("en");
    }
}
