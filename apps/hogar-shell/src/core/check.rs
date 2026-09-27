//! Checks what the config file says without running or loading anything, since [`Config::load`] writes a starter config when none exists.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::ops::Range;
use std::path::{Path, PathBuf};

use config::{Config, GLOBAL_ONLY_SECTIONS, LoadError};
use services::notifications::{Urgency, notify_status, withdraw_status};
use toml::de::{DeTable, DeValue};
use util::report::{Finding, Report, Span};

/// How many new findings a notification lists before it counts the rest. A card is read at a glance; the full list is what `config check` is for.
const LISTED: usize = 3;

/// What is wrong with a config that has already been parsed, attributed to `file`: every id it names that its owner does not have. Spans are left to whoever has the text.
fn problems(config: &Config, file: &Path) -> Report {
    let mut report = config.dashboard.check(file);
    report.merge(modules::utilities::check(&config.utilities, file));
    report.merge(modules::statusicons::check(&config.status_icons, file));
    report.merge(config.theme.check(file));
    report
}

/// Keys that said where something is drawn, which the layout says now. Each is an error rather than something ignored: a user editing one would otherwise see nothing happen.
pub(crate) const RETIRED: &[&str] = &[
    "bars",
    "corners",
    "widgets",
    "general.show_over_fullscreen",
    "stack.edge",
    "stack.align",
    "stack.width",
];

fn retired_in(file: &Path, text: &str) -> Report {
    let Ok(document) = DeTable::parse(text) else {
        return Report::default();
    };
    let document = DeValue::Table(document.into_inner());
    let mut report = retired(&document, file);
    for finding in report.findings_mut() {
        finding.span = span_of(&document, &finding.key).map(|bytes| Span::locate(text, bytes));
    }
    report
}

fn retired(document: &DeValue, file: &Path) -> Report {
    let mut report = Report::default();
    let layout = crate::core::layouts::file_for_edits();
    for key in RETIRED {
        if span_of(document, key).is_some() {
            report.error(Finding::new(
                file,
                *key,
                telar::t!(
                    "config.moved_to_layout",
                    key = key,
                    layout = layout.display().to_string()
                ),
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
fn check_text(file: &Path, text: &str, global: Option<&Path>) -> Report {
    let config: Config = match toml::from_str(text) {
        Ok(config) => config,
        Err(error) => {
            let mut report = Report::default();
            report.error(unparsable(file, &error, Some(text)));
            return report;
        }
    };
    let mut report = problems(&config, file);
    if let Ok(document) = DeTable::parse(text) {
        let document = DeValue::Table(document.into_inner());
        report.merge(retired(&document, file));
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
fn check_overrides(path: &Path) -> Report {
    let mut report = Report::default();
    for file in overrides(path) {
        match std::fs::read_to_string(&file) {
            Ok(text) => report.merge(check_text(&file, &text, Some(path))),
            Err(error) => report.error(unreadable(&file, &error)),
        }
    }
    report
}

/// The files at `path` as they are on disk, for `hogar-shell config check`: `config.toml` and every monitor override beside it, each read and parsed here. A missing `config.toml` is not a problem — the shell writes its starter config there on first run — and is never created by asking.
pub(crate) fn on_disk(path: &Path) -> Report {
    let mut report = match std::fs::read_to_string(path) {
        Ok(text) => check_text(path, &text, None),
        Err(error) if error.kind() == ErrorKind::NotFound => Report::default(),
        Err(error) => {
            let mut report = Report::default();
            report.error(unreadable(path, &error));
            report
        }
    };
    report.merge(check_overrides(path));
    report
}

/// Reports config.toml and the monitor overrides' state; on a failed reload, reports the file's own failure rather than the still-running config, since that config describes a text the user has already edited away.
pub fn running(config: &Config, path: &Path, failed: Option<&LoadError>) -> Report {
    let mut report = Report::default();
    match failed {
        Some(LoadError::Parse(error)) => report.error(unparsable(path, error, None)),
        Some(LoadError::Io(error)) => report.error(unreadable(path, error)),
        None => {
            report.merge(problems(config, path));
            if let Ok(text) = std::fs::read_to_string(path) {
                report.merge(retired_in(path, &text));
            }
        }
    }
    report.merge(check_overrides(path));
    report
}

/// `hogar-shell config check`: the report on what is at `path`, failing when there is an error so a script can branch on it, and saying plainly when there is nothing wrong.
pub(crate) fn command(path: &Path) -> Result<String, String> {
    let report = on_disk(path);
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

    /// A config written by hand with one mistake per section the report still knows about, beside ids that are fine, so a report that named the wrong entry — or every entry in a list with one bad one — fails here.
    const FIXTURE: &str = r#"[dashboard]
tabs = ["dash", "wether"]

[utilities]
toggles = ["wifi", "teleporter"]
"#;

    /// The report, snapshotted: one entry each for the tab and the toggle, at the line and column the id is written, in the order `config check` prints them — and nothing for the ids that are fine.
    #[test]
    fn a_config_with_an_unknown_tab_and_toggle_reports_both() {
        telar::set_locale("en");
        let report = check_text(Path::new("config.toml"), FIXTURE, None);

        assert_eq!(
            report.render(),
            "config.toml:2:17: error: dashboard.tabs[1]: the dashboard has no page called 'wether'\n\
             config.toml:5:20: error: utilities.toggles[1]: there is no toggle called 'teleporter'\n",
            "each is named once, where it is written, by the owner that knows it is wrong"
        );
    }

    /// The other ids that used to vanish without a word, one of each, beside names that are fine: a status icon the cluster has none for, and a theme, accent and `[theme.colors]` token the palette has none for.
    const MORE_DROPS: &str = r##"[status_icons]
icons = ["volume", "wfi"]

[theme]
name = "gruvbx"
accent = "cyna"

[theme.colors]
base = "#2e3440"
bse = "#2e3440"
"##;

    /// Each is reported once, at the line it is written on, by the owner that knows the name: the cluster for its icons, the palette for its names and tokens. The theme and the accent are warnings, since the shell puts a palette and an accent of its own in their place.
    #[test]
    fn every_other_silent_drop_is_reported_where_it_is_written() {
        telar::set_locale("en");
        let report = check_text(Path::new("config.toml"), MORE_DROPS, None);

        assert_eq!(
            report.render(),
            "config.toml:2:20: error: status_icons.icons[1]: there is no status icon called 'wfi'\n\
             config.toml:10:7: error: theme.colors.bse: there is no colour token called 'bse', so this colour is not applied\n\
             config.toml:5:8: warning: theme.name: there is no theme called 'gruvbx', so the shell uses nord\n\
             config.toml:6:10: warning: theme.accent: there is no accent called 'cyna', so the palette's own accent is used\n",
            "one entry per name nothing answers to"
        );
    }

    /// Every key that used to say where something is drawn is now the layout's to say, so writing one is an error rather than something silently followed, and it names the layout file to move it to.
    #[test]
    fn a_config_naming_a_retired_layout_key_reports_it_at_the_layout_file() {
        telar::set_locale("en");
        let layout = crate::core::layouts::file_for_edits().display().to_string();
        let text = r#"[bars.top]
start = ["workspaces"]

[corners]
top_left = "clock"

[widgets]
clock = "top"

[general]
show_over_fullscreen = true

[stack]
edge = "top"
align = "start"
width = 320
"#;
        let report = check_text(Path::new("config.toml"), text, None);

        assert_eq!(
            report.errors.len(),
            RETIRED.len(),
            "one error per retired key: {}",
            report.render()
        );
        for key in RETIRED {
            let finding = report
                .errors
                .iter()
                .find(|finding| finding.key == *key)
                .unwrap_or_else(|| panic!("no finding for '{key}' in:\n{}", report.render()));
            assert!(
                finding.span.is_some(),
                "'{key}' is placed where it is written"
            );
            assert_eq!(
                finding.message,
                telar::t!(
                    "config.moved_to_layout",
                    key = *key,
                    layout = layout.as_str()
                ),
                "'{key}' names the layout file to move it to"
            );
        }
        assert_eq!(
            report.render(),
            format!(
                "config.toml:1:2: error: bars: {}\n\
                 config.toml:4:1: error: corners: {}\n\
                 config.toml:7:1: error: widgets: {}\n\
                 config.toml:11:24: error: general.show_over_fullscreen: {}\n\
                 config.toml:14:8: error: stack.edge: {}\n\
                 config.toml:15:9: error: stack.align: {}\n\
                 config.toml:16:9: error: stack.width: {}\n",
                telar::t!(
                    "config.moved_to_layout",
                    key = "bars",
                    layout = layout.as_str()
                ),
                telar::t!(
                    "config.moved_to_layout",
                    key = "corners",
                    layout = layout.as_str()
                ),
                telar::t!(
                    "config.moved_to_layout",
                    key = "widgets",
                    layout = layout.as_str()
                ),
                telar::t!(
                    "config.moved_to_layout",
                    key = "general.show_over_fullscreen",
                    layout = layout.as_str()
                ),
                telar::t!(
                    "config.moved_to_layout",
                    key = "stack.edge",
                    layout = layout.as_str()
                ),
                telar::t!(
                    "config.moved_to_layout",
                    key = "stack.align",
                    layout = layout.as_str()
                ),
                telar::t!(
                    "config.moved_to_layout",
                    key = "stack.width",
                    layout = layout.as_str()
                ),
            ),
            "the exact rendering `config check` prints, one line per retired key"
        );
    }

    /// A file that never wrote any of the retired keys reports none of them.
    #[test]
    fn a_config_naming_no_retired_key_reports_none() {
        telar::set_locale("en");
        let report = retired(
            &{
                let document = toml::de::DeTable::parse("[dashboard]\ntabs = [\"dash\"]\n")
                    .expect("valid toml");
                toml::de::DeValue::Table(document.into_inner())
            },
            Path::new("config.toml"),
        );
        assert!(report.is_clean(), "{}", report.render());
    }

    /// A monitor override can write a retired key just as easily as `config.toml`, and is checked the same way, against the file it is actually in.
    #[test]
    fn a_monitor_override_naming_a_retired_key_is_reported_too() {
        telar::set_locale("en");
        let layout = crate::core::layouts::file_for_edits().display().to_string();
        let report = check_text(
            Path::new("monitors/DP-1/config.toml"),
            "[bars.top]\nstart = [\"clock\"]\n",
            Some(Path::new("config.toml")),
        );

        assert_eq!(
            report.render(),
            format!(
                "monitors/DP-1/config.toml:1:2: error: bars: {}\n",
                telar::t!(
                    "config.moved_to_layout",
                    key = "bars",
                    layout = layout.as_str()
                )
            ),
        );
    }

    /// The configs a user is handed — the starter bar a first run writes and the annotated file `config schema` prints — must check clean, or the first thing `config check` says to a new user is that the shell's own defaults are wrong.
    #[test]
    fn the_configs_the_shell_hands_out_report_nothing() {
        let starter = toml::to_string_pretty(&Config::starter()).expect("the starter serialises");
        let schema = config::schema::render(None).expect("the schema renders");
        for (name, text) in [("starter", starter), ("schema", schema)] {
            let report = check_text(Path::new("config.toml"), &text, None);
            assert!(
                report.is_clean(),
                "the {name} config reports:\n{}",
                report.render()
            );
        }
    }

    /// `config check` answers a script as well as a person: it fails on an error and only on one, and when there is nothing to report it says so and names what it read — the way `deps` says "nothing is missing" rather than printing nothing.
    #[test]
    fn config_check_fails_on_an_error_and_says_plainly_when_nothing_is_wrong() {
        telar::set_locale("en");
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("check-command");
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = dir.join("config.toml");
        let _ = std::fs::remove_file(&path);

        let absent = command(&path).expect("a config that is not there yet is not an error");
        assert!(
            absent.starts_with("there is no ") && !path.exists(),
            "a missing config is said to be missing, and is not created by asking: {absent}"
        );

        std::fs::write(&path, "[dashboard]\ntabs = [\"dash\"]\n").expect("a config to check");
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
            "[general]\nlanguage = \"es\"\n\n[dashboard]\ntabs = [\"dash\"]\n",
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
        let report = check_text(Path::new("config.toml"), "[shape]\ngap = \"tall\"\n", None);

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
        let dir = util::paths::isolated_root()
            .expect("a test process resolves under its scratch root")
            .join("check-unloaded");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("monitors/DP-1")).expect("a scratch directory");
        let path = dir.join("config.toml");
        let over = dir.join("monitors/DP-1/config.toml");
        std::fs::write(&path, "[dashboard]\ntabs = [\"dash\"]\n").expect("a config");
        std::fs::write(&over, "[utilities]\ntoggles = [\"clokc\"]\n").expect("an override");
        let last_good = Config::load(&path).expect("the last config that loaded");
        std::fs::write(&path, "[dashboard\ntabs = ").expect("a broken save");
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

        std::fs::write(&path, "[dashboard]\ntabs = [\"dash\"]\n").expect("the fix");
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
            assert!(telar::t!("config.global_only", section = "general").contains("[general]"));
            assert!(
                telar::t!(
                    "config.moved_to_layout",
                    key = "bars",
                    layout = "layouts/custom.toml"
                )
                .contains("bars")
            );
        }
        telar::set_locale("en");
    }
}
