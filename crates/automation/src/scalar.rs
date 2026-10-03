//! Text read as a number, a truth value or a colour: one way for a variable set from a command line and a source reading a command's output alike, so `on` means the same wherever it is written.

use telar::Color;
use util::report::Message;
use util::text::clipped;

/// How much of a mistaken text a message repeats back, its `…` included: a command's whole output is not worth repeating.
const SHOWN: usize = 40;

/// `text`, trimmed and cut short, as a message repeats a mistaken text back.
pub fn shown(text: &str) -> String {
    clipped(text.trim(), SHOWN)
}

/// `text`, trimmed, as a finite number.
pub fn number(text: &str) -> Result<f64, Message> {
    let text = text.trim();
    text.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| util::message!("finding.not_a_number", text = shown(text)))
}

/// `text`, trimmed and in any case, as `true`, `yes`, `on` or `1`, or `false`, `no`, `off`, `0` or nothing at all: a command that prints nothing says no.
pub fn truth(text: &str) -> Result<bool, Message> {
    let text = text.trim();
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" | "" => Ok(false),
        _ => Err(util::message!(
            "finding.not_true_or_false",
            text = shown(text)
        )),
    }
}

/// `text`, trimmed, as a `#rrggbb` (or `#rrggbbaa`) colour.
pub fn color(text: &str) -> Result<Color, Message> {
    let text = text.trim();
    Color::from_hex(text).ok_or_else(|| util::message!("finding.not_a_colour", text = shown(text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_is_finite_and_trimmed() {
        assert_eq!(number(" 42 "), Ok(42.0));
        assert_eq!(number("-1.5e2"), Ok(-150.0));
        assert!(number("forty").is_err());
        assert!(number("inf").is_err());
        assert!(number("NaN").is_err());
        assert!(number("").is_err());
    }

    #[test]
    fn a_truth_value_reads_words_digits_and_nothing() {
        for yes in ["true", "Yes", " ON ", "1"] {
            assert_eq!(truth(yes), Ok(true), "{yes}");
        }
        for no in ["false", "NO", "off", "0", "", "  "] {
            assert_eq!(truth(no), Ok(false), "{no:?}");
        }
        assert_eq!(
            truth("maybe").map_err(|why| why.english()),
            Err("`maybe` is not true or false".to_string())
        );
    }

    #[test]
    fn a_colour_is_hex_and_a_long_mistake_is_clipped() {
        assert!(color("#88c0d0").is_ok());
        assert!(
            color("teal")
                .unwrap_err()
                .english()
                .contains("`teal` is not a colour")
        );
        let long = "x".repeat(100);
        let refused = number(&long).unwrap_err().english();
        assert_eq!(
            refused,
            format!("`{}…` is not a number", "x".repeat(SHOWN - 1))
        );
    }
}
