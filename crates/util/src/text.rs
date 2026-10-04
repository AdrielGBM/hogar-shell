//! Text cut to fit where it is shown — a title on a chip, a notification's summary on a lock screen, a value in a field, a command's output repeated back in a message — and text shown so that it cannot mislead: somebody else's command, an id a bundle chose, a reply printed on a terminal.

/// `text` in at most `max` characters, the last of them `…` where it had to be cut. Counts characters, not bytes, so a title with accents or CJK is never cut mid-codepoint; the space a cut leaves before the `…` is dropped.
pub fn clipped(text: &str, max: usize) -> String {
    let Some(kept) = max.checked_sub(1) else {
        return String::new();
    };
    if text.char_indices().nth(max).is_none() {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(kept)
        .map_or(text.len(), |(at, _)| at);
    format!("{}…", text[..end].trim_end())
}

/// How many spaces in a row [`shown`] writes as they are; a longer run is written as its count, so a wide gap cannot push the rest of a command out of sight.
pub const SPACE_RUN: usize = 3;

/// `text` as a person can read it without being misled: a line break, a carriage return, a tab, a terminal escape and every other control character, every bidirectional control, every invisible or formatting character ([`hides`]) and every space that is not U+0020 is written out as `\n`, `\r`, `\t` or `\u{…}`, and a run of more than [`SPACE_RUN`] spaces as `\u{20}{×n}`. What is shown is then what runs, character for character, however the text was made to look.
pub fn shown(text: &str) -> String {
    escaped(text, Keep::Nothing)
}

/// [`shown`] for text whose lines and columns are its meaning — a reply printed on a terminal, a table, a parser's report with a caret under the spot: line breaks, tabs and runs of spaces stay as they are, and everything else [`shown`] writes out is written out.
pub fn shown_lines(text: &str) -> String {
    escaped(text, Keep::Layout)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    Nothing,
    Layout,
}

fn escaped(text: &str, keep: Keep) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' | '\t' if keep == Keep::Layout => out.push(c),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ' ' if keep == Keep::Nothing => {
                let mut run = 1;
                while chars.next_if_eq(&' ').is_some() {
                    run += 1;
                }
                match run > SPACE_RUN {
                    true => out.push_str(&format!("\\u{{20}}{{×{run}}}")),
                    false => out.extend(std::iter::repeat_n(' ', run)),
                }
            }
            c if hides(c) || is_odd_space(c) => out.push_str(&format!("\\u{{{:x}}}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// Whether `c` is drawn as nothing, as something it is not, or changes how what is around it is drawn: a control character, a bidirectional control ([`is_bidi_control`]), a mark that is invisible on its own (a soft hyphen, a zero-width space or joiner, a word joiner, a byte-order mark), a filler that draws as blank (the Hangul fillers, U+3164 and U+FFA0), a variation selector, a Mongolian free variation selector, a tag character (U+E0000–U+E007F), an interlinear annotation mark or a musical formatting control. None of them belongs in an id, a command or an address, and each can make one read as something else.
pub fn hides(c: char) -> bool {
    c.is_control()
        || is_bidi_control(c)
        || matches!(
            c,
            '\u{00ad}'
                | '\u{034f}'
                | '\u{115f}'
                | '\u{1160}'
                | '\u{17b4}'
                | '\u{17b5}'
                | '\u{180b}'..='\u{180f}'
                | '\u{200b}'..='\u{200d}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{2060}'..='\u{2065}'
                | '\u{206a}'..='\u{206f}'
                | '\u{3164}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
                | '\u{ffa0}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0000}'..='\u{e007f}'
                | '\u{e0100}'..='\u{e01ef}'
        )
}

/// Whether `c` changes the direction text is laid out in — an embedding, override or isolate (U+202A–U+202E, U+2066–U+2069), or a directional mark (U+200E, U+200F, U+061C): what makes a command read differently from how it runs.
pub fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}' | '\u{061c}'
    )
}

/// A space that is not U+0020: it looks like one and is not one to whatever reads the text.
fn is_odd_space(c: char) -> bool {
    matches!(
        c,
        '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// `text` laid out left to right as one block whatever it holds: wrapped in a left-to-right isolate (U+2066 … U+2069), so letters of a right-to-left script inside it cannot carry the characters around them — a pipe, a semicolon, a path — into another order on screen. For [`shown`] text, which holds no bidirectional control of its own that could close the isolate early.
pub fn isolated(text: &str) -> String {
    format!("\u{2066}{text}\u{2069}")
}

/// `text` as the body of a notification, which is read as markup (the freedesktop `body-markup` capability): `&`, `<` and `>` written as entities, so nothing in it opens a tag or names an entity.
pub fn markup_escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_fits_is_untouched() {
        assert_eq!(clipped("short", 40), "short");
        assert_eq!(clipped("exact", 5), "exact");
        assert_eq!(clipped("", 3), "");
    }

    #[test]
    fn a_cut_counts_characters_and_ends_in_an_ellipsis_within_the_limit() {
        assert_eq!(clipped("mañana señor", 6), "mañan…");
        assert_eq!(clipped("東京の夜", 3), "東京…");
        assert_eq!(clipped(&"x".repeat(50), 40).chars().count(), 40);
    }

    #[test]
    fn a_cut_drops_the_space_before_the_ellipsis() {
        assert_eq!(clipped("one two three", 5), "one…");
    }

    #[test]
    fn no_room_is_no_text() {
        assert_eq!(clipped("anything", 0), "");
        assert_eq!(clipped("ab", 1), "…");
    }

    /// Every character that could make a command read as something else is written out, and plain text — accents, other scripts, punctuation — is left alone.
    #[test]
    fn whatever_could_disguise_a_text_is_written_out() {
        assert_eq!(shown("curl x | sh"), "curl x | sh");
        assert_eq!(shown("mañana 東京 «ok»"), "mañana 東京 «ok»");
        assert_eq!(shown("a\nb\r\tc"), "a\\nb\\r\\tc");
        assert_eq!(shown("\u{1b}[2J"), "\\u{1b}[2J");
        assert_eq!(shown("rm \u{202e}fdp.exe"), "rm \\u{202e}fdp.exe");
        assert_eq!(shown("a\u{200b}b\u{feff}"), "a\\u{200b}b\\u{feff}");
        for hidden in [
            '\u{e0041}',
            '\u{e007f}',
            '\u{fe0f}',
            '\u{e0100}',
            '\u{034f}',
            '\u{180b}',
            '\u{180d}',
            '\u{115f}',
            '\u{1160}',
            '\u{3164}',
            '\u{ffa0}',
            '\u{fff9}',
            '\u{fffb}',
            '\u{2066}',
            '\u{200e}',
        ] {
            let text = format!("a{hidden}b");
            assert_eq!(
                shown(&text),
                format!("a\\u{{{:x}}}b", u32::from(hidden)),
                "U+{:04X}",
                u32::from(hidden)
            );
        }
        assert_eq!(
            shown("rm\u{a0}-rf"),
            "rm\\u{a0}-rf",
            "a space that is not one"
        );
    }

    /// A wide gap cannot push the rest of a command out of sight: a run longer than a few spaces is written as its count, and a short one stays as it is.
    #[test]
    fn a_long_run_of_spaces_is_written_as_its_count() {
        assert_eq!(shown("ls   -l"), "ls   -l");
        assert_eq!(
            shown(&format!("ls{}; rm -rf ~", " ".repeat(200))),
            "ls\\u{20}{×200}; rm -rf ~"
        );
        assert_eq!(shown(&shown(&" ".repeat(9))), shown(&" ".repeat(9)));
    }

    /// Lines, tabs and columns of padding are what a reply on a terminal is made of, so they stay; an escape or a bidirectional control in it does not.
    #[test]
    fn a_reply_keeps_its_lines_and_columns_and_nothing_that_rewrites_the_terminal() {
        assert_eq!(
            shown_lines("ok\nname\t3 pending    \u{1b}]0;owned\u{7}"),
            "ok\nname\t3 pending    \\u{1b}]0;owned\\u{7}"
        );
        assert_eq!(shown_lines("a\r\u{202e}b"), "a\\r\\u{202e}b");
    }

    #[test]
    fn markup_is_written_as_entities() {
        assert_eq!(
            markup_escaped("<b>x</b> & <a href='y'>"),
            "&lt;b&gt;x&lt;/b&gt; &amp; &lt;a href='y'&gt;"
        );
    }

    #[test]
    fn an_isolated_text_is_wrapped_in_a_left_to_right_isolate() {
        assert_eq!(isolated("ls"), "\u{2066}ls\u{2069}");
    }
}
