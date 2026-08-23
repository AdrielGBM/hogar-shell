//! `hogar-shell(1)` and `hogar-shell(5)`, generated rather than written.
//!
//! Same reason as `--list` and `config schema`, one step further out: the manual a distribution installs is the
//! copy furthest from the source and the one nobody re-reads, so writing it by hand is writing something that
//! will be wrong by the next release. The command page walks [`TARGETS`] and the config page walks
//! [`config::schema::outline`] — the two tables that already cannot drift from what the shell does — so a
//! command or a key reaches the manual by existing.
//!
//! Roff by hand rather than through scdoc or mandoc: the output is a few hundred lines of a format that has not
//! moved in decades, and a generator that needs a tool installed to produce documentation is a build dependency
//! a packager pays for nothing.

use std::fmt::Write;

use config::schema::{Entry, Table};

use super::commands::TARGETS;

/// Every way the binary can be invoked, in one place: `--help` prints these as a usage block and the manual as
/// its synopsis, so a form cannot appear in one and be missing from the other.
pub const FORMS: &[(&str, &str)] = &[
    ("[run]", "start the shell"),
    (
        "<target> <cmd> [args]",
        "send a command to the running shell",
    ),
    ("toggle <module>", "shorthand for `panel toggle <module>`"),
    ("launcher", "shorthand for `launcher toggle`"),
    ("--list", "list every command the shell answers"),
    ("config schema [name]", "print the annotated default config"),
    (
        "man commands|config",
        "print this manual, or the config one, as roff",
    ),
    ("--help | --version", ""),
];

/// The page header. The date field is deliberately empty: a manual stamped with the day it was generated
/// differs from the committed copy every day, and the check that keeps the two identical would fail for a
/// reason that is not drift.
fn header(section: u8, summary: &str) -> String {
    format!(
        ".TH HOGAR-SHELL {section} \"\" \"hogar-shell {}\" \"hogar-shell\"\n.SH NAME\nhogar-shell \\- {summary}\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// The typographic characters the source's prose actually uses, as roff escapes.
///
/// A manual has to survive being read by an `nroff` that treats its input as Latin-1 unless something thought
/// to pipe it through `preconv`, which is where an em dash becomes three bytes of noise. Escaping is the only
/// spelling that renders the same everywhere. Anything not in this table fails
/// `the_manual_is_ascii_whatever_the_locale`, and that is the point: a character earns a place here by someone
/// deciding what it should look like, rather than by turning into a question mark on a stranger's terminal.
const TYPOGRAPHY: &[(char, &str)] = &[
    ('—', "\\(em"),
    ('–', "\\(en"),
    ('…', "..."),
    ('°', "\\(de"),
    ('×', "\\(mu"),
    ('±', "\\(+-"),
    ('•', "\\(bu"),
    ('→', "\\(->"),
    ('←', "\\(<-"),
    ('↔', "\\(<>"),
    ('≥', "\\(>="),
    ('≤', "\\(<="),
    ('‘', "\\(oq"),
    ('’', "\\(cq"),
    ('“', "\\(lq"),
    ('”', "\\(rq"),
    ('á', "\\('a"),
    ('é', "\\('e"),
    ('í', "\\('i"),
    ('ó', "\\('o"),
    ('ú', "\\('u"),
    ('ñ', "\\(~n"),
];

/// Text as roff.
///
/// A backslash opens an escape; a line opening with `.` or `'` is a request, so a doc comment whose wrapping
/// happens to start a line with one would silently become a formatting command; a bare hyphen sets as a
/// typographic dash in a page whose whole purpose is names a reader copies into a terminal; and everything
/// above ASCII goes through [`TYPOGRAPHY`].
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\e"),
            '-' => escaped.push_str("\\-"),
            other => match TYPOGRAPHY.iter().find(|(from, _)| *from == other) {
                Some((_, roff)) => escaped.push_str(roff),
                None => escaped.push(other),
            },
        }
    }
    escaped
        .lines()
        .map(|line| {
            if line.starts_with('.') || line.starts_with('\'') {
                format!("\\&{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A doc comment as manual prose. Its blank lines become explicit vertical space rather than roff's own
/// paragraph break, which inside a `.TP` body would end the tagged paragraph and drop the indent for
/// everything after it.
fn prose(doc: &str) -> String {
    let mut out = String::new();
    for line in escape(doc).lines() {
        if line.trim().is_empty() {
            out.push_str(".sp\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// `hogar-shell(1)`: the invocation forms, then every target and command the shell answers.
pub(crate) fn commands_page() -> String {
    let mut out = header(1, "a Wayland desktop shell for Hyprland");

    out.push_str(".SH SYNOPSIS\n");
    for (index, (form, _)) in FORMS.iter().enumerate() {
        if index > 0 {
            out.push_str(".br\n");
        }
        let _ = writeln!(out, ".B hogar-shell\n{}", escape(form));
    }

    out.push_str(
        ".SH DESCRIPTION\n\
         hogar-shell is a Wayland desktop shell: bars, panels, a launcher, a dashboard, a lock screen,\n\
         notifications, screen capture and wallpaper\\-derived theming, configured in TOML.\n\
         .PP\n\
         Started with no arguments it runs the shell. With arguments it is a client: the request is sent to the\n\
         running shell over a Unix socket and the reply printed, so every action the shell has is available to a\n\
         compositor keybind or a script. The exit status mirrors the reply, so a caller can tell a refused\n\
         command from one that worked without parsing prose.\n\
         .PP\n\
         The shell also registers its actions as XDG global shortcuts, which can be bound through the desktop\n\
         portal instead of by command.\n",
    );

    out.push_str(".SH OPTIONS\n");
    for (flag, help) in [
        ("\\-\\-help, \\-h", "print the usage block and exit"),
        ("\\-\\-version, \\-V", "print the version and exit"),
        (
            "\\-\\-list, \\-s",
            "print every target, command and argument the shell answers",
        ),
    ] {
        let _ = writeln!(out, ".TP\n.B {flag}\n{help}");
    }
    out.push_str(
        ".PP\n\
         .BR \"config schema\" ,\n\
         .B deps\n\
         and\n\
         .B man\n\
         are answered by the binary rather than sent to the shell: each is a function of this build and this\n\
         machine, and the case a dependency report is for is the one where nothing started.\n",
    );

    out.push_str(
        ".SH COMMANDS\n\
         A request is a target, a command and its arguments.\n\
         .BR hogar-shell (5)\n\
         documents the configuration file.\n",
    );
    for target in TARGETS {
        let _ = writeln!(out, ".SS {}", escape(target.name));
        for command in target.commands {
            let tag = if command.args.is_empty() {
                format!("\\fB{}\\fR", escape(command.name))
            } else {
                format!(
                    "\\fB{}\\fR \\fI{}\\fR",
                    escape(command.name),
                    escape(command.args)
                )
            };
            let _ = writeln!(out, ".TP\n{tag}\n{}", prose(command.help).trim_end());
        }
    }

    out.push_str(
        ".SH FILES\n\
         .TP\n\
         .I ~/.config/hogar-shell/config.toml\n\
         Everything, hot\\-reloaded on save. Written annotated on first run.\n\
         .TP\n\
         .I ~/.config/hogar-shell/tokens.toml\n\
         Design\\-token overrides. Deliberately unstable \\- the config's\n\
         .B [theme]\n\
         section is the supported surface.\n\
         .TP\n\
         .I ~/.config/hogar-shell/monitors/<output>/config.toml\n\
         Per\\-monitor overrides, same shape as the global file.\n\
         .TP\n\
         .I ~/.config/hogar-shell/state.json\n\
         Runtime state the shell owns, such as the current wallpaper. Not settings.\n\
         .TP\n\
         .I $XDG_RUNTIME_DIR/hogar-shell/<instance>.sock\n\
         The command socket, one per compositor instance.\n",
    );

    out.push_str(
        ".SH ENVIRONMENT\n\
         .TP\n\
         .B XDG_CONFIG_HOME\n\
         Where the config directory is looked for; falls back to\n\
         .IR $HOME/.config .\n\
         .TP\n\
         .B XDG_RUNTIME_DIR\n\
         Where the command socket is created.\n\
         .TP\n\
         .B HYPRLAND_INSTANCE_SIGNATURE\n\
         Names the socket, so two compositors on one login session get one socket each. Outside Hyprland the\n\
         name is still stable and the client still finds the shell.\n\
         .TP\n\
         .B RUST_LOG\n\
         Log filter; defaults to\n\
         .BR info .\n",
    );

    out.push_str(
        ".SH EXAMPLES\n\
         Start the shell from the compositor:\n\
         .PP\n\
         .EX\n\
         exec\\-once = hogar-shell\n\
         .EE\n\
         .PP\n\
         Bind an action to a key:\n\
         .PP\n\
         .EX\n\
         bind = SUPER, SPACE, exec, hogar-shell launcher\n\
         bind = SUPER, N, exec, hogar-shell panel toggle notifications\n\
         bind = , XF86AudioRaiseVolume, exec, hogar-shell volume up\n\
         .EE\n\
         .PP\n\
         Write a config with every key in it, then edit it down:\n\
         .PP\n\
         .EX\n\
         hogar-shell config schema > ~/.config/hogar-shell/config.toml\n\
         .EE\n",
    );

    out.push_str(".SH SEE ALSO\n.BR hogar-shell (5)\n");
    out
}

/// `hogar-shell(5)`: every configuration section, from the same outline `config schema` prints as TOML.
pub(crate) fn config_page() -> Result<String, String> {
    let mut out = header(5, "configuration file format");
    out.push_str(
        ".SH SYNOPSIS\n\
         .I ~/.config/hogar-shell/config.toml\n\
         .SH DESCRIPTION\n\
         hogar-shell is configured in TOML. Every key below is optional; the default shown is what the shell uses\n\
         when the key is absent, so a working config is any subset of this page.\n\
         .PP\n\
         The file is watched and re\\-read on save, and the change applies without a restart. A file under\n\
         .I ~/.config/hogar-shell/monitors/<output>/\n\
         has the same shape and overrides the global one for that monitor.\n\
         .PP\n\
         .B hogar-shell config schema\n\
         prints this same reference as a complete, valid config file, and\n\
         .B hogar-shell config schema <section>\n\
         prints one section of it. Both come from the running build rather than from this page.\n\
         .SH SECTIONS\n",
    );
    for table in &config::schema::outline(None)? {
        render_table(table, &mut out);
    }
    out.push_str(".SH SEE ALSO\n.BR hogar-shell (1)\n");
    Ok(out)
}

/// One table and everything under it. Sub-tables become headings of their own rather than nesting, which is
/// what the file itself does: `[theme.scale]` is a header a reader types, not an indent.
fn render_table(table: &Table, out: &mut String) {
    let _ = writeln!(out, ".SS [{}]", escape(&table.path));
    if let Some(doc) = table.doc {
        out.push_str(&prose(doc));
    }
    // The outline orders a table's own keys before its sub-tables and lists, so one pass emits each heading
    // after the keys that belong to it rather than before them.
    for entry in &table.entries {
        match entry {
            Entry::Key { name, default, doc } => {
                let _ = writeln!(out, ".TP\n.B {}", escape(name));
                // The break belongs between prose and its default, not under a bare key name — most of these
                // are one line, and a blank line above every one of them reads as a page of gaps.
                if let Some(doc) = doc {
                    out.push_str(&prose(doc));
                    out.push_str(".sp\n");
                }
                match default {
                    Some(value) => {
                        let _ = writeln!(out, "Default: \\fB{}\\fR", escape(&value.to_string()));
                    }
                    None => out.push_str("Unset by default.\n"),
                }
            }
            Entry::Table(nested) => render_table(nested, out),
            Entry::List {
                path,
                doc,
                elements,
            } => {
                let _ = writeln!(out, ".SS [[{}]]", escape(path));
                if let Some(doc) = doc {
                    out.push_str(&prose(doc));
                }
                out.push_str(".PP\nThe entries a fresh install starts with:\n.PP\n.EX\n");
                for (index, element) in elements.iter().enumerate() {
                    if index > 0 {
                        out.push('\n');
                    }
                    let _ = writeln!(out, "[[{}]]", escape(path));
                    for line in escape(&toml::to_string(element).unwrap_or_default()).lines() {
                        let _ = writeln!(out, "{line}");
                    }
                }
                out.push_str(".EE\n");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed pages under `man/`, which packaging installs and a cross-compiled build cannot generate.
    fn committed(file: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../man")
            .join(file)
    }

    /// Both pages, paired with the file each is committed as.
    fn pages() -> [(&'static str, String); 2] {
        [
            ("hogar-shell.1", commands_page()),
            (
                "hogar-shell.5",
                config_page().expect("the config page generates"),
            ),
        ]
    }

    /// The check that makes a checked-in generated file safe to have: a key added to the config or a command
    /// added to the table without regenerating fails here, rather than shipping a manual that quietly lies.
    ///
    /// `UPDATE_MAN=1 cargo test -p hogar-shell --lib man` rewrites them.
    #[test]
    fn the_committed_manual_matches_what_this_build_generates() {
        for (file, generated) in pages() {
            let path = committed(file);
            if std::env::var_os("UPDATE_MAN").is_some() {
                std::fs::create_dir_all(path.parent().expect("man/")).expect("create man/");
                std::fs::write(&path, &generated).expect("write the page");
                continue;
            }
            let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(
                on_disk, generated,
                "man/{file} is out of date; regenerate with `UPDATE_MAN=1 cargo test -p hogar-shell --lib man`"
            );
        }
    }

    #[test]
    fn every_target_and_command_reaches_the_manual() {
        let page = commands_page();
        for target in TARGETS {
            assert!(
                page.contains(&format!(".SS {}\n", target.name)),
                "'{}' is missing from the manual",
                target.name
            );
            for command in target.commands {
                assert!(
                    page.contains(&format!("\\fB{}\\fR", escape(command.name))),
                    "'{} {}' is missing from the manual",
                    target.name,
                    command.name
                );
            }
        }
    }

    #[test]
    fn every_config_section_reaches_the_manual() {
        let page = config_page().expect("the page generates");
        for table in config::schema::outline(None).expect("the outline builds") {
            assert!(
                page.contains(&format!(".SS [{}]\n", table.path)),
                "'[{}]' is missing from the manual",
                table.path
            );
        }
        assert!(
            page.contains(".SS [[idle.stages]]\n"),
            "a list of tables too"
        );
    }

    /// Roff's live characters, in text that comes from doc comments nobody wrote for a manual.
    #[test]
    fn text_that_would_be_read_as_roff_is_escaped() {
        assert_eq!(escape("a-b"), "a\\-b");
        assert_eq!(escape(".hidden"), "\\&.hidden");
        assert_eq!(escape("'quoted"), "\\&'quoted");
        assert_eq!(escape("a\\b"), "a\\eb");
        // Only at the start of a line: a full stop mid-sentence is prose, not a request.
        assert_eq!(escape("end. next"), "end. next");
        // A mapped character keeps the escape it maps to, hyphens and all.
        assert_eq!(escape("a — b"), "a \\(em b");
        assert_eq!(escape("dark → light"), "dark \\(-> light");
    }

    /// A page containing one raw em dash renders as three bytes of noise under an `nroff` that reads its input
    /// as Latin-1, and a reader has no way to tell that from the shell's own text.
    #[test]
    fn the_manual_is_ascii_whatever_the_locale() {
        for (file, page) in pages() {
            let stray: Vec<char> = page.chars().filter(|c| !c.is_ascii()).collect();
            assert!(
                stray.is_empty(),
                "{file} carries characters with no roff spelling: {stray:?} — add them to TYPOGRAPHY"
            );
        }
    }

    #[test]
    fn a_blank_line_in_a_doc_comment_does_not_end_the_tagged_paragraph() {
        assert_eq!(prose("one\n\ntwo"), "one\n.sp\ntwo\n");
    }
}
