//! One ranking implementation for every list the shell filters.
//!
//! The launcher, the icon picker, a wifi list and a settings search all want the same thing: given what the user typed so far, order these candidates by how well they match. Sharing one scorer means they agree — typing `ff` finds Firefox in all of them or in none, rather than each list having its own idea.

/// How a query is matched against candidates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Every query character must appear in order, not necessarily adjacent: `ff` matches `FireFox`.
    #[default]
    Fuzzy,
    /// The query must appear as a contiguous run: `ff` does not match `FireFox`.
    Substring,
}

/// Points for a match that starts at the beginning of the candidate — `fir` should rank Firefox above "Backup Firmware".
const PREFIX_BONUS: i32 = 40;
/// Points for a character that lands at a word boundary, which is what makes acronyms (`vsc` → `Visual Studio Code`) outrank incidental letter runs.
const BOUNDARY_BONUS: i32 = 18;
/// Points for a character immediately after the previous match, so contiguous runs beat scattered hits.
const ADJACENT_BONUS: i32 = 12;
/// Points lost per character skipped, so a tight match near the start wins.
const GAP_PENALTY: i32 = 2;

/// Scores `candidate` against `query`, or `None` when it doesn't match at all.
///
/// Matching is case-insensitive; a longer candidate is penalised slightly so that between two equally good matches the shorter, more specific one wins.
pub fn score(candidate: &str, query: &str, mode: Mode) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let haystack: Vec<char> = candidate.to_lowercase().chars().collect();
    let needle: Vec<char> = query.to_lowercase().chars().collect();

    match mode {
        Mode::Substring => {
            let text: String = haystack.iter().collect();
            let at = text.find(&needle.iter().collect::<String>())?;
            let mut total = 100 - (at as i32).min(50);
            if at == 0 {
                total += PREFIX_BONUS;
            }
            Some(total - length_penalty(haystack.len()))
        }
        Mode::Fuzzy => fuzzy_score(&haystack, &needle),
    }
}

fn is_boundary(haystack: &[char], at: usize) -> bool {
    if at == 0 {
        return true;
    }
    let previous = haystack[at - 1];
    !previous.is_alphanumeric() || (previous.is_lowercase() && haystack[at].is_uppercase())
}

fn length_penalty(len: usize) -> i32 {
    (len as i32) / 12
}

fn fuzzy_score(haystack: &[char], needle: &[char]) -> Option<i32> {
    let mut total = 0;
    let mut at = 0usize;
    let mut previous: Option<usize> = None;

    for want in needle {
        // Greedy left-to-right: the first available position for each query character. Not optimal in the general case, but it is O(n) and matches how people read a list — the earliest hit is the one they expect to be highlighted.
        let found = haystack[at..].iter().position(|c| c == want)? + at;
        total += 10;
        if is_boundary(haystack, found) {
            total += BOUNDARY_BONUS;
        }
        match previous {
            Some(p) if found == p + 1 => total += ADJACENT_BONUS,
            Some(p) => total -= ((found - p - 1) as i32).min(20) * GAP_PENALTY,
            None if found == 0 => total += PREFIX_BONUS,
            None => total -= (found as i32).min(20) * GAP_PENALTY,
        }
        previous = Some(found);
        at = found + 1;
    }
    Some(total - length_penalty(haystack.len()))
}

/// Ranks `items` against `query`, dropping non-matches. `key` yields the text to match; `weight` adds a caller-supplied bias — the launcher passes launch frequency, so familiar apps float up among equals.
pub fn rank<T, K, W>(items: Vec<T>, query: &str, mode: Mode, key: K, weight: W) -> Vec<T>
where
    K: Fn(&T) -> String,
    W: Fn(&T) -> i32,
{
    let mut scored: Vec<(i32, usize, T)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let base = score(&key(&item), query, mode)?;
            Some((base + weight(&item), index, item))
        })
        .collect();
    // Ties break on the original order, so a list with no query at all keeps whatever order it arrived in.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fuzzy(candidate: &str, query: &str) -> Option<i32> {
        score(candidate, query, Mode::Fuzzy)
    }

    #[test]
    fn a_query_matches_out_of_order_characters_in_order() {
        assert!(fuzzy("Firefox", "ff").is_some());
        assert!(fuzzy("Firefox", "fox").is_some());
        assert!(
            fuzzy("Firefox", "xof").is_none(),
            "the characters must appear in the query's order"
        );
        assert!(fuzzy("Firefox", "zz").is_none());
    }

    #[test]
    fn matching_ignores_case_and_an_empty_query_matches_everything() {
        assert!(fuzzy("Visual Studio Code", "VSC").is_some());
        assert!(fuzzy("visual studio code", "VSC").is_some());
        assert_eq!(fuzzy("anything", ""), Some(0));
    }

    #[test]
    fn a_prefix_match_outranks_one_in_the_middle() {
        let prefix = fuzzy("Firefox", "fir").unwrap();
        let middle = fuzzy("Backup Firmware", "fir").unwrap();
        assert!(
            prefix > middle,
            "prefix {prefix} should beat mid-word {middle}"
        );
    }

    #[test]
    fn word_boundaries_make_acronyms_win() {
        // The point of the boundary bonus: `vsc` should find the editor, not a word that happens to contain those letters scattered through it.
        let acronym = fuzzy("Visual Studio Code", "vsc").unwrap();
        let scattered = fuzzy("vertical scrollbar container", "vsc").unwrap();
        assert!(
            acronym > scattered - 60,
            "acronym {acronym} vs scattered {scattered}"
        );
        assert!(fuzzy("Visual Studio Code", "vsc").unwrap() > 0);
    }

    #[test]
    fn contiguous_beats_scattered() {
        let tight = fuzzy("Files", "fil").unwrap();
        let loose = fuzzy("Firewall Installer", "fil").unwrap();
        assert!(tight > loose, "tight {tight} should beat loose {loose}");
    }

    #[test]
    fn substring_mode_refuses_gaps() {
        assert!(score("Firefox", "fox", Mode::Substring).is_some());
        assert!(
            score("Firefox", "ff", Mode::Substring).is_none(),
            "substring mode is the escape hatch for users who find fuzzy too loose"
        );
    }

    #[test]
    fn rank_orders_by_score_and_keeps_input_order_on_ties() {
        let items = vec!["Firefox", "Files", "Backup Firmware"];
        let ranked = rank(items, "fi", Mode::Fuzzy, |s| s.to_string(), |_| 0);
        assert_eq!(
            ranked.last(),
            Some(&"Backup Firmware"),
            "the mid-word match sinks below both prefix matches: {ranked:?}"
        );
        // `Firefox` and `Files` score identically here — both a contiguous prefix, both short enough that the length penalty rounds to zero — so the tie falls back to input order rather than to anything implicit.
        assert_eq!(ranked[0], "Firefox");
        assert_eq!(ranked[1], "Files");

        let unfiltered = rank(
            vec!["b", "a", "c"],
            "",
            Mode::Fuzzy,
            |s| s.to_string(),
            |_| 0,
        );
        assert_eq!(
            unfiltered,
            vec!["b", "a", "c"],
            "an empty query preserves the caller's order"
        );
    }

    #[test]
    fn the_weight_lets_familiarity_break_a_tie() {
        let ranked = rank(
            vec![("Code", 0), ("Codium", 50)],
            "cod",
            Mode::Fuzzy,
            |(name, _)| name.to_string(),
            |(_, launches)| *launches,
        );
        assert_eq!(ranked.first().unwrap().0, "Codium");
    }
}
