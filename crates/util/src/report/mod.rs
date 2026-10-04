//! What a check found wrong with a file the user wrote, and where in it.
//!
//! The shape is the smallest one every checker can fill: the file, the key path inside it, where in the text that key sits when the checker had the text to look in, and a [`Message`] saying what is wrong, in no language until it is shown. Nothing here knows about bars or modules, and of TOML only how a string's quotes sit around what it reads as ([`Span::within_toml_string`]), because the config check that fills it today is not meant to be the only one — the layout model a later sprint brings gets a checker of its own, and two report types would be two renderings of "what is wrong", two exit-status rules and two ways for `hogar-shell` to disagree with itself.
//!
//! Errors and warnings are kept apart rather than tagged, because a caller does different things with them: an error is something the file asks for that the shell cannot do, and `config check` fails on one; a warning is something the shell did *instead* of what was asked, which is worth saying and not worth failing a script over.

use std::fmt;
use std::ops::Range;
use std::path::PathBuf;

#[macro_use]
mod message;
mod expression;

pub use message::{Arg, ENGLISH, IntoArg, Message, untranslated};

/// Where a finding sits in its file: as bytes, for a tool that highlights it, and as a line and column, for a person reading a terminal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Span {
    pub bytes: Range<usize>,
    /// 1-based.
    pub line: usize,
    /// 1-based and counted in characters: a column counted in bytes points past the spot on any line with an accent before it.
    pub column: usize,
}

impl Span {
    /// Places `bytes` in `text`. An offset past the end, or inside a character, is pulled back to the nearest boundary before it, so a span that went stale while the file was being edited describes a nearby spot rather than panicking.
    pub fn locate(text: &str, bytes: Range<usize>) -> Self {
        let start = text.floor_char_boundary(bytes.start);
        let before = &text[..start];
        let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
        Self {
            line: before.matches('\n').count() + 1,
            column: text[line_start..start].chars().count() + 1,
            bytes,
        }
    }

    /// Places `inner`, a byte range of what the TOML string written at `value` in `text` reads as, on those bytes of the file. A string whose bytes are not what it reads as — triple-quoted, or a basic string with escapes — is placed whole, at `value`, since no offset into what it reads as is an offset into the file.
    pub fn within_toml_string(text: &str, value: Range<usize>, inner: Range<usize>) -> Self {
        let raw = &text[value.clone()];
        let literal = raw.starts_with('\'') && !raw.starts_with("'''");
        let basic = raw.starts_with('"') && !raw.starts_with("\"\"\"") && !raw.contains('\\');
        if raw.len() < 2 || !(literal || basic) {
            return Self::locate(text, value);
        }
        let quoted = value.start + 1..value.end - 1;
        let clamp = |at: usize| quoted.start + at.min(quoted.len());
        Self::locate(text, clamp(inner.start)..clamp(inner.end))
    }
}

/// One thing wrong with a file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Finding {
    pub file: PathBuf,
    /// The dotted path to the offending value, with list positions in brackets — `bars.top.center[1]`. Empty when what is wrong is the file as a whole, which is what a syntax error is.
    pub key: String,
    /// Where `key` sits in the text; `None` when whoever found it had only the parsed value to go on.
    pub span: Option<Span>,
    /// What is wrong, in no language yet: the checker knows what went wrong, so it is the one that says it, and whoever shows it picks the language (DEC-27).
    pub message: Message,
}

impl Finding {
    pub fn new(file: impl Into<PathBuf>, key: impl Into<String>, message: Message) -> Self {
        Self {
            file: file.into(),
            key: key.into(),
            span: None,
            message,
        }
    }

    /// `file:line:column`, or the file alone without a span — the prefix compilers print, which is what lets a terminal or an editor that knows it turn the line into a jump to the spot.
    ///
    /// The path is written out where it could disguise the line ([`crate::text::shown`]).
    pub fn location(&self) -> String {
        let file = crate::text::shown(&self.file.display().to_string());
        match &self.span {
            Some(span) => format!("{file}:{}:{}", span.line, span.column),
            None => file,
        }
    }
}

/// In English, as the command line and the log say it. The key holds ids a file chose, so it is written out where it could disguise the line ([`crate::text::shown`]); the message does the same for every value it quotes ([`Message::render_in`]).
impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = self.message.english();
        match self.key.is_empty() {
            true => write!(f, "{message}"),
            false => write!(f, "{}: {message}", crate::text::shown(&self.key)),
        }
    }
}

/// Everything one check found, across however many files it read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,
}

impl Report {
    pub fn error(&mut self, finding: Finding) {
        self.errors.push(finding);
    }

    pub fn warn(&mut self, finding: Finding) {
        self.warnings.push(finding);
    }

    pub fn merge(&mut self, other: Report) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }

    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty()
    }

    /// Errors first, then warnings.
    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.errors.iter().chain(&self.warnings)
    }

    pub fn findings_mut(&mut self) -> impl Iterator<Item = &mut Finding> {
        self.errors.iter_mut().chain(&mut self.warnings)
    }

    /// How much there is, as `2 errors, 1 warning`.
    pub fn summary(&self) -> String {
        let count = |n: usize, noun: &str| match n {
            1 => format!("1 {noun}"),
            n => format!("{n} {noun}s"),
        };
        match (self.errors.len(), self.warnings.len()) {
            (errors, 0) => count(errors, "error"),
            (0, warnings) => count(warnings, "warning"),
            (errors, warnings) => {
                format!("{}, {}", count(errors, "error"), count(warnings, "warning"))
            }
        }
    }

    /// One line per finding, as `location: severity: key: message`, in English: what `config check` and `layout check` print.
    pub fn render(&self) -> String {
        let line = |severity: &str, finding: &Finding| {
            format!("{}: {severity}: {finding}\n", finding.location())
        };
        self.errors
            .iter()
            .map(|finding| line("error", finding))
            .chain(self.warnings.iter().map(|finding| line("warning", finding)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A column is where a person looks, so it counts what they see: an accented id earlier on the line would otherwise push the caret one column right for every accent, onto a character that is not the start of anything.
    #[test]
    fn a_column_counts_characters_not_bytes() {
        let text = "[clock]\ncenter = [\"reloj-más\", \"clokc\"]\n";
        let at = text.find("\"clokc\"").expect("the fixture has the id");

        let span = Span::locate(text, at..at + 7);

        assert_eq!(
            (span.line, span.column),
            (2, 24),
            "the id sits on the second line, 23 characters in — 'á' is two bytes and one column"
        );
    }

    /// A span can outlive the text it was taken from — the file is edited between the check and the read — and a report is the last thing that should take the shell down over it.
    #[test]
    fn a_span_past_the_end_of_the_text_is_placed_at_the_end() {
        let span = Span::locate("a = 1\n", 40..44);

        assert_eq!(
            (span.line, span.column),
            (2, 1),
            "a span past a one-line file lands after its last newline"
        );
    }

    #[test]
    fn a_span_inside_a_plain_string_lands_on_its_bytes_in_the_file() {
        let text = "a = \"x + $nope\"\nb = '$nope'\n";
        let basic = text.find('"').unwrap()..text.find('\n').unwrap();
        let literal = text.find('\'').unwrap()..text.rfind('\'').unwrap() + 1;

        let in_basic = Span::within_toml_string(text, basic, 4..9);
        let in_literal = Span::within_toml_string(text, literal, 0..5);

        assert_eq!(&text[in_basic.bytes.clone()], "$nope");
        assert_eq!((in_basic.line, in_basic.column), (1, 10));
        assert_eq!(&text[in_literal.bytes.clone()], "$nope");
        assert_eq!((in_literal.line, in_literal.column), (2, 6));
    }

    #[test]
    fn a_string_whose_bytes_differ_from_what_it_reads_is_placed_whole() {
        let text = "a = \"\\t$nope\"\nb = '''$nope'''\n";
        let escaped = 4..text.find('\n').unwrap();
        let triple = text.find("'''").unwrap()..text.len() - 1;

        let in_escaped = Span::within_toml_string(text, escaped.clone(), 1..6);
        let in_triple = Span::within_toml_string(text, triple.clone(), 0..5);

        assert_eq!(in_escaped.bytes, escaped);
        assert_eq!(in_triple.bytes, triple);
    }

    #[test]
    fn a_span_past_the_string_is_kept_inside_its_quotes() {
        let text = "a = \"ab\"\n";

        let span = Span::within_toml_string(text, 4..8, 1..40);

        assert_eq!(span.bytes, 6..7);
    }

    /// Somebody else's text reaches a terminal written out: a key holding an id a file chose, a value a message quotes and words shown as they are, each with its escapes and bidirectional controls spelled, while a parser's report keeps its lines.
    #[test]
    fn what_a_finding_quotes_is_written_out_when_it_is_rendered() {
        let mut report = Report::default();
        report.warn(Finding::new(
            "layouts/x\u{1b}[2J.toml",
            "outputs.*.layers.top.areas.\u{1b}]0;owned\u{7}",
            message!("expression.unknown_name", name = "\u{202e}hs.lruc"),
        ));
        report.error(Finding::new(
            "layouts/x.toml",
            "",
            Message::verbatim("line 1\n  | a = \"\u{1b}[31m\"\n  ^"),
        ));

        let rendered = report.render();

        assert!(
            !rendered.contains('\u{1b}') && !rendered.contains('\u{202e}'),
            "{rendered}"
        );
        assert!(rendered.contains("layouts/x\\u{1b}[2J.toml: warning: outputs.*.layers.top.areas.\\u{1b}]0;owned\\u{7}: unknown name `\\u{202e}hs.lruc`"), "{rendered}");
        assert!(
            rendered.contains("line 1\n  | a = \"\\u{1b}[31m\"\n  ^"),
            "{rendered}"
        );
    }

    #[test]
    fn a_rendered_line_reads_like_a_compiler_diagnostic() {
        let mut report = Report::default();
        let mut placed = Finding::new(
            "config.toml",
            "bars.top.center[1]",
            message!("expression.unknown_name", name = "x"),
        );
        placed.span = Some(Span::locate("\n\n  x", 4..5));
        report.error(placed);
        report.warn(Finding::new(
            "monitors/DP-1/config.toml",
            "",
            Message::verbatim("not read"),
        ));
        telar::set_locale("es");

        assert_eq!(
            report.render(),
            "config.toml:3:3: error: bars.top.center[1]: unknown name `x`\n\
             monitors/DP-1/config.toml: warning: not read\n",
            "`file:line:column: severity:` is the prefix editors and terminals turn into a link; a finding with no span \
             or no key drops that part rather than printing an empty one, and it is in English whatever the shell speaks"
        );
        telar::set_locale("en");
        assert_eq!(report.summary(), "1 error, 1 warning");
    }
}
