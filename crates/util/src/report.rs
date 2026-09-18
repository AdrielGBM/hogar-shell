//! What a check found wrong with a file the user wrote, and where in it.
//!
//! The shape is the smallest one every checker can fill: the file, the key path inside it, where in the text that key sits when the checker had the text to look in, and a sentence. Nothing here knows about bars, modules or TOML, because the config check that fills it today is not meant to be the only one — the layout model a later sprint brings gets a checker of its own, and two report types would be two renderings of "what is wrong", two exit-status rules and two ways for `hogar-shell` to disagree with itself.
//!
//! Errors and warnings are kept apart rather than tagged, because a caller does different things with them: an error is something the file asks for that the shell cannot do, and `config check` fails on one; a warning is something the shell did *instead* of what was asked, which is worth saying and not worth failing a script over.

use std::fmt;
use std::ops::Range;
use std::path::PathBuf;

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
}

/// One thing wrong with a file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Finding {
    pub file: PathBuf,
    /// The dotted path to the offending value, with list positions in brackets — `bars.top.center[1]`. Empty when what is wrong is the file as a whole, which is what a syntax error is.
    pub key: String,
    /// Where `key` sits in the text; `None` when whoever found it had only the parsed value to go on.
    pub span: Option<Span>,
    /// The sentence a user reads, already in their language. The checker is the one that knows what went wrong, so it is the one that says it.
    pub message: String,
}

impl Finding {
    pub fn new(
        file: impl Into<PathBuf>,
        key: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file: file.into(),
            key: key.into(),
            span: None,
            message: message.into(),
        }
    }

    /// `file:line:column`, or the file alone without a span — the prefix compilers print, which is what lets a terminal or an editor that knows it turn the line into a jump to the spot.
    pub fn location(&self) -> String {
        match &self.span {
            Some(span) => format!("{}:{}:{}", self.file.display(), span.line, span.column),
            None => self.file.display().to_string(),
        }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.key, self.message)
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

    /// One line per finding, as `location: severity: key: message`.
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
        let text = "[bars.top]\ncenter = [\"reloj-más\", \"clokc\"]\n";
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
    fn a_rendered_line_reads_like_a_compiler_diagnostic() {
        let mut report = Report::default();
        let mut placed = Finding::new(
            "config.toml",
            "bars.top.center[1]",
            "no module is called 'x'",
        );
        placed.span = Some(Span::locate("\n\n  x", 4..5));
        report.error(placed);
        report.warn(Finding::new("monitors/DP-1/config.toml", "", "not read"));

        assert_eq!(
            report.render(),
            "config.toml:3:3: error: bars.top.center[1]: no module is called 'x'\n\
             monitors/DP-1/config.toml: warning: not read\n",
            "`file:line:column: severity:` is the prefix editors and terminals turn into a link; a finding with no span \
             or no key drops that part rather than printing an empty one"
        );
        assert_eq!(report.summary(), "1 error, 1 warning");
    }
}
