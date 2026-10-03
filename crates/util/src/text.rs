//! Text cut to fit where it is shown: a title on a chip, a notification's summary on a lock screen, a value in a field, a command's output repeated back in a message.

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
}
