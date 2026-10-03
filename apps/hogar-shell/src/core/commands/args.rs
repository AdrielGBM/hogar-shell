//! Turning a command line into what it acts on: the arguments, the monitor it names, and the readings a reply prints.
use std::borrow::Cow;
use std::ops::{Deref, Range};

use services::pipewire::NodeKind;
use surfaces::transient;

/// A command's arguments: the words after the target and the command, and the text they say.
///
/// They arrive in one of two shapes. A **line** — a layout action, a rule, a keybind, a raw socket client — is text a person wrote in one string: its words are what whitespace separates, and its text is the line as written, quotes and runs of spaces being the user's. **Words** — the `hogar-shell …` client's arguments — come apart already, as a program's arguments do: a word with spaces in it is still one word, and their text is the words joined by single spaces, so nothing inside a word is lost.
///
/// A command reads its arguments as words, but one that takes free text — a variable's value, a toast — reads it with [`Args::rest`], and `shell run` reads it as a command for `sh` with [`Args::command`].
pub(crate) struct Args<'a> {
    text: Cow<'a, str>,
    words: Vec<&'a str>,
    starts: Vec<usize>,
    /// Whether these came as words rather than as a line.
    split: bool,
}

impl<'a> Args<'a> {
    /// The whitespace-separated words of the line `text`.
    pub(crate) fn of(text: &'a str) -> Self {
        let words: Vec<&str> = text.split_whitespace().collect();
        let starts = words
            .iter()
            .map(|word| word.as_ptr() as usize - text.as_ptr() as usize)
            .collect();
        Self {
            text: Cow::Borrowed(text),
            words,
            starts,
            split: false,
        }
    }

    /// `words` exactly as given, as a program's arguments arrive.
    pub(crate) fn split(words: &'a [String]) -> Self {
        let mut starts = Vec::with_capacity(words.len());
        let mut at = 0;
        for word in words {
            starts.push(at);
            at += word.len() + 1;
        }
        Self {
            text: Cow::Owned(words.join(" ")),
            words: words.iter().map(String::as_str).collect(),
            starts,
            split: true,
        }
    }

    /// These arguments from word `from` on, the words before it dropped.
    pub(crate) fn after(mut self, from: usize) -> Self {
        let from = from.min(self.words.len());
        self.words.drain(..from);
        self.starts.drain(..from);
        self
    }

    /// The text from word `from` through the last, or nothing when there is no such word.
    pub(crate) fn rest(&self, from: usize) -> &str {
        self.text(from..self.words.len())
    }

    /// The text from the first word of `words` through the last, or nothing for an empty range.
    pub(crate) fn text(&self, words: Range<usize>) -> &str {
        if words.is_empty() || words.end > self.words.len() {
            return "";
        }
        let last = words.end - 1;
        &self.text[self.starts[words.start]..self.starts[last] + self.words[last].len()]
    }

    /// The words from `from` on as a command line for `sh`: a line's text as written, since its quotes are already the user's `sh` quoting, or each of a program's arguments quoted so `sh` hands it on as the one word it was.
    pub(crate) fn command(&self, from: usize) -> String {
        match self.split {
            false => self.rest(from).to_string(),
            true => self
                .words
                .get(from..)
                .unwrap_or_default()
                .iter()
                .map(|word| sh_quoted(word))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// `word` as `sh` reads it back as one word with nothing expanded: as it is when every character is one `sh` leaves alone, else in single quotes.
fn sh_quoted(word: &str) -> Cow<'_, str> {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./,:@%+".contains(c));
    match plain {
        true => Cow::Borrowed(word),
        false => Cow::Owned(format!("'{}'", word.replace('\'', r"'\''"))),
    }
}

impl<'a> Deref for Args<'a> {
    type Target = [&'a str];

    fn deref(&self) -> &[&'a str] {
        &self.words
    }
}

/// A `--name <value>` pair among some words: where `--name` is, and the value after it.
pub(crate) struct Flag<'a> {
    pub(crate) at: usize,
    pub(crate) value: &'a str,
}

/// The `name` flag among `words`, wherever it is written, its value named `<value_name>` in a refusal. Refused when the value is missing or the flag is given twice.
pub(crate) fn flag<'a>(
    words: &[&'a str],
    name: &str,
    value_name: &str,
) -> Result<Option<Flag<'a>>, String> {
    let mut found = None;
    let mut at = 0;
    while at < words.len() {
        if words[at] != name {
            at += 1;
            continue;
        }
        let value = words
            .get(at + 1)
            .ok_or_else(|| format!("missing argument <{value_name}> after {name}"))?;
        if found.replace(Flag { at, value }).is_some() {
            return Err(format!("{name} is given twice"));
        }
        at += 2;
    }
    Ok(found)
}

/// `words` with `flag` and its value taken out.
pub(crate) fn without<'a>(words: &[&'a str], flag: Option<&Flag<'_>>) -> Vec<&'a str> {
    match flag {
        Some(flag) => [&words[..flag.at], &words[flag.at + 2..]].concat(),
        None => words.to_vec(),
    }
}

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

/// One row per node, tab-separated so a script can cut columns: id, level, mute, whether it is the default, and the label last because it is the only field that can contain spaces.
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
/// Resolved to a name rather than left as `None`, because `None` means "every screen" to the service and "wherever the user is looking" to a keybind — and a `wallpaper random` bound to a key should change the screen in front of them, not all of them.
///
/// A name that is not a monitor is refused. Accepting one writes an entry into the persisted assignment that no surface will ever read, and answers `ok` while changing nothing — which is how a stray `--features` from a dev harness ended up saved as a screen. It costs one lookup to make a typo say so. Which screen a wallpaper command changes: the one named, or every screen when none is.
///
/// "Every screen" and not "the focused one", because that is what each command's own help says it does, and because the focused-screen reading made `wallpaper clear` unable to do the one thing it exists for: it removed the focused monitor's entry, answered `cleared`, and left every other entry — including one written under a name no monitor has — sitting in the state file with no way left to reach it.
pub(crate) fn target_output(named: Option<&str>) -> Result<Option<String>, String> {
    named.map(validated).transpose()
}

/// Which screen a wallpaper command *reads*: the one named, else the focused one. A reading has to be about some screen, so here the focused one is the only sensible default.
pub(crate) fn reading_output(named: Option<&str>) -> Result<Option<String>, String> {
    match named {
        Some(name) => validated(name).map(Some),
        None => Ok(transient::focused_output()),
    }
}

/// Which displays a brightness *mutation* means: the one named, `all` of them, or — with nothing named — the primary one.
///
/// Deliberately not the wallpaper rule, where an unnamed mutation means every screen. A wallpaper is one desktop look; brightness is per-panel hardware, and `brightness up` is overwhelmingly a laptop's function key, which means *this* panel. `all` is there for the desk, and both are in the command's help so neither is a surprise.
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
/// Checked against the brightness snapshot rather than against the compositor's outputs: those are two different sets. A monitor with no DDC support is an output that cannot be dimmed, and a DDC monitor whose connector could not be resolved answers to `i2c-6` — a name no compositor has ever heard of.
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

/// Re-derives the dynamic palette after a wallpaper change, once the transition to the new image has finished. A no-op unless `[theme] name = "dynamic"`, so every wallpaper command can call it blind.
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

/// Switches the dashboard's page by its config id, refusing an unknown one by name — a keybind bound to a page that was renamed should say so rather than silently leaving the dashboard where it was.
pub(crate) fn set_dashboard_tab(name: &str) -> Result<(), String> {
    use config::DashboardTab;
    let tab = DashboardTab::from_id(name).ok_or_else(|| {
        let known: Vec<&str> = DashboardTab::ALL.iter().map(|t| t.id()).collect();
        format!("unknown tab '{name}', expected one of {}", known.join("|"))
    })?;
    modules::dashboard::set_tab(&dashboard_instance(), tab);
    Ok(())
}

/// The config id of the page the dashboard shows.
pub(crate) fn dashboard_tab() -> String {
    modules::dashboard::tab(&dashboard_instance())
        .id()
        .to_string()
}

fn dashboard_instance() -> ui::host::InstanceId {
    ui::host::InstanceId::of_module(modules::dashboard::ID)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rest_of_a_line_is_the_text_as_written() {
        let args = Args::of(r#"  notify-send "a   b"  --urgency  low  "#);
        assert_eq!(&*args, ["notify-send", "\"a", "b\"", "--urgency", "low"]);
        assert_eq!(args.rest(0), r#"notify-send "a   b"  --urgency  low"#);
        assert_eq!(args.rest(3), "--urgency  low");
        assert_eq!(args.text(0..3), r#"notify-send "a   b""#);
        assert_eq!(args.rest(5), "");
        assert_eq!(args.text(2..2), "");
        assert_eq!(Args::of("   ").rest(0), "");
        assert_eq!(args.command(0), r#"notify-send "a   b"  --urgency  low"#);
    }

    #[test]
    fn words_keep_their_spaces_and_their_text_is_them_joined() {
        let words = ["var", "set", "x", "a  b", "--type", "text"].map(String::from);
        let args = Args::split(&words).after(2);
        assert_eq!(&*args, ["x", "a  b", "--type", "text"]);
        assert_eq!(args.rest(1), "a  b --type text");
        assert_eq!(args.text(0..2), "x a  b");
        assert_eq!(args.rest(4), "");
        assert_eq!(Args::split(&[]).rest(0), "");
    }

    #[test]
    fn words_become_a_command_sh_splits_back_into_the_same_words() {
        let words = [
            "notify-send",
            "a  b",
            "it's",
            "$HOME",
            "",
            "--urgency=low",
            "/tmp/x.png",
        ]
        .map(String::from);
        let command = Args::split(&words).command(0);
        assert_eq!(
            command,
            r#"notify-send 'a  b' 'it'\''s' '$HOME' '' '--urgency=low' /tmp/x.png"#
        );
        assert_eq!(Args::split(&words).command(5), "'--urgency=low' /tmp/x.png");
        assert_eq!(Args::split(&words).command(9), "");
    }

    #[test]
    fn sh_hands_each_quoted_word_on_as_it_was_given() {
        let words = ["printf", "%s|", "a  b", "it's", "$HOME", "*", "x;y"].map(String::from);
        let limits = util::process::Limits {
            timeout: std::time::Duration::from_secs(5),
            max_line: 1024,
            max_run: 1024,
        };
        match util::process::run_line(&Args::split(&words).command(0), limits) {
            util::process::Outcome::Ok(said) => assert_eq!(said, "a  b|it's|$HOME|*|x;y|"),
            other => panic!("sh did not run it: {other:?}"),
        }
    }

    #[test]
    fn a_flag_is_found_wherever_it_is_written_once() {
        let words = ["a", "--type", "number", "b"];
        let found = flag(&words, "--type", "t").unwrap().expect("it is there");
        assert_eq!((found.at, found.value), (1, "number"));
        assert_eq!(without(&words, Some(&found)), ["a", "b"]);
        assert!(flag(&["a"], "--type", "t").unwrap().is_none());
        assert_eq!(without(&["a"], None), ["a"]);
        assert_eq!(
            flag(&["a", "--type"], "--type", "t").err(),
            Some("missing argument <t> after --type".to_string())
        );
        assert_eq!(
            flag(&["--type", "a", "--type", "b"], "--type", "t").err(),
            Some("--type is given twice".to_string())
        );
    }
}
